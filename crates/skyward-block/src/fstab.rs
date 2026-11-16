//! The table a phone keeps of its own filesystems: where each one is, what
//! it holds, and how it is encrypted.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the phone's own partitions keep the table.
pub const FILES: [&str; 2] = ["/vendor/etc/fstab.qcom", "/odm/etc/fstab.qcom"];

/// The booted system's table must outlive its partitions: after an update
/// takes /vendor and /odm away, wiping or closing Data still needs it.
static TABLE: Table = Table { kept: OnceLock::new() };

#[derive(Default)]
struct Table {
    kept: OnceLock<Vec<Entry>>,
}

/// One filesystem the phone knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub device: String,
    pub mount_point: String,
    pub filesystem: String,
    /// How the phone mounts it: the kernel's flags and the filesystem's
    /// own options, as one list.
    pub options: Vec<String>,
    /// What the phone says about the filesystem beyond how to mount it,
    /// such as how it is encrypted.
    pub extras: Vec<String>,
}

impl Entry {
    /// The value of an extra written as `name=value`.
    pub fn extra(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}=");
        self.extras.iter().find_map(|e| e.strip_prefix(&prefix))
    }
}

/// Every filesystem a table names. A phone may name the same mount point
/// more than once, once per filesystem it might hold.
pub fn entries(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [device, mount_point, filesystem, options, extras] = fields[..] else { continue };
        out.push(Entry {
            device: device.to_string(),
            mount_point: mount_point.to_string(),
            filesystem: filesystem.to_string(),
            options: options.split(',').map(str::to_string).collect(),
            extras: extras.split(',').map(str::to_string).collect(),
        });
    }
    out
}

/// Every entry read from the given table files, in the phone's order.
fn read_files(files: &[&str]) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for file in files {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("read {file}")),
        };
        out.extend(entries(&text));
    }
    Ok(out)
}

impl Table {
    fn read(&self, files: &[&str]) -> Result<Vec<Entry>> {
        if let Some(kept) = self.kept.get() {
            return Ok(kept.clone());
        }
        let read = read_files(files);
        // A reader may have started before the installer kept the table
        // and finished after its partitions came off.
        self.kept.get().cloned().map(Ok).unwrap_or(read)
    }

    fn keep(&self, files: &[&str]) -> Result<()> {
        if self.kept.get().is_none() {
            let _ = self.kept.set(read_files(files)?);
        }
        Ok(())
    }
}

/// Keep the booted system's complete table before installation takes its
/// partitions away. Further reads use this copy until Recovery exits,
/// including after a later package changes what starts next. Reading a
/// table during startup does not keep it before all partitions are ready.
pub fn keep() -> Result<()> {
    TABLE.keep(&FILES)
}

/// Every entry of the phone's own table, kept before its partitions leave
/// the tree or read from those partitions while they are still there.
fn table() -> Result<Vec<Entry>> {
    TABLE.read(&FILES)
}

/// What the phone's own table says about `mount_point`.
pub fn read(mount_point: &str) -> Result<Vec<Entry>> {
    Ok(table()?.into_iter().filter(|e| e.mount_point == mount_point).collect())
}

/// What the phone's own table says about the partition `name`: the first
/// entry naming it, by whichever path the table reaches it.
pub fn partition(name: &str) -> Result<Option<Entry>> {
    Ok(table()?.into_iter().find(|e| names(e, name)))
}

/// Whether an entry is of the partition `name`.
fn names(entry: &Entry, name: &str) -> bool {
    Path::new(&entry.device).file_name().is_some_and(|n| n == name)
}

/// The partition an entry names, as it is reachable now: of the booted
/// slot, where the phone keeps one of it per slot. A phone's table points
/// at its storage controller by a path a recovery need not have.
pub fn device(entry: &Entry) -> Result<String> {
    let path = match entry.extras.iter().any(|e| e == "slotselect") {
        true => format!("{}{}", entry.device, crate::slot_suffix()?.with_context(|| format!("{} is of a slot, and the phone has none", entry.device))?),
        false => entry.device.clone(),
    };
    if Path::new(&path).exists() {
        return Ok(path);
    }
    let name = Path::new(&path).file_name().with_context(|| format!("{path} names no partition"))?;
    let by_name = Path::new(crate::BY_NAME).join(name);
    if by_name.exists() {
        return Ok(by_name.to_string_lossy().into_owned());
    }
    anyhow::bail!("neither {path} nor {} is here", by_name.display())
}

/// Mount the partition `name` where and as the phone's own table mounts
/// it, and say where. Whatever shows there already, as what an earlier run
/// of the recovery service mounted does, is taken as it is.
pub fn mount(name: &str) -> Result<PathBuf> {
    mount_with_context(name, None)
}

/// Mount as the phone does, with an explicitly configured recovery-policy
/// context. An empty context omits it; None keeps the installed fstab's.
pub fn mount_with_context(name: &str, context: Option<&str>) -> Result<PathBuf> {
    let mut entry = partition(name)?.with_context(|| format!("the phone's own table does not mount {name}"))?;
    if let Some(context) = context {
        entry.options.retain(|option| !option.starts_with("context="));
        if !context.is_empty() {
            entry.options.push(format!("context={context}"));
        }
    }
    let at = PathBuf::from(&entry.mount_point);
    if crate::mount::is_mount_point(&at)? {
        return Ok(at);
    }
    std::fs::create_dir_all(&at).with_context(|| format!("make {}", at.display()))?;
    crate::mount::mount(Path::new(&device(&entry)?), &at, &entry.filesystem, &entry.options)?;
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
# a comment
/dev/block/bootdevice/by-name/metadata /metadata f2fs noatime,nosuid wait,check,formattable
/dev/block/bootdevice/by-name/userdata /data f2fs noatime,discard latemount,wait,fileencryption=aes-256-xts:aes-256-cts:v2,keydirectory=/metadata/vold/metadata_encryption,metadata_encryption=aes-256-xts:wrappedkey_v0,quota
/dev/block/bootdevice/by-name/userdata /data mifs noatime,discard latemount,wait,keydirectory=/metadata/vold/metadata_encryption
";

    #[test]
    fn a_mount_point_can_be_named_once_per_filesystem_it_might_hold() {
        let data: Vec<Entry> = entries(TABLE).into_iter().filter(|e| e.mount_point == "/data").collect();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].filesystem, "f2fs");
        assert_eq!(data[1].filesystem, "mifs");
        assert_eq!(data[0].device, "/dev/block/bootdevice/by-name/userdata");
    }

    #[test]
    fn how_the_phone_mounts_a_filesystem_is_kept_apart_from_the_rest() {
        let data = &entries(TABLE)[1];
        assert_eq!(data.options, ["noatime", "discard"]);
        assert!(data.extras.iter().any(|e| e == "latemount") && !data.extras.iter().any(|e| e == "noatime"));
    }

    #[test]
    fn what_the_phone_says_about_encryption_is_read_off_the_entry() {
        let data = &entries(TABLE)[1];
        assert_eq!(data.extra("metadata_encryption"), Some("aes-256-xts:wrappedkey_v0"));
        assert_eq!(data.extra("keydirectory"), Some("/metadata/vold/metadata_encryption"));
        assert_eq!(data.extra("quota"), None, "a word on its own has no value");
        assert_eq!(data.extra("keydir"), None, "a shorter name is another extra");
    }

    #[test]
    fn an_entry_is_of_the_partition_its_path_ends_in() {
        let modem = &entries(
            "/dev/block/bootdevice/by-name/modem /vendor/firmware_mnt vfat ro,uid=1000,context=u:object_r:firmware_file:s0 wait,slotselect\n",
        )[0];
        assert!(names(modem, "modem"));
        assert!(!names(modem, "modemfirmware") && !names(modem, "mode"), "a name is matched whole");
        assert_eq!(modem.options[0], "ro");
        assert!(modem.extras.iter().any(|e| e == "slotselect"), "one of each slot");
    }

    #[test]
    fn comments_and_short_lines_are_passed_over() {
        assert!(entries("# nothing\n\n/dev/x /y f2fs rw\n").is_empty());
        assert_eq!(entries(TABLE).len(), 3);
    }

    #[test]
    fn the_complete_table_survives_the_installed_partitions_being_removed() {
        let root = tempfile::tempdir().unwrap();
        let vendor = root.path().join("vendor.fstab");
        let odm = root.path().join("odm.fstab");
        std::fs::write(&vendor, TABLE).unwrap();
        std::fs::write(&odm, "/dev/block/by-name/cache /cache ext4 nosuid,nodev wait,check,formattable\n").unwrap();
        let files = [vendor.to_str().unwrap(), odm.to_str().unwrap()];
        let table = Table::default();
        let before = table.read(&files).unwrap();
        table.keep(&files).unwrap();
        std::fs::remove_file(&vendor).unwrap();
        std::fs::remove_file(&odm).unwrap();
        assert_eq!(table.read(&files).unwrap(), before, "Data, metadata, cache and all filesystem alternatives remain available");
        assert!(read_files(&files).unwrap().is_empty(), "the original files really are gone");
        assert_eq!(before.len(), 4);
    }

    #[test]
    fn an_early_read_does_not_keep_a_table_before_its_partitions_are_mounted() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("fstab");
        let files = [file.to_str().unwrap()];
        let table = Table::default();
        assert!(table.read(&files).unwrap().is_empty());
        std::fs::write(&file, TABLE).unwrap();
        table.keep(&files).unwrap();
        assert_eq!(table.read(&files).unwrap(), entries(TABLE));
    }

    #[test]
    fn another_install_keeps_the_booted_table_and_a_new_recovery_reads_the_new_one() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("fstab");
        let files = [file.to_str().unwrap()];
        std::fs::write(&file, TABLE).unwrap();
        let table = Table::default();
        table.keep(&files).unwrap();
        let replacement = "/dev/block/by-name/userdata /data ext4 noatime wait,formattable\n";
        std::fs::write(&file, replacement).unwrap();
        table.keep(&files).unwrap();
        assert_eq!(table.read(&files).unwrap(), entries(TABLE), "choosing another next slot does not change this boot's Data policy");
        assert_eq!(Table::default().read(&files).unwrap(), entries(replacement), "a new Recovery reads its own installed system");
    }
}
