//! An Android system held by the installed first stage's DSU support.
//! Android owns the image mapping and its separate userdata; Skyward
//! installs a prepared package and selects which system starts next.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use zip::{CompressionMethod, ZipArchive};

#[cfg(target_os = "android")]
mod compat;

pub const MANIFEST: &str = "android.toml";
pub const FOLDER: &str = "/data/gsi/dsu";
pub const PREFIX: &str = "skyward-";
/// The Android init retained beside the private DSU compatibility entry.
pub const ORIGINAL_INIT: &str = "/system/bin/init.skyward-original";
pub const PARTITIONS: &[&str] = &["system", "system_ext", "product", "mi_ext", "vendor", "odm", "vendor_dlkm", "odm_dlkm", "system_dlkm"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub device: String,
    /// Raw images, with the payload's verified SHA-256 values.
    pub images: BTreeMap<String, Image>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    pub fn check(&self) -> Result<()> {
        ensure!(!self.id.is_empty() && self.id.len() <= 32 && self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c)), "invalid Android system id");
        ensure!(self.id != "installed" && self.id != "." && self.id != "..", "the Android system id is reserved");
        ensure!(!self.name.trim().is_empty() && !self.version.trim().is_empty(), "the Android system has no name or version");
        ensure!(self.device == "zizhan", "Android dual boot is currently supported only for zizhan");
        ensure!(self.images.contains_key("system"), "the Android package has no system image");
        for (name, image) in &self.images {
            ensure!(PARTITIONS.contains(&name.as_str()), "{name} cannot be installed as an Android DSU image");
            ensure!(image.size > 0 && image.size <= 128 << 30 && image.size.is_multiple_of(4096), "{name} has an invalid image size");
            ensure!(image.sha256.len() == 64 && image.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{name} has an invalid SHA-256");
        }
        Ok(())
    }

    pub fn bytes(&self) -> u64 {
        self.images.values().map(|image| image.size).sum()
    }

    pub fn slot(&self) -> String {
        format!("{PREFIX}{}", self.id)
    }
}

/// What Skyward records after the Android image service completes an
/// install. `enabled` is refreshed from that service before displaying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub manifest: Manifest,
    pub userdata: u64,
    pub enabled: bool,
    /// Native DSU can remain enabled after exhausting its boot attempts.
    /// Refreshed at runtime, so that fallback shows the main system selected.
    #[serde(skip)]
    pub starts: bool,
    /// Written before allocation so interrupted installs remain removable.
    pub complete: bool,
    pub boot: String,
    pub vendor_boot: String,
    pub dtbo: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Allocating { done: u64, total: u64 },
    Writing { done: u64, total: u64 },
    Finishing,
}

/// The installed image service permits one initial boot attempt; `ok`
/// records a successful boot and permits subsequent boots as well.
pub fn starts(enabled: bool, status: &str) -> bool {
    enabled && matches!(status.trim(), "0" | "ok")
}

pub fn read<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<Manifest> {
    let mut text = String::new();
    zip.by_name(MANIFEST).context("open android.toml")?.take(1 << 20).read_to_string(&mut text)?;
    let manifest: Manifest = toml::from_str(&text).context("read android.toml")?;
    manifest.check()?;
    for (name, image) in &manifest.images {
        let entry = zip.by_name(&format!("{name}.img")).with_context(|| format!("open {name}.img"))?;
        ensure!(entry.size() == image.size, "{name}.img has a different size from the manifest");
        ensure!(entry.compression() == CompressionMethod::Stored, "{name}.img must be stored uncompressed for streaming installation");
    }
    Ok(manifest)
}

#[cfg(target_os = "android")]
mod sys {
    use super::*;
    use skyward_binder::driver::{Binder, Handle, OwnedHandle};
    use skyward_binder::parcel::Parcel;
    use skyward_binder::{service, status};
    use std::fs::{self, File, OpenOptions};
    use std::io::{SeekFrom, ErrorKind};
    use std::os::fd::AsFd;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const SERVICE: &str = "gsiservice";
    const INTERFACE: &str = "android.gsi.IGsiService";
    const LIBRARIES: &str = "/mnt/system/system/lib64:/apex/com.android.runtime/lib64/bionic:/apex/com.android.runtime/lib64";
    const METADATA: &str = "/metadata/gsi/dsu";
    const IMAGE_INTERFACE: &str = "android.gsi.IImageService";

    pub struct Client {
        service: OwnedHandle,
    }

    impl Client {
        pub fn open() -> Result<Self> {
            let binder = Arc::new(Binder::open()?);
            let mut found = service::find(&binder, SERVICE)?;
            if found.is_none() {
                ensure!(Path::new("/mnt/system/system/bin/gsid").is_file(), "the installed Android system has no image service");
                let loader = ["/system/bin/bootstrap/linker64", "/apex/com.android.runtime/bin/linker64"].into_iter().find(|p| Path::new(p).is_file()).context("the installed Android runtime is not prepared")?;
                fs::create_dir_all("/skyward/run")?;
                let log = OpenOptions::new().create(true).append(true).open("/skyward/run/gsid.log")?;
                let mut child = Command::new(loader).arg("/mnt/system/system/bin/gsid").env("LD_LIBRARY_PATH", LIBRARIES)
                    .stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn().context("start the installed Android image service")?;
                let until = Instant::now() + Duration::from_secs(10);
                while found.is_none() {
                    ensure!(child.try_wait()?.is_none(), "the Android image service exited before registering");
                    if Instant::now() >= until {
                        let _ = child.kill();
                        let _ = child.wait();
                        anyhow::bail!("the Android image service did not register");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    found = service::find(&binder, SERVICE)?;
                }
                std::thread::spawn(move || { let _ = child.wait(); });
            }
            let service = found.context("the Android image service did not register")?.into_owned(Arc::clone(&binder))?;
            Ok(Self { service })
        }

        fn boolean(&self, method: u32) -> Result<bool> {
            let service = &self.service;
            let reply = service.transact(method, &Parcel::call(INTERFACE))?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            read.bool()
        }

        fn success(&self, method: u32, call: &Parcel) -> Result<()> {
            let service = &self.service;
            let reply = service.transact(method, call)?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            let code = read.i32()?;
            ensure!(code == 0, "Android image service method {method} failed with code {code}");
            Ok(())
        }

        pub fn installed(&self) -> Result<bool> { self.boolean(13) }
        pub fn enabled(&self) -> Result<bool> { self.boolean(7) }
        pub fn busy(&self) -> Result<bool> { self.boolean(9) }

        pub fn starts(&self) -> Result<bool> {
            Ok(super::starts(self.enabled()?, &fs::read_to_string(Path::new(METADATA).join("install_status"))?))
        }

        pub fn active(&self) -> Result<String> {
            let service = &self.service;
            let reply = service.transact(15, &Parcel::call(INTERFACE))?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            read.string()
        }

        pub fn enable(&self, slot: &str) -> Result<()> {
            let mut call = Parcel::call(INTERFACE);
            call.bool(false);
            call.string(slot);
            self.success(5, &call)
        }

        pub fn disable(&self) -> Result<()> {
            ensure!(self.boolean(12)?, "the Android image service could not disable the second system");
            Ok(())
        }

        pub fn cancel(&self) -> Result<()> {
            ensure!(self.boolean(8)?, "the Android image service could not cancel the install");
            Ok(())
        }

        pub fn begin(&self, slot: &str) -> Result<()> {
            let mut call = Parcel::call(INTERFACE);
            call.string(&format!("{FOLDER}/{slot}"));
            self.success(18, &call)
        }

        pub fn partition(&self, name: &str, size: u64, readonly: bool) -> Result<()> {
            ensure!(size <= i64::MAX as u64, "an Android partition is too large");
            let mut call = Parcel::call(INTERFACE);
            call.string(name);
            call.i64(size as i64);
            call.bool(readonly);
            self.success(20, &call)
        }

        pub fn stream(&self, file: &File, size: u64) -> Result<()> {
            ensure!(size <= i64::MAX as u64, "an Android image is too large");
            let mut call = Parcel::call(INTERFACE);
            call.file(file.as_fd())?;
            call.i64(size as i64);
            let service = &self.service;
            let reply = service.transact(1, &call)?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            ensure!(read.bool()?, "the Android image service could not write the image stream");
            Ok(())
        }

        pub fn close_partition(&self) -> Result<()> { self.success(21, &Parcel::call(INTERFACE)) }
        pub fn finish(&self) -> Result<()> { self.success(19, &Parcel::call(INTERFACE)) }

        fn images(&self, slot: &str) -> Result<Handle<'_>> {
            let mut call = Parcel::call(INTERFACE);
            call.string(&format!("dsu/{slot}"));
            let service = &self.service;
            let reply = service.transact(23, &call)?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            let number = read.object()?.context("the Android image service returned no image manager")?;
            Handle::hold(self.service.binder(), number)
        }

        /// Native removeGsi waits for Android's vold, which a recovery does
        /// not run. Use its image manager for this one owned slot instead.
        pub fn remove(&self, manifest: &Manifest) -> Result<()> {
            manifest.check()?;
            let slot = manifest.slot();
            ensure!(!self.installed()? || self.active()? == slot, "another Android install is active");
            self.cancel()?;
            if self.installed()? { self.disable()?; }
            let folder = Path::new(FOLDER).join(&slot);
            let metadata = Path::new(METADATA).join(&slot);
            if folder.exists() && metadata.exists() {
                check_folder(&folder, Path::new(FOLDER))?;
                check_folder(&metadata, Path::new(METADATA))?;
                let images = self.images(&slot)?;
                let reply = images.transact(8, &Parcel::call(IMAGE_INTERFACE))?;
                let mut read = reply.read();
                status::ok(&mut read)?;
                let count = read.i32()?;
                ensure!((0..=32).contains(&count), "the image service returned an invalid image count");
                let names = (0..count).map(|_| read.string()).collect::<Result<Vec<_>>>()?;
                for name in names {
                    let partition = name.strip_suffix("_gsi").context("an unexpected image is present in the Android slot")?;
                    ensure!(PARTITIONS.contains(&partition) || ["userdata", "scratch"].contains(&partition), "an unexpected image is present: {name}");
                    let mut call = Parcel::call(IMAGE_INTERFACE);
                    call.string(&name);
                    let reply = images.transact(6, &call)?;
                    let mut read = reply.read();
                    status::ok(&mut read)?;
                    if read.bool()? {
                        let reply = images.transact(4, &call)?;
                        status::ok(&mut reply.read())?;
                    }
                    let reply = images.transact(2, &call)?;
                    status::ok(&mut reply.read())?;
                }
            }
            remove_folder(&folder, Path::new(FOLDER))?;
            remove_folder(&metadata, Path::new(METADATA))?;
            let keys = Path::new("/metadata/vold/metadata_encryption/dsu");
            remove_folder(&keys.join(&slot), keys)?;
            for name in ["install_status", "one_shot_boot", "active", "booted", "lp_names"] {
                remove_file(&Path::new(METADATA).join(name))?;
            }
            File::open(METADATA)?.sync_all()?;
            Ok(())
        }
    }

    fn check_folder(path: &Path, parent: &Path) -> Result<()> {
        ensure!(path.parent() == Some(parent), "an Android folder is outside its expected parent");
        ensure!(fs::canonicalize(path)? == fs::canonicalize(parent)?.join(path.file_name().context("the Android folder has no name")?), "the Android folder redirects elsewhere");
        Ok(())
    }

    fn remove_folder(path: &Path, parent: &Path) -> Result<()> {
        if path.try_exists()? {
            check_folder(path, parent)?;
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(())
    }

    fn remove_file(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
        }
    }

    /// The first stage leaves this marker across a reboot into Recovery.
    /// Only clear it during an explicit operation, with metadata writable.
    pub fn prepare() -> Result<()> {
        ensure!(Path::new("/skyward/device.toml").is_file(), "Android image operations require Skyward Recovery");
        let mounts = fs::read_to_string("/proc/mounts")?;
        ensure!(!mounts.lines().any(|l| l.split_whitespace().next().is_some_and(|s| s.contains("_gsi"))), "Android DSU filesystems are still mounted");
        remove_file(&Path::new(METADATA).join("booted"))
    }

    pub fn matches(client: &Client, recorded: &Installed) -> Result<bool> {
        Ok(client.installed()? && client.active()? == recorded.manifest.slot())
    }

    pub fn room(package: &Path, manifest: &Manifest) -> Result<u64> {
        let compatibility = super::compat::extra_space(package, manifest)?;
        Ok(crate::install::free()?.saturating_sub(manifest.bytes() + compatibility + skyward_block::size_bytes(Path::new(skyward_block::BY_NAME).join("super"))? + (1 << 30)))
    }

    /// Install into Android's image service, leaving the main system selected.
    /// An incomplete record is saved before the first allocation, and retained
    /// if cleanup fails, so a subsequent Recovery can remove the partial slot.
    pub fn install(package: &Path, manifest: &Manifest, userdata: u64, progress: &mut dyn FnMut(Step) -> Result<()>) -> Result<()> {
        ensure!(skyward_sys::prop("ro.product.device").as_deref() == Some(manifest.device.as_str()), "the Android package is for a different device");
        ensure!(userdata >= 8_000_000_000 && userdata.is_multiple_of(4096), "the independent Android data space must be at least 8 GB and block aligned");
        let mut zip = ZipArchive::new(File::open(package)?)?;
        ensure!(read(&mut zip)? == *manifest, "the Android package description changed");
        crate::install::writable(|| {
            use crate::state::{State, PATH};
            let path = Path::new(PATH);
            let mut state = State::load(path)?;
            ensure!(state.system.is_none() && state.android.is_none() && state.occupied.is_none() && state.switching.is_none(), "a second system or unfinished switch exists already");
            ensure!(skyward_vab::snapshot_state()? == skyward_vab::SnapshotState::None, "finish the installed system's update before installing");
            ensure!(userdata <= room(package, manifest)?, "there is not enough free space for the Android images and independent data");
            let client = Client::open()?;
            ensure!(!client.installed()? && !client.busy()?, "Android already has an installed or unfinished dynamic system");
            let folder = Path::new(FOLDER).join(manifest.slot());
            let metadata = Path::new(METADATA).join(manifest.slot());
            ensure!(!folder.try_exists()? && !metadata.try_exists()?, "an unrecorded Android slot already exists; it has not been overwritten");
            let slot = skyward_vab::current_slot()?.context("not an A/B phone")?;
            state.android = Some(Installed {
                manifest: manifest.clone(), userdata, enabled: false, starts: false, complete: false,
                boot: crate::install::sum_of(&crate::store::partition(Path::new(skyward_block::BY_NAME), "boot", slot))?,
                vendor_boot: crate::install::sum_of(&crate::store::partition(Path::new(skyward_block::BY_NAME), "vendor_boot", slot))?,
                dtbo: crate::install::sum_of(&crate::store::partition(Path::new(skyward_block::BY_NAME), "dtbo", slot))?,
            });
            state.save(path)?;
            let made = (|| -> Result<()> {
                prepare()?;
                let vendor = super::compat::vendor(manifest)?;
                let vendor_size = vendor.as_ref().map_or(0, |p| p.size);
                let total = manifest.bytes() + vendor_size;
                fs::create_dir_all(&folder)?;
                fs::create_dir_all(&metadata)?;
                client.begin(&manifest.slot())?;
                progress(Step::Allocating { done: 0, total: userdata + total })?;
                client.partition("userdata", userdata, false)?;
                client.close_partition()?;
                let mut allocated = userdata;
                let mut allocation_total = userdata + total;
                let mut written = 0;
                for (name, image) in &manifest.images {
                    let offset = zip.by_name(&format!("{name}.img"))?.data_start().context("the image has no data offset")?;
                    let converted = super::compat::prepare(manifest, name, package, offset, image.size)?;
                    let size = converted.as_ref().map_or(image.size, |p| p.size);
                    allocation_total += size.saturating_sub(image.size);
                    progress(Step::Allocating { done: allocated, total: allocation_total })?;
                    client.partition(name, size, true)?;
                    allocated += size;
                    let mut original = File::open(package)?;
                    original.seek(SeekFrom::Start(offset))?;
                    let file = converted.as_ref().map_or(&original, |p| &p.file);
                    let mut left = size;
                    while left > 0 {
                        let credit = ((size - left) as u128 * image.size as u128 / size as u128) as u64;
                        progress(Step::Writing { done: written + credit, total })?;
                        let bytes = left.min(16 << 20);
                        client.stream(file, bytes)?;
                        left -= bytes;
                    }
                    client.close_partition()?;
                    written += image.size;
                }
                if let Some(vendor) = &vendor {
                    progress(Step::Allocating { done: allocated, total: allocation_total })?;
                    client.partition("vendor", vendor.size, true)?;
                    let mut left = vendor.size;
                    while left > 0 {
                        progress(Step::Writing { done: written, total })?;
                        let bytes = left.min(16 << 20);
                        client.stream(&vendor.file, bytes)?;
                        left -= bytes;
                        written += bytes;
                    }
                    client.close_partition()?;
                }
                progress(Step::Writing { done: written, total })?;
                progress(Step::Finishing)?;
                client.finish()?;
                ensure!(client.installed()? && client.active()? == manifest.slot() && !client.enabled()?, "the Android image service did not finish in the main-system boot state");
                state.android.as_mut().expect("recorded above").complete = true;
                state.save(path)
            })();
            if let Err(error) = made {
                return match client.remove(manifest) {
                    Ok(()) => { state.android = None; state.save(path)?; Err(error) }
                    Err(cleanup) => Err(error.context(format!("the incomplete install remains recorded for removal: {cleanup:#}"))),
                };
            }
            Ok(())
        })
    }
}

#[cfg(target_os = "android")]
pub use sys::{Client, install, matches, prepare, room};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_initial_android_boot_can_be_selected_again_after_fallback() {
        assert!(starts(true, "0"));
        assert!(starts(true, "ok"));
        for status in ["1", "2", "disabled", "wipe", "", "unknown"] {
            assert!(!starts(true, status), "{status} will not start the second system");
        }
        assert!(!starts(false, "ok"));
    }

    #[cfg(target_os = "android")]
    #[test]
    #[ignore = "requires Skyward with the installed Android 15 runtime mounted"]
    fn the_native_image_service_stays_referenced_between_transactions() {
        let client = Client::open().unwrap();
        assert!(!client.busy().unwrap());
        let installed = client.installed().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
        assert_eq!(client.installed().unwrap(), installed);
        // With no partition install open, gsid rejects the stream itself.
        // Reaching that rejection also tests the real parcel-fd ABI.
        let error = client.stream(&std::fs::File::open("/dev/null").unwrap(), 0).unwrap_err();
        assert_eq!(error.to_string(), "the Android image service could not write the image stream");
    }

    #[test]
    fn android_packages_only_name_isolated_system_images() {
        let mut manifest = Manifest { id: "hyperos-3".into(), name: "HyperOS 3".into(), version: "OS3.0.9.0.VLRCNXM".into(), device: "zizhan".into(), images: BTreeMap::from([("system".into(), Image { size: 4096, sha256: "12".repeat(32) })]) };
        manifest.check().unwrap();
        assert_eq!(manifest.slot(), "skyward-hyperos-3");
        for name in ["boot", "metadata", "userdata", "modem", "../system", "super"] {
            manifest.images.insert(name.into(), Image { size: 4096, sha256: "12".repeat(32) });
            assert!(manifest.check().is_err(), "{name} cannot replace a real partition");
            manifest.images.remove(name);
        }
        manifest.device = "another".into();
        assert!(manifest.check().is_err());
    }

    #[test]
    fn android_images_have_the_declared_size_and_a_supported_stream_format() {
        use std::io::Cursor;
        let bytes = [0x5au8; 4096];
        let manifest = Manifest { id: "stream".into(), name: "Android".into(), version: "15".into(), device: "zizhan".into(), images: BTreeMap::from([("system".into(), Image { size: bytes.len() as u64, sha256: "ab".repeat(32) })]) };
        let make = |manifest: &Manifest, image: &[u8]| {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file(MANIFEST, options).unwrap();
            zip.write_all(toml::to_string(manifest).unwrap().as_bytes()).unwrap();
            zip.start_file("system.img", options).unwrap();
            zip.write_all(image).unwrap();
            zip.finish().unwrap().into_inner()
        };
        let mut zip = ZipArchive::new(Cursor::new(make(&manifest, &bytes))).unwrap();
        assert_eq!(read(&mut zip).unwrap(), manifest);
        let mut short = ZipArchive::new(Cursor::new(make(&manifest, &[0; 2048]))).unwrap();
        assert!(read(&mut short).is_err());
        let mut reserved = manifest;
        reserved.id = "installed".into();
        assert!(reserved.check().is_err());
    }
}
