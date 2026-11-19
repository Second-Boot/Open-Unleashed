//! PORT-DEVIATION D63 (`RE/notes/PORT-DEVIATIONS.md`): the race panel and the message of the enhancements' key (`0`). **Not the original's** - `nfs5.exe` has
//! neither. Render-side and UI only: nothing here is read by the simulation, a replay or the network.
//!
//! * **The panel** (`` ` `` opens and closes it in a race, a replay or photo mode): a compact box on the left edge with the frame
//!   statistics and a graph of the last frames' times at the top and the switches of the enhanced game below (`nfs_renderer::ENHANCEMENT_REGISTRY`), `[` / `]` select a row and
//!   `,` / `.` step its level through [`App::enh_step`] - the very path of the tab: the renderer takes it at once and
//!   `enhancements.json` keeps it. Closed at every race start, its state is never saved.
//! * **The message** (`0`, `App::enh_key`; it was F12 until F12 went to the original's `I_HORIZON`): for two seconds, in host time, near the top centre: whether the enhancements are on or off after the press.
//!
//! The keys are physical key codes (they work on any layout). They are read before the game's bindings: `` ` `` is taken even if the
//! player bound it in the controls screen, the other four only while the panel is open (a binding on them works with it closed).
//! A key's release always goes on to the input, so a key held when the panel took its press is not left held. Nothing is counted,
//! timed or drawn while the panel is closed and no message is up (the renderer's draw counters and GPU timestamps are on only
//! while it is open: `nfs_renderer::live_set`).
//!
//! **TEST GLUE** (not the original's): `NFS_RACE_PANEL=<tick>:<op>[,...]` (ops `open`, `close`, `row+`, `row-`, `lvl+`, `lvl-`, `enh`
//! (the enhancements' key), `<token>:<delta>` to step a row by its token, e.g. `E1:+1`) runs the panel's actions at game ticks, and
//! the `NFS_RACE_KEYS` tokens `bq`, `lb`, `rb`, `comma`, `period`, `0` press its keys (`photo.rs`).

use crate::*;
use nfs_game::enhanced;
use nfs_renderer::{ENHANCEMENT_REGISTRY as REGISTRY, EnhancementSwitch};
use std::collections::VecDeque;
use winit::keyboard::KeyCode;

/// How long the enhancements key's message stays up.
const TOAST: Duration = Duration::from_secs(2);
/// Frames kept for the statistics (about ten seconds at 60 fps).
const RING: usize = 720;
/// How often the numbers on the panel are renewed.
const REFRESH: Duration = Duration::from_millis(250);

/// The graph's height in lines of text.
const GRAPH_LINES: f32 = 4.0;
/// The time at the graph's top: a slower frame's bar stops there (its time is in the numbers above). The marks at 60 and 30 fps divide
/// the height in three equal parts.
const GRAPH_MAX_MS: f32 = 50.0;
const GRAPH_60_MS: f32 = 1000.0 / 60.0;
const GRAPH_30_MS: f32 = 1000.0 / 30.0;
/// The bars a graph about 240 pixels wide or wider shows at least (a narrower one: one a pixel).
const GRAPH_BARS: f32 = 240.0;

/// One line of the rows' list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelRow {
    pub name: String,
    pub level: String,
    /// the row is on but a row it needs is off: it does nothing now (the note says what it needs)
    pub idle: Option<String>,
}

/// The numbers at the top of the panel, renewed four times a second.
#[derive(Debug, Clone, Default)]
struct Numbers {
    fps: Option<f32>,
    avg_ms: Option<f32>,
    worst_ms: Option<f32>,
    low1: Option<f32>,
    tps: Option<f32>,
    draws: u32,
    tris: u64,
    gpu_total: Option<f64>,
    gpu_top: Vec<(String, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Open,
    Close,
    Row(i32),
    Level(i32),
    /// the enhancements' key (`enh_toggle_all`)
    Enh,
    Token(&'static str, i32),
}

/// A row changed by the panel whose cost is being watched: the slowest frame in the second after it.
struct Hitch {
    token: &'static str,
    level: u8,
    step_ms: f32,
    since: Instant,
    worst: f32,
    frames: u32,
}

#[derive(Default)]
pub(crate) struct RacePanel {
    open: bool,
    sel: usize,
    scroll: usize,
    font: Option<BitmapFont>,
    font_tried: bool,
    white: Option<TextureId>,
    /// `(when, seconds since the frame before)` of the last frames drawn with the panel open
    marks: VecDeque<(Instant, f32)>,
    last: Option<Instant>,
    tick_mark: Option<(Instant, u64)>,
    tps: Option<f32>,
    shown: Numbers,
    shown_at: Option<Instant>,
    toast: Option<(String, Instant)>,
    script: Option<Vec<(u64, Op)>>,
    script_at: usize,
    hitch: Option<Hitch>,
}

/// What the panel does with a key event.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Route {
    /// not the panel's: on to photo mode, the game's bindings and the input
    Pass,
    Toggle,
    Move(i32),
    Step(i32),
    /// taken and ignored (an auto-repeat of a key that acts once)
    Take,
}

/// The panel's key rules. A release is never taken (a key held when the panel took its press must not stay held in the input);
/// `` ` `` always works where the panel is usable, the other four only while it is open; `[` `]` repeat, the rest act once.
fn route(code: KeyCode, pressed: bool, repeat: bool, usable: bool, open: bool) -> Route {
    if !pressed || !usable {
        return Route::Pass;
    }
    match code {
        KeyCode::Backquote if repeat => Route::Take,
        KeyCode::Backquote => Route::Toggle,
        _ if !open => Route::Pass,
        KeyCode::BracketLeft => Route::Move(-1),
        KeyCode::BracketRight => Route::Move(1),
        KeyCode::Comma | KeyCode::Period if repeat => Route::Take,
        KeyCode::Comma => Route::Step(-1),
        KeyCode::Period => Route::Step(1),
        _ => Route::Pass,
    }
}

/// A frame's bar in the graph: its share of the graph's height and its colour. A frame as long as a mark, give or take a millisecond of
/// pacing jitter, still counts as meeting it.
fn graph_bar(ms: f32) -> (f32, [f32; 4]) {
    let colour = if ms <= GRAPH_60_MS + 1.0 {
        GREEN
    } else if ms <= GRAPH_30_MS + 1.0 {
        ORANGE
    } else {
        RED
    };
    ((ms / GRAPH_MAX_MS).clamp(0.0, 1.0), colour)
}

/// The pixel-space drawing of the panel: whole pixels of the render target, one font.
struct Pen<'a> {
    font: &'a BitmapFont,
    white: TextureId,
    w: f32,
    h: f32,
}

impl Pen<'_> {
    fn quad(&self, out: &mut Vec<Sprite>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        out.push(Sprite {
            texture: self.white,
            pos: [x / self.w, y / self.h],
            size: [w / self.h, h / self.h],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color,
            rotation: 0.0,
            pivot: [0.0, 0.0],
            tri: None,
            corner_colors: None,
            depth: None,
        });
    }

    /// The width of `s` drawn `size` pixels tall.
    fn width(&self, s: &str, size: f32) -> f32 {
        self.font.measure(s).0 * size / self.font.line_px()
    }

    /// `s` with its top-left at `(x, y)`; returns its width.
    fn text(&self, out: &mut Vec<Sprite>, s: &str, x: f32, y: f32, size: f32, color: [f32; 4]) -> f32 {
        let scale = size / self.font.line_px();
        let (glyphs, adv) = self.font.place(s);
        for g in &glyphs {
            out.push(Sprite {
                texture: self.font.texture(),
                pos: [(x + g.x * scale).round() / self.w, (y + g.y * scale).round() / self.h],
                size: [(g.w * scale).round().max(1.0) / self.h, (g.h * scale).round().max(1.0) / self.h],
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                color,
                rotation: 0.0,
                pivot: [0.0, 0.0],
                tri: None,
                corner_colors: None,
                depth: None,
            });
        }
        adv * scale
    }

    /// `s` cut from the end until it is no wider than `max`.
    fn fit(&self, s: &str, size: f32, max: f32) -> String {
        let mut t: String = s.to_string();
        while !t.is_empty() && self.width(&t, size) > max {
            t.pop();
        }
        t
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GREY: [f32; 4] = [0.62, 0.62, 0.62, 1.0];
const ORANGE: [f32; 4] = [0.98, 0.70, 0.0, 1.0];
const GREEN: [f32; 4] = [0.55, 0.9, 0.55, 1.0];
const RED: [f32; 4] = [0.95, 0.25, 0.2, 1.0];

fn ms(v: Option<f32>) -> String {
    v.map_or("-".to_string(), |v| format!("{v:.1}"))
}

fn count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1.0e6)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1.0e3)
    } else {
        n.to_string()
    }
}

impl App {
    fn ru(&self) -> bool {
        nfs_game::photo::is_russian(&self.fe.strings)
    }

    /// A port string in the install's language, as the game's fonts draw it.
    fn panel_str(&self, en: &str, ru: &str) -> String {
        nfs_game::photo::as_game_text(if self.ru() { ru } else { en })
    }

    /// The panel works where there is a race (a replay and photo mode included), never in the front end.
    fn panel_available(&self) -> bool {
        self.vehicle.is_some() && self.intro.is_none() && self.enh_mode != enhanced::Mode::HardOriginal
    }

    /// A race starts: the panel is closed (and its script starts over).
    pub(crate) fn panel_close(&mut self) {
        if self.panel.open {
            nfs_renderer::live_set(false);
        }
        self.panel.open = false;
        self.panel.script_at = 0;
        self.panel.hitch = None;
    }

    fn panel_set_open(&mut self, open: bool) {
        if self.panel.open == open {
            return;
        }
        self.panel.open = open;
        self.panel.marks.clear();
        self.panel.last = None;
        self.panel.tick_mark = None;
        self.panel.tps = None;
        self.panel.shown_at = None;
        self.panel.shown = Numbers::default();
        nfs_renderer::live_set(open);
        tracing::info!(open, "race panel (D63)");
    }

    /// A keyboard event of the window, before the game's bindings and before photo mode: `true` = the panel took it.
    pub(crate) fn panel_key_event(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let winit::keyboard::PhysicalKey::Code(code) = ev.physical_key else { return false };
        self.panel_key_code(code, ev.state == ElementState::Pressed, ev.repeat)
    }

    /// [`Self::panel_key_event`] after the winit decode (the scripted `NFS_RACE_KEYS` glue calls this one).
    pub(crate) fn panel_key_code(&mut self, code: KeyCode, pressed: bool, repeat: bool) -> bool {
        let usable = self.panel_available() && !self.replay_save.up;
        match route(code, pressed, repeat, usable, self.panel.open) {
            Route::Pass => return false,
            Route::Toggle => self.panel_set_open(!self.panel.open),
            Route::Move(d) => self.panel_move(d),
            Route::Step(d) => self.panel_step(d),
            Route::Take => {}
        }
        true
    }

    fn panel_move(&mut self, d: i32) {
        let n = REGISTRY.len() as i32;
        self.panel.sel = (self.panel.sel as i32 + d).rem_euclid(n.max(1)) as usize;
    }

    fn panel_step(&mut self, d: i32) {
        let Some(e) = REGISTRY.get(self.panel.sel) else { return };
        self.panel_hitch_log();
        let t0 = Instant::now();
        // a step during the original-look view (the `0` key) moves the player's own set, not the all-off one the view shows: the set is never lost to a stray key
        if let Some(own) = self.enh_stash.take() {
            self.enh_set(own, false);
        }
        self.enh_step(&format!("{}{}", enhanced::ACCESSOR_PREFIX, e.token), d);
        let level = self.enh.level(EnhancementSwitch(self.panel.sel as u8));
        self.panel.hitch = Some(Hitch { token: e.token, level, step_ms: t0.elapsed().as_secs_f32() * 1000.0, since: Instant::now(), worst: 0.0, frames: 0 });
    }

    /// The rows of the list, in the install's language: one function so that more rows (the registry's next ones) need nothing else.
    pub(crate) fn panel_rows(&self) -> Vec<PanelRow> {
        let l = self.ru() as usize;
        let s = self.enh;
        REGISTRY
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let level = e.levels.get(s.shown_level(EnhancementSwitch(i as u8)) as usize).map_or("", |c| c[l]);
                let idle = s.blocked(e.token).map(|b| nfs_game::photo::as_game_text(&enhanced::reason_text(&nfs_renderer::ENHANCEMENT_DEPENDS[b.dep], b.by, l == 1)));
                PanelRow { name: nfs_game::photo::as_game_text(e.name[l]), level: nfs_game::photo::as_game_text(level), idle }
            })
            .collect()
    }

    /// The enhancements key's message: what the press did.
    pub(crate) fn panel_toast(&mut self, on: bool, nothing_to_compare: bool) {
        let t = match (on, nothing_to_compare) {
            (true, _) => self.panel_str("enhancements on", "улучшения вкл"),
            (false, false) => self.panel_str("enhancements off", "улучшения выкл"),
            (false, true) => self.panel_str("enhancements off (every row is off)", "улучшения выкл (все строки выключены)"),
        };
        self.panel.toast = Some((t, Instant::now()));
    }

    /// Once a frame, first thing: the frame time of the panel's statistics and the watch on a changed row.
    pub(crate) fn panel_frame_mark(&mut self) {
        if !self.panel.open {
            return;
        }
        let now = Instant::now();
        if let Some(prev) = self.panel.last {
            let dt = now.duration_since(prev).as_secs_f32();
            self.panel.marks.push_back((now, dt));
            while self.panel.marks.len() > RING {
                self.panel.marks.pop_front();
            }
            if let Some(h) = self.panel.hitch.as_mut() {
                h.worst = h.worst.max(dt);
                h.frames += 1;
            }
        }
        self.panel.last = Some(now);
        if self.panel.hitch.as_ref().is_some_and(|h| h.since.elapsed() >= Duration::from_millis(1500)) {
            self.panel_hitch_log();
        }
    }

    /// Report the watch on the last changed row: the step's own time and the slowest frame after it.
    fn panel_hitch_log(&mut self) {
        if let Some(h) = self.panel.hitch.take() {
            tracing::info!(row = h.token, level = h.level, step_ms = h.step_ms, worst_frame_ms = h.worst * 1000.0, frames = h.frames, "race panel: a row changed (D63)");
        }
    }

    fn panel_numbers(&mut self) {
        let now = Instant::now();
        if self.panel.shown_at.is_some_and(|t| now.duration_since(t) < REFRESH) {
            return;
        }
        self.panel.shown_at = Some(now);
        let p = &self.panel;
        let second: Vec<f32> = p.marks.iter().filter(|(t, _)| now.duration_since(*t) <= Duration::from_secs(1)).map(|(_, d)| *d).collect();
        let mut n = Numbers::default();
        if second.len() >= 2 {
            n.fps = Some(second.len() as f32);
            n.avg_ms = Some(second.iter().sum::<f32>() / second.len() as f32 * 1000.0);
            n.worst_ms = Some(second.iter().cloned().fold(0.0, f32::max) * 1000.0);
        }
        // 1 % low: the mean of the slowest hundredth of the last ~10 s of frames (needs a hundred of them to mean anything)
        if p.marks.len() >= 100 {
            let mut all: Vec<f32> = p.marks.iter().map(|(_, d)| *d).collect();
            all.sort_by(|a, b| b.total_cmp(a));
            let k = (all.len() / 100).max(1);
            let mean = all[..k].iter().sum::<f32>() / k as f32;
            if mean > 0.0 {
                n.low1 = Some(1.0 / mean);
            }
        }
        // game ticks a second over the last second or more
        match self.panel.tick_mark {
            Some((t0, k0)) if now.duration_since(t0) >= Duration::from_secs(1) => {
                self.panel.tps = Some((self.tick.saturating_sub(k0)) as f32 / now.duration_since(t0).as_secs_f32());
                self.panel.tick_mark = Some((now, self.tick));
            }
            None => self.panel.tick_mark = Some((now, self.tick)),
            _ => {}
        }
        n.tps = self.panel.tps;
        let live = nfs_renderer::live_stats();
        n.draws = live.draws;
        n.tris = live.tris;
        n.gpu_total = live.gpu_total_ms;
        let mut top = live.gpu_passes;
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        top.truncate(3);
        n.gpu_top = top;
        tracing::debug!(fps = ?n.fps, avg_ms = ?n.avg_ms, worst_ms = ?n.worst_ms, low1 = ?n.low1, tps = ?n.tps, gpu_ms = ?n.gpu_total, draws = n.draws, tris = n.tris, top = ?n.gpu_top, "race panel numbers");
        self.panel.shown = n;
    }

    /// Load the panel's font once, when something is to be drawn.
    fn panel_font_ready(&mut self) {
        if self.panel.font.is_some() || self.panel.font_tried {
            return;
        }
        let Some(r) = self.renderer.as_mut() else { return };
        self.panel.font_tried = true;
        self.panel.white = Some(r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255])));
        let dir = self.paths.game_data().join("DashHud");
        self.panel.font = GamePaths::find_file(&dir, "UniversC12b.ffn")
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
            .map(|f| BitmapFont::upload(r, f));
        if self.panel.font.is_none() {
            tracing::warn!("race panel: UniversC12b.ffn not found, no text");
        }
    }

    fn toast_alive(&self) -> bool {
        self.panel.toast.as_ref().is_some_and(|(_, t)| self.glue.shot_target.is_some() || t.elapsed() < TOAST)
    }

    /// Append the panel and the message to the scene's sprites (after everything else, so photo mode's still cuts them off).
    pub(crate) fn panel_overlay(&mut self) {
        if !self.toast_alive() {
            self.panel.toast = None;
        }
        let open = self.panel.open && self.panel_available();
        if self.panel.open && !open {
            self.panel_close();
        }
        if !open && self.panel.toast.is_none() {
            return;
        }
        self.panel_font_ready();
        if open {
            self.panel_numbers();
        }
        // the window's pixels (a scripted still: its own size)
        let (w, h, _) = self.hud_geometry();
        let (Some(font), Some(white)) = (self.panel.font.as_ref(), self.panel.white) else { return };
        let pen = Pen { font, white, w: w as f32, h: h as f32 };
        let mut out = Vec::new();
        // text height: 11 px at 480 lines, 17 at 800, 25 at 1200, at most 40
        let size = (h as f32 / 48.0).round().clamp(11.0, 40.0);
        if let Some((t, _)) = &self.panel.toast {
            let ts = (size * 1.35).round();
            let tw = pen.width(t, ts);
            let (bw, bh) = (tw + ts * 1.2, ts * 1.6);
            let (x, y) = (((w as f32 - bw) / 2.0).round(), (h as f32 * 0.05).round());
            pen.quad(&mut out, x, y, bw, bh, [0.0, 0.0, 0.0, 0.6]);
            pen.text(&mut out, t, (x + ts * 0.6).round(), (y + ts * 0.3).round(), ts, WHITE);
        }
        if open {
            self.panel_draw(&pen, &mut out, size);
        }
        self.scene.sprites.extend(out);
    }

    /// The panel itself.
    fn panel_draw(&self, pen: &Pen, out: &mut Vec<Sprite>, size0: f32) {
        let n = &self.panel.shown;
        let (sw, sh) = (pen.w, pen.h);
        let rows = self.panel_rows();
        let s = |en: &str, ru: &str| self.panel_str(en, ru);
        // the statistics
        let mut stats: Vec<(String, [f32; 4])> = Vec::new();
        stats.push((format!("{} {}   {} {} / {} ms", s("FPS", "FPS"), n.fps.map_or("-".into(), |v| format!("{v:.0}")), s("frame", "кадр"), ms(n.avg_ms), ms(n.worst_ms)), WHITE));
        stats.push((format!("{} {} fps   {} {}", s("1% low", "1% худшие"), n.low1.map_or("-".into(), |v| format!("{v:.0}")), s("tick/s", "тик/с"), n.tps.map_or("-".into(), |v| format!("{v:.1}"))), WHITE));
        let gpu = n.gpu_total.map_or("-".to_string(), |v| format!("{v:.2}"));
        stats.push((format!("GPU {gpu} ms   {} {}   {} {}", s("draws", "вызовов"), if nfs_renderer::timestamps_supported() || n.draws > 0 { n.draws.to_string() } else { "-".into() }, s("tris", "треуг."), if n.tris > 0 { count(n.tris) } else { "-".into() }), WHITE));
        for (label, v) in &n.gpu_top {
            stats.push((format!("  {label}  {v:.2} ms"), GREY));
        }
        let (rw, rh) = (pen.w as u32, pen.h as u32);
        // an upscaler sets the internal size itself and the render scale row is not in effect: not this panel's number
        let inner = if self.enh.temporal_upscaler() {
            "-".to_string()
        } else {
            let scale = nfs_renderer::render_scale(self.enh.level(nfs_renderer::E3));
            format!("{}x{}", (rw as f32 * scale).round() as u32, (rh as f32 * scale).round() as u32)
        };
        let (ww, wh) = (rw, rh);
        stats.push((format!("3D {inner}   {} {ww}x{wh}", s("window", "окно")), GREY));
        stats.push((nfs_renderer::ADAPTER_LABEL.get().cloned().unwrap_or_else(|| "-".into()), GREY));
        let speed = self.vehicle.as_ref().map(|v| (v.speed() * 3.6).round() as i32);
        let lap = self.race.as_ref().map(|r| format!("{}/{}", r.laps + 1, self.num_laps.max(1)));
        stats.push((format!("{} {}   {} {}   {} {}", s("tick", "тик"), self.tick, speed.map_or("-".into(), |v| v.to_string()), s("km/h", "км/ч"), s("lap", "круг"), lap.unwrap_or_else(|| "-".into())), WHITE));
        stats.push((format!("{} {}   {} {}", s("track", "трасса"), self.track.name, s("car", "машина"), self.player_car_id.map_or("-".into(), |c| c.to_string())), WHITE));
        let keys = s("` close  [ ] row  , . change", "` закрыть  [ ] строка  , . изменить");
        let view = self.enh_stash.is_some().then(|| s("0 view: the original's look", "0: вид оригинала"));

        // the box: at most a third of the window wide, the text shrinks to fit
        let pad = (size0 * 0.5).round();
        let max_w = (sw * 0.34 - 2.0 * pad).max(100.0);
        let widest = |size: f32| -> f32 {
            let mut m = stats.iter().map(|(t, _)| pen.width(t, size)).fold(0.0f32, f32::max);
            m = m.max(pen.width(&keys, size));
            for r in &rows {
                m = m.max(pen.width(&format!("{}  < {} >", r.name, r.level), size));
            }
            m
        };
        let mut size = size0;
        let want = widest(size);
        if want > max_w {
            size = (size * max_w / want).floor().max(8.0);
        }
        let content_w = widest(size).min(max_w);
        let line = (size * 1.25).round();
        let box_w = (content_w + 2.0 * pad).round();
        // the height: the statistics, a gap, the rows that fit, the key line
        let top = (size * 0.5).round();
        let avail = (sh * 0.92 - top - 2.0 * pad).max(line * 6.0);
        let graph_h = (line * GRAPH_LINES).round();
        let fixed = line * (stats.len() as f32 + 2.5 + view.is_some() as u32 as f32) + graph_h;
        let fit = (((avail - fixed) / line).floor() as usize).clamp(3, rows.len().max(3));
        let visible = fit.min(rows.len());
        // the first visible row keeps the selected one in view
        let sel = self.panel.sel.min(rows.len().saturating_sub(1));
        let mut first = self.panel.scroll.min(rows.len().saturating_sub(visible));
        if sel < first {
            first = sel;
        } else if sel >= first + visible {
            first = sel + 1 - visible;
        }
        let more_above = first > 0;
        let more_below = first + visible < rows.len();
        let box_h = (fixed + line * visible as f32 + 2.0 * pad).round();
        let (x0, y0) = (top, top);
        pen.quad(out, x0, y0, box_w, box_h, [0.0, 0.0, 0.0, 0.62]);
        let (tx, mut ty) = (x0 + pad, y0 + pad);
        for (i, (t, c)) in stats.iter().enumerate() {
            let t = pen.fit(t, size, content_w);
            pen.text(out, &t, tx, ty, size, *c);
            ty += line;
            // under the two lines of frame numbers
            if i == 1 {
                self.panel_graph(pen, out, tx, ty + (line * 0.25).round(), content_w, graph_h, (size * 0.8).round());
                ty += (line * 0.5).round() + graph_h;
            }
        }
        ty += line * 0.5;
        if let Some(v) = &view {
            pen.text(out, v, tx, ty - line * 0.5, size, GREEN);
            ty += line;
        }
        for (i, r) in rows.iter().enumerate().skip(first).take(visible) {
            let selected = i == sel;
            if selected {
                pen.quad(out, x0 + 2.0, ty - 1.0, box_w - 4.0, line, [0.98, 0.55, 0.0, 0.35]);
            }
            let colour = if selected { ORANGE } else if r.idle.is_some() { GREY } else { WHITE };
            let lvl = if selected && r.idle.is_none() { format!("< {} >", r.level) } else { r.level.clone() };
            let mut note = String::new();
            if r.idle.is_some() {
                note.push('*');
            }
            let lw = pen.width(&lvl, size);
            let name = pen.fit(&format!("{}{}", r.name, note), size, (content_w - lw - size).max(size));
            pen.text(out, &name, tx, ty, size, colour);
            pen.text(out, &lvl, tx + content_w - lw, ty, size, colour);
            ty += line;
        }
        // the key line; scroll marks and the selected row's note share it
        let tail = rows.get(sel).and_then(|r| r.idle.clone()).map(|t| format!("* {t}")).unwrap_or(keys);
        let tail = pen.fit(&tail, size, content_w - size * 1.5);
        pen.text(out, &tail, tx, ty + line * 0.25, size, GREY);
        if more_above || more_below {
            let m = format!("{}{}", if more_above { "^" } else { "" }, if more_below { "v" } else { "" });
            let mw = pen.width(&m, size);
            pen.text(out, &m, tx + content_w - mw, ty + line * 0.25, size, GREY);
        }
        let _ = more_above;
    }

    /// The graph of the last frames' times, renewed every frame: one bar a frame, the newest on the right, with marks at 60 and 30 fps.
    fn panel_graph(&self, pen: &Pen, out: &mut Vec<Sprite>, x: f32, y: f32, w: f32, h: f32, size: f32) {
        let w = w.floor().max(1.0);
        let bar_w = (w / GRAPH_BARS).floor().max(1.0);
        let bars = (w / bar_w) as usize;
        pen.quad(out, x, y, w, h, [0.0, 0.0, 0.0, 0.45]);
        let bottom = y + h;
        for (k, (_, dt)) in self.panel.marks.iter().rev().take(bars).enumerate() {
            let (share, colour) = graph_bar(dt * 1000.0);
            let bar_h = (share * h).round().max(1.0);
            pen.quad(out, x + w - (k + 1) as f32 * bar_w, bottom - bar_h, bar_w, bar_h, colour);
        }
        for (mark, label) in [(GRAPH_60_MS, "16.7"), (GRAPH_30_MS, "33.3"), (GRAPH_MAX_MS, "50 ms")] {
            let my = (bottom - mark / GRAPH_MAX_MS * h).round();
            pen.quad(out, x, my, w, 1.0, [1.0, 1.0, 1.0, 0.4]);
            pen.text(out, label, x + 2.0, my + 1.0, size, WHITE);
        }
    }

    // ---- TEST GLUE ----

    fn panel_parse_script() -> Vec<(u64, Op)> {
        let Some(spec) = nfs_game::config::cfg().nfs_race_panel.as_str() else { return Vec::new() };
        let mut v: Vec<(u64, Op)> = spec
            .split(',')
            .filter_map(|e| {
                let (t, op) = e.trim().split_once(':')?;
                let t = t.trim().parse::<u64>().ok()?;
                let op = op.trim();
                let op = match op {
                    "open" => Op::Open,
                    "close" => Op::Close,
                    "row+" => Op::Row(1),
                    "row-" => Op::Row(-1),
                    "lvl+" => Op::Level(1),
                    "lvl-" => Op::Level(-1),
                    "enh" => Op::Enh,
                    other => {
                        let (tok, d) = other.split_once(':')?;
                        let tok = REGISTRY.iter().find(|e| e.token.eq_ignore_ascii_case(tok.trim()))?.token;
                        Op::Token(tok, d.trim().parse().ok()?)
                    }
                };
                Some((t, op))
            })
            .collect();
        v.sort_by_key(|(t, _)| *t);
        v
    }

    /// **TEST GLUE** `NFS_RACE_PANEL`: the actions whose tick has come. Called from the scene build and from the scripted run's loop.
    pub(crate) fn panel_script_step(&mut self) {
        if !nfs_game::config::cfg().nfs_race_panel.is_set() || self.vehicle.is_none() {
            return;
        }
        if self.panel.script.is_none() {
            self.panel.script = Some(Self::panel_parse_script());
        }
        while let Some(&(t, op)) = self.panel.script.as_ref().and_then(|s| s.get(self.panel.script_at)) {
            if t > self.tick {
                break;
            }
            self.panel.script_at += 1;
            match op {
                Op::Open => self.panel_set_open(true),
                Op::Close => self.panel_set_open(false),
                Op::Row(d) => self.panel_move(d),
                Op::Level(d) => self.panel_step(d),
                Op::Enh => self.enh_toggle_all(),
                Op::Token(tok, d) => {
                    self.enh_step(&format!("{}{tok}", enhanced::ACCESSOR_PREFIX), d);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's rows are the registry's, each with a caption and a level caption in both languages.
    #[test]
    fn every_registry_row_has_text() {
        for e in REGISTRY {
            assert!(!e.name[0].is_empty() && !e.name[1].is_empty());
            assert!(e.levels.iter().all(|l| !l[0].is_empty() && !l[1].is_empty()));
        }
    }

    /// Keys: only the five are the panel's; releases and everything else go on; closed, only the backquote is taken.
    #[test]
    fn the_panel_takes_only_its_keys() {
        use KeyCode::*;
        for k in [KeyA, ArrowUp, Space, F12, Escape, KeyK, F11, Digit1, Digit0, Tab, Enter, Slash] {
            for (usable, open) in [(true, true), (true, false), (false, false)] {
                assert_eq!(route(k, true, false, usable, open), Route::Pass, "{k:?}");
            }
        }
        // closed: the backquote only
        assert_eq!(route(Backquote, true, false, true, false), Route::Toggle);
        for k in [BracketLeft, BracketRight, Comma, Period] {
            assert_eq!(route(k, true, false, true, false), Route::Pass);
        }
        // open
        assert_eq!(route(Backquote, true, false, true, true), Route::Toggle);
        assert_eq!(route(BracketLeft, true, false, true, true), Route::Move(-1));
        assert_eq!(route(BracketRight, true, true, true, true), Route::Move(1));
        assert_eq!(route(Comma, true, false, true, true), Route::Step(-1));
        assert_eq!(route(Period, true, false, true, true), Route::Step(1));
        assert_eq!(route(Period, true, true, true, true), Route::Take);
        assert_eq!(route(Backquote, true, true, true, true), Route::Take);
        // a release never: the input must see it
        for k in [Backquote, BracketLeft, BracketRight, Comma, Period] {
            assert_eq!(route(k, false, false, true, true), Route::Pass);
            // and not usable (the front end, a typed name): nothing is taken
            assert_eq!(route(k, true, false, false, true), Route::Pass);
        }
    }

    /// A frame's bar: the height grows with the time and stops at the top; the colour says which mark the frame met, with a millisecond's
    /// grace for pacing jitter.
    #[test]
    fn a_bar_tells_the_frame_time() {
        assert_eq!(graph_bar(0.0), (0.0, GREEN));
        assert_eq!(graph_bar(25.0).0, 0.5);
        assert_eq!(graph_bar(500.0), (1.0, RED));
        assert_eq!(graph_bar(GRAPH_60_MS + 0.5).1, GREEN);
        assert_eq!(graph_bar(GRAPH_60_MS + 1.5).1, ORANGE);
        assert_eq!(graph_bar(GRAPH_30_MS + 0.5).1, ORANGE);
        assert_eq!(graph_bar(GRAPH_30_MS + 1.5).1, RED);
    }

    #[test]
    fn counts_read_short() {
        assert_eq!(count(999), "999");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(count(1_234_567), "1.23M");
        assert_eq!(ms(None), "-");
        assert_eq!(ms(Some(16.666)), "16.7");
    }
}
