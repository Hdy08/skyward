//! Oplus' component OTA query protocol, as used by the installed OnePlus
//! updater. Device properties and the negotiation public key come from the
//! installed system, so a recovery build does not freeze a firmware version.

use super::Release;
use aes::cipher::{KeyIvInit, StreamCipher};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{rngs::OsRng, RngCore};
use rsa::{pkcs8::DecodePublicKey, Oaep, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

type AesCtr = ctr::Ctr128BE<aes::Aes256>;

/// The stock updater uses this value when Android's OpenID service is
/// unavailable. Querying from recovery requires no account, IMEI or serial.
const DEVICE_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    props: HashMap<String, String>,
    public_key: String,
    key_version: String,
    brand: String,
}

impl Installed {
    pub fn from_system(props: HashMap<String, String>, features: &str, server: &str) -> Result<Self> {
        let xml = roxmltree::Document::parse(features).context("read the installed OTA app features")?;
        let feature = |name: &str| -> Result<String> {
            xml.descendants()
                .find(|n| n.attribute("name") == Some(name))
                .and_then(|n| n.attribute("args"))
                .and_then(|a| a.strip_prefix("String:"))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .with_context(|| format!("the installed system defines no {name}"))
        };
        let base = feature("com.oplus.ota.component_update_url")?;
        ensure!(base.starts_with("https://") && server == format!("{}/update/v6", base.trim_end_matches('/')),
            "the installed system's OTA region uses {base}, different from the configured server");
        let this = Self {
            props,
            public_key: feature("com.oplus.ota.pubkey")?,
            key_version: feature("com.oplus.ota.pubkey_ver")?,
            brand: feature("com.oplus.ota.brand")?,
        };
        for name in ["ro.product.name", "ro.build.version.ota", "ro.build.version.release", "ro.oplus.components.list", "ro.build.oplus_nv_id"] {
            ensure!(!this.prop(name).is_empty(), "the installed system says no {name}");
        }
        ensure!(this.prop("ro.build.version.ota").starts_with(&format!("{}_", this.prop("ro.product.name"))),
            "the installed OTA version does not match its model");
        Ok(this)
    }

    fn prop(&self, name: &str) -> &str {
        self.props.get(name).map(String::as_str).unwrap_or("")
    }

    fn question(&self, unlocked: bool, now: u64) -> Result<Value> {
        let mut components = Vec::new();
        for name in self.prop("ro.oplus.components.list").split(',').filter(|s| !s.is_empty()) {
            let image = format!("ro.oplus.image.{name}.version");
            let manifest = format!("ro.oplus.version.{name}");
            let version = if self.prop(&image).is_empty() { self.prop(&manifest) } else { self.prop(&image) };
            ensure!(!version.is_empty(), "the installed system says no version for {name}");
            components.push(json!({"componentName": if name == "base" { "system_vendor" } else { name }, "componentVersion": version}));
        }
        Ok(json!({
            "components": components,
            "mode": 0,
            "time": now,
            "isRooted": "0",
            "isLocked": !unlocked,
            // Always request a full package. Incremental component images
            // are unsuitable for recovery's full OTA install path.
            "type": "1",
            "registrationId": "",
            "securityPatch": self.prop("ro.build.version.security_patch"),
            "securityPatchVendor": self.prop("ro.vendor.build.security_patch"),
            "strategyVersion": "",
            "customizeName": self.prop("ro.oplus.image.customize.name"),
            "customizeType": self.prop("ro.oplus.image.customize.type"),
            "supportDLTaste": "1",
            "otaAppVersion": 16000023,
            "deviceId": DEVICE_ID,
            "duid": DEVICE_ID,
            "h5LinkVersion": 6,
            "cacheVersion": {"id":"", "otaVersion":"", "checkOtaVersionTime":-1, "sotaVersion":"", "checkSotaVersionTime":-1, "tasteSotaVersion":""},
            "cota": {"cotaVersion":"", "cotaVersionName":"", "buildType":""},
            "opex": {"check":false},
            "sota": {"sotaProtocolVersion":"2", "sotaVersion":"", "otaUpdateTime":-1, "updateViaReboot":2, "frameworkVer":"-1", "supportLightH":"1", "sotaProtocolVersionNew":[]},
        }))
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let confidential = self.prop("ro.build.version.oplusrom.confidential");
        let coloros = confidential.strip_prefix('V').unwrap_or(confidential).split('_').next().unwrap_or("1.0.0");
        let project = if self.prop("ro.separate.soft").is_empty() { self.prop("ro.boot.prjname") } else { self.prop("ro.separate.soft") };
        let operator = if self.prop("ro.oplus.pipeline.carrier").is_empty() { "default" } else { self.prop("ro.oplus.pipeline.carrier") };
        vec![
            ("language", "zh-CN".into()),
            ("newLanguage", "zh-CN".into()),
            ("romVersion", self.prop("ro.build.display.id").into()),
            ("androidVersion", format!("Android {}", self.prop("ro.build.version.release"))),
            ("colorOSVersion", format!("ColorOS{coloros}")),
            ("infVersion", "1".into()),
            ("otaVersion", self.prop("ro.build.version.ota").into()),
            ("model", self.prop("ro.product.name").into()),
            ("mode", "manual".into()),
            ("nvCarrier", self.prop("ro.build.oplus_nv_id").into()),
            ("pipelineKey", self.prop("ro.oplus.pipeline_key").into()),
            ("operator", operator.into()),
            ("companyId", self.prop("ro.oplus.company_id").into()),
            ("prjNum", project.into()),
            ("brand", self.brand.clone()),
            ("brandSota", self.prop("ro.product.brand").into()),
            ("osType", self.prop("ro.oplus.image.my_stock.type").into()),
            ("marketName", STANDARD.encode(self.prop("ro.vendor.oplus.market.name"))),
            ("version", "2".into()),
            ("deviceId", hex::encode(Sha256::digest(DEVICE_ID.as_bytes()))),
        ]
    }
}

/// One question uses a fresh AES key and IV. RSA-OAEP carries the base64
/// AES key; the reply uses that same AES key with its own IV.
struct Session {
    key: [u8; 32],
    protected: String,
}

impl Session {
    fn new(installed: &Installed, now: u64) -> Result<Self> {
        let mut key = [0; 32];
        OsRng.try_fill_bytes(&mut key).context("make an OTA session key")?;
        let der = STANDARD.decode(&installed.public_key).context("read the OTA public key's base64")?;
        let rsa = RsaPublicKey::from_public_key_der(&der).context("read the OTA RSA public key")?;
        let protected = rsa.encrypt(&mut OsRng, Oaep::new::<sha1::Sha1>(), STANDARD.encode(key).as_bytes())
            .context("protect the OTA session key")?;
        let protected = json!({"SCENE_1": {"protectedKey": STANDARD.encode(protected), "version": (now + 86_400_000).to_string(), "negotiationVersion": installed.key_version}}).to_string();
        Ok(Self { key, protected })
    }

    fn seal(&self, question: &Value) -> Result<String> {
        let mut iv = [0; 16];
        OsRng.try_fill_bytes(&mut iv).context("make an OTA message IV")?;
        let mut cipher = serde_json::to_vec(question)?;
        AesCtr::new((&self.key).into(), (&iv).into()).apply_keystream(&mut cipher);
        let params = json!({"cipher": STANDARD.encode(cipher), "iv": STANDARD.encode(iv)}).to_string();
        Ok(json!({"params": params}).to_string())
    }

    fn open(&self, answer: &Value) -> Result<Option<String>> {
        let code = answer.get("responseCode").and_then(Value::as_u64).context("the OTA server says no responseCode")?;
        ensure!(code == 200, "the OnePlus OTA server answered {code}: {}", answer.get("errMsg").and_then(Value::as_str).unwrap_or("no explanation"));
        let Some(body) = answer.get("body").filter(|b| !b.is_null()) else { return Ok(None) };
        let envelope: Value = serde_json::from_str(body.as_str().context("the OTA response body is not an encrypted envelope")?)?;
        let iv = STANDARD.decode(envelope["iv"].as_str().context("the OTA reply says no IV")?)?;
        ensure!(iv.len() == 16, "the OTA reply's IV is not 16 bytes");
        let mut cipher = STANDARD.decode(envelope["cipher"].as_str().context("the OTA reply says no cipher")?)?;
        AesCtr::new_from_slices(&self.key, &iv).expect("an AES-256 key and a 16-byte IV").apply_keystream(&mut cipher);
        Ok(Some(String::from_utf8(cipher).context("the opened OTA reply is not text")?))
    }
}

/// An assembled full OTA ZIP, never an incremental image or an app/module
/// update. Download-check URLs are exchanged for CDN URLs when downloading.
pub fn release(body: &str, installed: &Installed) -> Result<Option<Release>> {
    let body: Value = serde_json::from_str(body).context("read the opened OnePlus OTA answer")?;
    let Some(ota) = body.get("otaVersion").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return Ok(None) };
    ensure!(ota.starts_with(&format!("{}_", installed.prop("ro.product.name"))), "the OTA answer is for another phone");
    ensure!(ota.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)), "the OTA answer's version is not a safe package name");
    let components = body["components"].as_array().context("the OTA answer says no components")?;
    ensure!(components.len() == 1 && components[0]["componentName"] == "my_manifest", "the OTA answer is not one assembled full system package");
    let packet = &components[0]["componentPackets"];
    ensure!(number(&packet["type"]) == Some(1), "the OTA answer supplies an incremental package instead of a full package");
    let length = number(&packet["size"]).filter(|n| *n > 0).context("the OTA package has no valid size")?;
    let md5 = packet["md5"].as_str().context("the OTA package says no MD5")?.trim().to_lowercase();
    ensure!(md5.len() == 32 && md5.bytes().all(|b| b.is_ascii_hexdigit()), "the OTA package's MD5 is invalid");
    let mut urls = Vec::new();
    for name in ["manualUrl", "url"] {
        if let Some(url) = packet[name].as_str().filter(|s| !s.is_empty()) {
            let uri: ureq::http::Uri = url.parse().context("read the official OTA package URL")?;
            ensure!(uri.scheme_str() == Some("https") && uri.host().is_some(), "the OTA package URL is not HTTPS");
            if !urls.iter().any(|u| u == url) {
                urls.push(url.to_owned());
            }
        }
    }
    ensure!(!urls.is_empty(), "the OTA package says no download endpoint");
    let version = body.get("realVersionName").and_then(Value::as_str).filter(|s| !s.is_empty())
        .or_else(|| body.get("versionName").and_then(Value::as_str).filter(|s| !s.is_empty())).unwrap_or(ota).to_owned();
    Ok(Some(Release {
        version,
        name: format!("{ota}_all.zip"),
        urls,
        md5,
        size: format!("{:.2}G", length as f64 / 1_073_741_824.0),
        length: Some(length),
    }))
}

/// Obtain a package address using the installed updater's download
/// headers. A HEAD request to downloadCheck returns JSON, not a ZIP: the
/// stock updater uses a ranged GET and receives an HTTPS CDN redirect.
/// Device headers stay on the regional OTA server, never on the CDN.
pub fn locate(agent: &ureq::Agent, server: &str, installed: &Installed, release: &Release) -> Result<Vec<String>> {
    let base = server.strip_suffix("/update/v6").context("the OnePlus OTA server has no /update/v6 path")?;
    let origin: ureq::http::Uri = base.parse()?;
    let mut located = Vec::new();
    let mut last = None;
    let mut timestamp = None;
    for url in &release.urls {
        let uri = https(url)?;
        if uri.path() != "/downloadCheck" {
            located.push(url.clone());
            continue;
        }
        ensure!(uri.authority() == origin.authority(), "the package download check belongs to another OTA region");
        let ts = match &timestamp {
            Some(ts) => ts,
            None => {
                let mut response = agent.get(format!("{base}/ts")).call().context("ask the OTA server for its download time")?;
                ensure!(response.status().is_success(), "the OTA time server answered HTTP {}", response.status());
                let text = response.body_mut().with_config().limit(128).read_to_string()?;
                let value: u64 = text.trim().parse().context("the OTA server's time is not milliseconds")?;
                timestamp.insert(value.to_string())
            }
        };
        let result = locate_one(agent, url, &installed.download_headers(ts));
        match result {
            Ok(url) if !located.contains(&url) => located.push(url),
            Ok(_) => {}
            Err(e) => last = Some(e),
        }
    }
    if located.is_empty() {
        return Err(last.unwrap_or_else(|| anyhow::anyhow!("the OnePlus package has no download address")));
    }
    Ok(located)
}

impl Installed {
    fn download_headers(&self, timestamp: &str) -> Vec<(&'static str, String)> {
        let names = ["language", "androidVersion", "colorOSVersion", "otaVersion", "model", "nvCarrier", "brand", "osType", "operator", "prjNum", "marketName"];
        let mut headers: Vec<_> = self.headers().into_iter().filter(|(name, _)| names.contains(name)).collect();
        headers.extend([
            ("mode", "0".into()),
            ("id", hex::encode(md5::Md5::digest(DEVICE_ID.as_bytes()))),
            ("ts", timestamp.into()),
            ("userId", "oplus-ota|16000023".into()),
        ]);
        headers
    }
}

fn https(url: &str) -> Result<ureq::http::Uri> {
    let uri: ureq::http::Uri = url.parse().context("read an official OTA address")?;
    ensure!(uri.scheme_str() == Some("https") && uri.host().is_some()
        && uri.authority().is_some_and(|a| !a.as_str().contains('@')), "the official OTA address is not HTTPS");
    Ok(uri)
}

fn locate_one(agent: &ureq::Agent, url: &str, headers: &[(&str, String)]) -> Result<String> {
    let mut request = agent.get(url).header("Range", "bytes=0-0");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let mut response = request.config().max_redirects(0).build().call().context("ask OnePlus for its package address")?;
    match response.status().as_u16() {
        301 | 302 | 303 | 307 | 308 => {
            let url = response.headers().get("Location").context("the OnePlus download redirect has no address")?.to_str()?;
            https(url)?;
            Ok(url.to_owned())
        }
        200 => {
            let text = response.body_mut().with_config().limit(64 * 1024).read_to_string().context("read the OnePlus download answer")?;
            let answer: Value = serde_json::from_str(&text).context("the OnePlus download check returned no CDN address")?;
            let code = answer.get("responseCode").and_then(number).context("the OnePlus download answer says no responseCode")?;
            bail!("the OnePlus download server answered {code}")
        }
        status => bail!("the OnePlus download server answered HTTP {status}"),
    }
}

fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

pub fn ask(agent: &ureq::Agent, server: &str, installed: &Installed, unlocked: bool) -> Result<Option<Release>> {
    let now: u64 = SystemTime::now().duration_since(UNIX_EPOCH).context("read the time for the OTA question")?.as_millis().try_into()?;
    let session = Session::new(installed, now)?;
    let mut request = agent.post(server).header("Content-Type", "application/json; charset=utf-8");
    for (name, value) in installed.headers() {
        request = request.header(name, value);
    }
    let mut response = request.header("protectedKey", &session.protected)
        .send(session.seal(&installed.question(unlocked, now)?)?).with_context(|| format!("ask {server}"))?;
    ensure!(response.status().is_success(), "the OTA server answered HTTP {}", response.status());
    let text = response.body_mut().read_to_string().context("read the OnePlus OTA server's answer")?;
    let answer = serde_json::from_str(&text).context("read the OnePlus OTA response envelope")?;
    session.open(&answer)?.map(|body| release(&body, installed)).unwrap_or(Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Installed {
        Installed {
            props: HashMap::from([
                ("ro.product.name".into(), "PJZ110".into()),
                ("ro.build.version.ota".into(), "PJZ110_11.C.88_1880_202604101200".into()),
                ("ro.oplus.components.list".into(), "base,my_manifest".into()),
                ("ro.oplus.image.base.version".into(), ".current".into()),
                ("ro.oplus.version.base".into(), ".stale".into()),
                ("ro.oplus.version.my_manifest".into(), "manifest.current".into()),
            ]),
            public_key: String::new(), key_version: "1615879139745".into(), brand: "oneplus".into(),
        }
    }

    #[test]
    fn the_key_and_region_come_from_the_installed_system() {
        let xml = r#"<features>
            <app_feature name="com.oplus.ota.component_update_url" args="String:https://component-ota-cn.allawntech.com"/>
            <app_feature name="com.oplus.ota.pubkey" args="String:cHVibGlj"/>
            <app_feature name="com.oplus.ota.pubkey_ver" args="String:1615879139745"/>
            <app_feature name="com.oplus.ota.brand" args="String:oneplus"/>
        </features>"#;
        let mut props = installed().props;
        props.insert("ro.build.version.release".into(), "16".into());
        props.insert("ro.build.oplus_nv_id".into(), "10010111".into());
        let server = "https://component-ota-cn.allawntech.com/update/v6";
        let i = Installed::from_system(props.clone(), xml, server).unwrap();
        assert_eq!(i.public_key, "cHVibGlj");
        assert_eq!(i.key_version, "1615879139745");
        assert_eq!(i.brand, "oneplus");
        assert!(Installed::from_system(props.clone(), xml, "https://another-region.example/update/v6").is_err());
        props.insert("ro.product.name".into(), "CPH2653".into());
        assert!(Installed::from_system(props, xml, server).is_err());
    }

    #[test]
    fn the_question_reads_installed_components_and_uses_no_personal_identifier() {
        let i = installed();
        let q = i.question(true, 1234).unwrap();
        assert_eq!(q["components"][0], json!({"componentName":"system_vendor", "componentVersion":".current"}));
        assert_eq!(q["components"][1]["componentVersion"], "manifest.current");
        assert_eq!(q["type"], "1");
        assert_eq!(q["isLocked"], false);
        assert_eq!(q["deviceId"], DEVICE_ID);
        assert_eq!(q["duid"], DEVICE_ID);
        assert!(q["sota"]["sotaProtocolVersionNew"].is_array());
        assert_eq!(i.headers().iter().find(|(k, _)| *k == "otaVersion").unwrap().1, "PJZ110_11.C.88_1880_202604101200");
    }

    // The public response metadata returned for this phone on 2026-11-14,
    // with signed download tokens and unrelated presentation fields removed.
    fn full() -> Value {
        json!({"otaVersion":"PJZ110_11.C.93_1930_202608031342", "realVersionName":"PJZ110_16.0.10.501(CN01)",
            "components":[{"componentName":"my_manifest", "componentPackets":{
                "type":"1", "size":"8975091784", "md5":"6bd2956979795c267a062462ad7fc50f",
                "manualUrl":"https://component-ota-cn.allawntech.com/downloadCheck?test=manual",
                "url":"https://component-ota-cn.allawntech.com/downloadCheck?test=auto"}}]})
    }

    #[test]
    fn an_assembled_full_ota_has_its_display_version_exact_size_and_digest() {
        let r = release(&full().to_string(), &installed()).unwrap().unwrap();
        assert_eq!(r.version, "PJZ110_16.0.10.501(CN01)");
        assert_eq!(r.length, Some(8_975_091_784));
        assert_eq!(r.md5, "6bd2956979795c267a062462ad7fc50f");
        assert!(r.name.ends_with("_all.zip"));
    }

    #[test]
    fn incremental_modular_mismatched_and_malformed_packages_are_not_full_otas() {
        for mutate in 0..5 {
            let mut v = full();
            match mutate {
                0 => v["components"][0]["componentPackets"]["type"] = json!(0),
                1 => v["components"][0]["componentName"] = json!("my_product"),
                2 => v["otaVersion"] = json!("CPH2653_11.A.01"),
                3 => v["components"][0]["componentPackets"]["md5"] = json!("bad"),
                _ => v["otaVersion"] = json!("PJZ110_../../invalid"),
            }
            assert!(release(&v.to_string(), &installed()).is_err());
        }
        assert!(release(r#"{"sota":{"sotaVersion":"apps"}}"#, &installed()).unwrap().is_none());
    }

    fn download_server(reply: String) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/downloadCheck?test=manual", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() { break; }
                request.push_str(&line);
            }
            stream.write_all(reply.as_bytes()).unwrap();
            request.to_lowercase()
        });
        (url, worker)
    }

    #[test]
    fn download_check_uses_a_ranged_get_stock_headers_and_returns_the_cdn_without_forwarding_them() {
        let cdn = "https://cdn.example/full.zip?signature=test";
        let (url, worker) = download_server(format!("HTTP/1.1 302 Found\r\nLocation: {cdn}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        let headers = installed().download_headers("1794672000000");
        let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
        assert_eq!(locate_one(&agent, &url, &headers).unwrap(), cdn);
        let request = worker.join().unwrap();
        assert!(request.starts_with("get /downloadcheck?test=manual "));
        for header in ["range: bytes=0-0", "mode: 0", "ts: 1794672000000", "userid: oplus-ota|16000023", "model: pjz110"] {
            assert!(request.contains(header), "{header}");
        }
        assert!(request.contains(&format!("id: {}", hex::encode(md5::Md5::digest(DEVICE_ID.as_bytes())))));
        assert!(!request.contains("protectedkey:") && !request.contains("authorization:"));
    }

    #[test]
    fn download_denials_and_insecure_redirects_are_errors_instead_of_packages() {
        let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
        for reply in [
            "HTTP/1.1 200 OK\r\nContent-Length: 21\r\nConnection: close\r\n\r\n{\"responseCode\":2306}",
            "HTTP/1.1 302 Found\r\nLocation: http://cdn.example/full.zip\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: https://user:secret@cdn.example/full.zip\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ] {
            let (url, worker) = download_server(reply.into());
            assert!(locate_one(&agent, &url, &[]).is_err());
            worker.join().unwrap();
        }
        let mut r = release(&full().to_string(), &installed()).unwrap().unwrap();
        r.urls = vec!["https://another-region.example/downloadCheck?token=private".into()];
        assert!(locate(&agent, "https://component-ota-cn.allawntech.com/update/v6", &installed(), &r).is_err());
    }

    #[test]
    fn failed_queries_and_broken_envelopes_are_not_reported_as_no_update() {
        let s = Session { key: [0x35; 32], protected: String::new() };
        assert!(s.open(&json!({"responseCode":2201,"body":null,"errMsg":"invalid params"})).is_err());
        assert!(s.open(&json!({"responseCode":200,"body":"not an envelope"})).is_err());
        assert_eq!(s.open(&json!({"responseCode":200,"body":null})).unwrap(), None);
    }

    #[test]
    fn the_wire_envelope_uses_a_string_params_and_a_new_iv() {
        let s = Session { key: [0x35; 32], protected: String::new() };
        let question = json!({"otaVersion":"test"});
        let a: Value = serde_json::from_str(&s.seal(&question).unwrap()).unwrap();
        let b: Value = serde_json::from_str(&s.seal(&question).unwrap()).unwrap();
        assert_ne!(a, b);
        let opened = s.open(&json!({"responseCode":200,"body":a["params"]})).unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&opened).unwrap(), question);
    }
}
