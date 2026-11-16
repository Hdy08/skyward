//! The init_boot image the other system starts from: the bootloader joins
//! its ramdisk to the installed system's vendor ramdisk, and the program
//! Skyward puts in it finds the system's root in the data partition and
//! starts the system's own first program from it.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use skyward_block::area::Area;
use skyward_bootimg::{lz4_legacy, BootImage, Ramdisk};
use std::path::Path;

/// Where this phone's generic first-stage ramdisk lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Separate,
    InBoot,
}

impl Layout {
    /// Both slots must use the same layout. A partial init_boot pair is
    /// not a phone whose boot ramdisk can safely be assumed instead.
    pub fn detect(by_name: &Path) -> Result<Option<Self>> {
        let pair = |name: &str| -> Result<[bool; 2]> {
            Ok([by_name.join(format!("{name}_a")).try_exists()?, by_name.join(format!("{name}_b")).try_exists()?])
        };
        if pair("boot")? != [true, true] {
            return Ok(None);
        }
        Ok(match pair("init_boot")? {
            [true, true] => Some(Self::Separate),
            [false, false] => Some(Self::InBoot),
            _ => None,
        })
    }

    pub fn ramdisk_partition(self) -> &'static str {
        match self {
            Self::Separate => "init_boot",
            Self::InBoot => "boot",
        }
    }
}

/// Where the program finds what to start, in the ramdisk.
pub const START: &str = "skyward/system.toml";
/// The program, as the system's first program where the system brings no
/// ramdisk of its own.
pub const INIT: &str = "init";
/// The program, where the system brings its own ramdisk: it lays the root
/// out as a device and says where, for that ramdisk's own first program.
pub const MAP: &str = "skyward/map";

/// What the program reads of the system it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Start {
    /// The system's id, which the program names when the start stops.
    pub id: String,
    pub filesystem: String,
    pub init: String,
    pub root: Area,
}

impl Start {
    pub fn parse(text: &str) -> Result<Start> {
        toml::from_str(text).context("read what the system starts with")
    }
}

/// Skyward's archive in an init_boot image: `program` at `at`, and `start`
/// beside it, compressed as the vendor ramdisk before it is, for the kernel
/// to read on into it from there.
fn archive(at: &str, program: &[u8], start: &Start) -> Result<Vec<u8>> {
    let mut rd = Ramdisk { entries: Vec::new() };
    rd.add(at, 0o750, program.to_vec())?;
    rd.add(START, 0o644, toml::to_string(start).context("write what the system starts with")?.into_bytes())?;
    Ok(lz4_legacy(&rd.archive()?))
}

/// The init_boot image that starts the system `start` describes: the
/// header of `template`, the phone's own init_boot, and a ramdisk holding
/// `program` as the first program with `start` beside it.
pub fn init_boot(template: &[u8], program: &[u8], start: &Start) -> Result<Vec<u8>> {
    let mut image = BootImage::parse(template).context("read the phone's init_boot image")?;
    ensure!(image.kernel.is_empty(), "the phone's init_boot image holds a kernel");
    image.ramdisk = archive(INIT, program, start)?;
    image.to_bytes()
}

/// Android 12-era phones such as zizhan keep the generic ramdisk beside
/// the kernel in boot. Keep the package's kernel and header, replacing
/// its ramdisk, or taking the package's explicit init_boot ramdisk.
pub fn in_boot(template: &[u8], own: Option<&[u8]>, program: &[u8], start: &Start) -> Result<(Vec<u8>, Option<u64>)> {
    let mut image = BootImage::parse(template).context("read the second system's boot image")?;
    ensure!(!image.kernel.is_empty(), "the second system's boot image has no kernel");
    let own_ramdisk = match own {
        Some(own) => {
            let own = BootImage::parse(own).context("read the second system's init_boot image")?;
            ensure!(own.kernel.is_empty(), "the second system's init_boot image holds a kernel");
            image.ramdisk = own.ramdisk;
            let bytes = image.ramdisk.len() as u64;
            image.ramdisk.extend_from_slice(&archive(MAP, program, start)?);
            Some(bytes)
        }
        None => {
            image.ramdisk = archive(INIT, program, start)?;
            None
        }
    };
    Ok((image.to_bytes()?, own_ramdisk))
}

/// The system's own init_boot image `own` with the program as [`MAP`] and
/// `start` beside it, in an archive after its ramdisk's own; and the bytes
/// of that ramdisk of its own.
pub fn with_start(own: &[u8], program: &[u8], start: &Start) -> Result<(Vec<u8>, u64)> {
    let mut image = BootImage::parse(own).context("read the system's init_boot image")?;
    let own_ramdisk = image.ramdisk.len() as u64;
    image.ramdisk.extend_from_slice(&archive(MAP, program, start)?);
    Ok((image.to_bytes()?, own_ramdisk))
}

/// What an LZ4 archive in the legacy framing opens with, as Skyward's
/// archive does.
const LZ4_LEGACY: [u8; 4] = [0x02, 0x21, 0x4c, 0x18];

/// Whether `image` is an init_boot image that starts a system from the
/// data partition: one whose ramdisk ends in Skyward's archive, which says
/// in [`START`] what to start. The archive comes last in the ramdisk,
/// after the system's own where it brings one, so it is looked for from
/// each place an archive may open, the last first. Anything else an
/// init_boot partition may hold, an image that does not read among it, is
/// none of Skyward's.
pub fn starts_from_data(image: &[u8]) -> bool {
    let Ok(image) = BootImage::parse(image) else { return false };
    let mut before = image.ramdisk.len();
    while let Some(at) = image.ramdisk[..before].windows(LZ4_LEGACY.len()).rposition(|opening| opening == LZ4_LEGACY) {
        if Ramdisk::decode(&image.ramdisk[at..]).is_ok_and(|archive| archive.find(START).is_some()) {
            return true;
        }
        before = at;
    }
    false
}

/// The init_boot image `image`, as [`init_boot`] made it or, for a system
/// whose own ramdisk of `own_ramdisk` bytes comes first in it, as
/// [`with_start`] did, made again with `program` and `start`.
pub fn with_program(image: &[u8], own_ramdisk: Option<u64>, program: &[u8], start: &Start) -> Result<Vec<u8>> {
    let mut image = BootImage::parse(image).context("read the other system's init_boot image")?;
    let (kept, at) = match own_ramdisk {
        Some(bytes) => (bytes, MAP),
        None => (0, INIT),
    };
    ensure!(kept <= image.ramdisk.len() as u64, "the other system's own ramdisk of {kept} bytes is longer than the {} its init_boot image holds", image.ramdisk.len());
    image.ramdisk.truncate(kept as usize);
    image.ramdisk.extend_from_slice(&archive(at, program, start)?);
    image.to_bytes()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::tests::area;
    use abootimg_oxide::{HeaderV3, OsVersionPatch};

    /// A version 4 init_boot image with `ramdisk` and no kernel, filling a
    /// partition of a MiB.
    pub fn template(ramdisk: &[u8]) -> Vec<u8> {
        let header = HeaderV3 {
            kernel_size: 0,
            ramdisk_size: ramdisk.len() as u32,
            osversionpatch: OsVersionPatch(0),
            cmdline: Box::new([0; 1536]),
            v4_signature_size: Some(0),
        };
        let mut image = BootImage { header, kernel: Vec::new(), ramdisk: ramdisk.to_vec() }.to_bytes().unwrap();
        image.resize(1 << 20, 0);
        image
    }

    /// The entries of Skyward's archive `bytes`, by name, with their data.
    pub fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert_eq!(bytes[..4], [0x02, 0x21, 0x4c, 0x18], "LZ4 in the legacy framing, as the vendor ramdisk is");
        Ramdisk::decode(bytes).unwrap().entries.into_iter().map(|e| (e.name, e.data)).collect()
    }

    pub fn start() -> Start {
        Start { id: "test".into(), filesystem: "ext4".into(), init: "/sbin/init".into(), root: area("/data/skyward/systems/test", 1 << 20) }
    }

    pub fn kernel_image(version: u32) -> Vec<u8> {
        let mut image = BootImage::parse(&template(b"the original Android ramdisk")).unwrap();
        image.kernel = b"the Linux kernel".to_vec();
        image.header.cmdline[..10].copy_from_slice(b"test=true\0");
        if version == 3 {
            image.header.v4_signature_size = None;
        }
        image.to_bytes().unwrap()
    }

    #[test]
    fn a_matching_pair_of_boot_partitions_can_keep_the_generic_ramdisk() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Layout::detect(dir.path()).unwrap(), None);
        std::fs::write(dir.path().join("boot_a"), []).unwrap();
        assert_eq!(Layout::detect(dir.path()).unwrap(), None);
        std::fs::write(dir.path().join("boot_b"), []).unwrap();
        assert_eq!(Layout::detect(dir.path()).unwrap(), Some(Layout::InBoot));
        std::fs::write(dir.path().join("init_boot_a"), []).unwrap();
        assert_eq!(Layout::detect(dir.path()).unwrap(), None, "a partial init_boot pair is refused");
        std::fs::write(dir.path().join("init_boot_b"), []).unwrap();
        assert_eq!(Layout::detect(dir.path()).unwrap(), Some(Layout::Separate));
    }

    #[test]
    fn an_embedded_start_keeps_the_kernel_header_and_command_line() {
        for version in [3, 4] {
            let source = kernel_image(version);
            let (made, own) = in_boot(&source, None, b"first program", &start()).unwrap();
            let image = BootImage::parse(&made).unwrap();
            assert_eq!(image.header.header_version(), version);
            assert_eq!(image.kernel, b"the Linux kernel");
            assert_eq!(&image.header.cmdline[..10], b"test=true\0");
            assert_eq!(own, None);
            assert_eq!(entries(&image.ramdisk)[0], (INIT.into(), b"first program".to_vec()));
            assert!(starts_from_data(&made));
            let updated = with_program(&made, None, b"updated program", &start()).unwrap();
            let image = BootImage::parse(&updated).unwrap();
            assert_eq!(image.kernel, b"the Linux kernel");
            assert_eq!(entries(&image.ramdisk)[0].1, b"updated program");
        }
        assert!(in_boot(&template(b"ramdisk only"), None, b"program", &start()).is_err());
    }

    #[test]
    fn an_own_first_ramdisk_can_be_carried_in_boot_without_its_own_partition() {
        let own = template(b"the system's own first-stage archive");
        let (made, kept) = in_boot(&kernel_image(3), Some(&own), b"map program", &start()).unwrap();
        let image = BootImage::parse(&made).unwrap();
        let kept = kept.unwrap() as usize;
        assert_eq!(&image.ramdisk[..kept], b"the system's own first-stage archive");
        assert_eq!(entries(&image.ramdisk[kept..])[1], (MAP.into(), b"map program".to_vec()));
        let again = with_program(&made, Some(kept as u64), b"new map", &start()).unwrap();
        let image = BootImage::parse(&again).unwrap();
        assert_eq!(&image.ramdisk[..kept], b"the system's own first-stage archive");
        assert_eq!(entries(&image.ramdisk[kept..])[1].1, b"new map");
        assert!(in_boot(&kernel_image(3), Some(&kernel_image(4)), b"map", &start()).is_err());
    }

    #[test]
    fn the_new_ramdisk_starts_the_program_with_what_it_starts_beside_it() {
        let image = init_boot(&template(b"stock"), b"program", &start()).unwrap();
        let back = BootImage::parse(&image).unwrap();
        assert_eq!(back.header.header_version(), 4);
        assert!(back.kernel.is_empty());
        let found = entries(&back.ramdisk);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [INIT, "skyward", START]);
        assert_eq!(found[0].1, b"program");
        assert_eq!(Start::parse(std::str::from_utf8(&found[2].1).unwrap()).unwrap(), start());
    }

    #[test]
    fn a_systems_own_ramdisk_keeps_its_own_first_program() {
        let own = template(b"its own archive");
        let (image, own_ramdisk) = with_start(&own, b"program", &start()).unwrap();
        assert_eq!(own_ramdisk, b"its own archive".len() as u64);
        let back = BootImage::parse(&image).unwrap();
        let (theirs, ours) = back.ramdisk.split_at(b"its own archive".len());
        assert_eq!(theirs, b"its own archive", "its archive first");
        let names: Vec<String> = entries(ours).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["skyward", MAP, START], "and no first program of Skyward's");
    }

    #[test]
    fn an_image_that_starts_a_system_from_the_data_partition_is_told_from_any_other() {
        let mut ours = init_boot(&template(b"stock"), b"program", &start()).unwrap();
        // As a partition holds it: zeros to the partition's end.
        ours.resize(1 << 20, 0);
        assert!(starts_from_data(&ours));
        // A system's own archive is an LZ4 archive too, and Skyward's
        // comes after it.
        let its_own = lz4_legacy(&Ramdisk { entries: Vec::new() }.archive().unwrap());
        let (after_its_own, _) = with_start(&template(&its_own), b"program", &start()).unwrap();
        assert!(starts_from_data(&after_its_own));
        assert!(!starts_from_data(&template(&its_own)), "the installed system's own image");
        assert!(!starts_from_data(&template(b"stock")), "a ramdisk that is no archive");
        assert!(!starts_from_data(&vec![0u8; 1 << 20]), "a partition erased");
        assert!(!starts_from_data(b""));
    }

    #[test]
    fn an_image_made_again_carries_the_new_program_and_nothing_of_the_old() {
        let mut made = init_boot(&template(b"stock"), b"old program", &start()).unwrap();
        // As the store and the partition hold it: zeros to the partition's end.
        made.resize(made.len() + 8192, 0);
        let again = BootImage::parse(&with_program(&made, None, b"new program", &start()).unwrap()).unwrap();
        assert_eq!((again.header.header_version(), again.kernel.len()), (4, 0), "still an init_boot image");
        let found = entries(&again.ramdisk);
        assert_eq!(found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), [INIT, "skyward", START]);
        assert_eq!(found[0].1, b"new program");
        assert_eq!(Start::parse(std::str::from_utf8(&found[2].1).unwrap()).unwrap(), start());
    }

    #[test]
    fn a_systems_own_ramdisk_is_kept_when_its_image_is_made_again() {
        let (made, own_ramdisk) = with_start(&template(b"its own archive"), b"old program", &start()).unwrap();
        let again = BootImage::parse(&with_program(&made, Some(own_ramdisk), b"new", &start()).unwrap()).unwrap();
        let (theirs, ours) = again.ramdisk.split_at(own_ramdisk as usize);
        assert_eq!(theirs, b"its own archive");
        let found = entries(ours);
        assert_eq!(found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["skyward", MAP, START]);
        assert_eq!(found[1].1, b"new");
        assert!(with_program(&made, Some(1 << 20), b"new", &start()).is_err(), "an own ramdisk longer than the image's");
    }
}
