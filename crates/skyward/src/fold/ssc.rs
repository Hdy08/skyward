//! SSC's sensor lookup and on-change subscription over the installed QMI
//! C API. Only standard messages are sent: lookup, configure and disable.
//! Sensor events carry calibration and diagnostic values too; only the
//! first value, explicitly matching a configured fold state, is used.

use anyhow::{bail, ensure, Context, Result};
use prost::Message;
use skyward_config::Fold;
use std::ffi::{c_void, CStr};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Message)]
struct Uid {
    #[prost(fixed64, tag = "1")]
    low: u64,
    #[prost(fixed64, tag = "2")]
    high: u64,
}

#[derive(Clone, PartialEq, Message)]
struct Suspend {
    #[prost(uint32, required, tag = "1")]
    processor: u32,
    #[prost(uint32, required, tag = "2")]
    delivery: u32,
}

#[derive(Clone, PartialEq, Message)]
struct Payload {
    #[prost(bytes, tag = "2")]
    payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
struct Request {
    #[prost(message, required, tag = "1")]
    suid: Uid,
    #[prost(fixed32, required, tag = "2")]
    id: u32,
    #[prost(message, required, tag = "3")]
    suspend: Suspend,
    #[prost(message, required, tag = "4")]
    request: Payload,
}

#[derive(Clone, PartialEq, Message)]
struct Lookup {
    #[prost(string, tag = "1")]
    datatype: String,
    #[prost(bool, tag = "2")]
    updates: bool,
    #[prost(bool, tag = "3")]
    default_only: bool,
}

#[derive(Clone, PartialEq, Message)]
struct Found {
    #[prost(string, tag = "1")]
    datatype: String,
    #[prost(message, repeated, tag = "2")]
    suids: Vec<Uid>,
}

#[derive(Clone, PartialEq, Message)]
struct Events {
    #[prost(message, optional, tag = "1")]
    suid: Option<Uid>,
    #[prost(message, repeated, tag = "2")]
    events: Vec<SensorEvent>,
}

#[derive(Clone, PartialEq, Message)]
struct SensorEvent {
    #[prost(fixed32, tag = "1")]
    id: u32,
    #[prost(fixed64, tag = "2")]
    timestamp: u64,
    #[prost(bytes, tag = "3")]
    payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
struct Sample {
    #[prost(float, repeated, tag = "1")]
    values: Vec<f32>,
}

const LOOKUP: Uid = Uid { low: 0xabababababababab, high: 0xabababababababab };
const LOOKUP_REQUEST: u32 = 512;
const LOOKUP_EVENT: u32 = 768;
const ON_CHANGE: u32 = 514;
const SAMPLE: u32 = 1025;
const DISABLE: u32 = 10;

fn request(suid: Uid, id: u32, payload: Vec<u8>) -> Vec<u8> {
    Request { suid, id, suspend: Suspend { processor: 1, delivery: 0 }, request: Payload { payload } }.encode_to_vec()
}

type Decode = unsafe extern "C" fn(*mut c_void, i32, u32, *const c_void, u32, *mut c_void, u32) -> i32;
type Indication = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32, *mut c_void);
type Init = unsafe extern "C" fn(*mut c_void, u32, Indication, *mut c_void, *mut c_void, u32, *mut *mut c_void) -> i32;
type Send = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32, *mut c_void, u32, u32) -> i32;
type Release = unsafe extern "C" fn(*mut c_void) -> i32;
type Service = unsafe extern "C" fn(i32, i32, i32) -> *mut c_void;

struct Library(*mut c_void);

impl Library {
    fn open(name: &CStr) -> Result<Self> {
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW) };
        ensure!(!handle.is_null(), "load {}: {}", name.to_string_lossy(), loader_error());
        Ok(Self(handle))
    }

    /// The function signature must match the installed library's C API.
    unsafe fn symbol<T: Copy>(&self, name: &CStr) -> Result<T> {
        ensure!(std::mem::size_of::<T>() == std::mem::size_of::<*mut c_void>(), "invalid function pointer size");
        let symbol = unsafe { libc::dlsym(self.0, name.as_ptr()) };
        ensure!(!symbol.is_null(), "find {}: {}", name.to_string_lossy(), loader_error());
        Ok(unsafe { std::mem::transmute_copy(&symbol) })
    }
}

fn loader_error() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() { "no loader error".into() } else { unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned() }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { libc::dlclose(self.0); }
    }
}

struct Api {
    init: Init,
    send: Send,
    release: Release,
    decode: Decode,
    object: *mut c_void,
    _cci: Library,
    _sns: Library,
}

impl Api {
    fn open() -> Result<Self> {
        let sns = Library::open(c"libsnsapi.so")?;
        let cci = Library::open(c"libqmi_cci.so")?;
        unsafe {
            let service: Service = sns.symbol(c"SNS_CLIENT_SVC_get_service_object_internal_v01")?;
            // Refuse another wire ABI instead of guessing its structure sizes.
            let object = service(1, 3, 6);
            ensure!(!object.is_null(), "installed SSC service does not offer the supported QMI ABI 1.3/6");
            Ok(Self {
                init: cci.symbol(c"qmi_client_init_instance")?,
                send: cci.symbol(c"qmi_client_send_msg_sync")?,
                release: cci.symbol(c"qmi_client_release")?,
                decode: cci.symbol(c"qmi_client_message_decode")?,
                object,
                _cci: cci,
                _sns: sns,
            })
        }
    }
}

#[repr(C)]
struct QmiRequest {
    len: u32,
    payload: [u8; 1000],
    jumbo_valid: u8,
    jumbo: u8,
}

#[repr(C)]
#[derive(Default)]
struct Response {
    status: i32,
    error: i32,
    client_valid: u8,
    _pad: [u8; 7],
    client: u64,
    result_valid: u8,
    _pad2: [u8; 3],
    result: u32,
}

#[repr(C)]
struct Report {
    client: u64,
    len: u32,
    payload: [u8; 62000],
}

struct Callback {
    decode: Decode,
    events: SyncSender<Vec<u8>>,
}

unsafe extern "C" fn indication(client: *mut c_void, id: u32, data: *mut c_void, size: u32, cookie: *mut c_void) {
    if !matches!(id, 0x21 | 0x22) || cookie.is_null() {
        return;
    }
    let callback = unsafe { &*cookie.cast::<Callback>() };
    let mut report = Report { client: 0, len: 0, payload: [0; 62000] };
    let result = unsafe { (callback.decode)(client, 2, id, data, size, (&mut report as *mut Report).cast(), std::mem::size_of::<Report>() as u32) };
    if result == 0 && report.len as usize <= report.payload.len() {
        let _ = callback.events.try_send(report.payload[..report.len as usize].to_vec());
    }
}

struct Connection<'a> {
    api: &'a Api,
    client: *mut c_void,
    _callback: Box<Callback>,
}

impl<'a> Connection<'a> {
    fn open(api: &'a Api, events: SyncSender<Vec<u8>>) -> Result<Self> {
        let mut callback = Box::new(Callback { decode: api.decode, events });
        let mut client = std::ptr::null_mut();
        let result = unsafe { (api.init)(api.object, 0xffff, indication, (&mut *callback as *mut Callback).cast(), std::ptr::null_mut(), 10000, &mut client) };
        ensure!(result == 0 && !client.is_null(), "connect to SSC: QMI error {result}");
        Ok(Self { api, client, _callback: callback })
    }

    fn send(&self, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() <= 1000, "SSC request exceeds the QMI payload limit");
        let mut request = QmiRequest { len: bytes.len() as u32, payload: [0; 1000], jumbo_valid: 1, jumbo: 1 };
        request.payload[..bytes.len()].copy_from_slice(bytes);
        let mut response = Response::default();
        let result = unsafe {
            (self.api.send)(self.client, 0x20, (&mut request as *mut QmiRequest).cast(), std::mem::size_of::<QmiRequest>() as u32,
                (&mut response as *mut Response).cast(), std::mem::size_of::<Response>() as u32, 3000)
        };
        ensure!(result == 0, "send to SSC: QMI error {result}");
        ensure!(response.status == 0 && response.error == 0, "SSC refused the QMI request: {} / {}", response.status, response.error);
        ensure!(response.result_valid == 0 || response.result == 0, "SSC refused the sensor request: {}", response.result);
        Ok(())
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        // The API waits until its indication callbacks are finished before
        // returning; the callback and libraries stay alive until then.
        unsafe { (self.api.release)(self.client); }
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

/// Run in the vendor helper process; stdout carries only state changes.
pub fn watch(cfg: &Fold) -> Result<()> {
    unsafe { libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t); }
    let api = Api::open()?;
    let (send, receive) = mpsc::sync_channel(64);
    let connection = Connection::open(&api, send)?;
    let lookup = Lookup { datatype: cfg.sensor.clone(), updates: true, default_only: true };
    connection.send(&request(LOOKUP, LOOKUP_REQUEST, lookup.encode_to_vec()))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut selected = None;
    let mut last = None;
    while !STOP.load(Ordering::Relaxed) {
        if selected.is_none() && Instant::now() > deadline {
            bail!("{} was not found by SSC in 60 seconds", cfg.sensor);
        }
        let bytes = match receive.recv_timeout(Duration::from_millis(250)) {
            Ok(bytes) => bytes,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => return Err(error).context("read SSC events"),
        };
        let events = Events::decode(bytes.as_slice()).context("decode SSC events")?;
        for event in events.events {
            if events.suid == Some(LOOKUP) && event.id == LOOKUP_EVENT && selected.is_none() {
                let found = Found::decode(event.payload.as_slice()).context("decode SSC sensor lookup")?;
                if found.datatype == cfg.sensor && let Some(&uid) = found.suids.first() {
                    connection.send(&request(uid, ON_CHANGE, Vec::new()))?;
                    selected = Some(uid);
                    skyward_sys::kmsg(&format!("fold sensor: subscribed to {}", cfg.sensor))?;
                }
            } else if selected.is_some() && events.suid == selected && event.id == SAMPLE {
                let sample = Sample::decode(event.payload.as_slice()).context("decode fold sensor sample")?;
                let state = match sample.values.first() {
                    Some(&value) if value == cfg.folded_value => Some(true),
                    Some(&value) if value == cfg.unfolded_value => Some(false),
                    _ => None,
                };
                if let Some(state) = state && last != Some(state) {
                    println!("{}", u8::from(state));
                    std::io::stdout().flush()?;
                    last = Some(state);
                }
            }
        }
    }
    if let Some(selected) = selected {
        connection.send(&request(selected, DISABLE, Vec::new()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_qmi_structures_keep_their_wire_abi() {
        assert_eq!(std::mem::size_of::<QmiRequest>(), 1008);
        assert_eq!(std::mem::size_of::<Response>(), 32);
        assert_eq!(std::mem::offset_of!(Report, payload), 12);
        assert_eq!(std::mem::size_of::<Report>(), 62016);
    }

    #[test]
    fn decoding_a_real_fold_sample_uses_only_its_first_value() {
        let sample = Sample::decode(hex::decode("0a3000000000000033430000803f00000040000040400000803f2029403a1033e8ba00c0b1c400e022c5005005c5000000001003").unwrap().as_slice()).unwrap();
        assert_eq!(sample.values.len(), 12);
        assert_eq!(sample.values[0], 0.0);
        assert_eq!(sample.values[1], 179.0);
    }
}
