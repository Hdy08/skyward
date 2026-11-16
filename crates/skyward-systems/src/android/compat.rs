//! The installed Android 12 first stage synthesizes an ext4-only /mi_ext
//! entry for DSU. Retain the official files and metadata in an ext4 image;
//! the physical boot, vendor_boot and vendor need no modification. A
//! private system entry binds the DSU image to Xiaomi's other mi_ext path
//! before the original Android init loads properties and init scripts.

use anyhow::{ensure, Context, Result};
use skyward_block::{loopdev, mount, tools};
use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::os::{fd::AsRawFd, unix::{ffi::OsStrExt, fs::{FileExt, MetadataExt}}};
use std::path::{Path, PathBuf};
use super::{Manifest, ORIGINAL_INIT};

pub(super) struct Prepared {
    pub file: File,
    pub size: u64,
}

struct Mounted(Option<PathBuf>);

impl Mounted {
    fn open(device: &Path, at: &Path, filesystem: &str, readonly: bool) -> Result<Self> {
        let options = [if readonly { "ro" } else { "rw" }, "nodev", "nosuid", "noexec"].map(str::to_string);
        mount::mount(device, at, filesystem, &options)?;
        Ok(Self(Some(at.into())))
    }

    fn close(&mut self) -> Result<()> {
        if let Some(at) = &self.0 { mount::umount(at)?; }
        self.0 = None;
        Ok(())
    }
}

impl Drop for Mounted {
    fn drop(&mut self) { let _ = self.close(); }
}

fn needs_init(manifest: &Manifest, name: &str) -> Result<bool> {
    Ok(name == "system" && vendor_source(manifest)?.is_some())
}

pub(super) fn prepare(manifest: &Manifest, name: &str, package: &Path, offset: u64, length: u64) -> Result<Option<Prepared>> {
    let init = needs_init(manifest, name)?;
    if name != "mi_ext" && !init { return Ok(None); }
    source(package, offset, length, |source| from_tree(source, None, init))
}

fn source<T>(package: &Path, offset: u64, length: u64, read: impl FnOnce(&Path) -> Result<T>) -> Result<Option<T>> {
    let mut magic = [0; 4];
    File::open(package)?.read_exact_at(&mut magic, offset + 1024)?;
    let filesystem = if magic == 0xe0f5_e1e2u32.to_le_bytes() { "erofs" } else {
        let mut magic = [0; 2];
        File::open(package)?.read_exact_at(&mut magic, offset + 1080)?;
        ensure!(magic == [0x53, 0xef], "the Android compatibility image is neither EROFS nor ext4");
        "ext4"
    };

    fs::create_dir_all("/skyward/run")?;
    fs::create_dir_all("/data/gsi")?;
    let work = tempfile::Builder::new().prefix("android-mi-ext-").tempdir_in("/skyward/run")?;
    let source = work.path().join("source");
    fs::create_dir(&source)?;
    let source_loop = loopdev::scoped(package, offset, length, true)?;
    let mut source_mount = Mounted::open(&source_loop.path, &source, filesystem, true)?;
    let prepared = read(&source)?;
    source_mount.close()?;
    Ok(Some(prepared))
}

/// The current vendor's bind mount would cover the DSU /mi_ext with the
/// main system's image. Use the same vendor files in the DSU, changing only
/// that mount direction. The physical vendor partition is not written.
fn vendor_source(manifest: &Manifest) -> Result<Option<(PathBuf, String)>> {
    if !manifest.images.contains_key("mi_ext") || manifest.images.contains_key("vendor") { return Ok(None); }
    let source = fs::canonicalize("/vendor").context("locate the installed vendor filesystem")?;
    let fstab = fs::read_to_string(source.join("etc/fstab.qcom"))?;
    Ok(fstab_for_dsu(&fstab).map(|fstab| (source, fstab)))
}

pub(super) fn extra_space(package: &Path, manifest: &Manifest) -> Result<u64> {
    let mut bytes = match vendor_source(manifest)? { Some((source, _)) => image_size(space(&source)?) * 2, None => 0 };
    let mut zip = zip::ZipArchive::new(File::open(package)?)?;
    for (name, image) in &manifest.images {
        let init = needs_init(manifest, name)?;
        if name != "mi_ext" && !init { continue; }
        let offset = zip.by_name(&format!("{name}.img"))?.data_start().context("the image has no data offset")?;
        let size = source(package, offset, image.size, |at| prepared_size(at, init))?.context("the compatibility image could not be read")?;
        // The original raw-image space is already included by the caller.
        // Reserve its expansion and a temporary conversion file as well.
        bytes += size + size.saturating_sub(image.size);
    }
    Ok(bytes)
}

pub(super) fn vendor(manifest: &Manifest) -> Result<Option<Prepared>> {
    let Some((source, fstab)) = vendor_source(manifest)? else { return Ok(None) };
    Ok(Some(from_tree(&source, Some(&fstab), false)?))
}

fn fstab_for_dsu(text: &str) -> Option<String> {
    let mut changed = false;
    let mut result = String::new();
    for line in text.lines() {
        let mut fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() >= 5 && fields[0] == "mi_ext" && fields[1] == "/mnt/vendor/mi_ext" {
            fields[1] = "/mi_ext";
            changed = true;
            result.push_str(&fields.join("\t"));
        } else if fields.len() >= 5 && fields[0] == "/mnt/vendor/mi_ext" && fields[1] == "/mi_ext" && fields[3].split(',').any(|flag| flag == "bind") {
            fields[0] = "/mi_ext";
            fields[1] = "/mnt/vendor/mi_ext";
            changed = true;
            result.push_str(&fields.join("\t"));
        } else { result.push_str(line); }
        result.push('\n');
    }
    changed.then_some(result)
}

fn image_size(expanded: u64) -> u64 {
    ((expanded + expanded / 4 + (4 << 20)).max(8 << 20) + ((1 << 20) - 1)) & !((1 << 20) - 1)
}

fn prepared_size(source: &Path, init: bool) -> Result<u64> {
    let helper = if init { fs::metadata(Path::new("/").join(skyward_config::ramdisk::SYSTEM_INIT))?.len() } else { 0 };
    Ok(image_size(space(source)? + helper))
}

fn from_tree(source: &Path, fstab: Option<&str>, init: bool) -> Result<Prepared> {
    fs::create_dir_all("/skyward/run")?;
    fs::create_dir_all("/data/gsi")?;
    let work = tempfile::Builder::new().prefix("android-image-").tempdir_in("/skyward/run")?;
    let target = work.path().join("target");
    let view = work.path().join("source");
    fs::create_dir(&target)?;
    fs::create_dir(&view)?;
    // A nonrecursive bind excludes firmware and other mounts the Recovery
    // may have placed below /vendor; copy the vendor filesystem itself.
    mount::bind(source, &view)?;
    let mut view_mount = Mounted(Some(view.clone()));
    let source = view.as_path();
    // Count expanded file sizes, including metadata room. Compressed input
    // length alone cannot determine how large the ext4 filesystem must be.
    let size = prepared_size(source, init)?;
    ensure!(size <= mount::space(Path::new("/data"))?.available / 2, "not enough room to prepare the Android compatibility image");
    // Children reopen this unnamed file through the held descriptor. No
    // staging path remains after an interrupted install; a reboot also
    // releases any loop mount held when the process was interrupted.
    let file = tempfile::tempfile_in("/data/gsi")?;
    file.set_len(size)?;
    let file_path = PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()));
    let args = ["-q", "-F", "-t", "ext4", "-b", "4096", "-I", "512", "-m", "0", "-O", "^has_journal", "-d"].map(str::to_string);
    let mut args = args.to_vec();
    args.extend([tools::path(source)?, tools::path(&file_path)?]);
    let made = tools::find("mke2fs")?.run(&args)?;
    ensure!(made.status == 0, "prepare ext4 Android image: {}", made.said);
    let target_loop = loopdev::scoped(&file_path, 0, size, false)?;
    let mut target_mount = Mounted::open(&target_loop.path, &target, "ext4", false)?;
    // Some recovery mke2fs builds do not copy extended attributes with -d.
    // Copy all of them explicitly, including SELinux and file capabilities,
    // after ownership changes that could clear capabilities.
    if !source.join("lost+found").exists() { fs::remove_dir(target.join("lost+found"))?; }
    if let Some(fstab) = fstab { fs::write(target.join("etc/fstab.qcom"), fstab)?; }
    if init {
        let original = target.join(ORIGINAL_INIT.trim_start_matches('/'));
        let entry = target.join("system/bin/init");
        ensure!(!original.exists(), "the Android package already has a private init entry");
        ensure!(fs::symlink_metadata(&entry)?.is_file(), "the Android package has no regular system init");
        fs::rename(&entry, &original)?;
        fs::copy(Path::new("/").join(skyward_config::ramdisk::SYSTEM_INIT), &entry)?;
        metadata(&source.join("system/bin/init"), &original)?;
    }
    metadata(source, &target)?;
    target_mount.close()?;
    drop(target_loop);
    view_mount.close()?;
    file.sync_all()?;
    Ok(Prepared { file, size })
}

fn space(path: &Path) -> Result<u64> {
    space_on(path, fs::symlink_metadata(path)?.dev())
}

fn space_on(path: &Path, device: u64) -> Result<u64> {
    let stat = fs::symlink_metadata(path)?;
    if stat.dev() != device { return Ok(8192); }
    let mut bytes = stat.len().checked_add(8192).context("mi_ext size overflow")?;
    if stat.is_dir() {
        for entry in fs::read_dir(path)? {
            bytes = bytes.checked_add(space_on(&entry?.path(), device)?).context("Android image size overflow")?;
        }
    }
    ensure!(bytes <= 128 << 30, "expanded mi_ext is too large");
    Ok(bytes)
}

fn checked(result: libc::c_int, action: &str, path: &Path) -> Result<()> {
    if result < 0 { return Err(io::Error::last_os_error()).with_context(|| format!("{action} {}", path.display())); }
    Ok(())
}

fn metadata(from: &Path, to: &Path) -> Result<()> {
    let stat = fs::symlink_metadata(from)?;
    let source = CString::new(from.as_os_str().as_bytes())?;
    let target = CString::new(to.as_os_str().as_bytes())?;
    checked(unsafe { libc::lchown(target.as_ptr(), stat.uid(), stat.gid()) }, "copy ownership to", to)?;
    if !stat.is_symlink() {
        checked(unsafe { libc::chmod(target.as_ptr(), stat.mode() & 0o7777) }, "copy permissions to", to)?;
    }
    let size = unsafe { libc::llistxattr(source.as_ptr(), std::ptr::null_mut(), 0) };
    ensure!(size >= 0, "list attributes of {}: {}", from.display(), io::Error::last_os_error());
    let mut names = vec![0u8; size as usize];
    ensure!(unsafe { libc::llistxattr(source.as_ptr(), names.as_mut_ptr().cast(), names.len()) } == size, "read attributes of {}", from.display());
    for name in names.split(|b| *b == 0).filter(|name| !name.is_empty()) {
        let name = CString::new(name)?;
        let size = unsafe { libc::lgetxattr(source.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        ensure!(size >= 0, "read attribute size of {}", from.display());
        let mut value = vec![0u8; size as usize];
        ensure!(unsafe { libc::lgetxattr(source.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len()) } == size, "read attribute of {}", from.display());
        checked(unsafe { libc::lsetxattr(target.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0) }, "copy attributes to", to)?;
    }
    if stat.is_dir() {
        for item in fs::read_dir(from)? {
            let item = item?;
            metadata(&item.path(), &to.join(item.file_name()))?;
        }
    }
    let times = [libc::timespec { tv_sec: stat.atime(), tv_nsec: stat.atime_nsec() }, libc::timespec { tv_sec: stat.mtime(), tv_nsec: stat.mtime_nsec() }];
    checked(unsafe { libc::utimensat(libc::AT_FDCWD, target.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) }, "copy times to", to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn the_dsu_vendor_keeps_mi_ext_and_its_overlays_on_the_second_system() {
        let original = "# unchanged header\nmi_ext /mnt/vendor/mi_ext ext4 ro wait,slotselect,logical,first_stage_mount,nofail\nmi_ext /mnt/vendor/mi_ext erofs ro wait,slotselect,logical,first_stage_mount,nofail\n/mnt/vendor/mi_ext /mi_ext erofs ro,bind wait,nofail\noverlay /product/app overlay ro,lowerdir=/mnt/vendor/mi_ext/product/app:/product/app check,nofail\n";
        let fixed = fstab_for_dsu(original).unwrap();
        let entries: Vec<Vec<_>> = fixed.lines().filter(|line| !line.starts_with('#')).map(|line| line.split_whitespace().collect()).collect();
        assert_eq!(&entries[0][..3], ["mi_ext", "/mi_ext", "ext4"]);
        assert_eq!(&entries[1][..3], ["mi_ext", "/mi_ext", "erofs"]);
        assert_eq!(&entries[2][..2], ["/mi_ext", "/mnt/vendor/mi_ext"]);
        assert!(fixed.contains("lowerdir=/mnt/vendor/mi_ext/product/app:/product/app"));
        assert!(fstab_for_dsu(&fixed).is_none());
    }

    #[test]
    #[ignore = "requires root and TMPDIR on a filesystem supporting user extended attributes"]
    fn converted_files_keep_binary_attributes_ownership_links_and_directory_times() {
        let work = tempfile::tempdir().unwrap();
        let (source, target) = (work.path().join("source"), work.path().join("target"));
        for at in [&source, &target] {
            fs::create_dir(at).unwrap();
            fs::write(at.join("file"), b"official content").unwrap();
            symlink("file", at.join("link")).unwrap();
        }
        fs::set_permissions(source.join("file"), fs::Permissions::from_mode(0o640)).unwrap();
        let file = CString::new(source.join("file").as_os_str().as_bytes()).unwrap();
        let attribute = CString::new("user.skyward").unwrap();
        let value = b"binary\0attribute\xff";
        checked(unsafe { libc::lchown(file.as_ptr(), 12345, 23456) }, "test owner", &source).unwrap();
        checked(unsafe { libc::lsetxattr(file.as_ptr(), attribute.as_ptr(), value.as_ptr().cast(), value.len(), 0) }, "test attribute", &source).unwrap();
        metadata(&source, &target).unwrap();
        let (before, after) = (fs::symlink_metadata(source.join("file")).unwrap(), fs::symlink_metadata(target.join("file")).unwrap());
        assert_eq!((after.uid(), after.gid(), after.mode(), after.mtime(), after.mtime_nsec()), (before.uid(), before.gid(), before.mode(), before.mtime(), before.mtime_nsec()));
        let dest = CString::new(target.join("file").as_os_str().as_bytes()).unwrap();
        let mut copied = [0; 64];
        let length = unsafe { libc::lgetxattr(dest.as_ptr(), attribute.as_ptr(), copied.as_mut_ptr().cast(), copied.len()) };
        assert_eq!(length, value.len() as isize);
        assert_eq!(&copied[..length as usize], value);
        assert_eq!(fs::read_link(target.join("link")).unwrap(), Path::new("file"));
        assert_eq!(fs::metadata(target).unwrap().mtime_nsec(), fs::metadata(source).unwrap().mtime_nsec());
    }
}
