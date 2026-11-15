//! Compatibility for an explicitly pinned, locally supplied recovery base.

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use skyward_bootimg::{BootImage, Ramdisk};
use skyward_config::{ramdisk, DeviceConfig, RecoveryFormat};

/// Check the complete donor before parsing or changing any of it.
pub fn verify(cfg: &DeviceConfig, bytes: &[u8]) -> Result<()> {
    if let Some(base) = &cfg.recovery {
        let sum = hex::encode(Sha256::digest(bytes));
        ensure!(sum == base.sha256, "the recovery base has SHA-256 {sum}; [recovery] requires {}", base.sha256);
        match base.format {
            RecoveryFormat::Stock => {}
            RecoveryFormat::Orangefox => {
                ensure!(cfg.device.codename == "dodge", "OrangeFox base compatibility is implemented only for dodge");
                ensure!(bytes.len() == 104_857_600, "dodge's recovery partition is 104857600 bytes");
            }
        }
    }
    Ok(())
}

pub fn prepare(cfg: &DeviceConfig, image: &BootImage, rd: &mut Ramdisk) -> Result<()> {
    let custom = rd.entries.iter().any(|e| e.name == "FFiles" || e.name.starts_with("twres"));
    let Some(base) = &cfg.recovery else {
        ensure!(!custom, "this is a custom recovery; its compatibility and SHA-256 must be configured in [recovery]");
        return Ok(());
    };
    if let Some(header) = &base.binder_header {
        let mut props = rd.find(ramdisk::PROP_DEFAULT).context("the base has no prop.default")?.data.clone();
        props.extend_from_slice(format!("\nro.skyward.binder.header={header}\n").as_bytes());
        rd.replace(ramdisk::PROP_DEFAULT, props)?;
    }
    match base.format {
        RecoveryFormat::Stock => {
            ensure!(!custom, "the configured stock base contains a custom recovery ramdisk");
            let props = &rd.find(ramdisk::PROP_DEFAULT).context("the base has no prop.default")?.data;
            let device = format!("ro.product.system.device={}", cfg.device.codename);
            ensure!(props.split(|b| *b == b'\n').any(|l| l == device.as_bytes()), "the stock base's product is not {}", cfg.device.codename);
            // Android 12 stock recoveries do not carry Binder's service
            // directory. Supply only its init declaration here; Skyward
            // takes the binary and runtime from the installed system in RAM.
            if rd.find("system/bin/servicemanager").is_none() {
                rd.add("system/etc/init/skyward-servicemanager.rc", 0o644, include_bytes!("servicemanager.rc").to_vec())?;
                rd.add("skyward/installed-servicemanager", 0o644, Vec::new())?;
            }
        }
        RecoveryFormat::Orangefox => {
            ensure!(custom, "the configured OrangeFox base has no OrangeFox ramdisk");
            ensure!(image.header.header_version() == 4 && image.kernel.is_empty(), "dodge requires a v4 recovery image without a kernel");
            let props = &rd.find(ramdisk::PROP_DEFAULT).context("the base has no prop.default")?.data;
            ensure!(props.split(|b| *b == b'\n').any(|l| l == b"ro.product.system.device=dodge"), "the base's product is not dodge");
            // This donor's libbinder rejects RECO parcels. Preserve its
            // library and have Skyward use the SYST mark it was built with.
            let mut props = props.clone();
            if !props.ends_with(b"\n") {
                props.push(b'\n');
            }
            props.extend_from_slice(b"ro.skyward.binder.header=SYST\n");
            rd.replace(ramdisk::PROP_DEFAULT, props)?;

            // init has already read these before Skyward mounts the installed
            // vendor over the ramdisk's. Keeping their definitions would run
            // the donor's HALs before adopting the installed patch levels and
            // compete with the services Skyward discovers on the phone.
            rd.entries.retain(|e| {
                let name = e.name.as_str();
                !(name == "FFiles" || name.starts_with("FFiles/") || name == "twres" || name.starts_with("twres/")
                    || (name.starts_with("vendor/etc/init/") || name.starts_with("vendor/odm/etc/init/")) && name.ends_with(".rc")
                    || name.starts_with("system/etc/init/") && name.ends_with(".rc")
                        && name != "system/etc/init/hw/init.rc" && name != "system/etc/init/servicemanager.rc"
                    || ramdisk::COMMANDS.contains(&name))
            });
            rd.replace("init.recovery.qcom.rc", include_bytes!("dodge.rc").to_vec())?;
            // The donor's MTP actions bind USB before Skyward has supplied
            // its descriptors and duplicate the functionfs mounts. The
            // common init.rc and Skyward's mtp.rc own those actions now.
            rd.replace("init.recovery.usb.rc", include_bytes!("dodge-usb.rc").to_vec())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> BootImage {
        let mut header = vec![0u8; 4096];
        header[..8].copy_from_slice(b"ANDROID!");
        header[20..24].copy_from_slice(&1584u32.to_le_bytes());
        header[40..44].copy_from_slice(&4u32.to_le_bytes());
        BootImage::parse(&header).unwrap()
    }

    #[test]
    fn a_donor_is_refused_when_it_does_not_match_the_pinned_hash() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/dodge.toml")).unwrap();
        assert!(verify(&cfg, b"a different recovery").unwrap_err().to_string().contains("SHA-256"));
    }

    #[test]
    fn an_unconfigured_custom_recovery_is_still_refused() {
        let cfg = DeviceConfig::parse("[device]\ncodename = 'dodge'\n").unwrap();
        let mut rd = Ramdisk { entries: Vec::new() };
        rd.add("twres/ui.xml", 0o644, Vec::new()).unwrap();
        assert!(prepare(&cfg, &image(), &mut rd).unwrap_err().to_string().contains("custom recovery"));
    }

    #[test]
    fn an_older_stock_base_gets_a_service_directory_declaration_without_foreign_binaries() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/zizhan.toml")).unwrap();
        let mut rd = Ramdisk { entries: Vec::new() };
        rd.add(ramdisk::PROP_DEFAULT, 0o644, b"ro.product.system.device=zizhan\n".to_vec()).unwrap();
        rd.add("system/bin/init", 0o755, b"stock init".to_vec()).unwrap();
        prepare(&cfg, &image(), &mut rd).unwrap();
        assert_eq!(rd.find("system/bin/init").unwrap().data, b"stock init");
        assert!(rd.find("system/bin/servicemanager").is_none());
        assert!(rd.find("skyward/installed-servicemanager").is_some());
        assert!(rd.find("system/etc/init/skyward-servicemanager.rc").is_some());
        assert!(rd.find(ramdisk::PROP_DEFAULT).unwrap().data.ends_with(b"ro.skyward.binder.header=SYST\n"));
    }

    #[test]
    fn a_stock_base_of_another_device_is_refused() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/zizhan.toml")).unwrap();
        let mut rd = Ramdisk { entries: Vec::new() };
        rd.add(ramdisk::PROP_DEFAULT, 0o644, b"ro.product.system.device=another\n".to_vec()).unwrap();
        assert!(prepare(&cfg, &image(), &mut rd).unwrap_err().to_string().contains("product is not zizhan"));
    }

    #[test]
    fn the_donor_keeps_its_runtime_and_leaves_service_startup_to_skyward() {
        let cfg = DeviceConfig::parse(include_str!("../../../devices/dodge.toml")).unwrap();
        let mut rd = Ramdisk { entries: Vec::new() };
        for name in ["FFiles/installer.zip", "twres/ui.xml", "vendor/etc/init/keymint.rc", "vendor/odm/etc/init/weaver.rc",
            "system/etc/init/keystore2.rc", "system/etc/init/android.hardware.boot@1.2-service.rc", "system/bin/resetprop"] {
            rd.add(name, 0o644, b"old".to_vec()).unwrap();
        }
        for name in ["system/etc/init/hw/init.rc", "system/etc/init/servicemanager.rc", "system/bin/init", "system/bin/adbd", "system/lib64/libc.so", "sepolicy"] {
            rd.add(name, 0o644, b"kept".to_vec()).unwrap();
        }
        rd.add(ramdisk::PROP_DEFAULT, 0o644, b"ro.product.system.device=dodge\n".to_vec()).unwrap();
        rd.add("init.recovery.qcom.rc", 0o750, b"start vendor.keymint\n".to_vec()).unwrap();
        rd.add("init.recovery.usb.rc", 0o750, b"old USB actions\n".to_vec()).unwrap();
        prepare(&cfg, &image(), &mut rd).unwrap();
        assert!(String::from_utf8_lossy(&rd.find(ramdisk::PROP_DEFAULT).unwrap().data).contains("ro.skyward.binder.header=SYST\n"));
        assert!(rd.find("system/bin/resetprop").is_none());
        assert!(!rd.entries.iter().any(|e| e.name.starts_with("FFiles") || e.name.starts_with("twres")));
        assert!(rd.find("vendor/etc/init/keymint.rc").is_none());
        assert!(rd.find("vendor/odm/etc/init/weaver.rc").is_none());
        assert!(rd.find("system/etc/init/keystore2.rc").is_none());
        assert!(rd.find("system/etc/init/android.hardware.boot@1.2-service.rc").is_none());
        for name in ["system/etc/init/hw/init.rc", "system/etc/init/servicemanager.rc", "system/bin/init", "system/bin/adbd", "system/lib64/libc.so", "sepolicy"] {
            assert_eq!(rd.find(name).unwrap().data, b"kept");
        }
        let board = String::from_utf8_lossy(&rd.find("init.recovery.qcom.rc").unwrap().data);
        assert!(board.contains("ro.boot.usbcontroller") && board.contains("skyward.modules.ready"));
        assert!(board.contains("setprop sys.usb.config_disable 0"), "MTP must be able to restart adbd after switching through none");
        assert!(!board.contains("prepdecrypt") && !board.contains("start vendor."));
        let usb = String::from_utf8_lossy(&rd.find("init.recovery.usb.rc").unwrap().data);
        assert!(!usb.contains("mount functionfs") && !usb.contains("sys.usb.config=mtp,adb"));
    }
}
