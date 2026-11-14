//! Regression cases distilled from PJZ110_16.0.5.703(CN01)'s init and
//! VINTF declarations. Empty executable markers stand in for proprietary
//! binaries; the scripts and descriptions still go through the real reader.

use skyward_programs::naming::{self, Does, GATEKEEPER_LEVEL};
use skyward_programs::phone::{Partition, Phone};
use skyward_programs::secure::{self, Step};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn phone(at: &Path, level: Option<&str>) -> Phone {
    let scripts = [
        ("vendor", "contexthub", "//move to odm to control project\n//service vendor.contexthub-default /vendor/bin/hw/android.hardware.contexthub-service.qmi\n//    user context_hub\n"),
        ("vendor", "gatekeeper", "on init\n    exec u:r:vendor_init-qti-keymaster-sh:s0 -- /vendor/bin/init.qti.keymaster.sh\n\n\
            service vendor.gatekeeper_default /vendor/bin/hw/android.hardware.gatekeeper-service-qti\n    user system\n    group system\n    disabled\n\
            on property:vendor.gatekeeper.is_security_level_spu=0\n    enable vendor.gatekeeper_default\n\n\
            service vendor.gatekeeper_spu /vendor/bin/hw/android.hardware.gatekeeper-service-spu-qti\n    user system\n    group system\n    disabled\n\
            on property:vendor.gatekeeper.is_security_level_spu=1\n    enable vendor.gatekeeper_spu\n"),
        ("vendor", "keymint", "service vendor.keymint /vendor/bin/hw/android.hardware.security.keymint-service-qti\n    user system\n    group system drmrpc\n"),
        ("vendor", "boot", "service vendor.boot /vendor/bin/hw/android.hardware.boot-service.qti\n    user root\n    group root\n"),
        ("odm", "weaver-service", "service vendor.nxp_weaver /odm/bin/hw/android.hardware.weaver-service.nxp\n    user vendor_nxp_weaver\n    group system drmrpc\n    capabilities WAKE_ALARM\n"),
        ("odm", "secure_element-service", "service vendor.secure_element /odm/bin/hw/android.hardware.secure_element-service.qti\n    user system\n    group nfc system\n"),
    ];
    for (partition, name, text) in scripts {
        let dir = at.join(partition).join("etc/init");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.rc")), text).unwrap();
        for line in text.lines() {
            if let Some(line) = line.strip_prefix("service ") {
                let executable = line.split_whitespace().nth(1).unwrap();
                let file = at.join(executable.trim_start_matches('/'));
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, []).unwrap();
            }
        }
    }
    let mut manifest = String::from("<manifest version=\"1.0\" type=\"device\">");
    fs::write(at.join("vendor/bin/init.qti.keymaster.sh"), "#! /vendor/bin/sh\nsetprop vendor.gatekeeper.is_security_level_spu 0\n").unwrap();
    for (package, interface, instance) in [
        ("android.hardware.security.keymint", "IKeyMintDevice", "default"),
        ("android.hardware.gatekeeper", "IGatekeeper", "default"),
        ("android.hardware.boot", "IBootControl", "default"),
        ("android.hardware.weaver", "IWeaver", "default"),
        ("android.hardware.secure_element", "ISecureElement", "eSE1"),
        ("android.se.omapi", "ISecureElementService", "default"),
    ] {
        manifest.push_str(&format!("<hal format=\"aidl\"><name>{package}</name><version>1</version><fqname>{interface}/{instance}</fqname></hal>"));
    }
    manifest.push_str("</manifest>");
    let descriptions = at.join("vendor/etc/vintf");
    fs::create_dir_all(&descriptions).unwrap();
    fs::write(descriptions.join("manifest.xml"), manifest).unwrap();
    let mut properties = HashMap::from([("ro.boot.hardware".into(), "qcom".into())]);
    if let Some(level) = level {
        properties.insert(GATEKEEPER_LEVEL.into(), level.into());
    }
    let partitions = ["vendor", "odm"].into_iter().map(|name| Partition { name: name.into(), mounted: Path::new("/").join(name), at: at.join(name) }).collect();
    Phone::read(partitions, properties, None).unwrap()
}

#[test]
fn the_installed_script_selects_gatekeeper_before_its_service_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let unknown = phone(dir.path(), None);
    let steps = naming::steps(&unknown).unwrap();
    let [Does::Run(selector)] = &steps[..] else { panic!("run the selector once: {steps:?}") };
    assert_eq!(selector.service.path, "/system/bin/sh");
    assert!(selector.service.args[0].replace('\\', "/").ends_with("/vendor/bin/init.qti.keymaster.sh"));
    assert!(selector.service.oneshot);
    assert!(secure::steps(&unknown).is_err(), "do not guess between the two Gatekeepers");
    for (level, ending) in [("0", "gatekeeper-service-qti"), ("1", "gatekeeper-service-spu-qti")] {
        let selected = phone(dir.path(), Some(level));
        assert!(naming::steps(&selected).unwrap().is_empty(), "a selector already read is not run again");
        let plan = secure::steps(&selected).unwrap();
        let gatekeepers: Vec<_> = secure::programs(&plan).into_iter().filter(|s| s.service.path.contains("gatekeeper")).collect();
        assert_eq!(gatekeepers.len(), 1);
        assert!(gatekeepers[0].service.path.ends_with(ending));
    }
}

#[test]
fn oneplus_nxp_weaver_starts_after_its_element_and_omapi() {
    let dir = tempfile::tempdir().unwrap();
    let plan = secure::steps(&phone(dir.path(), Some("0"))).unwrap();
    let omapi = plan.iter().position(|s| *s == Step::ServeElements).unwrap();
    let Step::Start(element) = &plan[omapi - 1] else { panic!("start the secure element before OMAPI") };
    assert!(element.service.path.ends_with("secure_element-service.qti"));
    let Step::Start(weaver) = &plan[omapi + 1] else { panic!("start Weaver after OMAPI") };
    assert!(weaver.service.path.ends_with("weaver-service.nxp"));
    assert_eq!(weaver.service.user.as_deref(), Some("vendor_nxp_weaver"));
    assert_eq!(weaver.service.groups, ["system", "drmrpc"]);
    assert_eq!(weaver.service.capabilities, ["WAKE_ALARM"]);
    assert!(weaver.anew, "a weaver holds the OMAPI objects of the current process");
}
