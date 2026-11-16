//! Making a block device out of part of a file, which is how the bundles a
//! phone keeps its shared libraries in are opened.

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_os = "android")]
mod android {
    use anyhow::{Context, Result};
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};

    const CONTROL: &str = "/dev/loop-control";

    const GET_FREE: u32 = 0x4C82;
    const CONFIGURE: u32 = 0x4C0A;

    /// The device only ever reads. It stays pointed at the file until the
    /// recovery ends, which is as long as anything mounted from it lasts.
    const READ_ONLY: u32 = 1;
    const AUTO_CLEAR: u32 = 4;
    const DIRECT_IO: u32 = 16;

    /// Every field is a plain number or a run of bytes, so the request is
    /// made by zeroing it and filling in what matters.
    #[repr(C)]
    struct Info {
        device: u64,
        inode: u64,
        rdevice: u64,
        offset: u64,
        size_limit: u64,
        number: u32,
        encrypt_type: u32,
        encrypt_key_size: u32,
        flags: u32,
        file_name: [u8; 64],
        crypt_name: [u8; 64],
        encrypt_key: [u8; 32],
        init: [u64; 2],
    }

    #[repr(C)]
    struct Config {
        fd: u32,
        block_size: u32,
        info: Info,
        reserved: [u64; 8],
    }

    /// Open `file` from `offset` for `length` bytes as a block device, and
    /// give its path. The device goes away once whatever is mounted from it
    /// is unmounted.
    pub fn attach(file: &Path, offset: u64, length: u64) -> Result<PathBuf> {
        let backing = File::open(file).with_context(|| format!("open {}", file.display()))?;
        let control = File::open(CONTROL).with_context(|| format!("open {CONTROL}"))?;
        let number = unsafe { libc::ioctl(control.as_raw_fd(), GET_FREE as libc::Ioctl) };
        if number < 0 {
            return Err(std::io::Error::last_os_error()).context("ask for a free loop device");
        }
        let path = free_device(number as u32)?;
        let device = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut config: Config = unsafe { std::mem::zeroed() };
        config.fd = backing.as_raw_fd() as u32;
        config.info.offset = offset;
        config.info.size_limit = length;
        config.info.flags = READ_ONLY;
        if unsafe { libc::ioctl(device.as_raw_fd(), CONFIGURE as libc::Ioctl, &mut config) } < 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("point {} at {}", path.display(), file.display()));
        }
        Ok(path)
    }

    /// A writable, direct-I/O view of a raw partition. An unencrypted
    /// filesystem can hold that partition exclusively, preventing dm-linear
    /// from claiming it directly. The loop keeps raw offsets unchanged.
    /// Retain this handle until the dm table has opened it; autoclear then
    /// detaches it after the last mapped user closes it.
    pub struct Writable {
        pub path: PathBuf,
        _device: File,
    }

    /// A bounded file view kept until its mount has been released. Closing
    /// the last user automatically detaches it, including on failed work.
    pub struct Scoped {
        pub path: PathBuf,
        _device: File,
    }

    pub fn scoped(file: &Path, offset: u64, length: u64, readonly: bool) -> Result<Scoped> {
        let backing = File::options().read(true).write(!readonly).open(file)?;
        let control = File::open(CONTROL)?;
        let number = unsafe { libc::ioctl(control.as_raw_fd(), GET_FREE as libc::Ioctl) };
        if number < 0 {
            return Err(std::io::Error::last_os_error()).context("ask for a temporary loop device");
        }
        let path = free_device(number as u32)?;
        let device = File::options().read(true).write(true).open(&path)?;
        let mut config: Config = unsafe { std::mem::zeroed() };
        config.fd = backing.as_raw_fd() as u32;
        config.info.offset = offset;
        config.info.size_limit = length;
        config.info.flags = AUTO_CLEAR | if readonly { READ_ONLY } else { 0 };
        if unsafe { libc::ioctl(device.as_raw_fd(), CONFIGURE as libc::Ioctl, &mut config) } < 0 {
            return Err(std::io::Error::last_os_error()).context("make a bounded temporary loop view");
        }
        Ok(Scoped { path, _device: device })
    }

    pub fn writable(file: &Path) -> Result<Writable> {
        let backing = File::options().read(true).write(true).open(file).with_context(|| format!("open {}", file.display()))?;
        let control = File::open(CONTROL).with_context(|| format!("open {CONTROL}"))?;
        let number = unsafe { libc::ioctl(control.as_raw_fd(), GET_FREE as libc::Ioctl) };
        if number < 0 { return Err(std::io::Error::last_os_error()).context("ask for a writable loop device"); }
        let path = free_device(number as u32)?;
        let device = File::options().read(true).write(true).open(&path)?;
        let mut config: Config = unsafe { std::mem::zeroed() };
        config.fd = backing.as_raw_fd() as u32;
        config.info.flags = AUTO_CLEAR | DIRECT_IO;
        if unsafe { libc::ioctl(device.as_raw_fd(), CONFIGURE as libc::Ioctl, &mut config) } < 0 {
            return Err(std::io::Error::last_os_error()).context("make a writable raw-partition loop view");
        }
        Ok(Writable { path, _device: device })
    }

    /// Where the kernel put the loop device it just handed out: Android
    /// keeps its block devices in their own directory.
    fn free_device(number: u32) -> Result<PathBuf> {
        for dir in ["/dev/block", "/dev"] {
            let path = Path::new(dir).join(format!("loop{number}"));
            if path.exists() {
                return Ok(path);
            }
        }
        anyhow::bail!("the kernel handed out loop device {number}, which is in neither /dev/block nor /dev")
    }
}
