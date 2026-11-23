//! update_engine_sideload, the phone's own program that applies an A/B
//! update package: how it is started, and what it reports while it works.

use crate::package::Update;

/// Where the recovery ramdisk keeps it.
pub const PROGRAM: &str = "/system/bin/update_engine_sideload";

/// Its arguments for `update`, read from the file `package`; it writes
/// its status lines to `status_fd`.
pub fn arguments(package: &str, update: &Update, status_fd: i32) -> Vec<String> {
    vec![
        format!("--payload=file://{package}"),
        format!("--offset={}", update.payload_offset),
        format!("--headers={}", update.properties),
        format!("--status_fd={status_fd}"),
    ]
}

/// For a manual install with signature checks off, update_engine must see
/// the OTA archive as absent; an empty archive is a broken verifier. The
/// engine still checks the payload size and hash from the original headers.
#[cfg(target_os = "android")]
pub(crate) fn payload_signatures(command: &mut std::process::Command, verify: bool) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::ffi::CString;
    use std::io;
    use std::os::unix::{ffi::OsStrExt, process::CommandExt};
    use std::path::Path;
    use std::ptr::null;

    let store = Path::new(crate::signature::CERTIFICATES);
    if verify || !store.try_exists().with_context(|| format!("look for {}", store.display()))? {
        return Ok(());
    }
    let directory = CString::new(store.parent().expect("the trust store is in a directory").as_os_str().as_bytes())?;
    // The engine reads a local file. Hide the OTA store only in its mount
    // namespace, leaving recovery and subsequent installs on the original.
    unsafe {
        command.pre_exec(move || {
            if libc::unshare(libc::CLONE_NEWNS) != 0
                || libc::mount(null(), c"/".as_ptr(), null(), libc::MS_PRIVATE | libc::MS_REC, null()) != 0
                || libc::mount(
                    c"tmpfs".as_ptr(), directory.as_ptr(), c"tmpfs".as_ptr(),
                    libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                    c"mode=0755,size=4096".as_ptr().cast(),
                ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

/// A line it writes to the status descriptor that matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Step 1 writes the partitions; step 2 runs the package's last steps,
    /// after the written partitions are read back.
    Step(u32),
    /// It failed with this error code.
    Error(i32),
}

pub fn status(line: &str) -> Option<Status> {
    let text = line.strip_prefix("ui_print ")?;
    if let Some(step) = text.strip_prefix("Step ") {
        return step.split_once('/')?.0.parse().ok().map(Status::Step);
    }
    let code = text.strip_prefix("Error applying update: ")?;
    code.split_whitespace().next()?.parse().ok().map(Status::Error)
}

/// What its log on stderr tells beyond the status lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Log {
    /// A partition the payload writes, listed once before writing starts.
    Partition(String),
    /// Super has no room for snapshots, so it writes over the partitions
    /// of the slot the phone runs from.
    Overwriting,
    /// It started reading back the `index`th partition it wrote, from 0.
    Hashing { index: usize, partition: String },
    /// The engine's combined payload/application progress, as a percent.
    Progress(u8),
}

pub fn log(line: &str) -> Option<Log> {
    if let Some(rest) = after(line, "PartitionInfo new ") {
        let (name, _) = rest.split_once(' ')?;
        return Some(Log::Partition(name.to_string()));
    }
    if line.contains("Will overwrite existing partitions.") {
        return Some(Log::Overwriting);
    }
    if let Some(rest) = after(line, ", overall progress ") {
        let percent: u8 = rest.strip_suffix('%')?.parse().ok()?;
        return (percent <= 100).then_some(Log::Progress(percent));
    }
    let rest = after(line, "Hashing partition ")?;
    let (index, rest) = rest.split_once(" (")?;
    let (name, _) = rest.split_once(')')?;
    Some(Log::Hashing { index: index.parse().ok()?, partition: name.to_string() })
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|at| &line[at + marker.len()..])
}

/// The engine's writing, readback and finalization phases. ZIP directory
/// reads and signature checks are not evidence that partitions were written.
/// The stock sideload program rounds `set_progress` to whole 0/1 values;
/// its detailed log retains the actual application percentage.
#[cfg(any(target_os = "android", test))]
#[derive(Default)]
pub(crate) struct Progress {
    written: u8,
    partitions: usize,
    verifying: Option<(usize, String)>,
    finishing: bool,
}

#[cfg(any(target_os = "android", test))]
impl Progress {
    pub fn status(&mut self, status: Status) {
        if status == Status::Step(2) {
            self.finishing = true;
        }
    }

    pub fn log(&mut self, log: &Log) {
        match log {
            Log::Partition(_) => self.partitions += 1,
            Log::Progress(percent) => self.written = self.written.max(*percent),
            Log::Hashing { index, partition } => self.verifying = Some((*index, partition.clone())),
            Log::Overwriting => {}
        }
    }

    pub fn stage(&self) -> crate::Stage {
        if self.finishing {
            crate::Stage::Finishing
        } else if let Some((index, partition)) = &self.verifying {
            crate::Stage::Verifying { index: *index, count: self.partitions.max(index + 1), partition: partition.clone() }
        } else {
            crate::Stage::Writing { done: u64::from(self.written), total: 100 }
        }
    }
}

/// What an update_engine error code comes down to for whoever installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// The package is not signed with the keys this phone trusts.
    Signature,
    /// The package is damaged.
    Damaged,
    /// It is older than what the recovery allows.
    Older,
    /// Super has no room for the new partitions.
    NoSpace,
    /// Reading the package failed: the computer stopped sending.
    Transfer,
    /// Writing a partition failed.
    Write,
    /// The written partitions read back wrong.
    Verify,
    /// The package's own last step failed.
    PostInstall,
    /// The package is in a format this update_engine does not know.
    Unsupported,
    Other,
}

impl Trouble {
    pub fn of(code: i32) -> Trouble {
        match code {
            12 | 18 | 22 | 24 | 25 | 26 | 33 | 39 => Trouble::Signature,
            10 | 11 | 21 | 23 | 27 | 28 | 29 | 32 | 38 => Trouble::Damaged,
            51 => Trouble::Older,
            60 => Trouble::NoSpace,
            9 => Trouble::Transfer,
            7 | 14 | 20 => Trouble::Write,
            15 | 16 | 47 | 56 => Trouble::Verify,
            5 | 63 => Trouble::PostInstall,
            6 | 17 | 44 | 45 => Trouble::Unsupported,
            _ => Trouble::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Metadata;

    #[cfg(target_os = "android")]
    #[test]
    #[ignore = "requires root in a recovery with its OTA certificate store"]
    fn only_the_unsigned_engine_loses_access_to_the_ota_certificates() {
        let store = crate::signature::CERTIFICATES;
        let original = std::fs::read(store).unwrap();
        assert!(!original.is_empty());
        for verify in [true, false, true] {
            let mut child = std::process::Command::new("/system/bin/sh");
            let check = if verify { "test -s \"$1\"" } else { "test ! -e \"$1\"" };
            child.args(["-c", check, "ota-certificates", store]);
            payload_signatures(&mut child, verify).unwrap();
            assert!(child.status().unwrap().success(), "verify = {verify}");
            assert_eq!(std::fs::read(store).unwrap(), original);
        }
    }

    #[test]
    fn the_command_line_is_the_one_recovery_builds() {
        let update = Update {
            payload_offset: 2365,
            payload_size: 7_103_041_280,
            properties: "FILE_HASH=abc=\nFILE_SIZE=7103041280\n".into(),
            metadata: Metadata::default(),
        };
        assert_eq!(
            arguments("/sideload/package.zip", &update, 9),
            [
                "--payload=file:///sideload/package.zip",
                "--offset=2365",
                "--headers=FILE_HASH=abc=\nFILE_SIZE=7103041280\n",
                "--status_fd=9",
            ]
        );
    }

    #[test]
    fn status_lines_give_steps_and_errors() {
        assert_eq!(status("ui_print Step 1/2"), Some(Status::Step(1)));
        assert_eq!(status("ui_print Step 2/2"), Some(Status::Step(2)));
        assert_eq!(
            status("ui_print Error applying update: 51 (ErrorCode::kPayloadTimestampError)"),
            Some(Status::Error(51))
        );
        assert_eq!(status("progress 0.5 0"), None);
        assert_eq!(status("set_progress 1"), None);
    }

    #[test]
    fn the_log_names_partitions_and_the_one_being_read_back() {
        assert_eq!(
            log("[INFO:delta_performer.cc(267)] PartitionInfo new vendor_boot sha256: 3f8a size: 100663296"),
            Some(Log::Partition("vendor_boot".into()))
        );
        assert_eq!(
            log("[INFO:filesystem_verifier_action.cc(377)] Hashing partition 3 (odm) on device /dev/block/dm-12"),
            Some(Log::Hashing { index: 3, partition: "odm".into() })
        );
        assert_eq!(
            log("[INFO:dynamic_partition_control_android.cc(1349)] Will overwrite existing partitions. Slot B may be unbootable until update finishes!"),
            Some(Log::Overwriting)
        );
        assert_eq!(log("[INFO:delta_performer.cc(267)] PartitionInfo old odm sha256: 00 size: 4"), None);
        assert_eq!(log("[INFO:delta_performer.cc(108)] Completed 10/400 operations (2%)"), None);
    }

    #[test]
    fn application_progress_comes_from_the_engine_not_the_zip_offset() {
        let line = "[INFO:delta_performer.cc(113)] Completed 12/400 operations (3%), 65536/7103041280 bytes downloaded (0%), overall progress 2%";
        assert_eq!(log(line), Some(Log::Progress(2)));
        for text in ["0", "50", "99", "100"] {
            assert_eq!(log(&format!("Completed operations, overall progress {text}%")), Some(Log::Progress(text.parse().unwrap())));
        }
        for text in ["-1%", "101%", "256%", "NaN%", "50", "1.5%", "%"] {
            assert_eq!(log(&format!("Completed operations, overall progress {text}")), None);
        }
        assert_eq!(Progress::default().stage(), crate::Stage::Writing { done: 0, total: 100 });
    }

    #[test]
    fn writing_moves_only_with_application_progress_and_never_backwards() {
        let mut progress = Progress::default();
        progress.status(Status::Step(1));
        progress.log(&Log::Partition("system".into()));
        assert_eq!(progress.stage(), crate::Stage::Writing { done: 0, total: 100 });
        for percent in [1, 25, 25, 50, 49, 99, 100] {
            progress.log(&Log::Progress(percent));
            let expected = if percent == 49 { 50 } else { percent };
            assert_eq!(progress.stage(), crate::Stage::Writing { done: u64::from(expected), total: 100 });
        }
    }

    #[test]
    fn readback_and_finishing_keep_their_phase_when_outputs_interleave() {
        let mut progress = Progress::default();
        progress.log(&Log::Partition("system".into()));
        progress.log(&Log::Partition("vendor".into()));
        progress.log(&Log::Hashing { index: 0, partition: "system".into() });
        progress.log(&Log::Progress(100));
        progress.status(Status::Step(1));
        assert_eq!(progress.stage(), crate::Stage::Verifying { index: 0, count: 2, partition: "system".into() });
        progress.status(Status::Step(2));
        progress.log(&Log::Hashing { index: 1, partition: "vendor".into() });
        progress.status(Status::Step(1));
        assert_eq!(progress.stage(), crate::Stage::Finishing);
    }

    #[test]
    fn error_codes_come_down_to_their_trouble() {
        assert_eq!(Trouble::of(26), Trouble::Signature);
        assert_eq!(Trouble::of(51), Trouble::Older);
        assert_eq!(Trouble::of(9), Trouble::Transfer);
        assert_eq!(Trouble::of(60), Trouble::NoSpace);
        assert_eq!(Trouble::of(1), Trouble::Other);
    }
}
