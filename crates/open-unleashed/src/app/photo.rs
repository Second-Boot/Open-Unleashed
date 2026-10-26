//! PORT-DEVIATION D46 (photo mode, `RE/notes/PORT-DEVIATIONS.md`) on the application's side.
//! **Not in the original, render-only, always on**: no option, switch or variable turns it off. Photo mode is a
//! free camera for a paused race or a replay: the simulation, the network, the replay data and the camera slot's own state are never touched — the slot builds its camera exactly
//! as it does without photo mode (its previous camera, slice and orbit state are put back before every pass and kept
//! after it), and only the *finished* camera of the frame is replaced for drawing (`photo_apply`). A paused race does
//! not tick and a replay is frozen while photo mode is up, so nothing of the game moves under the camera.
//!
//! * **Available** (`photo_entry`): a race (a front-end launch or a replay of one), not a network race — in a network
//!   race the pause action does not stop the race (0x415110's game-type-3 branch at 0x4151ED: no pause menu, the
//!   race runs on; `RE/notes/hud.md` §5), so photo mode is **not offered there**: no key, no pause-menu item (the
//!   network race's replay included) — and either the pause menu up (its quit question not) or a replay playing. The key is **F11 or K** (**K only on macOS**, where F11 is the system's Show Desktop:
//!   `nfs_game::photo::PHOTO_F11`): the original's `I_VIEWDISTANCE` is `SC_F11` but names a key its table does not
//!   have (`nfs_input::default_binding`), K is unbound. The pause menu gets a fourth item, «photo mode»
//!   (`pause_items`). In a replay the key freezes it (the bar's pause) for as long as photo mode is up.
//! * **Controls**: W A S D move, Q / E down / up, the arrow keys or the right mouse button look, Z / X roll, the
//!   wheel or `-` / `=` zoom (the vertical angle of view), Shift faster, Ctrl slower; a pad's sticks move and look,
//!   the triggers go down / up, the shoulders roll, the d-pad's up / down zoom. Enter / Space (pad A) save a still;
//!   `6` cycles its size 1x / 2x / 4x of the window; H (pad Y) the game's HUD (hidden by default); Tab the hint;
//!   R resets the camera; F11 / K / Esc / P / Backspace (pad B) leave. Every key goes to photo mode while it is up
//!   (the mouse buttons and the pad's menu keys too: neither may reach the hidden pause menu).
//! * **The still** is the frame the renderer draws, rendered again at `scale x` the window (`Renderer::render_still`,
//!   scaled down only past the device's limits) and written as a PNG (RGB) to `<Pictures>/PorscheUnleashed/`
//!   (`nfs_assets::platform::photo_dir`; `NFS_PHOTO_DIR` names another) on a thread of its own, without the hint.
//!   The file name carries the track, the UTC date and time and the size.
//! * **TEST GLUE** `NFS_PHOTO=<op;op;...>` (`photo_script`): in a `--screenshot` run photo mode is entered and the
//!   ops applied before the still: `fwd=m`, `right=m`, `up=m` (move), `yaw=deg`, `pitch=deg`, `roll=deg`,
//!   `fov=deg`, `hud=on|off`, `shot=<1|2|4>` (save a still now). `NFS_RACE_KEYS` (`race_keys_script`) feeds key
//!   presses through the window handler's photo-mode and pause-menu paths.

use crate::*;
use nfs_game::photo::{S_ENTER, S_FAILED, S_FOV, S_KEYS_MOVE, S_KEYS_PAD, S_KEYS_SHOT, S_KEYS_ZOOM, S_SAVED, S_SIZE, S_TITLE};
use std::collections::HashSet;
use winit::keyboard::{KeyCode, PhysicalKey};

/// The still's sizes, as multiples of the window.
pub(crate) const STILL_SCALES: [u32; 3] = [1, 2, 4];
/// The vertical angle of view's limits, degrees.
const FOV_MIN: f32 = 10.0;
const FOV_MAX: f32 = 110.0;
/// Metres per second of the free camera at normal speed; Shift x 5, Ctrl x 0.2.
const MOVE_SPEED: f32 = 8.0;
/// Radians per second of the arrow keys / a pad's right stick; radians per mouse pixel.
const LOOK_SPEED: f32 = 1.2;
const MOUSE_LOOK: f32 = 0.0025;
const ROLL_SPEED: f32 = 0.8;

/// What one frame of input asks of the free camera (all axes -1..1; the angles in the units of the speeds above).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(crate) struct PhotoInput {
    pub(crate) forward: f32,
    pub(crate) right: f32,
    pub(crate) up: f32,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    /// positive zooms in
    pub(crate) zoom: f32,
    pub(crate) fast: bool,
    pub(crate) slow: bool,
    /// mouse pixels turned this frame (already limited to a drag)
    pub(crate) mouse: (f32, f32),
}

/// The free camera (the part of photo mode that is plain data, so it can be tested).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FreeCam {
    pub(crate) pos: Vec3,
    /// radians; 0 looks along +Z, turning toward +X (the world is left-handed: +X is to the right of +Z)
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    /// the vertical angle of view, radians
    pub(crate) fov_y: f32,
}

impl FreeCam {
    /// The free camera standing where `c` is, looking where it looks, rolled as it is.
    pub(crate) fn from_camera(c: &Camera) -> Self {
        let f = (c.target - c.position).normalize_or_zero();
        let f = if f == Vec3::ZERO { Vec3::Z } else { f };
        let (yaw, pitch) = (f.x.atan2(f.z), f.y.clamp(-1.0, 1.0).asin());
        let mut s = Self { pos: c.position, yaw, pitch, roll: 0.0, fov_y: c.fov_y_radians };
        let (r, u0) = (s.right(), s.up_level());
        let up = c.up.normalize_or_zero();
        if up != Vec3::ZERO {
            s.roll = up.dot(r).atan2(up.dot(u0));
        }
        s
    }

    pub(crate) fn forward(&self) -> Vec3 {
        let (sy, cy, sp, cp) = (self.yaw.sin(), self.yaw.cos(), self.pitch.sin(), self.pitch.cos());
        Vec3::new(sy * cp, sp, cy * cp)
    }

    /// The camera's right (the way the picture's right edge lies), level.
    pub(crate) fn right(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin())
    }

    /// Up with no roll: the forward vector's derivative by the pitch.
    fn up_level(&self) -> Vec3 {
        let (sy, cy, sp, cp) = (self.yaw.sin(), self.yaw.cos(), self.pitch.sin(), self.pitch.cos());
        Vec3::new(-sp * sy, cp, -sp * cy)
    }

    /// Up with the roll (positive tilts it toward the right: the camera rolls clockwise).
    pub(crate) fn up(&self) -> Vec3 {
        (self.up_level() * self.roll.cos() + self.right() * self.roll.sin()).normalize()
    }

    /// The camera for drawing, with the near and far planes of the slot's.
    pub(crate) fn camera(&self, near: f32, far: f32) -> Camera {
        Camera { position: self.pos, target: self.pos + self.forward(), up: self.up(), fov_y_radians: self.fov_y, near, far }
    }

    /// One frame of input over `dt` seconds.
    pub(crate) fn step(&mut self, dt: f32, i: &PhotoInput) {
        let dt = dt.clamp(0.0, 0.1);
        let speed = MOVE_SPEED * if i.fast { 5.0 } else if i.slow { 0.2 } else { 1.0 };
        // a zoomed-in view turns more slowly, as a lens does
        let fov_k = (self.fov_y / 1.0).clamp(0.2, 1.4);
        self.pos += (self.forward() * i.forward + self.right() * i.right + Vec3::Y * i.up) * speed * dt;
        self.yaw = (self.yaw + i.yaw * LOOK_SPEED * dt * fov_k + i.mouse.0 * MOUSE_LOOK * fov_k).rem_euclid(std::f32::consts::TAU);
        let lim = 89.0f32.to_radians();
        self.pitch = (self.pitch + i.pitch * LOOK_SPEED * dt * fov_k - i.mouse.1 * MOUSE_LOOK * fov_k).clamp(-lim, lim);
        self.roll = (self.roll + i.roll * ROLL_SPEED * dt).rem_euclid(std::f32::consts::TAU);
        if self.roll > std::f32::consts::PI {
            self.roll -= std::f32::consts::TAU;
        }
        self.fov_y = (self.fov_y * (-i.zoom * 0.9 * dt).exp()).clamp(FOV_MIN.to_radians(), FOV_MAX.to_radians());
    }

    /// The wheel: `ticks` > 0 zooms in.
    pub(crate) fn wheel(&mut self, ticks: f32) {
        self.fov_y = (self.fov_y * (-ticks * 0.08).exp()).clamp(FOV_MIN.to_radians(), FOV_MAX.to_radians());
    }
}

/// What photo mode keeps between frames.
#[derive(Default)]
pub(crate) struct PhotoState {
    pub(crate) active: bool,
    cam: Option<FreeCam>,
    /// the pose it was entered with (R goes back to it)
    entry: Option<FreeCam>,
    /// the camera slot's camera and slice as its last pass built them (put back before the next pass)
    real_camera: Option<Camera>,
    real_slice: Option<usize>,
    /// the tracker slice of the free camera (the sky and the fog follow it)
    cam_slice: Option<usize>,
    /// the game's HUD shown (hidden by default), the hint shown
    hud: bool,
    hint: bool,
    scale_ix: usize,
    /// the replay was frozen by photo mode (and is let go on leaving)
    froze_replay: bool,
    keys: HashSet<KeyCode>,
    look_drag: bool,
    pad: nfs_input::gamepad::PhotoPad,
    /// a still to take after this frame is drawn
    pending_still: bool,
    /// where the hint's sprites start in `scene.sprites` (a still has none of them)
    hint_from: usize,
    toast: Option<(String, Instant)>,
    /// the PNG writers still at work (joined when the program exits)
    writers: Vec<std::thread::JoinHandle<()>>,
    /// stills taken (two in one second get a counter in their name)
    shots: u32,
    /// entered by the test glue (`NFS_PHOTO`): stills are sized from `NFS_SHOT_SIZE` and written before returning
    scripted: bool,
}

impl PhotoState {
    /// Whether the game's HUD is hidden by photo mode.
    pub(crate) fn hides_hud(&self) -> bool {
        self.active && !self.hud
    }

    pub(crate) fn finish_writers(&mut self) {
        for h in self.writers.drain(..) {
            let _ = h.join();
        }
    }
}

/// How photo mode was reached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PhotoEntry {
    /// the pause menu is up
    Pause,
    /// a replay plays (the key freezes it)
    Replay,
}

/// What [`photo_entry_rule`] decides from.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct PhotoWhere {
    /// photo mode is up already
    pub(crate) active: bool,
    /// a race (or a replay of one) with a renderer
    pub(crate) race: bool,
    /// `race_game_type == game_type::NETWORK`
    pub(crate) network: bool,
    /// a port dialog over the race (the replay save, its overwrite question, a network notice)
    pub(crate) dialog: bool,
    pub(crate) replay_playing: bool,
    pub(crate) paused: bool,
    /// the pause menu's quit question is up
    pub(crate) quit_question: bool,
}

/// The availability rule (see the module notes): never in a network race; the pause menu up without its quit
/// question, or a replay playing.
pub(crate) fn photo_entry_rule(w: PhotoWhere) -> Option<PhotoEntry> {
    if w.active || !w.race || w.network || w.dialog {
        return None;
    }
    if w.replay_playing {
        return Some(PhotoEntry::Replay);
    }
    (w.paused && !w.quit_question).then_some(PhotoEntry::Pause)
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + (m <= 2) as i64, m, d)
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC) of a Unix time.
pub(crate) fn utc_stamp(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// A track name made safe for a file name.
fn file_safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if t.is_empty() { "track".into() } else { t }
}

impl App {
    /// Line `k` of the port's photo-mode strings (`nfs_game::photo::PHOTO_STRINGS`) in the install's language.
    pub(crate) fn photo_text(&self, k: usize) -> String {
        nfs_game::photo::photo_text(&self.fe.strings, k)
    }

    /// Whether the window is the render target a still is sized from.
    fn photo_base_size(&self) -> Option<(u32, u32)> {
        if self.screenshot.is_some() || self.photo.scripted {
            let g = shot_geometry();
            return Some((g.0, g.1));
        }
        self.renderer.as_ref().map(|r| r.size())
    }

    /// How photo mode can be entered now, if it can (see the module notes).
    pub(crate) fn photo_entry(&self) -> Option<PhotoEntry> {
        photo_entry_rule(PhotoWhere {
            active: self.photo.active,
            race: self.renderer.is_some() && self.vehicle.is_some(),
            network: self.fe.race_game_type == Some(game_type::NETWORK),
            dialog: self.replay_save.up || self.replay_save.overwrite || self.net_notice.is_some(),
            replay_playing: self.playback_running(),
            paused: self.paused,
            quit_question: self.pause_confirm.is_some(),
        })
    }

    /// FOCUS-INPUT: the window lost the focus — no key or drag is held any more.
    pub(crate) fn photo_release_keys(&mut self) {
        self.photo.keys.clear();
        self.photo.look_drag = false;
    }

    /// PORT-DEVIATION D46: how many buttons the pause menu has — `HPauseMenu`'s three (0x42A440), plus the photo-mode
    /// item when photo mode can be entered from the menu (not in a network race).
    pub(crate) fn pause_items(&self) -> usize {
        if self.paused && self.pause_confirm.is_none() && self.photo_entry() == Some(PhotoEntry::Pause) { 4 } else { 3 }
    }

    /// The pause menu's photo-mode button caption (the port's own string).
    pub(crate) fn photo_item_label(&self) -> String {
        self.photo_text(S_TITLE)
    }

    /// Enter photo mode from the camera the slot built last frame.
    pub(crate) fn photo_enter(&mut self) -> bool {
        let Some(entry) = self.photo_entry() else { return false };
        let cam = FreeCam::from_camera(&self.camera);
        if entry == PhotoEntry::Replay && !self.paused && !self.replay.frozen {
            self.replay.frozen = true;
            self.photo.froze_replay = true;
        }
        let keep = (self.photo.scale_ix, std::mem::take(&mut self.photo.writers), self.photo.shots);
        self.photo = PhotoState {
            active: true,
            cam: Some(cam),
            entry: Some(cam),
            real_camera: Some(self.camera),
            real_slice: self.cam_slice,
            cam_slice: self.cam_slice,
            hint: true,
            scale_ix: keep.0,
            froze_replay: self.photo.froze_replay,
            pad: self.gamepads.photo_pad().unwrap_or_default(),
            writers: keep.1,
            shots: keep.2,
            scripted: self.photo.scripted,
            ..Default::default()
        };
        // a pad's menu key that came with the press must not reach the hidden pause menu
        self.input.pad_menu.clear();
        tracing::info!(?entry, pos = ?cam.pos, "photo mode on");
        true
    }

    /// Leave photo mode: the slot's camera and slice back, the replay let go if photo mode froze it.
    pub(crate) fn photo_exit(&mut self) {
        if !self.photo.active {
            return;
        }
        if let Some(c) = self.photo.real_camera.take() {
            self.camera = c;
        }
        self.cam_slice = self.photo.real_slice;
        if std::mem::take(&mut self.photo.froze_replay) {
            self.replay.frozen = false;
        }
        self.photo.active = false;
        self.photo.keys.clear();
        self.photo.look_drag = false;
        // a pad press that came in while photo mode was up (Start is the pause action, B the menu's "continue")
        // must not outlive it
        let _ = self.input.take_pressed(Action::Pause);
        self.input.pad_menu.clear();
        tracing::info!("photo mode off");
    }

    /// A keyboard event while photo mode is up (swallowed: `true`), or the key that enters it.
    pub(crate) fn photo_key_event(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let PhysicalKey::Code(code) = ev.physical_key else { return false };
        self.photo_key_code(code, ev.state == ElementState::Pressed, ev.repeat)
    }

    /// [`Self::photo_key_event`] after the winit decode (the scripted `NFS_RACE_KEYS` glue calls this one).
    pub(crate) fn photo_key_code(&mut self, code: KeyCode, pressed: bool, repeat: bool) -> bool {
        if !self.photo.active {
            return pressed && !repeat && (code == KeyCode::KeyK || (nfs_game::photo::PHOTO_F11 && code == KeyCode::F11)) && self.photo_enter();
        }
        if pressed {
            self.photo.keys.insert(code);
            if !repeat {
                self.photo_edge(code);
            }
        } else {
            self.photo.keys.remove(&code);
        }
        true
    }

    /// **TEST GLUE** (`NFS_RACE_KEYS=<tok;tok;...>`, a `--screenshot` race run): key presses in the window
    /// handler's own order, photo mode's hook first and then the pause menu's keys. Tokens: `pause` (the menu
    /// up, as Esc does), `k`, `f11`, `esc`, `enter`, `up`, `down`. Logs the state after each. `true` when it ran.
    pub(crate) fn race_keys_script(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(spec) = nfs_game::config::cfg().nfs_race_keys.as_str().filter(|s| !s.trim().is_empty()) else { return false };
        for tok in spec.split(';').map(str::trim).filter(|t| !t.is_empty()) {
            let code = match tok {
                "pause" => {
                    self.paused = true;
                    self.pause_sel = 0;
                    self.pause_confirm = None;
                    None
                }
                "k" => Some(KeyCode::KeyK),
                "f11" => Some(KeyCode::F11),
                "esc" => Some(KeyCode::Escape),
                "enter" => Some(KeyCode::Enter),
                "up" => Some(KeyCode::ArrowUp),
                "down" => Some(KeyCode::ArrowDown),
                other => {
                    tracing::warn!(tok = other, "NFS_RACE_KEYS: unknown token");
                    None
                }
            };
            if let Some(code) = code {
                if !self.photo_key_code(code, true, false) && self.paused {
                    let key = match code {
                        KeyCode::ArrowUp => Some(MenuKey::Up),
                        KeyCode::ArrowDown => Some(MenuKey::Down),
                        KeyCode::Enter => Some(MenuKey::Enter),
                        _ => None,
                    };
                    if let Some(k) = key {
                        self.pause_menu_key(k, event_loop);
                    }
                }
                self.photo_key_code(code, false, false);
            }
            tracing::info!(tok, photo = self.photo.active, paused = self.paused, items = self.pause_items(), sel = self.pause_sel, "NFS_RACE_KEYS");
        }
        true
    }

    /// A key pressed (not held) in photo mode.
    pub(crate) fn photo_edge(&mut self, code: KeyCode) {
        match code {
            KeyCode::KeyK | KeyCode::Escape | KeyCode::KeyP | KeyCode::Backspace => self.photo_exit(),
            KeyCode::F11 if nfs_game::photo::PHOTO_F11 => self.photo_exit(),
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => self.photo.pending_still = true,
            KeyCode::KeyH => self.photo.hud = !self.photo.hud,
            KeyCode::Tab => self.photo.hint = !self.photo.hint,
            KeyCode::Digit6 => self.photo.scale_ix = (self.photo.scale_ix + 1) % STILL_SCALES.len(),
            KeyCode::KeyR => {
                if let (Some(e), Some(c)) = (self.photo.entry, self.photo.cam.as_mut()) {
                    *c = e;
                }
            }
            _ => {}
        }
    }

    /// The mouse wheel while photo mode is up (`true` = taken).
    pub(crate) fn photo_wheel(&mut self, delta: &winit::event::MouseScrollDelta) -> bool {
        if !self.photo.active {
            return false;
        }
        let ticks = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
            winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
        };
        if let Some(c) = self.photo.cam.as_mut() {
            c.wheel(ticks);
        }
        true
    }

    /// A mouse button while photo mode is up (`true` = taken, all of them: a click must not reach the hidden pause
    /// menu); the right one is the look drag.
    pub(crate) fn photo_mouse_button(&mut self, button: MouseButton, state: ElementState) -> bool {
        if !self.photo.active {
            return false;
        }
        if button == MouseButton::Right {
            self.photo.look_drag = state == ElementState::Pressed;
        }
        true
    }

    /// The input of this frame from the held keys, the pad and the mouse.
    fn photo_gather(&mut self) -> PhotoInput {
        let k = |c: KeyCode| self.photo.keys.contains(&c) as i32 as f32;
        let axis = |pos: &[KeyCode], neg: &[KeyCode]| pos.iter().map(|&c| k(c)).fold(0.0f32, f32::max) - neg.iter().map(|&c| k(c)).fold(0.0f32, f32::max);
        let mut i = PhotoInput {
            forward: axis(&[KeyCode::KeyW], &[KeyCode::KeyS]),
            right: axis(&[KeyCode::KeyD], &[KeyCode::KeyA]),
            up: axis(&[KeyCode::KeyE, KeyCode::PageUp], &[KeyCode::KeyQ, KeyCode::PageDown]),
            yaw: axis(&[KeyCode::ArrowRight], &[KeyCode::ArrowLeft]),
            pitch: axis(&[KeyCode::ArrowUp], &[KeyCode::ArrowDown]),
            roll: axis(&[KeyCode::KeyX], &[KeyCode::KeyZ]),
            zoom: axis(&[KeyCode::Equal, KeyCode::NumpadAdd], &[KeyCode::Minus, KeyCode::NumpadSubtract]),
            fast: k(KeyCode::ShiftLeft) + k(KeyCode::ShiftRight) > 0.0,
            slow: k(KeyCode::ControlLeft) + k(KeyCode::ControlRight) > 0.0,
            mouse: (0.0, 0.0),
        };
        // the pointer's raw motion since the last frame, kept only while the right button drags
        let (dx, dy) = std::mem::take(&mut self.input.mouse_delta);
        if self.photo.look_drag {
            i.mouse = (dx, dy);
        }
        if let Some(p) = self.gamepads.photo_pad() {
            i.right += p.left.0;
            i.forward += p.left.1;
            i.yaw += p.right.0;
            i.pitch += p.right.1;
            i.up += p.trigger_right - p.trigger_left;
            i.roll += p.bumper_right as i32 as f32 - p.bumper_left as i32 as f32;
            i.zoom += p.dpad_up as i32 as f32 - p.dpad_down as i32 as f32;
            i.fast |= p.left_thumb;
            let prev = self.photo.pad;
            if p.south && !prev.south {
                self.photo.pending_still = true;
            }
            if p.east && !prev.east {
                self.photo_exit();
            }
            if p.north && !prev.north {
                self.photo.hud = !self.photo.hud;
            }
            self.photo.pad = p;
        }
        // the world is drawn mirrored with the MIRROR option: what is right on the screen is left in the world
        if self.mirror {
            i.right = -i.right;
            i.yaw = -i.yaw;
            i.roll = -i.roll;
            i.mouse.0 = -i.mouse.0;
        }
        i
    }

    /// Once a frame before the scene is built: move the free camera.
    pub(crate) fn photo_step(&mut self) {
        if !self.photo.active {
            return;
        }
        let i = self.photo_gather();
        let dt = self.frame_dt;
        if let Some(c) = self.photo.cam.as_mut() {
            c.step(dt, &i);
        }
        // the pad's Start is the pause action (`Gamepads::poll` presses it), its d-pad / A / B the pause menu's keys
        // (D10): never let them queue for the hidden menu
        let _ = self.input.take_pressed(Action::Pause);
        self.input.pad_menu.clear();
        if self.photo.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(4)) {
            self.photo.toast = None;
        }
    }

    /// The start of `rebuild_scene`: the camera slot's camera and slice are what its own last pass left, so the pass
    /// sees no difference from a game without photo mode.
    pub(crate) fn photo_frame_begin(&mut self) {
        if !self.photo.active {
            return;
        }
        if self.vehicle.is_none() {
            // the race went away under it
            self.photo.active = false;
            self.photo.froze_replay = false;
            return;
        }
        if let Some(c) = self.photo.real_camera {
            self.camera = c;
        }
        self.photo.cam_slice = self.cam_slice;
        self.cam_slice = self.photo.real_slice;
    }

    /// Right after the camera slot built `self.camera`: keep it, draw from the free camera instead.
    pub(crate) fn photo_apply(&mut self) {
        if !self.photo.active {
            return;
        }
        self.photo.real_camera = Some(self.camera);
        self.photo.real_slice = self.cam_slice;
        self.cam_slice = self.photo.cam_slice;
        if let Some(c) = self.photo.cam {
            let (near, far) = (self.camera.near, self.camera.far);
            self.camera = c.camera(near, far);
        }
    }

    /// The status line of the hint: the title, the still's size, the angle of view.
    fn photo_status_line(&self) -> String {
        let (bw, bh) = self.photo_base_size().unwrap_or((0, 0));
        let s = STILL_SCALES[self.photo.scale_ix];
        let c = self.photo.cam.unwrap_or_else(|| FreeCam::from_camera(&self.camera));
        format!("{}   {} {}x {}x{}   {} {:.0}", self.photo_text(S_TITLE), self.photo_text(S_SIZE), s, bw * s, bh * s, self.photo_text(S_FOV), c.fov_y.to_degrees())
    }

    /// The hint of photo mode (and the one-line pointer to it on the pause menu / in a replay), appended after the
    /// HUD's own sprites. Also marks where a still ends.
    pub(crate) fn photo_hint(&mut self) {
        self.photo.hint_from = self.scene.sprites.len();
        let Some(h) = self.hud.as_ref().filter(|h| h.font.is_some()) else { return };
        let aspect = self.renderer.as_ref().map_or(HUD_REF_W / HUD_REF_H, |r| r.aspect());
        let lines: Vec<String> = if self.photo.active {
            if !self.photo.hint && self.photo.toast.is_none() {
                return;
            }
            let mut v = Vec::new();
            if let Some((t, _)) = &self.photo.toast {
                v.push(t.clone());
            }
            if self.photo.hint {
                v.push(self.photo_status_line());
                for k in [S_KEYS_MOVE, S_KEYS_ZOOM, S_KEYS_SHOT, S_KEYS_PAD] {
                    v.push(self.photo_text(k));
                }
            }
            v
        } else if self.photo_entry().is_some() && !self.hud_hidden {
            vec![self.photo_text(S_ENTER)]
        } else {
            return;
        };
        let (left, _top, _right, bottom) = Hud::edges();
        let line_h = FONT_SMALL + 3.0;
        let width = lines.iter().map(|l| h.text_width(l, FONT_SMALL)).fold(0.0f32, f32::max);
        let height = line_h * lines.len() as f32 + 6.0;
        let (x, y) = (left + 10.0, bottom - 10.0 - height);
        let mut out = vec![h.quad(aspect, x - 4.0, y, width + 12.0, height, [0.0, 0.0, 0.0, 0.45])];
        let from = out.len();
        for (n, l) in lines.iter().enumerate() {
            h.text(&mut out, aspect, l, (x, y + 3.0 + line_h * n as f32), FONT_SMALL, [1.0, 1.0, 1.0, 1.0]);
        }
        Hud::resolve_text(&mut out[from..], (0.0, 0.0));
        PENDING_TEXT.with(|p| p.borrow_mut().clear());
        self.scene.sprites.extend(out);
    }

    /// After the frame was drawn: the still asked for, taken from that frame's scene.
    pub(crate) fn photo_after_frame(&mut self) {
        if std::mem::take(&mut self.photo.pending_still) && self.photo.active {
            self.photo_save_still(STILL_SCALES[self.photo.scale_ix]);
        }
    }

    /// Render the scene as it is at `scale` x the window and write it. The hint and the lens probe are not in it;
    /// the HUD is when photo mode shows it.
    pub(crate) fn photo_save_still(&mut self, scale: u32) {
        let Some((bw, bh)) = self.photo_base_size() else { return };
        let mut scene = self.scene.clone();
        scene.sprites.truncate(self.photo.hint_from.min(scene.sprites.len()));
        scene.lens_probe = None;
        let camera = self.camera;
        let Some(r) = self.renderer.as_mut() else { return };
        let (w, h, rgba) = match r.render_still(&scene, &camera, bw * scale, bh * scale) {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(error = %e, scale, "photo mode: the still could not be rendered");
                self.photo.toast = Some((self.photo_text(S_FAILED), Instant::now()));
                return;
            }
        };
        let dir = nfs_assets::platform::photo_dir();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.photo.shots += 1;
        let name = format!("PorscheUnleashed_{}_{}_{}x_{}.png", file_safe(&self.track.name), utc_stamp(secs), scale, self.photo.shots);
        let path = dir.join(&name);
        let sync = self.screenshot.is_some() || self.photo.scripted;
        let toast = format!("{}: {} ({}x{})", self.photo_text(S_SAVED), name, w, h);
        let write = move || {
            // RGB: the frame's alpha is not an image's
            let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let r = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| image::save_buffer(&path, &rgb, w, h, image::ColorType::Rgb8).map_err(|e| e.to_string()));
            match r {
                Ok(()) => tracing::info!(path = %path.display(), w, h, "photo mode: still written"),
                Err(e) => tracing::error!(error = e, path = %path.display(), "photo mode: the still could not be written"),
            }
        };
        if sync {
            write();
        } else {
            self.photo.writers.retain(|h| !h.is_finished());
            self.photo.writers.push(std::thread::spawn(write));
        }
        self.photo.toast = Some((toast, Instant::now()));
    }

    /// **TEST GLUE** (`NFS_PHOTO`, see the module notes): enter photo mode in a scripted still and apply the ops.
    /// `true` when it did (the caller rebuilds the scene).
    pub(crate) fn photo_script(&mut self) -> bool {
        let Some(spec) = nfs_game::config::cfg().nfs_photo.as_str().filter(|s| !s.trim().is_empty()) else { return false };
        if self.vehicle.is_none() || self.fe.race_game_type == Some(game_type::NETWORK) {
            tracing::warn!("NFS_PHOTO: photo mode is not available here");
            return false;
        }
        // a scripted still has no pause menu: enter as the pause would
        if !self.photo.active {
            let was = self.paused;
            self.paused = !self.playback_running();
            let ok = self.photo_enter();
            self.paused = was;
            if !ok {
                tracing::warn!("NFS_PHOTO: photo mode could not be entered");
                return false;
            }
        }
        self.photo.scripted = true;
        for op in spec.split(';').map(str::trim).filter(|o| !o.is_empty()) {
            let (name, arg) = op.split_once('=').unwrap_or((op, ""));
            let a = arg.split(',').next().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
            match name {
                "fwd" | "right" | "up" => {
                    let c = self.photo.cam.as_mut().expect("photo camera");
                    let dir = match name {
                        "fwd" => c.forward(),
                        "right" => c.right(),
                        _ => Vec3::Y,
                    };
                    c.pos += dir * a;
                }
                "yaw" | "pitch" | "roll" => {
                    let c = self.photo.cam.as_mut().expect("photo camera");
                    match name {
                        "yaw" => c.yaw = (c.yaw + a.to_radians()).rem_euclid(std::f32::consts::TAU),
                        "pitch" => c.pitch = (c.pitch + a.to_radians()).clamp(-89f32.to_radians(), 89f32.to_radians()),
                        _ => c.roll += a.to_radians(),
                    }
                }
                "fov" => {
                    let c = self.photo.cam.as_mut().expect("photo camera");
                    c.fov_y = a.clamp(FOV_MIN, FOV_MAX).to_radians();
                }
                "hud" => self.photo.hud = arg.trim() == "on",
                "shot" => {
                    self.rebuild_scene();
                    let s = a as u32;
                    if STILL_SCALES.contains(&s) {
                        self.photo_save_still(s);
                    }
                }
                other => tracing::warn!(op = other, "NFS_PHOTO: unknown op"),
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(pos: Vec3, target: Vec3, up: Vec3) -> Camera {
        Camera { position: pos, target, up, fov_y_radians: 0.9, near: 0.5, far: 700.0 }
    }

    #[test]
    fn a_camera_round_trips_through_the_free_camera() {
        for (f, up) in [
            (Vec3::new(0.3, 0.1, 1.0), Vec3::Y),
            (Vec3::new(-1.0, -0.4, 0.2), Vec3::Y),
            (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.2, 1.0, 0.0)),
            (Vec3::new(1.0, 0.5, -1.0), Vec3::new(-0.3, 1.0, 0.1)),
        ] {
            let c = cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(10.0, 2.0, 5.0) + f, up);
            let free = FreeCam::from_camera(&c);
            let back = free.camera(0.5, 700.0);
            let fwd = (back.target - back.position).normalize();
            assert!((fwd - f.normalize()).length() < 1e-4, "{fwd:?} vs {f:?}");
            // the up vector, once made perpendicular to the view, is the original's
            let want = (up - f.normalize() * up.dot(f.normalize())).normalize();
            assert!((back.up - want).length() < 1e-3, "{:?} vs {want:?}", back.up);
        }
    }

    #[test]
    fn the_axes_are_those_of_the_left_handed_world() {
        let c = FreeCam::from_camera(&cam(Vec3::ZERO, Vec3::Z, Vec3::Y));
        assert!((c.forward() - Vec3::Z).length() < 1e-5);
        // facing +Z, the right of the picture is +X (D3D's world)
        assert!((c.right() - Vec3::X).length() < 1e-5);
        // the view really puts +X on the right of the screen (the renderer's x flip)
        let vp = c.camera(0.5, 700.0).view_projection(1.5);
        let p = vp * glam::Vec4::new(1.0, 0.0, 10.0, 1.0);
        assert!(p.x / p.w > 0.0, "{p:?}");
    }

    #[test]
    fn input_moves_looks_zooms_and_stays_in_range() {
        let mut c = FreeCam::from_camera(&cam(Vec3::ZERO, Vec3::Z, Vec3::Y));
        c.step(0.1, &PhotoInput { forward: 1.0, ..Default::default() });
        assert!((c.pos - Vec3::new(0.0, 0.0, MOVE_SPEED * 0.1)).length() < 1e-5);
        let p = c.pos;
        c.step(0.1, &PhotoInput { forward: 1.0, fast: true, ..Default::default() });
        assert!((c.pos - p).length() > MOVE_SPEED * 0.1 * 4.0);
        // looking right turns toward +X
        let mut c = FreeCam::from_camera(&cam(Vec3::ZERO, Vec3::Z, Vec3::Y));
        c.step(0.1, &PhotoInput { yaw: 1.0, ..Default::default() });
        assert!(c.forward().x > 0.0);
        // the pitch stops short of the pole
        for _ in 0..200 {
            c.step(0.1, &PhotoInput { pitch: 1.0, ..Default::default() });
        }
        assert!(c.pitch < 90f32.to_radians() && c.up().is_finite());
        // the zoom has limits
        for _ in 0..500 {
            c.step(0.1, &PhotoInput { zoom: 1.0, ..Default::default() });
        }
        assert!((c.fov_y - FOV_MIN.to_radians()).abs() < 1e-5);
        for _ in 0..500 {
            c.step(0.1, &PhotoInput { zoom: -1.0, ..Default::default() });
        }
        assert!((c.fov_y - FOV_MAX.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn a_long_frame_is_limited() {
        let mut c = FreeCam::from_camera(&cam(Vec3::ZERO, Vec3::Z, Vec3::Y));
        c.step(5.0, &PhotoInput { forward: 1.0, ..Default::default() });
        assert!(c.pos.z <= MOVE_SPEED * 0.1 + 1e-4);
    }

    #[test]
    fn never_in_a_network_race() {
        let paused = PhotoWhere { race: true, paused: true, ..Default::default() };
        assert_eq!(photo_entry_rule(paused), Some(PhotoEntry::Pause));
        let replay = PhotoWhere { race: true, replay_playing: true, ..Default::default() };
        assert_eq!(photo_entry_rule(replay), Some(PhotoEntry::Replay));
        // a network race: neither the pause menu (no fourth item: `pause_items` asks this rule) nor the key
        assert_eq!(photo_entry_rule(PhotoWhere { network: true, ..paused }), None);
        assert_eq!(photo_entry_rule(PhotoWhere { network: true, ..replay }), None);
        // the quit question, a dialog, no race, already up, a race that runs
        assert_eq!(photo_entry_rule(PhotoWhere { quit_question: true, ..paused }), None);
        assert_eq!(photo_entry_rule(PhotoWhere { dialog: true, ..paused }), None);
        assert_eq!(photo_entry_rule(PhotoWhere { race: false, ..paused }), None);
        assert_eq!(photo_entry_rule(PhotoWhere { active: true, ..paused }), None);
        assert_eq!(photo_entry_rule(PhotoWhere { race: true, ..Default::default() }), None);
    }

    #[test]
    fn the_stamp_is_utc_and_sortable() {
        assert_eq!(utc_stamp(0), "1970-01-01_00-00-00");
        assert_eq!(utc_stamp(951_782_400 + 3661), "2000-02-29_01-01-01");
        assert_eq!(utc_stamp(1_791_000_000), "2026-10-03_04-00-00");
        assert_eq!(file_safe("alps/1 x"), "alps_1_x");
        assert_eq!(file_safe(""), "track");
    }
}
