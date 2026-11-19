use anyhow::{anyhow, Context, Result};
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventSummary, InputEvent, KeyCode, PropType, SynchronizationCode};
use skyward_config::Write;
use skyward_sys::sleep::Wakeful;
use skyward_ui::app::{Key, Touch};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RETRY: Duration = Duration::from_secs(1);

pub enum Event {
    /// A touch and when the panel reported it, on [`monotonic_ms`]'s clock.
    Touch(Touch, u64),
    /// A button going down, or coming back up.
    Key(Key, bool),
}

/// Milliseconds on the monotonic clock, which the touch panel stamps its
/// events with.
pub fn monotonic_ms() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    assert_eq!(rc, 0, "read the monotonic clock");
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

const KEYS: [KeyCode; 4] = [KeyCode::KEY_POWER, KeyCode::KEY_VOLUMEUP, KeyCode::KEY_VOLUMEDOWN, KeyCode::KEY_BACK];

fn key_of(code: KeyCode) -> Option<Key> {
    match code {
        KeyCode::KEY_POWER => Some(Key::Power),
        KeyCode::KEY_VOLUMEUP => Some(Key::VolumeUp),
        KeyCode::KEY_VOLUMEDOWN => Some(Key::VolumeDown),
        KeyCode::KEY_BACK => Some(Key::Back),
        _ => None,
    }
}

/// A device read failed because it was unplugged.
fn gone(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::ENODEV)
}

/// Slot 0's value of a multitouch axis. The kernel keeps multitouch values
/// per slot, apart from the axis state the ABS queries return, and hands
/// them out through EVIOCGMTSLOTS: the axis code, then one value per slot.
fn slot0_value(dev: &RawDevice, axis: AbsoluteAxisCode, slots: usize) -> io::Result<i32> {
    let mut buf = vec![0i32; 1 + slots];
    buf[0] = i32::from(axis.0);
    // _IOC(_IOC_READ, 'E', 0x0a, size)
    let request = (2u32 << 30) | (((buf.len() * 4) as u32) << 16) | (u32::from(b'E') << 8) | 0x0a;
    if unsafe { libc::ioctl(dev.as_raw_fd(), request as _, buf.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf[1])
}

/// The direct-touch panel. Only the first finger is followed (multitouch
/// protocol B, slot 0).
struct Panel {
    dev: RawDevice,
    path: PathBuf,
    x_range: (i32, i32),
    y_range: (i32, i32),
    slots: usize,
    slot: i32,
    down: bool,
    was_down: bool,
    raw: (i32, i32),
    moved: bool,
    /// The kernel dropped events that were not read in time; what arrives
    /// up to the next report is incomplete.
    dropped: bool,
    /// Log the first report carrying a finger, without its coordinates.
    reported: bool,
}

impl Panel {
    fn new(path: PathBuf, dev: RawDevice) -> Result<Self> {
        let mut x_range = None;
        let mut y_range = None;
        let mut slots = 1;
        for (axis, info) in dev.get_absinfo().context("read touch axis ranges")? {
            match axis {
                AbsoluteAxisCode::ABS_MT_POSITION_X => x_range = Some((info.minimum(), info.maximum())),
                AbsoluteAxisCode::ABS_MT_POSITION_Y => y_range = Some((info.minimum(), info.maximum())),
                AbsoluteAxisCode::ABS_MT_SLOT => slots = info.maximum() as usize + 1,
                _ => {}
            }
        }
        let x_range = x_range.ok_or_else(|| anyhow!("touch panel reports no X range"))?;
        let y_range = y_range.ok_or_else(|| anyhow!("touch panel reports no Y range"))?;
        // Stamp events with the clock the UI keeps time by: how fast a
        // finger moved when it lifted is measured from these stamps.
        // EVIOCSCLOCKID: _IOW('E', 0xa0, int)
        let request = (1u32 << 30) | (4u32 << 16) | (u32::from(b'E') << 8) | 0xa0;
        let clock: libc::c_int = libc::CLOCK_MONOTONIC;
        if unsafe { libc::ioctl(dev.as_raw_fd(), request as _, &clock) } < 0 {
            return Err(io::Error::last_os_error()).context("stamp touch events with the monotonic clock");
        }
        set_nonblocking(dev.as_raw_fd()).context("make touch non-blocking")?;
        let mut panel =
            Panel { dev, path, x_range, y_range, slots, slot: 0, down: false, was_down: false, raw: (0, 0), moved: false, dropped: false, reported: false };
        // The kernel drops a position equal to the one a slot already holds,
        // so a touch landing where the last one lifted carries no position;
        // start from what the panel holds now.
        panel.resync().context("read the touch state")?;
        skyward_sys::kmsg(&format!("touch {} {:?}: x {:?} y {:?}, {slots} slots", panel.path.display(), panel.dev.name().unwrap_or("?"), x_range, y_range))?;
        Ok(panel)
    }

    /// Read the current slot and slot 0's finger back from the panel.
    fn resync(&mut self) -> io::Result<()> {
        self.slot = self.dev.get_abs_state()?[usize::from(AbsoluteAxisCode::ABS_MT_SLOT.0)].value;
        self.down = slot0_value(&self.dev, AbsoluteAxisCode::ABS_MT_TRACKING_ID, self.slots)? >= 0;
        self.raw = (
            slot0_value(&self.dev, AbsoluteAxisCode::ABS_MT_POSITION_X, self.slots)?,
            slot0_value(&self.dev, AbsoluteAxisCode::ABS_MT_POSITION_Y, self.slots)?,
        );
        self.moved = true;
        Ok(())
    }

    fn read(&mut self, w: f32, h: f32, out: &mut Vec<Event>) -> io::Result<()> {
        let events: Vec<InputEvent> = match self.dev.fetch_events() {
            Ok(events) => events.collect(),
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };
        for e in events {
            match e.destructure() {
                // Skip to the next report and read the state back there.
                EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => self.dropped = true,
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    if std::mem::take(&mut self.dropped) {
                        self.resync()?;
                    }
                    if self.down && !self.reported {
                        self.reported = true;
                        let _ = skyward_sys::kmsg(&format!("touch {} {:?}: first contact received", self.path.display(), self.dev.name().unwrap_or("?")));
                    }
                    let time = e.as_ref().time;
                    let at = time.tv_sec as u64 * 1000 + time.tv_usec as u64 / 1000;
                    let span = |r: (i32, i32)| (r.1 - r.0).max(1) as f32;
                    let x = (self.raw.0 - self.x_range.0) as f32 / span(self.x_range) * w;
                    let y = (self.raw.1 - self.y_range.0) as f32 / span(self.y_range) * h;
                    match (self.was_down, self.down) {
                        (false, true) => out.push(Event::Touch(Touch::Down(x, y), at)),
                        (true, true) if self.moved => out.push(Event::Touch(Touch::Move(x, y), at)),
                        (true, false) => out.push(Event::Touch(Touch::Up, at)),
                        _ => {}
                    }
                    self.was_down = self.down;
                    self.moved = false;
                }
                _ if self.dropped => {}
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, v) => self.slot = v,
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, v) if self.slot == 0 => self.down = v >= 0,
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, v) if self.slot == 0 => {
                    self.raw.0 = v;
                    self.moved = true;
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, v) if self.slot == 0 => {
                    self.raw.1 = v;
                    self.moved = true;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The touchscreen and the hardware buttons. /dev/input is watched, so a
/// device that shows up later, such as a touch panel whose driver module
/// loads after the UI starts, is picked up when it appears.
pub struct Inputs {
    watch: OwnedFd,
    /// The watch and every device taken in: a touch or a key press keeps
    /// the phone awake until it is read.
    wakeful: Wakeful,
    panel: Option<Panel>,
    buttons: Vec<(PathBuf, RawDevice)>,
    seen: HashSet<PathBuf>,
    scanned_at: Instant,
    /// What the touchscreen is told each time its panel is taken in.
    touch_writes: Vec<Write>,
    touch_name: Option<String>,
    w: f32,
    h: f32,
}

impl Inputs {
    pub fn open(w: u32, h: u32, touch_writes: Vec<Write>, touch_name: Option<String>) -> Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error()).context("create inotify instance");
        }
        let watch = unsafe { OwnedFd::from_raw_fd(fd) };
        let dir = CString::new("/dev/input")?;
        // IN_ATTRIB too: ueventd sets a node's owner and mode right after
        // creating it, and the node may only open once that is done.
        if unsafe { libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB | libc::IN_DELETE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM) } < 0 {
            return Err(io::Error::last_os_error()).context("watch /dev/input");
        }
        let wakeful = Wakeful::new()?;
        wakeful.add(fd)?;
        let mut inputs = Inputs {
            watch,
            wakeful,
            panel: None,
            buttons: Vec::new(),
            seen: HashSet::new(),
            scanned_at: Instant::now(),
            touch_writes,
            touch_name,
            w: w as f32,
            h: h as f32,
        };
        inputs.scan()?;
        if inputs.panel.is_none() {
            skyward_sys::kmsg(&format!("no touch panel {:?} yet; waiting under /dev/input", inputs.touch_name))?;
        }
        Ok(inputs)
    }

    /// Take in the devices under /dev/input not seen before: the first
    /// direct multitouch device as the panel, and every device with a
    /// power, volume or back key as buttons. A panel taken in is told what
    /// the device description writes to the touchscreen.
    fn scan(&mut self) -> Result<()> {
        self.scanned_at = Instant::now();
        for (path, dev) in evdev::raw_stream::enumerate() {
            if !self.seen.insert(path.clone()) {
                continue;
            }
            let direct = dev.properties().contains(PropType::DIRECT);
            let mt = dev
                .supported_absolute_axes()
                .is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_MT_POSITION_X) && a.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y));
            let keys = dev.supported_keys().is_some_and(|k| KEYS.iter().any(|c| k.contains(*c)));
            eprintln!("input {}: {:?} direct={direct} multitouch={mt} keys={keys}", path.display(), dev.name().unwrap_or("?"));
            if direct && mt && self.panel.is_none() && self.touch_name.as_deref().is_none_or(|name| dev.name() == Some(name)) {
                self.wakeful.add(dev.as_raw_fd())?;
                self.panel = Some(Panel::new(path, dev)?);
                for w in &self.touch_writes {
                    fs::write(&w.path, &w.value).with_context(|| format!("write {} to {}", w.value, w.path))?;
                    skyward_sys::kmsg(&format!("wrote {} to {} for the touchscreen", w.value, w.path))?;
                }
            } else if keys {
                set_nonblocking(dev.as_raw_fd()).context("make buttons non-blocking")?;
                self.wakeful.add(dev.as_raw_fd())?;
                self.buttons.push((path, dev));
            }
        }
        Ok(())
    }

    /// Empty the watch's queue; true when /dev/input changed.
    fn changed(&self) -> Result<bool> {
        let mut buf = [0u8; 4096];
        let mut any = false;
        loop {
            let n = unsafe { libc::read(self.watch.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                any = true;
                continue;
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                ErrorKind::WouldBlock => return Ok(any),
                ErrorKind::Interrupted => continue,
                _ => return Err(err).context("read /dev/input changes"),
            }
        }
    }

    /// Whether a touch panel has come up.
    pub fn has_panel(&self) -> bool {
        self.panel.is_some()
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        (self.w, self.h) = (w as f32, h as f32);
    }

    /// Readable while a device has something queued, or /dev/input changed.
    pub fn fd(&self) -> RawFd {
        self.wakeful.fd()
    }

    /// Everything the devices have queued, as touches and key presses.
    pub fn read(&mut self) -> Result<Vec<Event>> {
        if self.changed()? || (self.panel.is_none() && self.scanned_at.elapsed() >= RETRY) {
            self.scan()?;
        }
        let mut out = Vec::new();
        let mut panel_removed = false;
        if let Some(panel) = &mut self.panel {
            match panel.read(self.w, self.h, &mut out) {
                Ok(()) => {}
                Err(e) if gone(&e) => {
                    skyward_sys::kmsg(&format!("touch {}: removed", panel.path.display()))?;
                    self.seen.remove(&panel.path);
                    self.panel = None;
                    panel_removed = true;
                }
                Err(e) => return Err(e).context("read touch events"),
            }
        }
        let mut removed = Vec::new();
        for (i, (path, b)) in self.buttons.iter_mut().enumerate() {
            match b.fetch_events() {
                Ok(events) => {
                    // The kernel's own repeats of a key held are left out;
                    // the UI times its own.
                    for e in events {
                        if let EventSummary::Key(_, code, value @ (0 | 1)) = e.destructure()
                            && let Some(k) = key_of(code)
                        {
                            out.push(Event::Key(k, value == 1));
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) if gone(&e) => {
                    eprintln!("input {}: removed", path.display());
                    removed.push(i);
                }
                Err(e) => return Err(e).context("read button events"),
            }
        }
        for i in removed.into_iter().rev() {
            let (path, _) = self.buttons.remove(i);
            self.seen.remove(&path);
        }
        // A replacement can have reused the same event path before the
        // old handle reports ENODEV. The first scan still saw that path as
        // owned; retry after dropping it rather than wait for another event
        // that may never come.
        if panel_removed {
            self.scan()?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::uinput::VirtualDevice;
    use evdev::{AbsInfo, AttributeSet, EventType, UinputAbsSetup};

    /// Only this process's named virtual panel is selected. No events are
    /// injected into either physical touchscreen or the running Recovery.
    fn panel(name: &str) -> VirtualDevice {
        let properties = AttributeSet::from_iter([PropType::DIRECT]);
        let mut builder = VirtualDevice::builder().unwrap().name(name).with_properties(&properties).unwrap();
        for (code, value, min, max) in [
            (AbsoluteAxisCode::ABS_MT_SLOT, 0, 0, 9),
            (AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1, 0, 65535),
            (AbsoluteAxisCode::ABS_MT_POSITION_X, 0, 0, 1000),
            (AbsoluteAxisCode::ABS_MT_POSITION_Y, 0, 0, 1000),
        ] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, AbsInfo::new(value, min, max, 0, 0, 0))).unwrap();
        }
        let mut panel = builder.build().unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let path = panel.enumerate_dev_nodes_blocking().unwrap().next().and_then(Result::ok);
            if path.is_some_and(|path| path.exists()) {
                break;
            }
            assert!(Instant::now() < until, "ueventd did not create the virtual panel node");
            std::thread::sleep(Duration::from_millis(10));
        }
        panel
    }

    fn tap(panel: &mut VirtualDevice, inputs: &mut Inputs) {
        panel.emit(&[
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_MT_SLOT.0, 0),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 12),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_MT_POSITION_X.0, 250),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 500),
        ]).unwrap();
        let events = inputs.read().unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::Touch(Touch::Down(x, y), _) if *x == 250.0 && *y == 500.0)));
        panel.emit(&[InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1)]).unwrap();
        assert!(inputs.read().unwrap().iter().any(|e| matches!(e, Event::Touch(Touch::Up, _))));
    }

    #[test]
    #[ignore = "requires root and /dev/uinput on an Android test device"]
    fn a_late_panel_is_found_even_if_its_notification_was_already_drained() {
        let name = format!("skyward-late-touch-{}", std::process::id());
        let mut inputs = Inputs::open(1000, 1000, Vec::new(), Some(name.clone())).unwrap();
        assert!(!inputs.has_panel());
        let mut device = panel(&name);
        inputs.changed().unwrap();
        inputs.scanned_at = Instant::now() - RETRY;
        inputs.read().unwrap();
        assert!(inputs.has_panel());
        tap(&mut device, &mut inputs);
    }

    #[test]
    #[ignore = "requires root and /dev/uinput on an Android test device"]
    fn a_removed_panel_is_replaced_without_waiting_for_another_notification() {
        let name = format!("skyward-replaced-touch-{}", std::process::id());
        let device = panel(&name);
        let mut inputs = Inputs::open(1000, 1000, Vec::new(), Some(name.clone())).unwrap();
        assert!(inputs.has_panel());
        drop(device);
        let mut replacement = panel(&name);
        // The watch was handled before the old evdev handle returned
        // ENODEV. There need not be another notification after this one.
        inputs.changed().unwrap();
        inputs.read().unwrap();
        assert!(inputs.has_panel());
        tap(&mut replacement, &mut inputs);
    }
}
