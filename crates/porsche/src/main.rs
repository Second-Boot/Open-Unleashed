//! Native macOS application entry point.
//!
//! Timing model (CONFIRMED by disassembly, `RE/notes/timing.md`): the original runs a fixed-step
//! game tick at **64 Hz** (15.625 ms, `GameTick` = 0x414FF0), paced by a 128 Hz multimedia timer;
//! controller input is latched at 32 Hz. No delta-time scaling exists in the original.
//!
//! Current state (M5 in progress): loads a real track and a real car, places the car at the
//! forward start pose and runs the recovered vehicle model (`nfs-physics::vehicle`) from
//! keyboard/gamepad input, with a chase camera.
//!
//! ```text
//! porsche                                  # menu: choose track and car, Enter to race
//! porsche [track] [car] [sim] [--screenshot out.png] [--autotest N [--follow]] [--opponents N] [--record run.csv]
//! keys: arrows = steer/throttle/brake, space = handbrake, C = camera, R = reset to track, Esc = quit
//! ```

mod career_ui;

use anyhow::Result;
use career_ui::{CareerOpponent, CareerUi};
use glam::{Mat4, Quat, Vec3};
use nfs_assets::{car::CarAssets, track::TrackAssets, GamePaths};
use nfs_game::{race::{grid_pose, RaceProgress, SliceTable}, racecar::{Controller, RaceCar}, records::{format_ticks, Records}, skill::{SkillEvent, SkillTest}, sky::Sky, text::BitmapFont, world::{CarModel, TrackWorld}};
use nfs_input::{gamepad::Gamepads, Action, InputState};
use nfs_physics::{specs::CarSpecs, vehicle::{Controls, Vehicle}};
use nfs_renderer::{Camera, Renderer, Scene, Sprite, TextureData, TextureId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// In-race HUD (`RE/notes/hud.md`, CONFIRMED): dial faces and needles from `nfs5.car` dial
/// records (`speedK/tach.fsh` entry = style, `needle2/needle.fsh` entry = needle style), 128 px
/// dials bottom-right, needle sweep `φ = start + v·deg` calibrated clockwise-from-12 as `φ − 90°`
/// (Boxster/996 records `−φ − 90°`, HYPOTHESIS on the projection sign), texts in `UniversC12b`.
/// Not reproduced: translucent trays, slide-in, track map, mirror.
struct Hud {
    dial: TextureId,
    /// speedometer needle: texture, (width, height) px, pivot px above the bottom
    needle: (TextureId, f32, f32, f32),
    hub: Option<TextureId>,
    kmh: nfs_formats::fedata::DialSpec,
    tach_spec: nfs_formats::fedata::DialSpec,
    /// countdown digits 3, 2, 1 (solid) from `321go.fsh` / `321go2.fsh`
    digits: [Option<TextureId>; 3],
    rings: Option<(TextureId, TextureId)>,
    font: Option<BitmapFont>,
    tach: Option<(TextureId, (TextureId, f32, f32, f32))>,
    /// 1×1 white texture for the translucent trays (HTray: dark box + orange outline)
    white: TextureId,
    /// track map (120×120 @ (0,358) in the original, an HRenderVisual top-down render; here the
    /// lap route rasterised once — MEDIUM) and its world→map projection (x0, z0, scale)
    map: Option<(TextureId, [f32; 3])>,
}

const MAP_PX: u32 = 120;

/// Race-start countdown (CONFIRMED values, `RE/notes/timing.md`): game ticks 256/320/384/448 =
/// 4/5/6/7 s; the race starts at tick 448 and brakes are forced until then.
const RACE_START_TICK: u64 = 448;
const COUNTDOWN_TICKS: [u64; 3] = [256, 320, 384];
/// HUD reference height (px) — sizes are fixed pixels in the original (hud.md §2).
const HUD_REF_H: f32 = 480.0;
const HUD_ORANGE: [f32; 4] = [0.98, 0.706, 0.0, 1.0];
const HUD_GREY: [f32; 4] = [0.706, 0.706, 0.706, 1.0];

impl Hud {
    fn load(r: &mut Renderer, paths: &GamePaths, dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>) -> Option<Self> {
        let dir = paths.game_data().join("DashHud");
        let load_idx = |r: &mut Renderer, file: &str, index: Option<usize>, entry: Option<&str>| -> Option<(TextureId, f32, f32)> {
            let p = GamePaths::find_file(&dir, file)?;
            let d = std::fs::read(p).ok()?;
            let f = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let i = match (index, entry) {
                (Some(i), _) if i < f.entries.len() => i,
                (_, Some(e)) => f.entries.iter().position(|x| x.name.trim() == e)?,
                _ => return None,
            };
            let img = f.decode(&d, i).ok()?;
            Some((r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)), img.width as f32, img.height as f32))
        };
        // defaults = '94 993 (style 4 / tach 6 / needle 2, pivot 18) when the car record is unknown
        let (kmh, tach_spec) = dials.unwrap_or((
            nfs_formats::fedata::DialSpec { style: 4, total_units: 300, needle_style: 2, start_deg: -40, deg_per_unit_x1000: 910, needle_pivot: 18 },
            nfs_formats::fedata::DialSpec { style: 6, total_units: 800, needle_style: 2, start_deg: -18, deg_per_unit_x1000: 320, needle_pivot: 18 },
        ));
        let needle_style = tach_spec.needle_style.max(0) as usize;
        let pivot = tach_spec.needle_pivot.max(0) as f32;
        let dial = load_idx(r, "speedK.fsh", Some(kmh.style.max(0) as usize), Some("300k"))?.0;
        let n = load_idx(r, "needle2.fsh", Some(needle_style), Some("need"))?;
        let hub = (needle_style == 0).then(|| load_idx(r, "needle2.fsh", None, Some("cent")).map(|h| h.0)).flatten();
        let tach = load_idx(r, "tach.fsh", Some(tach_spec.style.max(0) as usize), Some("964")).zip(load_idx(r, "needle.fsh", Some(needle_style), Some("need"))).map(|(t, nn)| (t.0, (nn.0, nn.1, nn.2, pivot)));
        let digits = [load_idx(r, "321go.fsh", None, Some("3Sol")).map(|x| x.0), load_idx(r, "321go.fsh", None, Some("2Sol")).map(|x| x.0), load_idx(r, "321go2.fsh", None, Some("1Sol")).map(|x| x.0)];
        let rings = load_idx(r, "321go.fsh", None, Some("circlefull")).zip(load_idx(r, "321go.fsh", None, Some("circleoutline"))).map(|(a, b)| (a.0, b.0));
        let font = GamePaths::find_file(&dir, "UniversC12b.ffn").and_then(|p| std::fs::read(p).ok()).and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()).map(|f| BitmapFont::upload(r, f));
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Some(Self { dial, needle: (n.0, n.1, n.2, pivot), hub, kmh, tach_spec, digits, rings, font, tach, white, map: None })
    }

    /// Rasterise the lap route into the map texture (north up, fitted with a 6 px margin).
    fn set_map(&mut self, r: &mut Renderer, table: &SliceTable) {
        let pts: Vec<(f32, f32)> = table.route.iter().map(|&i| (table.slices[i].centre.x, table.slices[i].centre.z)).collect();
        if pts.len() < 2 {
            return;
        }
        let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, z) in &pts {
            x0 = x0.min(x); x1 = x1.max(x); z0 = z0.min(z); z1 = z1.max(z);
        }
        let span = (x1 - x0).max(z1 - z0).max(1.0);
        let scale = (MAP_PX as f32 - 12.0) / span;
        let cx = (x0 + x1) * 0.5;
        let cz = (z0 + z1) * 0.5;
        let origin_x = cx - (MAP_PX as f32 * 0.5) / scale;
        let origin_z = cz - (MAP_PX as f32 * 0.5) / scale;
        let mut img = vec![0u8; (MAP_PX * MAP_PX * 4) as usize];
        let mut plot = |x: i32, y: i32| {
            if x >= 0 && y >= 0 && (x as u32) < MAP_PX && (y as u32) < MAP_PX {
                let o = ((y as u32 * MAP_PX + x as u32) * 4) as usize;
                img[o..o + 4].copy_from_slice(&[255, 255, 255, 220]);
            }
        };
        let to_px = |(x, z): (f32, f32)| (((x - origin_x) * scale) as i32, ((z - origin_z) * scale) as i32);
        for w in pts.windows(2).chain(std::iter::once(&[pts[pts.len() - 1], pts[0]][..])) {
            let ((ax, ay), (bx, by)) = (to_px(w[0]), to_px(w[1]));
            if (ax - bx).abs() > MAP_PX as i32 / 2 || (ay - by).abs() > MAP_PX as i32 / 2 {
                continue; // point-to-point tracks: no closing line
            }
            let steps = (ax - bx).abs().max((ay - by).abs()).max(1);
            for t in 0..=steps {
                plot(ax + (bx - ax) * t / steps, ay + (by - ay) * t / steps);
            }
        }
        let tex = r.upload_texture(&TextureData::single(MAP_PX, MAP_PX, img));
        self.map = Some((tex, [origin_x, origin_z, scale]));
    }

    /// Map tray with the route and car dots (player orange, others grey).
    fn map_sprites(&self, out: &mut Vec<Sprite>, player: Vec3, others: &[Vec3]) {
        let Some((tex, [ox, oz, scale])) = self.map else { return };
        let px = 1.0 / HUD_REF_H;
        self.tray(out, 0.0, 358.0, MAP_PX as f32, MAP_PX as f32);
        let size = MAP_PX as f32 * px;
        out.push(Sprite { texture: tex, pos: [0.0, 358.0 / 480.0], size: [size, size], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] });
        let mut dot = |p: Vec3, color: [f32; 4], d: f32| {
            let (mx, my) = ((p.x - ox) * scale, (p.z - oz) * scale);
            if mx < 0.0 || my < 0.0 || mx > MAP_PX as f32 || my > MAP_PX as f32 {
                return;
            }
            out.push(Sprite { texture: self.white, pos: [(mx - d * 0.5) * px / ASPECT_GUESS, (358.0 + my - d * 0.5) * px], size: [d * px, d * px], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5] });
        };
        for &o in others {
            dot(o, [0.75, 0.75, 0.75, 1.0], 3.0);
        }
        dot(player, HUD_ORANGE, 4.0);
    }

    /// HTray (0x434110): translucent dark box with a 1 px outline in the display colour (alpha 0x3F).
    fn tray(&self, out: &mut Vec<Sprite>, x: f32, y: f32, w: f32, h: f32) {
        let px = 1.0 / HUD_REF_H;
        let (x, y, w, h) = (x / 640.0, y / 480.0, w * px, h * px);
        let quad = |x: f32, y: f32, w: f32, h: f32, color: [f32; 4]| Sprite { texture: self.white, pos: [x, y], size: [w, h], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5] };
        out.push(quad(x, y, w, h, [0.0, 0.0, 0.0, 0.45]));
        let o = [0.98, 0.706, 0.0, 0.25];
        let line = px;
        out.push(quad(x, y, w, line, o));
        out.push(quad(x, y + h - line, w, line, o));
        out.push(quad(x, y, line, h, o));
        out.push(quad(x + w - line / ASPECT_GUESS, y, line, h, o));
    }

    /// Screen rotation (turns, counter-clockwise positive) for a dial value.
    fn needle_turns(spec: &nfs_formats::fedata::DialSpec, value: i32) -> f32 {
        let phi = spec.angle(value);
        // clockwise-from-12 = φ − 90° for the classic faces (hud.md §3.3). The Boxster / 996 records
        // (start > 0) rest at 270° − start on their faces and sweep clockwise from there; the
        // projection sign was not traced, so this is calibrated against the face (H32).
        let cw = if spec.start_deg > 0 { (270.0 - spec.start_deg as f32) + (phi - spec.start_deg as f32) } else { phi - 90.0 };
        // sprite rotation is clockwise-positive on screen (the renderer mirrors clip x)
        cw / 360.0
    }

    fn dial_sprites(&self, out: &mut Vec<Sprite>, face: TextureId, needle: (TextureId, f32, f32, f32), centre: [f32; 2], spec: &nfs_formats::fedata::DialSpec, value: i32) {
        let px = 1.0 / HUD_REF_H;
        let size = 128.0 * px;
        out.push(Sprite { texture: face, pos: [centre[0] - size * 0.5 / ASPECT_GUESS, centre[1] - size * 0.5], size: [size, size], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] });
        let (tex, nw, nh, pivot) = needle;
        let (w, h) = (nw * px, nh * px);
        let piv_y = 1.0 - pivot / nh;
        out.push(Sprite { texture: tex, pos: [centre[0] - w * 0.5 / ASPECT_GUESS, centre[1] - h * piv_y], size: [w, h], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: Self::needle_turns(spec, value), pivot: [0.5, piv_y] });
        if let Some(hub) = self.hub {
            let hs = 20.0 * px;
            out.push(Sprite { texture: hub, pos: [centre[0] - hs * 0.5 / ASPECT_GUESS, centre[1] - hs * 0.5], size: [hs, hs], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sprites(&self, speed_kmh: f32, rpm: f32, tick: u64, gear: usize, race: Option<&RaceProgress>, num_laps: u32, position: Option<(usize, usize)>) -> Vec<Sprite> {
        let mut out = Vec::new();
        // dials: speedometer tray (392,352) and tach (512,307) at 640×480, 128 px faces (hud.md §2/§6)
        let speedo_centre = [(392.0 + 64.0) / 640.0, (352.0 + 64.0) / 480.0];
        self.dial_sprites(&mut out, self.dial, self.needle, speedo_centre, &self.kmh, speed_kmh as i32);
        if let Some((td, tn)) = self.tach {
            let tach_centre = [(512.0 + 64.0) / 640.0, (307.0 + 64.0) / 480.0];
            self.dial_sprites(&mut out, td, tn, tach_centre, &self.tach_spec, (rpm / 10.0) as i32);
        }
        if let Some(f) = &self.font {
            let px = 1.0 / HUD_REF_H;
            // trays: digital speed 73×35 @ (460,444), gear 85×35 @ (555,444), time 73×35 @ (427,33), position 138×15 @ (502,1)
            self.tray(&mut out, 460.0, 444.0, 73.0, 35.0);
            self.tray(&mut out, 555.0, 444.0, 85.0, 35.0);
            if race.is_some() {
                self.tray(&mut out, 427.0, 33.0, 73.0, 35.0);
                self.tray(&mut out, 502.0, 1.0, 138.0, 15.0);
            }
            // digital speed / gear: value top-right orange, label bottom-left grey
            out.extend(f.sprites(&format!("{:03}", speed_kmh as i32), [500.0 / 640.0, 446.0 / 480.0], 16.0 * px, HUD_ORANGE));
            out.extend(f.sprites("km/h", [462.0 / 640.0, 464.0 / 480.0], 10.0 * px, HUD_GREY));
            let gear_s = match gear { 0 => "R".to_string(), 1 => "N".to_string(), g => (g - 1).to_string() };
            out.extend(f.sprites(&gear_s, [615.0 / 640.0, 446.0 / 480.0], 16.0 * px, HUD_ORANGE));
            out.extend(f.sprites("gear", [557.0 / 640.0, 464.0 / 480.0], 10.0 * px, HUD_GREY));
            if let Some(r) = race {
                let race_t = tick.saturating_sub(RACE_START_TICK);
                let lap_t = tick.saturating_sub(r.lap_start_tick.max(RACE_START_TICK));
                // race time tray 73×35 @ (427,33); lap counter / position 138×15 @ (502,1)
                let shown = if r.finished || num_laps >= 99 { race_t } else { lap_t };
                out.extend(f.sprites(&format_ticks(shown), [440.0 / 640.0, 36.0 / 480.0], 16.0 * px, HUD_ORANGE));
                out.extend(f.sprites(if r.finished { "total" } else if num_laps >= 99 { "time" } else { "lap time" }, [429.0 / 640.0, 54.0 / 480.0], 10.0 * px, HUD_GREY));
                let mut top = String::new();
                if num_laps < 99 {
                    top += &format!("LAP {}/{}", (r.laps + 1).min(num_laps), num_laps);
                }
                if let Some((p, n)) = position {
                    top += &format!("   POS {}/{}", p + 1, n);
                }
                out.extend(f.sprites(&top, [504.0 / 640.0, 3.0 / 480.0], 12.0 * px, HUD_ORANGE));
                if r.finished {
                    // results table (HBillBoard): lap times m:ss.hh
                    out.extend(f.sprites("BEST LAP / TIMES", [0.36, 0.26], 0.035, HUD_ORANGE));
                    for (i, t) in r.lap_times.iter().enumerate() {
                        out.extend(f.sprites(&format!("LAP {}   {}", i + 1, format_ticks(*t)), [0.38, 0.32 + i as f32 * 0.045], 0.035, [1.0, 0.9, 0.5, 1.0]));
                    }
                    out.extend(f.sprites("ENTER  MENU", [0.38, 0.32 + r.lap_times.len() as f32 * 0.045 + 0.03], 0.03, HUD_GREY));
                }
                // wrong way: centred, orange, blinking 64 frames on / 32 off (hud.md §5)
                if r.wrong_way_ticks > 0 && tick % 96 < 64 {
                    out.extend(f.sprites("WRONG WAY", [0.40, 0.48], 0.05, HUD_ORANGE));
                }
            }
        }
        // countdown: digit and rings centred (hud.md §5)
        if tick < RACE_START_TICK {
            let idx = COUNTDOWN_TICKS.iter().rposition(|&t| tick >= t);
            if let Some(i) = idx {
                let px = 1.0 / HUD_REF_H;
                let ring = 128.0 * px;
                if let Some((full, outline)) = self.rings {
                    out.push(Sprite { texture: full, pos: [0.5 - ring * 0.5 / ASPECT_GUESS, 0.5 - ring * 0.5], size: [ring, ring], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 0.5], rotation: 0.0, pivot: [0.5, 0.5] });
                    out.push(Sprite { texture: outline, pos: [0.5 - ring * 0.5 / ASPECT_GUESS, 0.5 - ring * 0.5], size: [ring, ring], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 0.25], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                if let Some(tex) = self.digits[i] {
                    let h = 0.22;
                    out.push(Sprite { texture: tex, pos: [0.5 - h * 0.4 / ASPECT_GUESS, 0.5 - h * 0.5], size: [h * 0.8, h], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] });
                }
            }
        }
        out
    }
}

/// Sprite x sizes are given in height units; positions are fractions of the width. The HUD is
/// laid out for the 4:3 reference, so x extents are converted with this aspect.
const ASPECT_GUESS: f32 = 640.0 / 480.0;

/// Original game tick: 1/64 s (see module docs).
const GAME_TICK: Duration = Duration::from_nanos(15_625_000);

/// Minimal frontend (M7 first cut): choose track and car, Enter starts the race.
/// The original's `.lay`-driven screens are not reproduced yet; this uses the catalogue tables.
struct Menu {
    /// 0 = quick race, 1 = evolution (career)
    mode: u8,
    tracks: Vec<String>,
    cars: Vec<nfs_formats::fedata::CarCatalogEntry>,
    track_idx: usize,
    car_idx: usize,
    opponents: u32,
    paint_idx: usize,
    /// quick-race laps (circuits; point-to-point tracks always run one)
    laps: u32,
    backwards: bool,
    mirror: bool,
    /// circuit tracks (`nfs5.trk +0x6E`), the only ones a Quick Knockout may use (`LoopedTrack`)
    circuits: Vec<String>,
    /// knockout series status line
    knockout_line: String,
    row: u8,
    font: Option<BitmapFont>,
}

impl Menu {
    fn rows(&self) -> u8 {
        match self.mode {
            0 | 4 | 5 => 7,
            3 => 2,
            _ => 4,
        }
    }

    fn sprites(&self, career: Option<&CareerUi>) -> Vec<Sprite> {
        let Some(f) = &self.font else { return Vec::new() };
        let mut out = f.sprites("PORSCHE UNLEASHED  -  NATIVE", [0.05, 0.08], 0.05, [1.0, 1.0, 1.0, 1.0]);
        let sel = |r: u8| if self.row == r { [1.0, 0.75, 0.1, 1.0] } else { [0.85, 0.85, 0.85, 1.0] };
        out.extend(f.sprites(&format!("MODE    < {} >", ["QUICK RACE", "EVOLUTION", "TOURNAMENT", "FACTORY DRIVER", "QUICK KNOCKOUT", "KNOCKOUT"][self.mode.min(5) as usize]), [0.08, 0.15], 0.04, sel(0)));
        if (1..=3).contains(&self.mode) {
            match career {
                // career rows share the cursor: menu row 1 = tournament, row 2 = car
                Some(c) => out.extend(c.sprites(f, (self.row > 0).then(|| self.row - 1))),
                None => out.extend(f.sprites("CAREER TABLES UNAVAILABLE", [0.08, 0.30], 0.045, [0.85, 0.85, 0.85, 1.0])),
            }
            out.extend(f.sprites("UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER BUY / RACE   ESC QUIT", [0.08, 0.85], 0.028, [0.6, 0.6, 0.6, 1.0]));
            return out;
        }
        if self.mode == 5 {
            out.extend(f.sprites(&self.knockout_line, [0.08, 0.30], 0.045, sel(1)));
        } else {
            out.extend(f.sprites(&format!("TRACK   < {} >", self.tracks[self.track_idx].to_uppercase()), [0.08, 0.30], 0.045, sel(1)));
        }
        let c = &self.cars[self.car_idx];
        out.extend(f.sprites(&format!("CAR     < {} >", c.display_name.to_uppercase()), [0.08, 0.40], 0.045, sel(2)));
        let paint = c.paint_sets.get(self.paint_idx % c.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
        out.extend(f.sprites(&format!("PAINT   < {} / {} >", self.paint_idx % c.paint_sets.len().max(1) + 1, c.paint_sets.len()), [0.08, 0.48], 0.045, sel(3)));
        out.extend(f.sprites("####", [0.42, 0.48], 0.045, [paint[0] as f32 / 255.0, paint[1] as f32 / 255.0, paint[2] as f32 / 255.0, 1.0]));
        out.extend(f.sprites(&format!("OPPONENTS  < {} >", self.opponents), [0.08, 0.58], 0.045, sel(4)));
        if self.mode == 4 {
            out.extend(f.sprites(&format!("LAPS    {}  (ONE CAR OUT PER LAP)", self.opponents.max(1)), [0.08, 0.66], 0.045, sel(5)));
        } else if self.mode == 5 {
            out.extend(f.sprites(&format!("RACES   {}   LAPS < {} >   (LAST CAR OUT EACH RACE)", self.opponents.max(1), self.laps), [0.08, 0.66], 0.04, sel(5)));
        } else {
            out.extend(f.sprites(&format!("LAPS    < {} >", self.laps), [0.08, 0.66], 0.045, sel(5)));
        }
        // Track1.Direction ids 0x19A..0x19D: forward / backward / forward mirrored / backward mirrored
        let dir = match (self.backwards, self.mirror) { (false, false) => "FORWARD", (true, false) => "BACKWARD", (false, true) => "FORWARD MIRRORED", (true, true) => "BACKWARD MIRRORED" };
        out.extend(f.sprites(&format!("DIRECTION  < {dir} >"), [0.08, 0.74], 0.045, sel(6)));
        out.extend(f.sprites("UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER START   ESC QUIT", [0.08, 0.85], 0.028, [0.6, 0.6, 0.6, 1.0]));
        out
    }
}

struct App {
    paths: GamePaths,
    menu: Option<Menu>,
    /// menu kept for returning after a race
    menu_template: Option<Menu>,
    career: Option<CareerUi>,
    /// opponents of the current career event (None = quick-race opponents)
    career_opponents: Option<Vec<CareerOpponent>>,
    pad_stick_latched: bool,
    /// Factory Driver mission in progress (skill test state and the mission record).
    skill: Option<SkillTest>,
    mission: Option<career_ui::MissionStart>,
    prev_car_pos: Vec3,
    /// Deferred world edits from the simulation (applied before the next scene rebuild).
    world_actions: Vec<WorldAction>,
    /// Poses before the last physics tick (player, opponents) for render-side interpolation:
    /// frames arrive at up to 120 Hz while the state advances at 64 Hz, so drawing the latest
    /// state directly makes the car (and the chase camera bolted to it) stutter.
    render_prev: Option<(Vec3, Quat)>,
    opp_prev: Vec<(Vec3, Quat)>,
    /// Seconds since the previous frame (camera low-pass time constants).
    frame_dt: f32,
    paused: bool,
    knockout: Option<KnockoutSeries>,
    player_grid_override: Option<u32>,
    /// Drive the track backwards (`REVERSE`): `_bstart` scene, reversed slice route, `<track>1.lsp`.
    backwards: bool,
    /// `MIRROR`: world drawn x-flipped, steering negated (modes.md §2.1, MEDIUM on the consumers).
    mirror: bool,
    /// Quick Knockout (`RACE_TYPE 3`): laps = opponents, the last car is out on every lap.
    quick_knockout: bool,
    player_eliminated: bool,
    records: Records,
    records_path: std::path::PathBuf,
    record_done: bool,
    track: TrackAssets,
    car: CarAssets,
    screenshot: Option<String>,
    /// scripted drive before the screenshot: seconds of full throttle (steer right for the last third)
    autotest: Option<f32>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<TrackWorld>,
    car_model: Option<CarModel>,
    sky: Option<Sky>,
    scene: Scene,
    camera: Camera,
    input: InputState,
    gamepads: Gamepads,
    car_pos: Vec3,
    car_rot: Quat,
    specs: CarSpecs,
    vehicle: Option<Vehicle>,
    camera_mode: u8,
    slices: Option<SliceTable>,
    race: Option<RaceProgress>,
    /// opponents (parked until the AI rules are implemented) with their own models
    opponents: Vec<RaceCar>,
    opponent_models: Vec<CarModel>,
    racing_line: Option<nfs_formats::ais::RacingLine>,
    num_opponents: u32,
    player_grid: u32,
    player_progress_tick: u64,
    /// `--record path`: per-32Hz-tick CSV of controls and vehicle state (regression fixtures)
    record: Option<std::io::BufWriter<std::fs::File>>,
    num_laps: u32,
    /// autotest: steer toward the centreline (pure pursuit) instead of scripted input
    follow_road: bool,
    car_half_width: f32,
    cam_dir: Vec3,
    cam_y: f32,
    wall_hits: u32,
    rolled_ticks: u32,
    resets: u32,
    hud: Option<Hud>,
    /// current car's HUD dial records (km/h, tach) from `nfs5.car`
    dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>,
    /// player body paint (catalogue set 0 by default)
    paint: [u8; 3],
    audio: Option<nfs_audio::AudioOutput>,
    engine_sound: Option<nfs_audio::engine::EngineSound>,
    sfx: Option<nfs_audio::sfx::CollisionSfx>,
    /// player gear on the previous tick (shift click)
    last_gear: usize,
    music: Option<nfs_audio::music::Music>,
    speech: Option<nfs_audio::speech::Speech>,
    countdown_said: u8,
    sound_set: String,
    last_frame: Instant,
    accumulator: Duration,
    tick: u64,
    frames: u32,
    fps_timer: Instant,
}

impl App {
    fn simulate(&mut self) {
        self.tick += 1;
        self.render_prev = Some((self.car_pos, self.car_rot));
        self.opp_prev = self.opponents.iter().map(|o| (o.vehicle.position, Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize())).collect();
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        if self.tick % 2 == 0 {
            // 32 Hz: latch controller, run the force step (Sc32)
            let mut steer = (self.input.steer() * 126.0) as i32; // right = +x in the body frame
            if self.mirror {
                steer = -steer;
            }
            if self.follow_road && let (Some(t), Some(r)) = (&self.slices, &self.race) {
                // pure pursuit on the slice ahead (autotest only; not the original AI)
                let ahead = t.add(r.slice, 4);
                let target = t.slices[ahead].centre;
                let to = v.orientation * (target - v.position); // body-frame vector (rows = axes)
                let angle = to.x.atan2(to.z.max(0.1));
                steer = ((angle / 0.4).clamp(-1.0, 1.0) * 126.0) as i32;
                // crude speed limit for the scripted drive: ease off above 110 km/h or in sharp bends
                let limit = if angle.abs() > 0.15 { 60.0 } else { 110.0 };
                if v.speed() * 3.6 > limit {
                    self.input.release(Action::Throttle);
                    self.input.press(Action::Brake);
                } else {
                    self.input.release(Action::Brake);
                }
            }
            let mut throttle = self.input.throttle();
            if self.follow_road {
                // autotest: ease off above 130 km/h and when steering hard
                if v.speed() > 36.0 || steer.abs() > 80 { throttle = throttle.min(0.35); }
            }
            // before the start tick the original forces full brake and no throttle (0x49DD20)
            v.race_started = self.tick >= RACE_START_TICK;
            // countdown speech (Phrase_kCountdown: SOR-a-3/2/1/Go at ticks 256/320/384/448)
            let phase = COUNTDOWN_TICKS.iter().chain(std::iter::once(&RACE_START_TICK)).filter(|&&t| self.tick >= t).count() as u8;
            if phase > self.countdown_said && let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
                self.countdown_said = phase;
                sp.say(a, ["SOR-a-3.bnk", "SOR-a-2.bnk", "SOR-a-1.bnk", "SOR-a-Go.bnk"][(phase - 1) as usize], 0.8);
            }
            let controls = Controls {
                throttle: (throttle * 254.0) as u8,
                brake: (self.input.brake() * 254.0) as u8,
                steer,
                handbrake: self.input.held(Action::Handbrake),
                gear_request: None,
            };
            v.step32(&controls, &self.specs, &w.collision);
            if let Some(rec) = self.record.as_mut() {
                use std::io::Write;
                let _ = writeln!(rec, "{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{},{}", self.tick, controls.throttle, controls.brake, controls.steer, controls.handbrake as u8, v.position.x, v.position.y, v.position.z, v.velocity.x, v.velocity.y, v.velocity.z, v.rpm, v.gear, self.race.as_ref().map(|r| r.slice).unwrap_or(0));
            }
            let drivable = |p: Vec3| w.collision.ground(p.x, p.y + 2.0, p.z, 6.0).map(|h| h.surface & 0xF != 0).unwrap_or(false);
            let landing = v.impact; // set by a hard landing during step32 (closing speed)
            v.impact = 0.0;
            if v.collide_walls(&w.walls, &drivable, 0.0, 0.0, 0.0) {
                self.wall_hits += 1;
                if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                    let j = v.impact;
                    if j >= v.speed() / 8.0 { s.wall_hit(a, j, v.last_wall_class, 0.8) } else { s.wall_scrape(a, v.speed(), v.last_wall_class, 0.6) }
                }
                v.impact = 0.0;
            } else if landing > 6.0 && let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                s.landing(a, landing, 0.8);
            }
            // reset to the current slice when the car has left the world or rolled over for > 2 s
            // (original: reset mode 3, 0x40F540, also bound to a key) — APPROXIMATION of the placement rule
            let fallen = self.slices.as_ref().zip(self.race.as_ref()).map(|(t, r)| v.position.y < t.slices[r.slice].centre.y - 15.0).unwrap_or(false);
            let rolled = v.cos_tilt < 0.3 || v.orientation.row(1).y < 0.3;
            self.rolled_ticks = if rolled { self.rolled_ticks + 1 } else { 0 };
            if (fallen || self.rolled_ticks > 64 || self.input.take_pressed(Action::Reset)) && let (Some(t), Some(r)) = (&self.slices, &self.race) {
                self.rolled_ticks = 0;
                self.resets += 1;
                let sl = &t.slices[r.slice];
                let fwd = sl.forward.normalize();
                let up = sl.up.normalize();
                let right = up.cross(fwd).normalize();
                v.position = sl.centre + up * (0.5 + self.car_model.as_ref().map(|c| c.wheel_radius).unwrap_or(0.3));
                v.orientation = glam::Mat3::from_cols(right, up, fwd).transpose();
                v.velocity = Vec3::ZERO;
                v.angular_velocity = Vec3::ZERO;
                v.rpm = 0.0;
                v.gear = 2;
                tracing::info!(slice = r.slice, "car reset to track");
            }
            // Factory Driver skill test: checkpoints / finish / cones (even ticks, 0x416170)
            if let Some(sk) = self.skill.as_mut() {
                let sides = [(v.wheels[0].world_pos, v.wheels[2].world_pos), (v.wheels[1].world_pos, v.wheels[3].world_pos)];
                let about_to_reset = v.orientation.row(1).y < 0.0;
                let mut events = sk.tick(self.tick, v.position, self.prev_car_pos, &sides, about_to_reset);
                events.extend(sk.knock_cones(v.position, &v.orientation, v.half_extents));
                for e in events {
                    match e {
                        SkillEvent::Checkpoint { sequence, .. } => {
                            let shown: Vec<usize> = sk.shown_arrow.into_iter().collect();
                            self.world_actions.push(WorldAction::Show(sk.arrows.clone(), shown));
                            tracing::info!(sequence, "checkpoint");
                        }
                        SkillEvent::Finish { time_ticks } => {
                            if let Some(r) = self.race.as_mut() {
                                r.finished = true;
                                r.lap_times.push(time_ticks);
                            }
                            tracing::info!(time_s = time_ticks as f32 / 64.0, cones = sk.cones_knocked(), "skill test finished");
                        }
                        SkillEvent::ConeHit { element, away } => {
                            self.world_actions.push(WorldAction::Knock(element, away));
                            if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                                s.pylon_hit(a, 0.8);
                            }
                        }
                        SkillEvent::Dnf => {
                            if let Some(r) = self.race.as_mut() {
                                r.finished = true;
                            }
                            tracing::info!("skill test DNF");
                        }
                    }
                }
                self.prev_car_pos = v.position;
            }
            // car–car contact (approximation, see nfs_physics::vehicle::collide_cars)
            let no_carcol = std::env::var_os("NFS_NO_CARCOL").is_some();
            for o in self.opponents.iter_mut() {
                if !no_carcol && let Some(effect) = nfs_physics::vehicle::collide_cars(v, self.specs.mass, &mut o.vehicle, o.specs.mass) && let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                    s.car_car(a, effect, 0.8);
                }
            }
            for i in 0..self.opponents.len() {
                if no_carcol { break; }
                let (l, r) = self.opponents.split_at_mut(i + 1);
                for o in r.iter_mut() {
                    let (ma, mb) = (l[i].specs.mass, o.specs.mass);
                    nfs_physics::vehicle::collide_cars(&mut l[i].vehicle, ma, &mut o.vehicle, mb);
                }
            }
            // opponents (parked controller until the AI rules land)
            let mut ko_laps: Vec<(usize, u32)> = Vec::new();
            if let Some(t) = &self.slices {
                // race order (ascending progress) for the avoidance rule: cars directly ahead of each opponent
                let mut order: Vec<(f32, Vec3, usize, f32, f32, bool)> = self.opponents.iter().map(|o| (o.progress.progress, o.vehicle.position, o.progress.slice, o.vehicle.v_long(), 0.9, false)).collect();
                if let Some(r) = &self.race { order.push((r.progress, v.position, r.slice, v.v_long(), self.car_half_width, true)); }
                order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                for (oi, o) in self.opponents.iter_mut().enumerate() {
                    o.vehicle.race_started = self.tick >= RACE_START_TICK;
                    let ahead: Vec<(Vec3, usize, f32, f32, bool)> = order.iter().filter(|e| e.0 > o.progress.progress).take(2).map(|e| (e.1, e.2, e.3, e.4, e.5)).collect();
                    let c = o.controls_for(&controls, t, self.racing_line.as_ref(), self.tick, &ahead);
                    o.vehicle.step32(&c, &o.specs, &w.collision);
                    if o.vehicle.collide_walls(&w.walls, &drivable, 0.0, 0.0, 0.0) { o.wall_hits += 1; }
                    let before = o.progress.progress;
                    let o_lap_done = o.progress.update(t, o.vehicle.position, o.vehicle.forward(), self.tick, self.num_laps);
                    if o.progress.progress > before { o.last_progress_tick = self.tick; }
                    if o_lap_done && !o.eliminated && o.progress.laps <= self.num_laps { ko_laps.push((oi, o.progress.laps)); }
                    // stuck / fallen: reset onto the slice (AI reset request car+0xD8B = 5)
                    let fallen = o.vehicle.position.y < t.slices[o.progress.slice].centre.y - 15.0;
                    let stuck = match &mut o.controller { Controller::Ai(ai) => ai.wants_reset(), _ => false };
                    if fallen || stuck || o.vehicle.cos_tilt < 0.3 {
                        let sl = &t.slices[o.progress.slice];
                        let fwd = sl.forward.normalize();
                        let up = sl.up.normalize();
                        let right = up.cross(fwd).normalize();
                        o.vehicle.reset_to(sl.centre + up * 0.8, glam::Mat3::from_cols(right, fwd.cross(right).normalize(), fwd).transpose());
                        o.resets += 1;
                    }
                }
            }
            // Quick Knockout cut for opponents that completed a lap this tick (modes.md §4)
            if self.quick_knockout && !ko_laps.is_empty() {
                let mut all: Vec<(usize, f32)> = self.opponents.iter().enumerate().map(|(i, o)| (i + 1, o.progress.progress)).collect();
                if let Some(r) = &self.race { all.push((0, r.progress)); }
                all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                let pos_of = |d: usize| all.iter().position(|(i, _)| *i == d).map(|p| p as i64 + 1).unwrap_or(99);
                for &(oi, lap) in &ko_laps {
                    let cut = self.num_laps as i64 - lap as i64 + 2;
                    if pos_of(oi + 1) == cut {
                        for &(d, _) in all.iter().filter(|(d, _)| pos_of(*d) >= cut) {
                            if d == 0 {
                                if let Some(r) = self.race.as_mut() && !r.finished {
                                    self.player_eliminated = true;
                                    r.finished = true;
                                    r.progress -= 0.01;
                                    if let (Some(a), Some(sp)) = (&self.audio, &self.speech) { sp.say(a, "KO-eliminated.bnk", 0.9); }
                                }
                            } else if let Some(o) = self.opponents.get_mut(d - 1) && !o.eliminated {
                                o.eliminated = true;
                                o.progress.finished = true;
                                o.progress.progress -= 0.01;
                                o.controller = Controller::Parked;
                                tracing::info!(opponent = o.name, lap, "knocked out");
                            }
                        }
                    }
                }
            }
            if let (Some(t), Some(r)) = (&self.slices, self.race.as_mut()) {
                let before = r.progress;
                let was_finished = r.finished;
                let lap_done = r.update(t, v.position, v.forward(), self.tick, self.num_laps);
                if r.progress > before { self.player_progress_tick = self.tick; }
                let n_cars = self.opponents.len() + 1;
                let place = 1 + self.opponents.iter().filter(|o| o.progress.progress > r.progress).count();
                if lap_done {
                    tracing::info!(lap = r.laps, time_s = r.lap_times.last().map(|t| *t as f32 / 64.0), "lap completed");
                    if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
                        let rnd = (self.tick as usize) ^ (r.laps as usize * 7919);
                        let ord = ["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th"];
                        if r.finished && !was_finished {
                            // results: kRacePlacing, then kLostRace when last of several
                            let name = if place == 1 { if rnd & 1 == 0 { "Place-youwonrace.bnk" } else { "Place-youplace1st.bnk" }.to_string() } else if place >= 8 { if rnd & 1 == 0 { "Place-youplace8th.bnk" } else { "Place-youplacelast.bnk" }.into() } else { format!("Place-youplace{}.bnk", ord[(place - 1).min(7)]) };
                            sp.say(a, &name, 0.9);
                            if place == n_cars && n_cars > 1 {
                                sp.say(a, if rnd & 2 == 0 { "PG-youlostrace.bnk" } else { "PG-Betterlucknext.bnk" }, 0.9);
                            }
                        } else if self.num_laps < 99 {
                            // Race_lapSpeech 0x4277D0 bundle (RE/notes/sfx.md §7.2)
                            let n = r.laps.saturating_sub(1) as usize; // lapsDone − 1
                            sp.say(a, &if n == 0 { "Lap-your1st.bnk".to_string() } else { format!("Lap-{}.bnk", ord[n.min(7)]) }, 0.9);
                            let lap_m = t.lap_len as f32 * nfs_game::race::SLICE_SPACING;
                            let leader = self.opponents.iter().map(|o| o.progress.progress).fold(r.progress, f32::max);
                            let speed = v.speed();
                            if n_cars == 2 {
                                // kPositionRel: seconds ahead (+) / behind (−) of the rival
                                if let Some(o) = self.opponents.first() && speed > 1.0 {
                                    let g = ((r.progress - o.progress.progress) * lap_m / speed) as i32;
                                    let bucket = if g > 30 { 0 } else if g >= 13 { 1 } else if g >= 7 { 2 } else if g == 6 { 3 } else if g == 5 { 4 } else if g == 4 { 5 } else if g == 3 { 6 } else if g == 2 { 7 } else if g == 1 { 8 } else if g >= -4 { 9 } else { 10 };
                                    sp.say(a, ["Ahead-youreway.bnk", "Lead-yougotgood.bnk", "Lead-youreinthe.bnk", "Lead-yougotsmall.bnk", "Ahead-yourejust.bnk", "Lead-barelyholding.bnk", "Race-itsclose.bnk", "Race-itstight.bnk", "Neck-youreneck.bnk", "Behind-yourejust.bnk", "Behind-Hurryup.bnk"][bucket], 0.9);
                                }
                            } else if n_cars >= 3 {
                                // kLapPosition (two phrasings) + kLapTimeBehind (whole seconds to the leader, speed > 13.4 m/s)
                                let p = (place - 1).min(7);
                                let name = if p == 7 { if rnd & 1 == 0 { "Place-8th.bnk".to_string() } else { "Place-Last.bnk".into() } } else if rnd & 2 == 0 { format!("Place-{}.bnk", ord[p]) } else { format!("Place-youin{}.bnk", ord[p]) };
                                sp.say(a, &name, 0.9);
                                if speed > 13.4 {
                                    let sec = ((leader - r.progress) * lap_m / speed).floor() as i32;
                                    if sec > 0 {
                                        let idx = sec.min(12) as usize;
                                        let name = match (idx, rnd & 4 == 0) {
                                            (12, true) => "Behind-Waybe.bnk".to_string(),
                                            (12, false) => "Back-Wayback.bnk".to_string(),
                                            (i, true) => format!("Behind-{}sec.bnk", i + 1),
                                            (i, false) => format!("Back-{}sec.bnk", i + 1),
                                        };
                                        sp.say(a, &name, 0.9);
                                    }
                                }
                            }
                            // kBestLap: this lap beat the session best (not on the first lap)
                            if r.lap_times.len() >= 2 && let Some(last) = r.lap_times.last() && r.lap_times[..r.lap_times.len() - 1].iter().all(|t| last < t) {
                                sp.say(a, ["Lap-best.bnk", "Lap-Pthebest.bnk", "Lap-Pyourbest.bnk"][rnd % 3], 0.9);
                            }
                            if r.laps + 1 == self.num_laps {
                                sp.say(a, ["Lap-Final.bnk", "Lap-yourfinal.bnk", "Lap-itsthefinal.bnk", "Lap-Last.bnk", "Lap-onemore.bnk", "Lap-thelast.bnk"][rnd % 6], 0.9);
                            }
                        }
                    }
                }
                if r.finished && !was_finished {
                    tracing::info!(place, n_cars, "race finished");
                }
                // track records (modes.md §6): player's total time on point-to-point tracks, best lap on circuits
                if r.finished && !self.record_done && !self.player_eliminated && self.mission.is_none() {
                    self.record_done = true;
                    let circuit = self.records.track(&self.track.name).map(|t| t.circuit).unwrap_or(false);
                    let time = if circuit { r.lap_times.iter().copied().min().unwrap_or(0) } else { r.lap_times.iter().sum::<u64>() } as u32;
                    let car_name = self.specs.name.clone();
                    if let Some(rank) = self.records.insert(&self.track.name, "DRIVER", &car_name, time) {
                        tracing::info!(rank, time = format_ticks(time as u64), circuit, "track record entered");
                        if let Err(e) = self.records.save(&self.records_path) { tracing::warn!(error = %e, "records save failed"); }
                        if rank == 0 && let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
                            let rnd = self.tick as usize;
                            sp.say(a, if circuit { ["Record-Lap.bnk", "Record-Newlap.bnk", "Record-Youbeatlap.bnk"][rnd % 3] } else { ["PG-beattrackrec.bnk", "PG-setnewtrackrec.bnk", "PG-setnewrecord4trk.bnk"][rnd % 3] }, 0.9);
                        }
                    }
                }
                // Quick Knockout (RACE_TYPE 3, modes.md §4): when the car that just completed lap L sits in
                // position cut = NUM_LAPS − L + 2 it and every car behind it are out
                if self.quick_knockout && lap_done && !r.finished {
                    let lap = r.laps;
                    let cut = self.num_laps as i64 - lap as i64 + 2;
                    if place as i64 == cut {
                        self.player_eliminated = true;
                        r.finished = true;
                        r.progress -= 0.01;
                        for o in self.opponents.iter_mut().filter(|o| o.progress.progress < r.progress + 0.01) {
                            if !o.progress.finished { o.eliminated = true; o.progress.finished = true; o.progress.progress -= 0.01; o.controller = Controller::Parked; }
                        }
                        if let (Some(a), Some(sp)) = (&self.audio, &self.speech) { sp.say(a, "KO-eliminated.bnk", 0.9); }
                        tracing::info!(lap, place, "player knocked out");
                    }
                }
            }
            if self.input.take_pressed(Action::CameraNext) {
                self.camera_mode = (self.camera_mode + 1) % 3;
            }
            self.input.end_tick();
        }
        v.step64(&self.specs);
        for o in self.opponents.iter_mut() {
            o.vehicle.step64(&o.specs);
            if self.tick % 32 == 0 { o.vehicle.renormalise(); }
        }
        if self.tick % 32 == 0 {
            v.renormalise();
        }
        self.car_pos = v.position;
        self.car_rot = Quat::from_mat3(&v.orientation.transpose()).normalize();
        if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
            sp.update(a);
        }
        if let (Some(a), Some(e)) = (&self.audio, &self.engine_sound) {
            e.update(a, v.rpm.max(600.0), v.throttle, 0.5);
            if v.gear != self.last_gear && self.last_gear >= 2 && v.gear >= 2 {
                e.play_shift(a, 0.6);
            }
        }
        self.last_gear = v.gear;
        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
            s.update();
            // tyre squeal producer 0x40FBC0: per-axle slip sums, upright and near the ground
            let upright = v.orientation.row(1).y > 0.5;
            let front = if upright { v.wheels[0].slip + v.wheels[1].slip } else { 0.0 };
            let rear = if upright { v.wheels[2].slip + v.wheels[3].slip } else { 0.0 };
            s.squeal(a, self.specs.class.min(2), v.wheels[0].surface, front, rear, 0.5);
        }
    }

    fn rebuild_scene(&mut self) {
        if let Some(w) = self.world.as_mut() {
            for a in self.world_actions.drain(..) {
                match a {
                    WorldAction::Show(all, shown) => w.set_shown(&all, &shown),
                    WorldAction::Knock(element, away) => w.knock_over(element, away),
                }
            }
        }
        self.scene.items.clear();
        self.scene.sprites.clear();
        self.scene.mirror = self.mirror && self.vehicle.is_some();
        if let (Some(h), Some(v)) = (&self.hud, &self.vehicle) {
            let position = if self.opponents.is_empty() { None } else {
                // rank the player among the opponents by progress (ties: earlier progress tick)
                let mut better = 0;
                if let Some(r) = &self.race {
                    for o in &self.opponents {
                        if o.progress.progress > r.progress || (o.progress.progress == r.progress && o.last_progress_tick < self.player_progress_tick) { better += 1; }
                    }
                }
                Some((better, self.opponents.len() + 1))
            };
            self.scene.sprites.extend(h.sprites(v.speed() * 3.6, v.rpm, self.tick, v.gear, self.race.as_ref(), self.num_laps, position));
            let others: Vec<Vec3> = self.opponents.iter().map(|o| o.vehicle.position).collect();
            let mut map = Vec::new();
            h.map_sprites(&mut map, v.position, &others);
            self.scene.sprites.extend(map);
            if let Some(f) = &h.font {
                if self.player_eliminated {
                    self.scene.sprites.extend(f.sprites("KNOCKED OUT", [0.38, 0.22], 0.06, [1.0, 0.3, 0.2, 1.0]));
                } else if let Some(t) = self.records.track(&self.track.name) && let Some(e) = t.entries.first() && self.mission.is_none() {
                    self.scene.sprites.extend(f.sprites(&format!("{}  {}", if t.circuit { "RECORD LAP" } else { "RECORD" }, format_ticks(e.time as u64)), [0.70, 0.03], 0.03, [0.9, 0.9, 0.9, 0.8]));
                }
            }
            if let (Some(f), Some(ms)) = (&h.font, &self.mission) {
                let m = &ms.mission;
                let mut line = format!("{}   ", ms.narrator.to_uppercase());
                if let Some(t) = m.time_limit_s {
                    line += &format!("GOAL {}:{:02}   ", t / 60, t % 60);
                }
                if let Some(sk) = &self.skill {
                    let cones = sk.cones_knocked();
                    line += &format!("CONES {cones}");
                    if let Some(nfs_formats::career::ConeRule::PenaltySeconds(s)) = m.cone_rule && cones > 0 {
                        line += &format!(" (+{} S)", s as usize * cones);
                    }
                    if let Some(d) = m.damage_limit_pct {
                        line += &format!("   DAMAGE {:.0}% / {d}%", v.damage_avg());
                    }
                    if sk.finished || sk.dnf {
                        let (pass, t) = sk.evaluate(m, 1, v.damage_avg());
                        let secs = t as f32 / 64.0;
                        line += &format!("   {}  {}:{:05.2}", if pass { "MISSION PASSED" } else { "MISSION FAILED" }, (secs / 60.0) as u32, secs % 60.0);
                    }
                }
                self.scene.sprites.extend(f.sprites(&line, [0.02, 0.13], 0.035, [1.0, 0.9, 0.5, 0.95]));
            }
        }
        // render pose = previous tick → current tick, by the accumulator fraction
        let alpha = (self.accumulator.as_secs_f32() / GAME_TICK.as_secs_f32()).clamp(0.0, 1.0);
        let (car_pos, car_rot) = match self.render_prev {
            Some((p0, r0)) if p0.distance(self.car_pos) < 5.0 => (p0.lerp(self.car_pos, alpha), r0.slerp(self.car_rot, alpha).normalize()),
            _ => (self.car_pos, self.car_rot),
        };
        let dt = self.frame_dt.clamp(0.001, 0.1);
        let fwd = car_rot * Vec3::Z;
        let rear_view = self.input.held(Action::RearView) && self.vehicle.is_some();
        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
            s.horn(a, self.input.held(Action::Horn) && !self.paused && self.vehicle.is_some(), 0.6);
        }
        if self.paused && let Some(h) = &self.hud && let Some(f) = &h.font {
            self.scene.sprites.extend(f.sprites("PAUSED", [0.42, 0.40], 0.07, [1.0, 1.0, 1.0, 1.0]));
            self.scene.sprites.extend(f.sprites("ENTER RESUME     ESC MENU", [0.33, 0.50], 0.035, [0.8, 0.8, 0.8, 1.0]));
        }
        if rear_view && self.camera_mode < 2 {
            // look back: from the bonnet towards the tail
            self.camera.position = car_pos + Vec3::Y * 1.1 - fwd * 0.5;
            self.camera.target = car_pos + Vec3::Y * 0.9 - fwd * 20.0;
        } else if self.camera_mode == 0 {
            // chase: behind and above, looking slightly ahead; heading and height are low-passed so
            // suspension pitch/roll and small yaw wobble do not shake the view
            let vel = self.vehicle.as_ref().map(|v| v.velocity).unwrap_or(Vec3::ZERO);
            let dir = if vel.length() > 3.0 { Vec3::new(vel.x, 0.0, vel.z).normalize_or_zero() } else { Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero() };
            // time-based low-pass (τ = 0.15 s heading, 0.08 s height) so the result does not depend on the frame rate
            let k_dir = 1.0 - (-dt / 0.15).exp();
            let k_y = 1.0 - (-dt / 0.08).exp();
            let smoothed = if self.cam_dir.length_squared() > 0.0 { (self.cam_dir * (1.0 - k_dir) + dir * k_dir).normalize_or_zero() } else { dir };
            self.cam_dir = smoothed;
            let ground_y = car_pos.y;
            self.cam_y = if self.cam_y == 0.0 { ground_y } else { self.cam_y * (1.0 - k_y) + ground_y * k_y };
            let base = Vec3::new(car_pos.x, self.cam_y, car_pos.z);
            self.camera.position = base - smoothed * 7.5 + Vec3::Y * 2.3;
            self.camera.target = base + Vec3::Y * 0.9 + smoothed * 2.0;
        } else if self.camera_mode == 1 {
            // bonnet camera
            self.camera.position = car_pos + Vec3::Y * 1.05 + fwd * 0.6;
            self.camera.target = car_pos + Vec3::Y * 0.9 + fwd * 20.0;
        } else {
            // trackside: nearest CAMERA_ELEMENT of `<track>_cameras.scn` looks at the car
            // (the original's camera trigger/switch logic, RE/notes/race-logic.md §3.3, is not reproduced)
            let cam = self.track.cameras.as_ref().and_then(|c| {
                c.of_kind(&nfs_formats::scn::ElementKind::Camera).map(|e| Vec3::from(e.position)).min_by(|a, b| a.distance_squared(self.car_pos).partial_cmp(&b.distance_squared(self.car_pos)).unwrap())
            });
            match cam {
                Some(p) if p.distance(car_pos) < 400.0 => {
                    self.camera.position = p;
                    self.camera.target = car_pos + Vec3::Y * 0.7;
                }
                _ => {
                    self.camera.position = car_pos - fwd * 7.5 + Vec3::Y * 2.3;
                    self.camera.target = car_pos + Vec3::Y * 0.9;
                }
            }
        }
        self.camera.far = 20000.0;
        if let Some(s) = &self.sky {
            self.scene.items.extend(s.draw_items(self.camera.position));
        }
        if let Some(w) = &self.world {
            self.scene.items.extend(w.visible_items());
            self.scene.ambient = w.ambient;
        }
        for (i, o) in self.opponents.iter().enumerate() {
            let (p1, r1) = (o.vehicle.position, Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize());
            let (p, r) = match self.opp_prev.get(i) {
                Some((p0, r0)) if p0.distance(p1) < 5.0 => (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize()),
                _ => (p1, r1),
            };
            let m = Mat4::from_rotation_translation(r, p);
            let w = &o.vehicle.wheels;
            self.scene.items.extend(self.opponent_models[o.model].draw_items(m, [w[0].rotation, w[1].rotation, w[2].rotation, w[3].rotation], w[0].steer / 256.0));
        }
        if let Some(c) = &self.car_model {
            let m = Mat4::from_rotation_translation(car_rot, car_pos);
            let (spin, steer) = match &self.vehicle {
                Some(v) => ([v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation], v.wheels[0].steer / 256.0),
                None => ([0.0; 4], 0.0),
            };
            self.scene.items.extend(c.draw_items(m, spin, steer));
        }
    }
}

/// Knockout series (`RE/notes/modes.md` §5, CONFIRMED): `n` races, the last-placed car drops out
/// after each race, the grid is the previous finishing order, the player finishing last ends the
/// series. Tracks are drawn at random ("make random list" 0x50D570).
struct KnockoutSeries {
    n: u32,
    race_idx: u32,
    tracks: Vec<String>,
    opponents: Vec<CareerOpponent>,
    alive: Vec<bool>,
    /// 1-based start positions: [0] = player, [i] = opponent i−1
    start_pos: Vec<u32>,
    active: bool,
    message: String,
}

/// World edits requested by the simulation.
enum WorldAction {
    Show(Vec<usize>, Vec<usize>),
    Knock(usize, Vec3),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    /// Backspace / East button: decline the finale, sell the selected car.
    Back,
}

impl App {
    /// Menu navigation shared by the keyboard and the gamepad.
    fn menu_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) {
        let Some(m) = self.menu.as_mut() else { return };
        let rows = m.rows();
        match key {
            MenuKey::Up => {
                m.row = (m.row + rows - 1) % rows;
                if (1..=3).contains(&m.mode) && m.row > 0 && let Some(c) = self.career.as_mut() { c.row = m.row - 1; }
            }
            MenuKey::Down => {
                m.row = (m.row + 1) % rows;
                if (1..=3).contains(&m.mode) && m.row > 0 && let Some(c) = self.career.as_mut() { c.row = m.row - 1; }
            }
            MenuKey::Left | MenuKey::Right => {
                let delta: i32 = if key == MenuKey::Left { -1 } else { 1 };
                if m.row == 0 {
                    m.mode = if self.career.is_some() { ((m.mode as i32 + delta).rem_euclid(6)) as u8 } else { [0u8, 4, 5][((match m.mode { 0 => 0, 4 => 1, _ => 2 } as i32 + delta).rem_euclid(3)) as usize] };
                    if m.mode == 4 && !m.circuits.iter().any(|c| c.eq_ignore_ascii_case(&m.tracks[m.track_idx])) && let Some(i) = m.tracks.iter().position(|t| m.circuits.iter().any(|c| c.eq_ignore_ascii_case(t))) {
                        m.track_idx = i;
                    }
                    if let Some(c) = self.career.as_mut() { c.set_mode(m.mode); }
                } else if (1..=3).contains(&m.mode) {
                    if let Some(c) = self.career.as_mut() { c.row = m.row - 1; c.move_col(delta); }
                } else {
                    let step = |i: usize, n: usize| ((i as i32 + delta).rem_euclid(n.max(1) as i32)) as usize;
                    match m.row {
                        1 => {
                            m.track_idx = step(m.track_idx, m.tracks.len());
                            if m.mode == 4 {
                                // Quick Knockout: circuits only
                                for _ in 0..m.tracks.len() {
                                    if m.circuits.iter().any(|c| c.eq_ignore_ascii_case(&m.tracks[m.track_idx])) { break }
                                    m.track_idx = step(m.track_idx, m.tracks.len());
                                }
                            }
                        }
                        2 => { m.car_idx = step(m.car_idx, m.cars.len()); m.paint_idx = 0 }
                        3 => m.paint_idx = if delta < 0 { m.paint_idx.saturating_sub(1) } else { m.paint_idx + 1 },
                        4 => m.opponents = (m.opponents as i32 + delta).clamp(if m.mode >= 4 { 1 } else { 0 }, 7) as u32,
                        5 => m.laps = (m.laps as i32 + delta).clamp(1, 10) as u32,
                        _ => {
                            let cur = (m.backwards as i32) | ((m.mirror as i32) << 1);
                            let next = (cur + delta).rem_euclid(4);
                            m.backwards = next & 1 != 0;
                            m.mirror = next & 2 != 0;
                        }
                    }
                }
            }
            MenuKey::Back => {
                if (1..=3).contains(&m.mode) && let Some(c) = self.career.as_mut() {
                    c.back();
                }
            }
            MenuKey::Enter if (1..=3).contains(&m.mode) => {
                let Some(req) = self.career.as_mut().and_then(|c| c.activate()) else { return };
                let template = Menu { mode: m.mode, tracks: m.tracks.clone(), cars: m.cars.clone(), track_idx: m.track_idx, car_idx: m.car_idx, opponents: m.opponents, paint_idx: m.paint_idx, laps: m.laps, backwards: m.backwards, mirror: m.mirror, circuits: m.circuits.clone(), knockout_line: m.knockout_line.clone(), row: m.row, font: m.font.take() };
                self.menu_template = Some(template);
                self.num_opponents = req.opponents.len() as u32;
                self.num_laps = req.laps;
                self.paint = req.paint;
                self.career_opponents = Some(req.opponents.clone());
                self.backwards = req.backwards;
                self.mirror = false;
                self.quick_knockout = false;
                let car = req.car;
                self.mission = req.mission.clone();
                self.skill = None;
                match (TrackAssets::load(&self.paths, &req.track_file), CarAssets::load(&self.paths, &car.model), CarAssets::load_sim(&self.paths, &car.sim)) {
                    (Ok(mut t), Ok(c), Ok(mut sim)) => {
                        nfs_game::career::apply_parts(&mut sim, &req.parts.iter().collect::<Vec<_>>());
                        if let Some(ms) = &req.mission && let Some(suffix) = &ms.scene_suffix {
                            match t.load_scene(suffix) {
                                Ok(Some(scene)) => {
                                    self.skill = Some(SkillTest::from_scene(&scene, &t.name));
                                    t.extra_scenes.push(scene);
                                }
                                other => tracing::error!(suffix, result = ?other.err(), "skill-test scene missing"),
                            }
                        }
                        self.track = t;
                        self.car = c;
                        let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        if let Some(cu) = &self.career {
                            self.specs.zone_masks = nfs_game::career::zone_masks_for(&cu.tables, &car);
                        }
                        self.sound_set = car.sound_set.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
                        let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => {
                        tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "career event load failed");
                        if let Some(cu) = self.career.as_mut() { cu.pending = None; cu.message = "EVENT LOAD FAILED".into(); }
                        self.career_opponents = None;
                        if let Some(mut mt) = self.menu_template.take() { mt.font = mt.font.take(); self.menu = Some(mt); }
                    }
                }
            }
            MenuKey::Enter if m.mode == 5 => {
                let car = m.cars[m.car_idx].clone();
                // new series unless one is active
                if !self.knockout.as_ref().map(|k| k.active).unwrap_or(false) {
                    let n = m.opponents.clamp(1, 7);
                    let mut seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)) | 1;
                    let mut next = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
                    let pool: Vec<String> = m.tracks.clone();
                    let tracks: Vec<String> = (0..n).map(|_| pool[(next() % pool.len() as u64) as usize].clone()).collect();
                    let mut used: Vec<String> = vec![car.model.to_lowercase()];
                    let mut opponents = Vec::new();
                    for i in 0..n {
                        let Some(entry) = m.cars.iter().filter(|c| !c.sim.is_empty() && c.year >= 1990 && !used.iter().any(|u| u.eq_ignore_ascii_case(&c.model))).nth((i as usize * 7 + (next() % 5) as usize) % 12) else { break };
                        used.push(entry.model.to_lowercase());
                        let paint = entry.paint_sets.get((i as usize + 1) % entry.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0x20, 0x40, 0xC0]);
                        opponents.push(CareerOpponent { car: entry.clone(), paint, name: entry.display_name.clone(), difficulty: 1.0, skill: [[1.0, 0.0]; 4], grid: None });
                    }
                    let n = opponents.len() as u32;
                    let mut start_pos = vec![n + 1];
                    start_pos.extend(1..=n);
                    self.knockout = Some(KnockoutSeries { n, race_idx: 0, tracks, opponents, alive: vec![true; n as usize], start_pos, active: true, message: String::new() });
                }
                let Some(k) = self.knockout.as_ref() else { return };
                let track_name = k.tracks[k.race_idx as usize].clone();
                let opponents: Vec<CareerOpponent> = k.opponents.iter().enumerate().filter(|(i, _)| k.alive[*i]).map(|(i, o)| CareerOpponent { grid: Some(k.start_pos[i + 1] - 1), ..o.clone() }).collect();
                self.player_grid_override = Some(k.start_pos[0] - 1);
                self.num_opponents = opponents.len() as u32;
                self.career_opponents = Some(opponents);
                self.quick_knockout = false;
                self.num_laps = m.laps;
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                let template = Menu { mode: m.mode, tracks: m.tracks.clone(), cars: m.cars.clone(), track_idx: m.track_idx, car_idx: m.car_idx, opponents: m.opponents, paint_idx: m.paint_idx, laps: m.laps, backwards: m.backwards, mirror: m.mirror, circuits: m.circuits.clone(), knockout_line: m.knockout_line.clone(), row: m.row, font: m.font.take() };
                self.paint = car.paint_sets.get(m.paint_idx % car.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
                self.menu_template = Some(template);
                tracing::info!(track = track_name, race = k.race_idx + 1, of = k.n, "knockout race");
                match (TrackAssets::load(&self.paths, &track_name), CarAssets::load(&self.paths, &car.model), CarAssets::load_sim(&self.paths, if car.sim.is_empty() { "default" } else { &car.sim })) {
                    (Ok(t), Ok(c), Ok(sim)) => {
                        self.track = t;
                        self.car = c;
                        let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        self.sound_set = car.sound_set.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
                        let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed"),
                }
            }
            MenuKey::Enter => {
                let (track_name, car) = (m.tracks[m.track_idx].clone(), m.cars[m.car_idx].clone());
                self.player_grid_override = None;
                let template = Menu { mode: m.mode, tracks: m.tracks.clone(), cars: m.cars.clone(), track_idx: m.track_idx, car_idx: m.car_idx, opponents: m.opponents, paint_idx: m.paint_idx, laps: m.laps, backwards: m.backwards, mirror: m.mirror, circuits: m.circuits.clone(), knockout_line: m.knockout_line.clone(), row: m.row, font: m.font.take() };
                self.num_opponents = m.opponents;
                self.quick_knockout = m.mode == 4;
                self.num_laps = if m.mode == 4 { m.opponents.max(1) } else { m.laps };
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                self.career_opponents = None;
                self.paint = car.paint_sets.get(m.paint_idx % car.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
                self.menu_template = Some(template);
                tracing::info!(track = track_name, car = car.model, sim = car.sim, "menu: start race");
                match (TrackAssets::load(&self.paths, &track_name), CarAssets::load(&self.paths, &car.model), CarAssets::load_sim(&self.paths, if car.sim.is_empty() { "default" } else { &car.sim })) {
                    (Ok(t), Ok(c), Ok(sim)) => {
                        self.track = t;
                        self.car = c;
                        let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        self.sound_set = car.sound_set.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
                        let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed"),
                }
            }
        }
    }

    /// Silence the race audio (engine layers, squeal / horn loops) when leaving to the menu.
    fn stop_race_audio(&mut self) {
        if let Some(a) = &self.audio {
            if let Some(e) = self.engine_sound.take() {
                e.stop(a);
            }
            if let Some(s) = self.sfx.as_mut() {
                s.stop_loops(a);
            }
        }
    }

    /// Mission over: evaluate the skill test (or the race position) and store the result.
    fn settle_mission(&mut self) {
        let Some(ms) = self.mission.clone() else { return };
        if !self.career.as_ref().is_some_and(|c| c.pending.as_ref().is_some_and(|p| p.mission.is_some())) {
            return;
        }
        let (order, times) = self.finishing_order();
        let position = order.iter().position(|d| *d == 0).map(|p| p + 1).unwrap_or(8);
        let damage = self.vehicle.as_ref().map(|v| v.damage_avg()).unwrap_or(0.0);
        let (pass, t, cones) = match &self.skill {
            Some(sk) => {
                let (pass, t) = sk.evaluate(&ms.mission, position, damage);
                (pass, t, sk.cones_knocked())
            }
            None => {
                // race mission / solo lap race: position and total time
                let finished = self.race.as_ref().map(|r| r.finished).unwrap_or(false);
                let t = if finished { times[0] } else { 99_999 };
                let mut pass = finished;
                if let Some(r) = ms.mission.min_rank { pass &= position <= r as usize; }
                if let Some(limit) = ms.mission.time_limit_s { pass &= t < limit as u64 * 64; }
                if let Some(d) = ms.mission.damage_limit_pct { pass &= damage <= d as f32; }
                (pass, t, 0)
            }
        };
        if let Some(c) = self.career.as_mut() {
            c.finish_mission(ms.index, pass, t, cones);
            tracing::info!(mission = ms.mission.code, pass, time_s = t as f32 / 64.0, cones, message = c.message, "mission recorded");
        }
    }

    /// Finishing order as driver indices (0 = player, 1.. = opponents in grid order): finished cars
    /// by total time, then unfinished cars by progress.
    fn finishing_order(&self) -> (Vec<usize>, [u64; 8]) {
        let key = |p: &RaceProgress, last_tick: u64| -> (u8, u64, i64) {
            if p.finished { (0, p.lap_times.iter().sum::<u64>(), 0) } else { (1, 0, -(p.progress * 1e6) as i64 + last_tick as i64 / 1_000_000) }
        };
        let mut all: Vec<(usize, (u8, u64, i64))> = Vec::new();
        let mut times = [0u64; 8];
        if let Some(r) = &self.race {
            all.push((0, key(r, self.player_progress_tick)));
            if r.finished { times[0] = r.lap_times.iter().sum(); }
        }
        for (i, o) in self.opponents.iter().enumerate() {
            all.push((i + 1, key(&o.progress, o.last_progress_tick)));
            if o.progress.finished && i + 1 < 8 { times[i + 1] = o.progress.lap_times.iter().sum(); }
        }
        all.sort_by_key(|(_, k)| *k);
        (all.into_iter().map(|(i, _)| i).collect(), times)
    }

    fn resume_inner(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("Porsche Unleashed (native)").with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut renderer = Renderer::new(window.clone()).expect("renderer");
        if let Some(m) = self.menu.as_mut() {
            let dir = self.paths.fe_data().join("Art");
            m.font = GamePaths::find_file(&dir, "Univers14b.ffn").and_then(|p| std::fs::read(p).ok()).and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()).map(|f| BitmapFont::upload(&mut renderer, f));
            self.renderer = Some(renderer);
            self.window = Some(window);
            self.last_frame = Instant::now();
            self.fps_timer = Instant::now();
            if let Some(path) = self.screenshot.take() {
                let mut scene = Scene { clear_color: [0.05, 0.05, 0.08, 1.0], ..Scene::default() };
                scene.sprites = m.sprites(self.career.as_ref());
                let rgba = self.renderer.as_mut().unwrap().render_to_image(&scene, &self.camera, 1280, 800).expect("render");
                image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8).expect("png");
                event_loop.exit();
            }
            return;
        }
        self.start_race(renderer, window, event_loop);
    }

    fn start_race(&mut self, mut renderer: Renderer, window: Arc<Window>, event_loop: &ActiveEventLoop) {
        let mut world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
        let car = CarModel::upload_with_env(&mut renderer, &self.car, &self.paths, 1, self.paint, world.env_map).expect("car");
        self.sky = Sky::upload(&mut renderer, &self.track);
        self.hud = Hud::load(&mut renderer, &self.paths, self.dials);
        if let (Some(h), Some(t)) = (self.hud.as_mut(), SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok()) {
            h.set_map(&mut renderer, &if self.backwards { t.reversed() } else { t });
        }
        if self.audio.is_none() {
            self.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        if let Some(a) = &self.audio {
            self.engine_sound = nfs_audio::engine::EngineSound::load(a, &self.paths.game_data().join("Sounds"), &self.sound_set).map_err(|e| tracing::warn!(error = %e, "engine sound unavailable")).ok();
            if self.sfx.is_none() {
                self.sfx = nfs_audio::sfx::CollisionSfx::load(&self.paths.game_data().join("Sounds")).map_err(|e| tracing::warn!(error = %e, "collision sfx unavailable")).ok();
            }
            if self.speech.is_none() {
                self.speech = nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech")).map_err(|e| tracing::warn!(error = %e, "speech unavailable")).ok();
            }
            self.countdown_said = 0;
            if self.music.is_none() && self.autotest.is_none() && self.screenshot.is_none() {
                // song choice: first entry of zzzymus.viv (the original's playlist logic is not recovered)
                self.music = nfs_audio::music::Music::play(a, &self.paths.game_data().join("Music"), 0, 0.35).map_err(|e| tracing::warn!(error = %e, "music unavailable")).ok();
                if let Some(m) = &self.music { tracing::info!(title = m.title, "music"); }
            }
        }
        // start pose from the slice-based grid rule (pole position, single racer)
        let table = SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok().map(|t| if self.backwards { t.reversed() } else { t });
        let start_scene = if self.backwards { self.track.start_backward.as_ref().or(self.track.start_forward.as_ref()) } else { self.track.start_forward.as_ref() };
        self.player_eliminated = false;
        self.record_done = false;
        let n_cars = self.num_opponents + 1;
        // grid order: the player starts last (HYPOTHESIS — the original reads the slot from the
        // per-player record 0x658200+idx*0x580, not yet traced), opponents fill the slots ahead
        self.player_grid = self.player_grid_override.unwrap_or(n_cars - 1);
        let rot = match &table {
            Some(t) => {
                let (pos, rot, slice) = grid_pose(t, start_scene, self.player_grid, n_cars);
                self.car_pos = pos;
                self.race = Some(RaceProgress::new(slice, t));
                tracing::info!(slice, lap_slices = t.lap_len, grid = self.player_grid, cars = n_cars, "grid pose");
                // opponents: distinct catalogue models, parked on their grid slots
                self.opponents.clear();
                self.opponent_models.clear();
                let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default()).unwrap_or_default();
                let mut used: Vec<String> = vec![self.car.model.to_lowercase()];
                // (catalogue entry, paint, name, bend-speed factor) per grid slot
                let plan: Vec<(nfs_formats::fedata::CarCatalogEntry, [u8; 3], String, Option<(f32, [[f32; 2]; 4])>, Option<u32>)> = match &self.career_opponents {
                    Some(list) => list.iter().map(|o| (o.car.clone(), o.paint, o.name.clone(), Some((o.difficulty, o.skill)), o.grid)).collect(),
                    None => (0..self.num_opponents)
                        .filter_map(|g| {
                            let entry = catalog.iter().filter(|c| !c.sim.is_empty() && c.year >= 1990 && !used.iter().any(|u| u.eq_ignore_ascii_case(&c.model))).nth((g as usize * 7) % 12)?.clone();
                            used.push(entry.model.to_lowercase());
                            // quick-race opponents use their catalogue colour sets (different set per grid slot)
                            let paint = entry.paint_sets.get((g as usize + 1) % entry.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0x20, 0x40, 0xC0]);
                            let name = entry.display_name.clone();
                            Some((entry, paint, name, None, None))
                        })
                        .collect(),
                };
                for (g, (entry, paint, name, skill, grid)) in plan.iter().enumerate() {
                    let g = grid.unwrap_or(g as u32);
                    let (Ok(assets), Ok(sim)) = (CarAssets::load(&self.paths, &entry.model), CarAssets::load_sim(&self.paths, &entry.sim)) else { continue };
                    let paint = *paint;
                    let Ok(model) = CarModel::upload_with_env(&mut renderer, &assets, &self.paths, 1, paint, world.env_map) else { continue };
                    let specs = CarSpecs::from_sim(&sim, entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935"));
                    let (p, r, sl) = grid_pose(t, start_scene, g, n_cars);
                    let mut y = p.y;
                    if let Some(h) = world.collision.ground(p.x, p.y + 5.0, p.z, 50.0) { y = h.point.y + model.wheel_radius; }
                    let mut vehicle = Vehicle::new(Vec3::new(p.x, y, p.z), r, &specs, model.half_track, model.wheel_radius);
                    vehicle.race_started = false;
                    vehicle.traction_control = true; // AI cars never spin their wheels (Tyre 0x4A20D0)
                    self.opponent_models.push(model);
                    let controller = match CarAssets::load_ais(&self.paths, &entry.model) {
                        Ok(ais) => {
                            let mut d = nfs_game::ai::AiDriver::new(ais, &specs, &sim.unknown_tail);
                            d.set_track(&self.track.name);
                            if let Some((difficulty, pairs)) = skill {
                                d.set_skill(*difficulty, *pairs);
                            }
                            d.rng ^= (g as u32 + 1).wrapping_mul(0x9E37_79B9);
                            Controller::Ai(Box::new(d))
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, model = entry.model, "no .ais: opponent parked");
                            Controller::Parked
                        }
                    };
                    self.opponents.push(RaceCar { name: name.clone(), vehicle, specs, progress: RaceProgress::new(sl, t), controller, grid_pos: g, model: self.opponent_models.len() - 1, last_progress_tick: 0, wall_hits: 0, resets: 0, eliminated: false });
                }
                self.racing_line = self.track.racing_line(&self.paths, self.backwards);
                tracing::info!(opponents = self.opponents.len(), racing_line = self.racing_line.as_ref().map(|l| l.points.len()), "opponents placed");
                rot
            }
            None => {
                self.car_pos = world.start;
                glam::Mat3::IDENTITY
            }
        };
        if let Some(h) = world.collision.ground(self.car_pos.x, self.car_pos.y + 5.0, self.car_pos.z, 50.0) {
            self.car_pos.y = h.point.y + car.wheel_radius;
        }
        self.car_rot = Quat::from_mat3(&rot.transpose()).normalize();
        let mut rot = rot;
        if let Some(sk) = &self.skill {
            // skill test: start pose from the type-2 trigger (0x410430), first arrow shown
            if let Some((pos, fwd)) = sk.start_pose() {
                let right = Vec3::Y.cross(fwd).normalize_or_zero();
                let up = fwd.cross(right).normalize_or_zero();
                rot = glam::Mat3::from_cols(right, up, fwd).transpose();
                self.car_pos = pos;
                if let Some(h) = world.collision.ground(pos.x, pos.y + 5.0, pos.z, 50.0) {
                    self.car_pos.y = h.point.y + car.wheel_radius;
                }
                self.car_rot = Quat::from_mat3(&rot.transpose()).normalize();
                if let Some(r) = self.race.as_mut() && let Some(t) = &table {
                    *r = RaceProgress::new(t.find_slice(pos, 0), t);
                }
            }
            let shown: Vec<usize> = sk.initial_arrow().into_iter().collect();
            world.set_shown(&sk.arrows, &shown);
            // laps never end a skill test: the finish trigger does
            self.num_laps = 99;
            tracing::info!(checkpoints = sk.checkpoints.len(), cones = sk.cones.len(), arrows = sk.arrows.len(), "skill test loaded");
        }
        self.prev_car_pos = self.car_pos;
        self.slices = table;
        self.car_half_width = (car.bbox_max.x - car.bbox_min.x).abs() * 0.5;
        let mut vehicle = Vehicle::new(self.car_pos, rot, &self.specs, car.half_track, car.wheel_radius);
        vehicle.automatic = true;
        self.vehicle = Some(vehicle);
        tracing::info!(pos = ?self.car_pos, "car placed");
        self.world = Some(world);
        self.car_model = Some(car);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.rebuild_scene();
        self.last_frame = Instant::now();
        self.fps_timer = Instant::now();
        if let Some(secs) = self.autotest.take() {
            let ticks = (secs * 64.0) as u32;
            for t in 0..ticks {
                let phase = t as f32 / ticks as f32;
                if t % 2 == 0 {
                    self.input.press(Action::Throttle);
                    if !self.follow_road && phase > 0.66 { self.input.press(Action::SteerRight); }
                }
                self.simulate();
                // NFS_AI_TRACE=<opponent index>: per-quarter-second trace of one AI car (target vs speed, pedals, slip)
                if t % 16 == 0 && let Some(idx) = std::env::var("NFS_AI_TRACE").ok().and_then(|v| v.parse::<usize>().ok()) && let Some(o) = self.opponents.get(idx) {
                    let target = match &o.controller { Controller::Ai(a) => a.target_speed * 3.6, _ => -1.0 };
                    let (n_ahead, t_off, t_dot) = match &o.controller { Controller::Ai(a) => (a.dbg_n, a.dbg_offset, a.dbg_dot), _ => (0, 0.0, 0.0) };
                    let car_off = self.slices.as_ref().map(|t| { let sl = &t.slices[o.progress.slice]; (o.vehicle.position - sl.centre).dot(sl.lateral) }).unwrap_or(0.0);
                    let lat = o.vehicle.v_local.x;
                    tracing::info!(t = t as f32 / 64.0, slice = o.progress.slice, speed = (o.vehicle.speed() * 3.6) as i32, target = target as i32, thr = (o.vehicle.throttle * 100.0) as i32, brk = (o.vehicle.brake * 100.0) as i32, steer = o.vehicle.steer, n_ahead, t_off = (t_off * 10.0).round() / 10.0, t_dot = (t_dot * 100.0).round() / 100.0, car_off = (car_off * 10.0).round() / 10.0, v_lat = lat, slip = ?o.vehicle.wheels.iter().map(|w| (w.slip * 10.0) as i32).collect::<Vec<_>>(), locked = ?o.vehicle.wheels.iter().map(|w| w.locked as u8).collect::<Vec<_>>(), walls = o.wall_hits, "ai trace");
                }
                if t % 64 == 0 && let Some(v) = &self.vehicle {
                    tracing::info!(t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, opp = ?self.opponents.iter().map(|o| (o.progress.slice, (o.vehicle.speed() * 3.6) as i32, match &o.controller { Controller::Ai(a) => (a.target_speed * 3.6) as i32, _ => -1 }, o.vehicle.gear, o.wall_hits, o.resets, o.vehicle.wheels[0].steer as i32, (o.vehicle.throttle * 100.0) as i32, (o.vehicle.brake * 100.0) as i32, o.vehicle.position.x as i32, o.vehicle.position.z as i32)).collect::<Vec<_>>(), wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), pos = ?v.position, "autotest");
                }
            }
            self.rebuild_scene();
            self.settle_mission();
            if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
                let (order, times) = self.finishing_order();
                if let Some(c) = self.career.as_mut() {
                    c.finish(&order, &times);
                    tracing::info!(order = ?order, message = c.message, money = c.career.money, "career event recorded");
                }
            }
            if self.screenshot.is_none() {
                // scripted runs never stay open
                event_loop.exit();
            }
        }
        if let Some(path) = self.screenshot.take() {
            let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, 1280, 800).expect("offscreen render");
            image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8).expect("write png");
            tracing::info!(path, "screenshot written");
            event_loop.exit();
        }
    }

}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.resume_inner(event_loop);
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.mouse_motion(delta.0, delta.1);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                self.input.key_event(&event);
                if self.menu.is_some() && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{KeyCode, PhysicalKey};
                    let key = match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowUp) => Some(MenuKey::Up),
                        PhysicalKey::Code(KeyCode::ArrowDown) => Some(MenuKey::Down),
                        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(MenuKey::Left),
                        PhysicalKey::Code(KeyCode::ArrowRight) => Some(MenuKey::Right),
                        PhysicalKey::Code(KeyCode::Enter) => Some(MenuKey::Enter),
                        PhysicalKey::Code(KeyCode::Backspace) => Some(MenuKey::Back),
                        _ => None,
                    };
                    if let Some(k) = key {
                        self.menu_key(k, event_loop);
                    }
                    self.input.end_tick();
                    return;
                }
                // Esc / P / Start: pause; while paused Enter resumes, Esc again leaves to the menu (or quits)
                if self.input.take_pressed(Action::Pause) {
                    if self.paused {
                        self.paused = false;
                        if let Some(menu) = self.menu_template.take() {
                            if let Some(c) = self.career.as_mut() { c.pending = None; }
                            if let Some(k) = self.knockout.as_mut() { k.active = false; k.message = "SERIES CANCELLED".into(); }
                            self.player_grid_override = None;
                            self.career_opponents = None;
                            self.skill = None;
                            self.mission = None;
                            self.stop_race_audio();
                            self.menu = Some(menu);
                            self.vehicle = None;
                            self.race = None;
                            self.world = None;
                            self.car_model = None;
                            self.sky = None;
                            self.scene = Scene::default();
                            self.input.end_tick();
                            return;
                        }
                        event_loop.exit();
                    } else if self.vehicle.is_some() {
                        self.paused = true;
                        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) { s.stop_loops(a); }
                    } else {
                        event_loop.exit();
                    }
                }
                if self.paused && event.state == winit::event::ElementState::Pressed && event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter) {
                    self.paused = false;
                    self.input.end_tick();
                    return;
                }
                // after the finish, Enter returns to the menu (window and renderer are kept)
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter)
                    && self.race.as_ref().map(|r| r.finished).unwrap_or(false)
                    && let Some(menu) = self.menu_template.take()
                {
                    self.settle_mission();
                    if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
                        let (order, times) = self.finishing_order();
                        let damage = self.vehicle.as_ref().map(|v| v.damage_avg()).unwrap_or(0.0);
                        if let Some(c) = self.career.as_mut() {
                            let garage_idx = c.pending.as_ref().and_then(|p| p.garage_idx);
                            c.finish(&order, &times);
                            // the owned car keeps its body damage (value formula 0x4F8E50; repairs are not modelled)
                            if let Some(i) = garage_idx && let Some(car) = c.career.garage.get_mut(i) {
                                car.damage_pct = car.damage_pct.max(damage);
                                let _ = c.career.save(&c.save_path);
                            }
                        }
                    }
                    self.skill = None;
                    self.mission = None;
                    self.career_opponents = None;
                    self.stop_race_audio();
                    let mut menu = menu;
                    if self.knockout.as_ref().map(|k| k.active).unwrap_or(false) {
                        let (order, _) = self.finishing_order();
                        if let Some(k) = self.knockout.as_mut() {
                            // Knockout_afterRace 0x4DE060: results in slot order, the last-placed car drops out
                            let remaining = k.n - k.race_idx;
                            let pos_of = |d: usize| order.iter().position(|x| *x == d).map(|p| p as u32 + 1).unwrap_or(99);
                            let alive_idx: Vec<usize> = (0..k.n as usize).filter(|i| k.alive[*i]).collect();
                            if pos_of(0) > remaining {
                                k.active = false;
                                k.message = format!("KNOCKED OUT IN RACE {} OF {}", k.race_idx + 1, k.n);
                            } else {
                                k.start_pos[0] = pos_of(0);
                                for (slot, i) in alive_idx.iter().enumerate() {
                                    let p = pos_of(slot + 1);
                                    if p <= remaining { k.start_pos[i + 1] = p } else { k.alive[*i] = false }
                                }
                                k.race_idx += 1;
                                if k.race_idx >= k.n {
                                    k.active = false;
                                    k.message = format!("KNOCKOUT WON  ({} RACES)", k.n);
                                } else {
                                    k.message = format!("RACE {} OF {} NEXT: {}   (P{} LAST RACE)   ENTER", k.race_idx + 1, k.n, k.tracks[k.race_idx as usize].to_uppercase(), pos_of(0));
                                }
                            }
                            menu.knockout_line = k.message.clone();
                        }
                    }
                    self.player_grid_override = None;
                    self.menu = Some(menu);
                    self.vehicle = None;
                    self.race = None;
                    self.world = None;
                    self.car_model = None;
                    self.sky = None;
                    self.scene = Scene::default();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested if self.menu.is_some() => {
                // gamepad: d-pad / left stick moves, South (A / cross) selects
                self.gamepads.poll(&mut self.input);
                let stick = self.input.pad.map(|p| p.steer).unwrap_or(0.0);
                let mut keys = Vec::new();
                if self.input.take_pressed(Action::SteerLeft) { keys.push(MenuKey::Left); }
                if self.input.take_pressed(Action::SteerRight) { keys.push(MenuKey::Right); }
                if self.input.take_pressed(Action::Throttle) { keys.push(MenuKey::Up); }
                if self.input.take_pressed(Action::Brake) { keys.push(MenuKey::Down); }
                if self.input.take_pressed(Action::ShiftUp) { keys.push(MenuKey::Enter); }
                if self.input.take_pressed(Action::Handbrake) { keys.push(MenuKey::Back); }
                if stick.abs() > 0.6 && !self.pad_stick_latched { keys.push(if stick < 0.0 { MenuKey::Left } else { MenuKey::Right }); }
                self.pad_stick_latched = stick.abs() > 0.6;
                self.input.end_tick();
                for k in keys {
                    self.menu_key(k, event_loop);
                    if self.menu.is_none() { return; }
                }
                let sprites = self.menu.as_ref().unwrap().sprites(self.career.as_ref());
                let mut scene = Scene { clear_color: [0.05, 0.05, 0.08, 1.0], ..Scene::default() };
                scene.sprites = sprites;
                if let Some(r) = self.renderer.as_mut() {
                    let _ = r.render(&scene, &self.camera);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.gamepads.poll(&mut self.input);
                let now = Instant::now();
                self.frame_dt = (now - self.last_frame).as_secs_f32();
                self.accumulator += now - self.last_frame;
                self.last_frame = now;
                if self.accumulator > Duration::from_millis(250) {
                    self.accumulator = Duration::from_millis(250);
                }
                if self.paused {
                    self.accumulator = Duration::ZERO;
                }
                while self.accumulator >= GAME_TICK {
                    self.simulate();
                    self.accumulator -= GAME_TICK;
                }
                self.rebuild_scene();
                if let Some(r) = self.renderer.as_mut()
                    && let Err(e) = r.render(&self.scene, &self.camera)
                {
                    tracing::error!(error = %e, "render failed");
                }
                self.frames += 1;
                if self.fps_timer.elapsed() >= Duration::from_secs(1) {
                    tracing::info!(fps = self.frames, tick = self.tick, "frame stats");
                    self.frames = 0;
                    self.fps_timer = Instant::now();
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let paths = GamePaths::locate()?;
    tracing::info!(root = %paths.root.display(), "located original game data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    // positional arguments = tokens that are neither flags nor flag values
    let flags_with_value = ["--screenshot", "--autotest", "--opponents", "--record", "--laps", "--career"];
    // flags without a value: --follow, --mission
    let mut positional = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip { skip = false; continue; }
        if a.starts_with("--") { skip = flags_with_value.contains(&a.as_str()); continue; }
        positional.push(a.clone());
    }
    let career_id: Option<i16> = args.iter().position(|a| a == "--career").and_then(|i| args.get(i + 1)?.parse().ok());
    let mission_flag = args.iter().any(|a| a == "--mission");
    let use_menu = positional.is_empty() && !args.iter().any(|a| a == "--autotest") && career_id.is_none() && !mission_flag;
    // `--career <id>`: start the next event of tournament <id> from the saved career (buying the
    // cheapest eligible car when the garage has none); used for headless smoke runs
    let mut career = if use_menu || career_id.is_some() || mission_flag { CareerUi::load(&paths).map_err(|e| tracing::warn!(error = %e, "career unavailable")).ok() } else { None };
    let mut career_request = None;
    // `--mission`: start the current Factory Driver mission headlessly (with --autotest)
    if mission_flag && let Some(ui) = career.as_mut() {
        ui.set_mode(3);
        career_request = Some(ui.activate_mission().ok_or_else(|| anyhow::anyhow!("cannot start a mission: {}", ui.message))?);
    }
    if let (Some(id), Some(ui)) = (career_id, career.as_mut()) {
        let idx = ui.tournaments().iter().position(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("tournament {id} not in the current era"))?;
        ui.t_idx = idx;
        ui.ensure_car();
        ui.car_sel = 0;
        career_request = Some(ui.activate().ok_or_else(|| anyhow::anyhow!("cannot start tournament {id}: {}", ui.message))?);
    }
    let track_name = career_request.as_ref().map(|r| r.track_file.as_str()).or(positional.first().map(String::as_str)).unwrap_or("alps");
    let car_name = career_request.as_ref().map(|r| r.car.model.as_str()).or(positional.get(1).map(String::as_str)).unwrap_or("993");
    let t0 = Instant::now();
    let mut track = TrackAssets::load(&paths, track_name)?;
    let mut skill = None;
    if let Some(ms) = career_request.as_ref().and_then(|r| r.mission.as_ref()) && let Some(suffix) = &ms.scene_suffix {
        if let Some(scene) = track.load_scene(suffix)? {
            skill = Some(SkillTest::from_scene(&scene, &track.name));
            track.extra_scenes.push(scene);
        }
    }
    let car = CarAssets::load(&paths, car_name)?;
    // simulation variant: catalogue entry whose model matches, else "<car>coupe36"-style guess
    let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(paths.fe_data().join("Data").join("nfs5.car"))?)?;
    let sim_name = career_request.as_ref().map(|r| r.car.sim.clone()).or_else(|| positional.get(2).cloned()).or_else(|| catalog.iter().find(|c| c.model.eq_ignore_ascii_case(car_name) && !c.sim.is_empty()).map(|c| c.sim.clone())).unwrap_or_else(|| "default".into());
    let mut sim = CarAssets::load_sim(&paths, &sim_name)?;
    if let Some(r) = &career_request {
        nfs_game::career::apply_parts(&mut sim, &r.parts.iter().collect::<Vec<_>>());
    }
    let sound_set = catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).map(|c| c.sound_set.clone()).unwrap_or_else(|| "sd7a".into());
    let paint = career_request.as_ref().map(|r| r.paint).or_else(|| catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).and_then(|c| c.paint_sets.first()).map(|p| p.colours[0])).unwrap_or([0xD4, 0x10, 0x10]);
    let race_class = sim_name.to_lowercase().starts_with("gt") || sim_name.starts_with("935");
    let specs = CarSpecs::from_sim(&sim, race_class);
    tracing::info!(sim = sim_name, name = specs.name, top_speed_mps = specs.top_speed, "car specs");
    tracing::info!(track = track_name, car = car_name, secs = t0.elapsed().as_secs_f32(), "assets loaded");
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned());
    let autotest = args.iter().position(|a| a == "--autotest").and_then(|i| args.get(i + 1)?.parse().ok());
    let follow_road = args.iter().any(|a| a == "--follow");
    let num_opponents: u32 = career_request.as_ref().map(|r| r.opponents.len() as u32).or_else(|| args.iter().position(|a| a == "--opponents").and_then(|i| args.get(i + 1)?.parse().ok())).unwrap_or(0);
    let num_laps: u32 = career_request.as_ref().map(|r| r.laps).or_else(|| args.iter().position(|a| a == "--laps").and_then(|i| args.get(i + 1)?.parse().ok())).unwrap_or(2).max(1);
    // Quick Knockout: laps = opponents (modes.md §4)
    let num_laps = if args.iter().any(|a| a == "--quick-knockout") { num_opponents.max(1) } else { num_laps };
    let record = args.iter().position(|a| a == "--record").and_then(|i| args.get(i + 1)).and_then(|p| {
        let mut w = std::io::BufWriter::new(std::fs::File::create(p).ok()?);
        use std::io::Write;
        writeln!(w, "tick,throttle,brake,steer,handbrake,x,y,z,vx,vy,vz,rpm,gear,slice").ok()?;
        Some(w)
    });
    let backwards = args.iter().any(|a| a == "--backwards") || career_request.as_ref().map(|r| r.backwards).unwrap_or(false);
    let records_path = std::env::var_os("NFS5_RECORDS_SAVE").map(std::path::PathBuf::from).unwrap_or_else(Records::default_save_path);
    let records = Records::load(&paths, &records_path);
    let menu = if use_menu {
        let mut cars = catalog.iter().filter(|c| !c.sim.is_empty()).cloned().collect::<Vec<_>>();
        cars.sort_by_key(|c| c.year);
        let car_idx = cars.iter().position(|c| c.model.eq_ignore_ascii_case("993") && c.sim.contains("coupe")).unwrap_or(0);
        Some(Menu { mode: std::env::var("NFS_MENU_MODE").ok().and_then(|v| v.parse().ok()).unwrap_or(0), tracks: TrackAssets::list(&paths), cars, track_idx: 0, car_idx, opponents: 3, paint_idx: 0, laps: num_laps, backwards, mirror: false, circuits: records.tracks.iter().filter(|t| t.circuit).map(|t| t.file.clone()).collect(), knockout_line: "KNOCKOUT   ENTER STARTS A SERIES".into(), row: 0, font: None })
    } else {
        None
    };
    if let (Some(m), Some(c)) = (&menu, career.as_mut()) {
        c.set_mode(m.mode);
    }
    let mission_start = career_request.as_ref().and_then(|r| r.mission.clone());

    let mut app = App {
        paths,
        menu,
        menu_template: None,
        career,
        career_opponents: career_request.map(|r| r.opponents),
        pad_stick_latched: false,
        skill,
        mission: mission_start,
        prev_car_pos: Vec3::ZERO,
        world_actions: Vec::new(),
        render_prev: None,
        opp_prev: Vec::new(),
        frame_dt: 1.0 / 60.0,
        paused: false,
        knockout: None,
        player_grid_override: None,
        backwards,
        mirror: args.iter().any(|a| a == "--mirror"),
        quick_knockout: args.iter().any(|a| a == "--quick-knockout"),
        player_eliminated: false,
        records,
        records_path,
        record_done: false,
        track,
        car,
        screenshot,
        autotest,
        window: None,
        renderer: None,
        world: None,
        car_model: None,
        sky: None,
        scene: Scene::default(),
        camera: Camera::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        car_pos: Vec3::ZERO,
        car_rot: Quat::IDENTITY,
        specs,
        vehicle: None,
        camera_mode: 0,
        slices: None,
        race: None,
        opponents: Vec::new(),
        opponent_models: Vec::new(),
        racing_line: None,
        num_opponents,
        player_grid: 0,
        player_progress_tick: 0,
        record,
        num_laps,
        follow_road,
        car_half_width: 0.9,
        cam_dir: Vec3::ZERO,
        cam_y: 0.0,
        wall_hits: 0,
        rolled_ticks: 0,
        resets: 0,
        hud: None,
        dials: catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).map(|c| (c.dial_kmh, c.dial_tach)),
        paint,
        audio: None,
        engine_sound: None,
        sfx: None,
        last_gear: 1,
        music: None,
        speech: None,
        countdown_said: 0,
        sound_set,
        last_frame: Instant::now(),
        accumulator: Duration::ZERO,
        tick: 0,
        frames: 0,
        fps_timer: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
