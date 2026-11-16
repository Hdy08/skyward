//! Convert verified framework images from a full OTA into a DSU package.
//! Kernel, vendor, firmware and the physical A/B layout remain the installed
//! system's; an OTA update engine or install script is never run here.

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const FRAMEWORK: &[&str] = &["system", "system_ext", "product", "mi_ext"];

#[derive(Serialize)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    device: String,
    images: BTreeMap<String, Image>,
}

#[derive(Serialize)]
struct Image {
    size: u64,
    sha256: String,
}

fn description(metadata: &str, device: &str, id: Option<&str>) -> Result<Manifest> {
    let props: BTreeMap<_, _> = metadata.lines().filter_map(|line| line.split_once('=')).collect();
    ensure!(device == "zizhan", "Android dual boot is currently supported only for zizhan");
    ensure!(props.get("pre-device").is_some_and(|value| value.split('|').any(|name| name.trim() == device)), "the official package does not target {device}");
    ensure!(props.get("ota-type") == Some(&"AB") && !props.contains_key("pre-build") && !props.contains_key("pre-build-incremental"), "a full A/B OTA is required");
    let version = props.get("post-build-incremental").context("the package has no target version")?.to_string();
    let generated: String = version.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '-' }).take(32).collect();
    let id = id.unwrap_or(&generated).to_string();
    ensure!(!id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c)), "invalid Android system id");
    ensure!(!["installed", ".", ".."].contains(&id.as_str()), "the Android system id is reserved");
    let name = if version.starts_with("OS") || version.starts_with("V816") { "HyperOS" } else { "MIUI" };
    Ok(Manifest { id, name: name.into(), version, device: device.into(), images: BTreeMap::new() })
}

pub fn prepare(package: &Path, config: &Path, out: &Path, id: Option<&str>) -> Result<()> {
    ensure!(!out.exists(), "{} already exists", out.display());
    let cfg = skyward_config::DeviceConfig::load(config)?;
    let mut source = ZipArchive::new(File::open(package)?)?;
    let mut metadata = String::new();
    source.by_name("META-INF/com/android/metadata")?.read_to_string(&mut metadata)?;
    let mut manifest = description(&metadata, &cfg.device.codename, id)?;
    let payload = unpayload::Payload::open(package)?;
    ensure!(!payload.is_incremental(), "the payload requires source images; use a full OTA");
    let partitions: Vec<String> = FRAMEWORK.iter().filter(|name| payload.partition(name).is_some()).map(|name| name.to_string()).collect();
    ensure!(partitions.iter().any(|name| name == "system"), "the payload has no system partition");
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let work = tempfile::tempdir_in(parent)?;
    let options = unpayload::ExtractOptions { output_dir: work.path().join("images"), partitions, memory_limit: 1 << 30, ..Default::default() };
    let images = payload.extract(&options, &|event| match event {
        unpayload::Event::Extracted(image) => println!("verified {}: {} bytes", image.partition, image.size),
        unpayload::Event::Started { partitions } => { for (name, bytes) in partitions { println!("extract {name}: {bytes} bytes"); } }
        _ => {}
    })?;
    for image in &images {
        manifest.images.insert(image.partition.clone(), Image { size: image.size, sha256: hex::encode(image.sha256.as_ref().context("the extracted image was not verified")?) });
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let mut zip = ZipWriter::new(file.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored).large_file(true);
    zip.start_file("android.toml", options)?;
    zip.write_all(toml::to_string(&manifest)?.as_bytes())?;
    for image in images {
        zip.start_file(format!("{}.img", image.partition), options)?;
        std::io::copy(&mut File::open(image.path)?, &mut zip)?;
    }
    zip.finish()?;
    file.as_file().sync_all()?;
    file.persist_noclobber(out).with_context(|| format!("save {}", out.display()))?;
    let mut file = File::open(out)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hash.update(&buf[..n]);
    }
    let checksum = hex::encode(hash.finalize());
    std::fs::write(out.with_extension("zip.sha256"), format!("{checksum}  {}\n", out.file_name().unwrap().to_string_lossy()))?;
    println!("Android second-system package: {}\nSHA-256: {checksum}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_official_package_has_to_match_the_configured_device() {
        let text = "ota-type=AB\npre-device=zizhan\npost-build-incremental=OS3.0.9.0.VLRCNXM\n";
        let manifest = description(text, "zizhan", None).unwrap();
        assert_eq!(manifest.version, "OS3.0.9.0.VLRCNXM");
        assert_eq!(manifest.id, "os3.0.9.0.vlrcnxm");
        assert!(description(text, "dodge", None).is_err());
        assert!(description(&format!("{text}pre-build=source\n"), "zizhan", None).is_err());
        assert!(description(text, "zizhan", Some("../bad")).is_err());
    }
}
