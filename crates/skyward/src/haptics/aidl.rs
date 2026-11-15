//! Calibrated effects supplied by the installed vibrator HAL. Awinic
//! motors expose no evdev force-feedback device; their HAL owns the
//! waveform, resonance and braking rather than a recovery guessing them.

use crate::waveform::share;
use anyhow::{ensure, Context, Result};
use skyward_binder::driver::{Binder, Handle};
use skyward_binder::parcel::Parcel;
use skyward_binder::{service, status};
use skyward_config::Strength;
use std::time::Duration;

const SERVICE: &str = "android.hardware.vibrator.IVibrator/default";
const INTERFACE: &str = "android.hardware.vibrator.IVibrator";
// Stable AIDL IVibrator v1 method order. These numbers remain the same
// in later versions of the interface.
const CAPABILITIES: u32 = 1;
const OFF: u32 = 2;
const ON: u32 = 3;
const PERFORM: u32 = 4;
const SUPPORTED: u32 = 5;
const AMPLITUDE: u32 = 6;
const AMPLITUDE_CONTROL: i32 = 4;
const CLICK: i32 = 0;
const TICK: i32 = 2;

pub struct Vibrator {
    binder: Binder,
    capabilities: i32,
    effects: Vec<i32>,
    strength: Strength,
}

impl Vibrator {
    pub fn open(strength: Strength) -> Result<Self> {
        let binder = Binder::open()?;
        let (capabilities, effects) = {
            let vibrator = service::find(&binder, SERVICE)?.context("the installed vibrator HAL has not registered")?;
            let reply = vibrator.transact(CAPABILITIES, &Parcel::call(INTERFACE))?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            let capabilities = read.i32()?;
            let reply = vibrator.transact(SUPPORTED, &Parcel::call(INTERFACE))?;
            let mut read = reply.read();
            status::ok(&mut read)?;
            let count = usize::try_from(read.i32()?)?;
            ensure!(count <= 128, "the vibrator returned too many effects: {count}");
            let effects = (0..count).map(|_| read.i32()).collect::<Result<Vec<_>>>()?;
            (capabilities, effects)
        };
        eprintln!("haptics: installed AIDL vibrator, capabilities {capabilities:#x}, effects {effects:?}");
        Ok(Self { binder, capabilities, effects, strength })
    }

    fn service(&self) -> Result<Handle<'_>> {
        service::find(&self.binder, SERVICE)?.context("the installed vibrator HAL stopped")
    }

    pub fn click(&mut self, position: f32) -> Result<Duration> {
        self.play(CLICK, share(&self.strength, position))
    }

    pub fn ticks(&mut self) -> Result<Duration> {
        self.play(TICK, 0.5)
    }

    fn play(&self, preferred: i32, strength: f32) -> Result<Duration> {
        let vibrator = self.service()?;
        status::ok(&mut vibrator.transact(OFF, &Parcel::call(INTERFACE))?.read()).context("stop the previous vibration")?;
        let effect = [preferred, CLICK].into_iter().find(|id| self.effects.contains(id)).or_else(|| self.effects.first().copied());
        if let Some(effect) = effect {
            let mut call = Parcel::call(INTERFACE);
            call.i32(effect);
            // EffectStrength: LIGHT=0, MEDIUM=1, STRONG=2. Let the HAL
            // choose its calibrated waveform at that supported strength.
            call.i32(if strength < 1.0 / 3.0 { 0 } else if strength < 2.0 / 3.0 { 1 } else { 2 });
            call.no_object();
            let reply = vibrator.transact(PERFORM, &call)?;
            let mut read = reply.read();
            status::ok(&mut read).context("play the vendor's vibration effect")?;
            let millis = read.i32()?;
            ensure!((1..=1000).contains(&millis), "the vibrator returned an invalid effect duration: {millis}");
            Ok(Duration::from_millis(millis as u64))
        } else {
            // Basic HALs may have no named effects. A finite pulse still
            // stops in the HAL without a timer in the UI process. Where
            // amplitude is fixed, vary its duration with the slider rather
            // than silently ignoring the configured strength.
            let millis = if self.capabilities & AMPLITUDE_CONTROL != 0 { 20 } else { (8.0 + 24.0 * strength).round() as u64 };
            let mut call = Parcel::call(INTERFACE);
            call.i32(millis as i32);
            call.no_object();
            status::ok(&mut vibrator.transact(ON, &call)?.read()).context("start the vendor's timed vibration")?;
            if self.capabilities & AMPLITUDE_CONTROL != 0 {
                let mut call = Parcel::call(INTERFACE);
                call.i32(strength.clamp(0.0, 1.0).to_bits() as i32);
                status::ok(&mut vibrator.transact(AMPLITUDE, &call)?.read())?;
            }
            Ok(Duration::from_millis(millis))
        }
    }
}

impl Drop for Vibrator {
    fn drop(&mut self) {
        if let Ok(vibrator) = self.service() {
            let _ = vibrator.transact(OFF, &Parcel::call(INTERFACE));
        }
    }
}
