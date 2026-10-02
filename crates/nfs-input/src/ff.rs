//! Rumble output through gilrs 0.11's `ff` module — the port's only force-feedback backend, and a
//! narrow one (`RE/notes/force-feedback.md` §7).
//!
//! gilrs can drive exactly one kind of effect: a **constant strong or weak motor magnitude** with an
//! envelope and a schedule, on Linux (evdev `FF_RUMBLE`) and Windows (XInput / Windows.Gaming.Input
//! rumble). On macOS `Gamepad::is_ff_supported()` is `false` and `Device::set_ff_state` is an empty
//! function (gilrs-core 0.6.8 `platform/macos/ff.rs`), so there the backend finds no device and
//! does nothing. Its time base is a 50 ms tick (`Ticks`), so no periodic waveform of the
//! original's 6…35 Hz can be shaped, and a DirectInput constant force, spring or condition has no
//! counterpart at all. The mapping onto this is therefore an **APPROXIMATION** and lives with the
//! caller (`porsche::app::force_feedback`), which labels each choice; this module only provides the
//! mechanism: two sustained motor effects whose level is changed through `Effect::set_gain`, and
//! one-shot pulses.

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Repeat, Replay, Ticks};
use gilrs::{GamepadId, Gilrs};

/// A sustained rumble: one effect per motor, played until it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motor {
    /// the heavy, low-frequency motor
    Strong,
    /// the light, high-frequency motor
    Weak,
}

#[derive(Default)]
struct Sustained {
    effect: Option<Effect>,
    playing: bool,
}

/// The rumble state on one pad.
#[derive(Default)]
pub struct Rumbler {
    pad: Option<GamepadId>,
    strong: Sustained,
    weak: Sustained,
}

impl Rumbler {
    /// The first connected pad that reports force-feedback support, if any. `false` on macOS.
    pub fn find_pad(gilrs: &Gilrs) -> Option<GamepadId> {
        gilrs.gamepads().find(|(_, g)| g.is_connected() && g.is_ff_supported()).map(|(id, _)| id)
    }

    fn pad(&mut self, gilrs: &Gilrs) -> Option<GamepadId> {
        if let Some(id) = self.pad
            && gilrs.connected_gamepad(id).is_some_and(|g| g.is_ff_supported())
        {
            return Some(id);
        }
        // lost or never found: drop what was built for the old one
        self.strong = Sustained::default();
        self.weak = Sustained::default();
        self.pad = Self::find_pad(gilrs);
        self.pad
    }

    /// Set a motor's sustained level, 0…1 (gain over a full-scale infinite effect). 0 stops it.
    pub fn sustain(&mut self, gilrs: &mut Gilrs, motor: Motor, level: f32) {
        let Some(pad) = self.pad(gilrs) else { return };
        let slot = match motor {
            Motor::Strong => &mut self.strong,
            Motor::Weak => &mut self.weak,
        };
        if level <= 0.0 {
            if slot.playing
                && let Some(e) = &slot.effect
            {
                let _ = e.stop();
            }
            slot.playing = false;
            return;
        }
        if slot.effect.is_none() {
            let kind = match motor {
                Motor::Strong => BaseEffectType::Strong { magnitude: u16::MAX },
                Motor::Weak => BaseEffectType::Weak { magnitude: u16::MAX },
            };
            let built = EffectBuilder::new()
                .add_effect(BaseEffect { kind, scheduling: Replay { play_for: Ticks::from_ms(1_000_000), ..Default::default() }, ..Default::default() })
                .gamepads(&[pad])
                .repeat(Repeat::Infinitely)
                .finish(gilrs);
            match built {
                Ok(e) => slot.effect = Some(e),
                Err(err) => {
                    tracing::debug!(%err, "rumble effect not created");
                    return;
                }
            }
        }
        if let Some(e) = &slot.effect {
            let _ = e.set_gain(level.min(1.0));
            if !slot.playing {
                let _ = e.play();
                slot.playing = true;
            }
        }
    }

    /// A one-shot strong pulse of `level` (0…1) for `ms` milliseconds (rounded up to 50 ms ticks).
    pub fn pulse(&mut self, gilrs: &mut Gilrs, level: f32, ms: u32) {
        let Some(pad) = self.pad(gilrs) else { return };
        let ticks = Ticks::from_ms(ms.max(1));
        let magnitude = (level.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
        let built = EffectBuilder::new()
            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude }, scheduling: Replay { play_for: ticks, ..Default::default() }, ..Default::default() })
            .gamepads(&[pad])
            .repeat(Repeat::For(ticks))
            .finish(gilrs);
        if let Ok(e) = built {
            let _ = e.play();
        }
    }

    /// Stop both sustained motors (a pause, a menu, the end of the race).
    pub fn stop_all(&mut self) {
        for slot in [&mut self.strong, &mut self.weak] {
            if slot.playing
                && let Some(e) = &slot.effect
            {
                let _ = e.stop();
            }
            slot.playing = false;
        }
    }

    /// Whether a pad that can rumble is connected.
    pub fn supported(&self, gilrs: &Gilrs) -> bool {
        Self::find_pad(gilrs).is_some()
    }
}
