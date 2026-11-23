//! The fold sensor selects a panel without restarting the pages or their
//! workers. The installed SSC libraries run in a child with the vendor's
//! runtime, and report only the folded state over a pipe.

use anyhow::{ensure, Context, Result};
use skyward_block::mount;
use skyward_config::Fold;
use skyward_sys::worker;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::fd::RawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod fade;
mod ssc;
pub(crate) use fade::PanelFade;
pub use ssc::watch;

const HOME: &str = "/skyward/run/fold";
const PROGRAM: &str = "/skyward/run/vendor/skyward-fold";

/// Give the sensor processor a writable copy of its registry in RAM. Its
/// normal startup rewrites a temporary JSON file even when calibration is
/// already current, so a read-only registry prevents sensors registering.
pub fn prepare(cfg: &Fold) -> Result<()> {
    let home = Path::new(HOME);
    let ready = home.join("registry-ready");
    if ready.try_exists()? {
        return Ok(());
    }
    let source = home.join("partition");
    let registry = home.join("registry");
    fs::create_dir_all(&source)?;
    fs::create_dir_all(&registry)?;
    let device = Path::new(skyward_block::BY_NAME).join(&cfg.registry_partition);
    mount::mount_ro(&device, &source)?;
    let copied = copy_tree(&source.join(&cfg.registry_path), &registry);
    let unmounted = mount::umount(&source);
    copied?;
    unmounted?;
    let target = Path::new(&cfg.registry_target);
    public_mountpoint(target)?;
    mount::mount(&registry, target, "none", &["bind".into()])?;
    fs::write(ready, b"1")?;
    skyward_sys::kmsg("fold sensor: registry copied into RAM from the read-only calibration partition")
}

/// The recovery runs with a private umask, but the vendor's system account
/// must be able to traverse the newly created mount point's parents.
fn public_mountpoint(target: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut parent = target;
    while !parent.try_exists()? {
        missing.push(parent);
        parent = parent.parent().context("sensor registry mount point has no existing parent")?;
    }
    fs::create_dir_all(target)?;
    for path in missing {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    ensure!(metadata.is_dir() || metadata.is_file(), "unsupported sensor registry entry {}", source.display());
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, target)?;
    }
    fs::set_permissions(target, fs::Permissions::from_mode(metadata.mode() & 0o777))?;
    std::os::unix::fs::lchown(target, Some(metadata.uid()), Some(metadata.gid()))?;
    Ok(())
}

/// Start only after the registry and the installed sensor programs are
/// ready to serve it. In particular, never restart the registry server
/// while the sensor processor is still initializing.
pub fn boot(cfg: &Fold) -> Result<()> {
    let started = Path::new(HOME).join("processor-started");
    if started.exists() {
        return Ok(());
    }
    for write in &cfg.writes {
        fs::write(&write.path, &write.value).with_context(|| format!("write {} to {} for the fold sensor", write.value, write.path))?;
    }
    fs::write(started, b"1")?;
    skyward_sys::kmsg("fold sensor: sensor processor startup requested")
}

pub struct Monitor {
    child: Child,
    reader: worker::Worker<Result<bool, String>>,
}

impl Monitor {
    pub fn start() -> Result<Self> {
        ensure!(Path::new(HOME).join("registry-ready").exists(), "fold sensor registry was not prepared");
        let path = Path::new(PROGRAM);
        fs::create_dir_all(path.parent().expect("the vendor helper has a directory"))?;
        let next = PathBuf::from(format!("{PROGRAM}.new"));
        fs::copy(std::env::current_exe()?, &next)?;
        fs::set_permissions(&next, fs::Permissions::from_mode(0o755))?;
        fs::rename(&next, path)?;
        let libraries = crate::secure::libraries()?.context("fold sensor needs the installed vendor runtime")?;
        let mut command = Command::new(path);
        command.arg("fold-watch").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .env("LD_LIBRARY_PATH", format!("/vendor/lib64:{}:/apex/com.android.runtime/lib64/bionic", libraries.display()));
        unsafe {
            command.pre_exec(|| crate::secure::enter_vendor_runtime());
        }
        let mut child = command.spawn().context("start the fold sensor client")?;
        let output = child.stdout.take().expect("the sensor client has a pipe");
        let reader = match worker::Worker::start("fold sensor", move |teller| {
            for line in BufReader::new(output).lines() {
                let state = match line {
                    Ok(line) if line == "0" => Ok(false),
                    Ok(line) if line == "1" => Ok(true),
                    Ok(line) => Err(format!("unexpected fold sensor output: {line}")),
                    Err(error) => Err(format!("read the fold sensor: {error}")),
                };
                if !teller.tell(state) {
                    break;
                }
            }
            teller.tell(Err("fold sensor client ended".into()));
        }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self { child, reader })
    }

    pub fn fd(&self) -> RawFd {
        self.reader.fd()
    }

    pub fn events(&self) -> Vec<Result<bool, String>> {
        self.reader.events()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM); }
        let until = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
