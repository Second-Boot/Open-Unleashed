//! Rumble output on macOS through Apple's **GameController** framework and **CoreHaptics**
//! (`RE/notes/force-feedback.md` §11) — the macOS counterpart of the gilrs backend in `crate::ff`
//! with the **same interface** (`Rumbler`: two sustained motor levels 0…1, one-shot strong pulses),
//! selected with `cfg(target_os = "macos")`. gilrs 0.11 has no force feedback on macOS
//! (`is_ff_supported` is `false`, `set_ff_state` is empty), so without this a pad gets nothing.
//!
//! The original game had DirectInput force feedback only; nothing here is the original's. The
//! mapping of its effects onto a pad's two motors is the caller's (`open_unleashed::app::force_feedback`,
//! **APPROXIMATION A2**: road → the Strong motor at a constant level, engine → the Weak motor,
//! jolt → a Strong pulse, the spring dropped) — this module is the same approximation on another
//! platform, it only provides the mechanism.
//!
//! Mechanism, per motor ("channel"):
//! * `GCController.controllers` → the first with `haptics` (`GCDeviceHaptics`) →
//!   `createEngineWithLocality:` with `GCHapticsLocalityLeftHandle` (Strong, the heavy low-frequency
//!   motor) or `GCHapticsLocalityRightHandle` (Weak). If the handle is not in `supportedLocalities`
//!   it falls back to `GCHapticsLocalityHandles`, then `GCHapticsLocalityDefault` (then both motors
//!   share the same actuators; the log says which locality each got).
//!   (`objc2-game-controller` gates `createEngineWithLocality:` to iOS/tvOS/visionOS, so it is sent
//!   with `msg_send!`; `CHHapticEngine` itself is from `objc2-core-haptics`.)
//! * a `CHHapticEngine`, started; its stopped / reset handlers set a flag, and the next call
//!   rebuilds the engine (a sleep, an audio route change, the pad dropping off).
//! * one long `CHHapticEventTypeHapticContinuous` event (intensity 1, 30 s) in a
//!   `CHHapticAdvancedPatternPlayer` with `loopEnabled`, so it never ends. The level is
//!   changed with `sendParameters:` (`CHHapticDynamicParameterIDHapticIntensityControl` = the 0…1
//!   level, a multiplier of the event's intensity 1): a level change is one call, not a new pattern.
//!   Level 0 stops the player.
//! * a pulse is a continuous event of the given intensity and duration, played once on the Strong
//!   engine through its own player (kept alive until it has finished).
//!
//! Threading: everything is called from the thread that calls the gilrs backend (the game's main
//! thread). GameController delivers connections through the main run loop, and `GCController.controllers`
//! may be empty until the app's run loop has run (winit runs it), so the controller is **queried
//! lazily, at most once a second**, on whichever tick wants a rumble; a pad that has gone away is
//! noticed by the same check (its engines are dropped), and one that reconnects is picked up again.
//! GameController may deliver haptics to the **frontmost app only**; a game in the background will
//! simply feel nothing.
//!
//! Every failure is logged once and otherwise ignored: no panic, no effect on the game. A failed
//! channel is retried every two seconds.

use crate::ff::Motor;
use block2::RcBlock;
use gilrs::Gilrs;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, msg_send};
use objc2_core_haptics::{
    CHHapticAdvancedPatternPlayer, CHHapticDynamicParameter, CHHapticDynamicParameterIDHapticIntensityControl, CHHapticEngine, CHHapticEngineStoppedReason, CHHapticEvent,
    CHHapticEventParameter, CHHapticEventParameterIDHapticIntensity, CHHapticEventTypeHapticContinuous, CHHapticPattern, CHHapticPatternPlayer,
};
use objc2_foundation::{NSArray, NSString};
use objc2_game_controller::{
    GCController, GCDevice, GCDeviceHaptics, GCHapticsLocality, GCHapticsLocalityDefault, GCHapticsLocalityHandles, GCHapticsLocalityLeftHandle, GCHapticsLocalityRightHandle,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// `CHHapticTimeImmediate`
const NOW: f64 = 0.0;
/// The sustained event's length; the player loops it, so this is only the loop's period.
const SUSTAINED_SECONDS: f64 = 30.0;
/// A level change smaller than this (1/255) is not sent.
const LEVEL_STEP: f32 = 1.0 / 255.0;
/// How often the controller list is looked at when there is none / to check the one in use.
const REQUERY: Duration = Duration::from_secs(1);
/// How long a failed channel waits before it tries again.
const RETRY: Duration = Duration::from_secs(2);

type Player = Retained<ProtocolObject<dyn CHHapticAdvancedPatternPlayer>>;

/// One motor: an engine on one locality and the looping player on it.
struct Channel {
    motor: Motor,
    locality: Option<String>,
    engine: Option<Retained<CHHapticEngine>>,
    player: Option<Player>,
    /// set by the engine's stopped / reset handlers (any thread)
    dead: Arc<AtomicBool>,
    playing: bool,
    sent: f32,
    retry_at: Option<Instant>,
    logged_failure: bool,
}

impl Channel {
    fn new(motor: Motor) -> Self {
        Self { motor, locality: None, engine: None, player: None, dead: Arc::new(AtomicBool::new(false)), playing: false, sent: -1.0, retry_at: None, logged_failure: false }
    }

    fn name(&self) -> &'static str {
        match self.motor {
            Motor::Strong => "strong",
            Motor::Weak => "weak",
        }
    }

    fn drop_engine(&mut self) {
        if let Some(e) = self.engine.take() {
            unsafe { e.stopWithCompletionHandler(std::ptr::null_mut()) };
        }
        self.player = None;
        self.playing = false;
        self.sent = -1.0;
        // a late callback of the old engine must not mark the next one dead
        self.dead = Arc::new(AtomicBool::new(false));
    }

    fn fail(&mut self, what: &str, err: &dyn std::fmt::Display) {
        if !self.logged_failure {
            tracing::warn!("macOS rumble ({}): {what}: {err}", self.name());
            self.logged_failure = true;
        }
        self.drop_engine();
        self.retry_at = Some(Instant::now() + RETRY);
    }

    /// The engine on this motor's locality, started; made on demand and again after a stop / reset.
    fn engine(&mut self, haptics: &GCDeviceHaptics) -> Option<&Retained<CHHapticEngine>> {
        if self.dead.swap(false, Ordering::AcqRel) {
            tracing::info!("macOS rumble ({}): engine stopped or reset, rebuilding", self.name());
            self.drop_engine();
        }
        if self.engine.is_none() {
            if self.retry_at.is_some_and(|t| Instant::now() < t) {
                return None;
            }
            self.retry_at = None;
            let (locality, name) = pick_locality(haptics, self.motor);
            let engine: Option<Retained<CHHapticEngine>> = unsafe { msg_send![haptics, createEngineWithLocality: locality] };
            let Some(engine) = engine else {
                self.fail("createEngineWithLocality returned nil", &name);
                return None;
            };
            let dead = self.dead.clone();
            let stopped = RcBlock::new(move |_reason: CHHapticEngineStoppedReason| dead.store(true, Ordering::Release));
            let dead = self.dead.clone();
            let reset = RcBlock::new(move || dead.store(true, Ordering::Release));
            unsafe {
                engine.setStoppedHandler(&*stopped as *const _ as *mut _);
                engine.setResetHandler(&*reset as *const _ as *mut _);
            }
            if let Err(e) = unsafe { engine.startAndReturnError() } {
                self.fail("engine start failed", &e.localizedDescription());
                return None;
            }
            tracing::info!("macOS rumble ({}): engine started on locality {name}", self.name());
            self.locality = Some(name);
            self.logged_failure = false;
            self.engine = Some(engine);
        }
        self.engine.as_ref()
    }

    fn make_player(&mut self) -> bool {
        let Some(engine) = self.engine.as_ref() else { return false };
        let built = unsafe { continuous_pattern(1.0, SUSTAINED_SECONDS).and_then(|p| engine.createAdvancedPlayerWithPattern_error(&p).map_err(|e| e.localizedDescription().to_string())) };
        match built {
            Ok(player) => {
                unsafe { player.setLoopEnabled(true) };
                self.player = Some(player);
                self.sent = -1.0;
                true
            }
            Err(e) => {
                self.fail("pattern player not created", &e);
                false
            }
        }
    }

    fn sustain(&mut self, haptics: &GCDeviceHaptics, level: f32) {
        let level = level.clamp(0.0, 1.0);
        if level <= 0.0 {
            self.stop();
            return;
        }
        if self.engine(haptics).is_none() {
            return;
        }
        if self.player.is_none() && !self.make_player() {
            return;
        }
        if self.playing && (level - self.sent).abs() < LEVEL_STEP {
            return;
        }
        let Some(player) = self.player.clone() else { return };
        // the level first (a stopped player takes it for its next start), then the start, then it again
        let send = |p: &Player| unsafe { send_level(p, level) };
        let _ = send(&player);
        if !self.playing {
            if let Err(e) = unsafe { player.startAtTime_error(NOW) } {
                // a stopped player that will not restart: one fresh player, one more try
                self.player = None;
                if !self.make_player() {
                    return;
                }
                let Some(p2) = self.player.clone() else { return };
                if let Err(e2) = unsafe { p2.startAtTime_error(NOW) } {
                    self.fail("player start failed", &format!("{} / {}", e.localizedDescription(), e2.localizedDescription()));
                    return;
                }
                self.playing = true;
                self.logged_failure = false;
                let _ = send(&p2);
                self.sent = level;
                return;
            }
            self.playing = true;
            self.logged_failure = false;
        }
        if let Err(e) = send(&player) {
            self.fail("sendParameters failed", &e);
            return;
        }
        self.sent = level;
    }

    fn stop(&mut self) {
        if self.playing
            && let Some(p) = &self.player
        {
            let _ = unsafe { p.stopAtTime_error(NOW) };
        }
        self.playing = false;
        self.sent = 0.0;
    }
}

/// The locality a motor wants, and its name for the log: the handle itself, else both handles,
/// else the default (guaranteed by GameController).
fn pick_locality(haptics: &GCDeviceHaptics, motor: Motor) -> (&'static GCHapticsLocality, String) {
    unsafe {
        let supported = haptics.supportedLocalities();
        let handle: (&'static GCHapticsLocality, &str) = match motor {
            Motor::Strong => (GCHapticsLocalityLeftHandle, "LeftHandle"),
            Motor::Weak => (GCHapticsLocalityRightHandle, "RightHandle"),
        };
        if supported.containsObject(handle.0) {
            (handle.0, handle.1.to_string())
        } else if supported.containsObject(GCHapticsLocalityHandles) {
            (GCHapticsLocalityHandles, "Handles (per-handle locality not supported)".to_string())
        } else {
            (GCHapticsLocalityDefault, "Default (per-handle locality not supported)".to_string())
        }
    }
}

/// A pattern of one continuous haptic event at `intensity` for `seconds`.
unsafe fn continuous_pattern(intensity: f32, seconds: f64) -> Result<Retained<CHHapticPattern>, String> {
    unsafe {
        let param = CHHapticEventParameter::initWithParameterID_value(CHHapticEventParameter::alloc(), CHHapticEventParameterIDHapticIntensity, intensity);
        let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
            CHHapticEvent::alloc(),
            CHHapticEventTypeHapticContinuous,
            &NSArray::from_slice(&[&*param]),
            NOW,
            seconds,
        );
        CHHapticPattern::initWithEvents_parameters_error(CHHapticPattern::alloc(), &NSArray::from_slice(&[&*event]), &NSArray::new()).map_err(|e| e.localizedDescription().to_string())
    }
}

/// `CHHapticDynamicParameterIDHapticIntensityControl` = `level`, immediately.
unsafe fn send_level(player: &Player, level: f32) -> Result<(), String> {
    unsafe {
        let p = CHHapticDynamicParameter::initWithParameterID_value_relativeTime(CHHapticDynamicParameter::alloc(), CHHapticDynamicParameterIDHapticIntensityControl, level, 0.0);
        player.sendParameters_atTime_error(&NSArray::from_slice(&[&*p]), NOW).map_err(|e| e.localizedDescription().to_string())
    }
}

/// The controller in use: the first that has haptics.
struct Pad {
    controller: Retained<GCController>,
    name: String,
    strong: Channel,
    weak: Channel,
    /// one-shot players still running, with when they end
    pulses: Vec<(Instant, Retained<ProtocolObject<dyn CHHapticPatternPlayer>>)>,
}

fn controller_name(c: &GCController) -> String {
    unsafe { c.vendorName().map(|n| n.to_string()).unwrap_or_else(|| "unnamed controller".to_string()) }
}

/// The first connected controller that has haptics, if any.
fn find_controller() -> Option<Retained<GCController>> {
    unsafe { GCController::controllers().iter().find(|c| c.haptics().is_some()) }
}

/// The rumble state on one pad (the macOS backend; the same interface as the gilrs `Rumbler`).
#[derive(Default)]
pub struct Rumbler {
    pad: Option<Pad>,
    checked: Option<Instant>,
    announced_none: bool,
}

impl Rumbler {
    /// Whether a controller that can rumble is connected (a fresh look at `GCController.controllers`).
    pub fn supported(&self, _gilrs: &Gilrs) -> bool {
        find_controller().is_some()
    }

    /// One line for the log: this backend, the controller it drives and its haptic localities.
    pub fn describe(&self, _gilrs: &Gilrs) -> String {
        match find_controller() {
            None => "GameController + CoreHaptics (macOS), no controller with haptics connected".to_string(),
            Some(c) => {
                let mut locs: Vec<String> = unsafe {
                    c.haptics().map(|h| h.supportedLocalities().iter().map(|l: Retained<NSString>| l.to_string()).collect()).unwrap_or_default()
                };
                locs.sort();
                format!("GameController + CoreHaptics (macOS), controller \"{}\", localities [{}]", controller_name(&c), locs.join(", "))
            }
        }
    }

    /// The pad to drive: the one in use while it is still connected, else the first with haptics
    /// (looked for at most once a second — the run loop may not have delivered it yet).
    fn pad(&mut self) -> Option<&mut Pad> {
        let due = self.checked.is_none_or(|t| t.elapsed() >= REQUERY);
        if due {
            self.checked = Some(Instant::now());
            if let Some(pad) = &self.pad {
                let still = unsafe { GCController::controllers().iter().any(|c| c == pad.controller) };
                if !still {
                    tracing::info!("macOS rumble: controller \"{}\" disconnected", pad.name);
                    self.pad = None;
                }
            }
            if self.pad.is_none() {
                match find_controller() {
                    Some(controller) => {
                        let name = controller_name(&controller);
                        tracing::info!("macOS rumble: controller \"{name}\" found");
                        self.announced_none = false;
                        self.pad = Some(Pad { controller, name, strong: Channel::new(Motor::Strong), weak: Channel::new(Motor::Weak), pulses: Vec::new() });
                    }
                    None if !self.announced_none => {
                        tracing::info!("macOS rumble: no controller with haptics (will look again)");
                        self.announced_none = true;
                    }
                    None => {}
                }
            }
        }
        self.pad.as_mut()
    }

    /// Set a motor's sustained level, 0…1. 0 stops it.
    pub fn sustain(&mut self, _gilrs: &mut Gilrs, motor: Motor, level: f32) {
        let Some(pad) = self.pad() else { return };
        let Some(haptics) = (unsafe { pad.controller.haptics() }) else { return };
        match motor {
            Motor::Strong => pad.strong.sustain(&haptics, level),
            Motor::Weak => pad.weak.sustain(&haptics, level),
        }
    }

    /// A one-shot strong pulse of `level` (0…1) for `ms` milliseconds, on the Strong engine.
    pub fn pulse(&mut self, _gilrs: &mut Gilrs, level: f32, ms: u32) {
        let Some(pad) = self.pad() else { return };
        let Some(haptics) = (unsafe { pad.controller.haptics() }) else { return };
        let level = level.clamp(0.0, 1.0);
        if level <= 0.0 {
            return;
        }
        let now = Instant::now();
        pad.pulses.retain(|(end, _)| *end > now);
        if pad.strong.engine(&haptics).is_none() {
            return;
        }
        let seconds = f64::from(ms.max(1)) / 1000.0;
        let Some(engine) = pad.strong.engine.as_ref() else { return };
        let result = unsafe {
            continuous_pattern(level, seconds).and_then(|pat| {
                let player = engine.createPlayerWithPattern_error(&pat).map_err(|e| e.localizedDescription().to_string())?;
                player.startAtTime_error(NOW).map_err(|e| e.localizedDescription().to_string())?;
                Ok(player)
            })
        };
        match result {
            Ok(player) => pad.pulses.push((now + Duration::from_secs_f64(seconds + 0.2), player)),
            Err(e) => pad.strong.fail("pulse failed", &e),
        }
    }

    /// Stop both sustained motors (a pause, a menu, the end of the race).
    pub fn stop_all(&mut self) {
        if let Some(pad) = self.pad.as_mut() {
            pad.strong.stop();
            pad.weak.stop();
        }
    }
}
