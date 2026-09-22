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
//! porsche [track] [car] [sim] [--fe[=layout]] [--screenshot out.png] [--autotest N [--follow]] [--opponents N] [--record run.csv]
//!        [--net host[:PORT] | --net join:HOST[:PORT] [--net-name NAME]]
//! keys: arrows = steer/throttle/brake, space = handbrake, C = camera, R = reset to track, Esc = quit
//! ```

mod accessors;
mod career_ui;
mod netplay;

use anyhow::Result;
use career_ui::{CareerOpponent, CareerUi};
use glam::{Mat4, Quat, Vec3};
use nfs_assets::{GamePaths, car::CarAssets, track::TrackAssets};
use nfs_game::layout::{ListCell, ListRow, TrackMapEvent};
use nfs_game::{
    lanes::{LaneCar, lane_of, lane_span},
    race::{RaceProgress, SliceTable, grid_pose},
    racecar::{Controller, RaceCar},
    records::{Records, format_ticks},
    skill::{SkillEvent, SkillTest},
    sky::Sky,
    text::BitmapFont,
    traffic::{Reference, TrafficSet, TrafficSpec},
    world::{CarModel, TrackWorld},
};
use nfs_input::{Action, InputState, gamepad::Gamepads};
use nfs_physics::{
    specs::CarSpecs,
    vehicle::{Controls, Vehicle},
};
use nfs_renderer::{Camera, Renderer, Scene, Sprite, TextureData, TextureId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
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
    /// `HReplay`'s twelve glyphs from `DashHud/replay.fsh`, by the file's own index
    /// (`RE/notes/hud.md` §5.2): 0 cam, 1 paus, 2 quit, 3 rest, 4 Irac, 5…8 the speed steps,
    /// 9 save, 10 next, 11 play.
    replay_glyphs: [Option<(TextureId, f32, f32)>; 12],
}

const MAP_PX: u32 = 120;

/// Race-start countdown (CONFIRMED values, `RE/notes/timing.md`): game ticks 256/320/384/448 =
/// 4/5/6/7 s; the race starts at tick 448 and brakes are forced until then.
const RACE_START_TICK: u64 = 448;
const COUNTDOWN_TICKS: [u64; 3] = [256, 320, 384];
/// HUD reference height (px) — sizes are fixed pixels in the original (hud.md §2).
const HUD_REF_H: f32 = 480.0;
const HUD_REF_W: f32 = 640.0;
const HUD_ORANGE: [f32; 4] = [0.98, 0.706, 0.0, 1.0];
/// **`HButtonVisual`'s two colour pairs** — 0x41B340, its vtable slot 6 (`RE/notes/hud.md` §5.3).
/// The slot sets two colours per button: the plate through `0x419F50(&this+0x14, …)` and the glyph
/// through `[this+0xF8]+0x24`. Idle (`this+0xFC == 0` and the pointer outside the box) is
/// **0xFFBC0000** on the plate — the same dark red the `.lay` files give a button's `maincolor` —
/// with a **white** glyph (0x41B4E0); hovered is **0xFFFAB400**, the HUD orange, with a **black**
/// glyph (0x41B3F8), and crossing into the box once plays sound **9** (`0x4AADB0`, 0x41B3C4).
/// This port has no in-race pointer, so only the idle pair is reachable.
const HBUTTON_IDLE: [f32; 4] = [0.737, 0.0, 0.0, 1.0];
const HBUTTON_IDLE_GLYPH: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const HUD_GREY: [f32; 4] = [0.706, 0.706, 0.706, 1.0];

impl Hud {
    /// `night` picks the `*N.fsh` faces and needles the original switches to when the night flag
    /// `word[0x628A28]` is set (`RE/notes/hud.md` §3.2).
    fn load(
        r: &mut Renderer,
        paths: &GamePaths,
        dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>,
        night: bool,
    ) -> Option<Self> {
        let dir = paths.game_data().join("DashHud");
        let load_idx = |r: &mut Renderer,
                        file: &str,
                        index: Option<usize>,
                        entry: Option<&str>|
         -> Option<(TextureId, f32, f32)> {
            let p = GamePaths::find_file(&dir, file)?;
            let d = std::fs::read(p).ok()?;
            let f = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let i = match (index, entry) {
                (Some(i), _) if i < f.entries.len() => i,
                (_, Some(e)) => f.entries.iter().position(|x| x.name.trim() == e)?,
                _ => return None,
            };
            let img = f.decode(&d, i).ok()?;
            Some((
                r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)),
                img.width as f32,
                img.height as f32,
            ))
        };
        // defaults = '94 993 (style 4 / tach 6 / needle 2, pivot 18) when the car record is unknown
        let (kmh, tach_spec) = dials.unwrap_or((
            nfs_formats::fedata::DialSpec {
                style: 4,
                total_units: 300,
                needle_style: 2,
                start_deg: -40,
                deg_per_unit_x1000: 910,
                needle_pivot: 18,
            },
            nfs_formats::fedata::DialSpec {
                style: 6,
                total_units: 800,
                needle_style: 2,
                start_deg: -18,
                deg_per_unit_x1000: 320,
                needle_pivot: 18,
            },
        ));
        let needle_style = tach_spec.needle_style.max(0) as usize;
        let pivot = tach_spec.needle_pivot.max(0) as f32;
        let (speed_f, tach_f, n2_f, n_f) = if night {
            ("speedKN.fsh", "tachN.fsh", "needle2N.fsh", "needleN.fsh")
        } else {
            ("speedK.fsh", "tach.fsh", "needle2.fsh", "needle.fsh")
        };
        let dial = load_idx(r, speed_f, Some(kmh.style.max(0) as usize), Some("300k"))?.0;
        let n = load_idx(r, n2_f, Some(needle_style), Some("need"))?;
        let hub = (needle_style == 0)
            .then(|| load_idx(r, n2_f, None, Some("cent")).map(|h| h.0))
            .flatten();
        let tach = load_idx(
            r,
            tach_f,
            Some(tach_spec.style.max(0) as usize),
            Some("964"),
        )
        .zip(load_idx(r, n_f, Some(needle_style), Some("need")))
        .map(|(t, nn)| (t.0, (nn.0, nn.1, nn.2, pivot)));
        let digits = [
            load_idx(r, "321go.fsh", None, Some("3Sol")).map(|x| x.0),
            load_idx(r, "321go.fsh", None, Some("2Sol")).map(|x| x.0),
            load_idx(r, "321go2.fsh", None, Some("1Sol")).map(|x| x.0),
        ];
        let rings = load_idx(r, "321go.fsh", None, Some("circlefull"))
            .zip(load_idx(r, "321go.fsh", None, Some("circleoutline")))
            .map(|(a, b)| (a.0, b.0));
        let font = GamePaths::find_file(&dir, "UniversC12b.ffn")
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
            .map(|f| BitmapFont::upload(r, f));
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Some(Self {
            dial,
            needle: (n.0, n.1, n.2, pivot),
            hub,
            kmh,
            tach_spec,
            digits,
            rings,
            font,
            tach,
            white,
            map: None,
            replay_glyphs: std::array::from_fn(|i| load_idx(r, "replay.fsh", Some(i), None)),
        })
    }

    /// Rasterise the lap route into the map texture (north up, fitted with a 6 px margin).
    fn set_map(&mut self, r: &mut Renderer, table: &SliceTable) {
        let pts: Vec<(f32, f32)> = table
            .route
            .iter()
            .map(|&i| (table.slices[i].centre.x, table.slices[i].centre.z))
            .collect();
        if pts.len() < 2 {
            return;
        }
        let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, z) in &pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
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
        let to_px = |(x, z): (f32, f32)| {
            (
                ((x - origin_x) * scale) as i32,
                ((z - origin_z) * scale) as i32,
            )
        };
        for w in pts
            .windows(2)
            .chain(std::iter::once(&[pts[pts.len() - 1], pts[0]][..]))
        {
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

    /// Results table (HBillBoard, hud.md §5/§9): rank, NAME, CAR, BEST LAP, TIME; dashes for no time.
    /// `rows` = (name, car, best lap ticks, total ticks) in finishing order; `player` = the player's row.
    /// `HRaceSummary` 0x42BCD0 / 0x42C080: the billboard's four columns and eight ordinal rows.
    /// The column headers are `FEStrings` **18111…18114** (`NAME`, `CAR`, `BEST LAP`, `TIME`), the
    /// row labels **18116…18123** (`1st`…`8th`), and a car with no time prints
    /// `"--------------"` rather than a dash. `title` is the billboard's, by game type: 18130
    /// `Race Summary`, 18129 `Single Race`, 18169 `Knockout`, 18175 `Quick Knockout`
    /// (`RE/notes/hud.md` §5). The box's own geometry is still this port's — see the same section.
    pub fn results_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        rows: &[(String, String, Option<u64>, Option<u64>)],
        player: usize,
        title: &str,
        head: &[String; 4],
        ord: &[String],
    ) {
        if self.font.is_none() {
            return;
        }
        let n = rows.len().max(1) as f32;
        let (x, y, w, h) = (60.0, 120.0, 520.0, 26.0 + 20.0 * n + 24.0);
        self.tray(out, aspect, x, y, w, h);
        let cols = [x + 8.0, x + 46.0, x + 190.0, x + 380.0, x + 450.0];
        self.text(out, aspect, title, x + 8.0, y - 16.0, 13.0, HUD_ORANGE);
        for (i, t) in head.iter().enumerate() {
            self.text(out, aspect, t, cols[i + 1], y + 6.0, 11.0, HUD_GREY);
        }
        for (k, (name, car, best, total)) in rows.iter().enumerate() {
            let yy = y + 26.0 + 20.0 * k as f32;
            let color = if k == player {
                HUD_ORANGE
            } else {
                [1.0, 1.0, 1.0, 0.95]
            };
            let none = || "--------------".to_string();
            let cells = [
                ord.get(k).cloned().unwrap_or_default(),
                name.chars().take(18).collect::<String>(),
                car.chars().take(24).collect::<String>(),
                best.map(format_ticks).unwrap_or_else(none),
                total.map(format_ticks).unwrap_or_else(none),
            ];
            for (i, t) in cells.iter().enumerate() {
                self.text(out, aspect, t, cols[i], yy, 12.0, color);
            }
        }
        self.text(
            out,
            aspect,
            "ENTER  CONTINUE",
            x + 8.0,
            y + h - 18.0,
            10.0,
            HUD_GREY,
        );
    }

    /// Map tray with the route and car dots (player orange, others grey).
    /// `0x433328` — the HUD blip loop walks the whole racer list. A car's blip takes its paint colour
    /// (`[[car+0x558]+0x218]`), floored to `0xFF202020` when every channel is under 0x14 so a black car
    /// still shows; a police car instead alternates blue and red twice a second (§17.7). The player's
    /// own blip keeps the display colour.
    fn blip_color(paint: [u8; 3], police: bool, tick: u64) -> [f32; 4] {
        if police {
            // 0xC00000FF <-> 0xC0FF0000, toggled every 0x20 ticks
            let red = (tick / 32) % 2 == 1;
            return if red {
                [1.0, 0.0, 0.0, 0.75]
            } else {
                [0.0, 0.0, 1.0, 0.75]
            };
        }
        let floored = if paint.iter().all(|&c| c < 0x14) {
            [0x20, 0x20, 0x20]
        } else {
            paint
        };
        [
            floored[0] as f32 / 255.0,
            floored[1] as f32 / 255.0,
            floored[2] as f32 / 255.0,
            1.0,
        ]
    }

    /// **`HReplay`'s control bar** — `RE/notes/hud.md` §5.2. Seven 21×18 cells of pitch 51, centred
    /// and thirty pixels up from the bottom (`nfs_game::replay::control_bar`, which is 0x42DEF0's
    /// arithmetic), each holding a `replay.fsh` glyph centred in its own box.
    ///
    /// `paused` picks `play` over `paus` in cell 2 (the original keeps both widgets, `this+0xC8` and
    /// `this+0xCC`, and draws one — 0x42D31E switches on `this+0x199`), and `speed` picks one of the
    /// four speed glyphs in cell 4. The frame the bar sits on is `this+0x164/+0x168`, three pixels up
    /// and left of the first cell; the original draws it through the widget's own vtable, which this
    /// port has not read, so it is the HUD's ordinary tray — **APPROXIMATION**, and the only part of
    /// the bar that is.
    fn replay_bar(&self, out: &mut Vec<Sprite>, aspect: f32, paused: bool, speed: nfs_game::replay::Speed) {
        use nfs_game::replay::{cell_glyph_origin, control_bar, BAR, BAR_CELL, BAR_PITCH, BAR_PLAY};
        let (cells, frame) = control_bar(HUD_REF_W, HUD_REF_H);
        self.tray(out, aspect, frame.0, frame.1, 6.0 * BAR_PITCH + BAR_CELL.0 + 6.0, BAR_CELL.1 + 6.0);
        for (i, cell) in cells.iter().enumerate() {
            let c = BAR[i];
            let glyph = match i {
                2 if paused => BAR_PLAY,
                4 => speed.bar_glyph(),
                _ => c.glyph,
            };
            // the plate, in the idle colour
            out.push(self.quad(aspect, cell.0, cell.1, BAR_CELL.0, BAR_CELL.1, HBUTTON_IDLE));
            let Some((tex, _, _)) = self.replay_glyphs[glyph] else { continue };
            let (x, y) = cell_glyph_origin(*cell, c.box_w, c.box_h);
            let (pos, size) = Self::place(aspect, x, y, c.box_w, c.box_h);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: HBUTTON_IDLE_GLYPH, rotation: 0.0, pivot: [0.5, 0.5] });
        }
    }

    fn map_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        player: Vec3,
        others: &[(Vec3, [f32; 4])],
    ) {
        let Some((tex, [ox, oz, scale])) = self.map else {
            return;
        };
        let m = MAP_PX as f32;
        self.tray(out, aspect, 0.0, 358.0, m, m);
        out.push(self.image(aspect, tex, 0.0, 358.0, m, m));
        let mut dot = |p: Vec3, color: [f32; 4], d: f32| {
            let (mx, my) = ((p.x - ox) * scale, (p.z - oz) * scale);
            if mx < 0.0 || my < 0.0 || mx > m || my > m {
                return;
            }
            out.push(self.quad(aspect, mx - d * 0.5, 358.0 + my - d * 0.5, d, d, color));
        };
        for &(o, c) in others {
            dot(o, c, 3.0);
        }
        dot(player, HUD_ORANGE, 4.0);
    }

    /// HTray (0x434110): translucent dark box with a 1 px outline in the display colour (alpha 0x3F).
    fn tray(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32) {
        out.push(self.quad(aspect, x, y, w, h, [0.0, 0.0, 0.0, 0.45]));
        let o = [0.98, 0.706, 0.0, 0.25];
        out.push(self.quad(aspect, x, y, w, 1.0, o));
        out.push(self.quad(aspect, x, y + h - 1.0, w, 1.0, o));
        out.push(self.quad(aspect, x, y, 1.0, h, o));
        out.push(self.quad(aspect, x + w - 1.0, y, 1.0, h, o));
    }

    /// Screen rotation (turns, clockwise positive) for a dial value — `hud.md` §3.3, H32 settled:
    /// the table `HDial` precomputes is filled backwards, so the needle sweeps clockwise by
    /// `deg_per_unit` per unit and every face, classic or Boxster, follows the same rule.
    fn needle_turns(spec: &nfs_formats::fedata::DialSpec, value: i32) -> f32 {
        spec.screen_cw_deg(value) / 360.0
    }

    /// Map a rectangle of the HUD's own **640×480** design space into sprite coordinates,
    /// pillarboxing it in a wider window exactly as the front end's screens are
    /// (`nfs_game::layout`'s `place`). Everything `hud.md` gives is in 640×480 pixels — the game
    /// was a 4:3 title — so a widescreen window must letterbox the layout instead of stretching the
    /// gaps between its pieces.
    fn place(aspect: f32, x: f32, y: f32, w: f32, h: f32) -> ([f32; 2], [f32; 2]) {
        let design = HUD_REF_W / HUD_REF_H;
        let (fw, x0) = if aspect > design {
            (design / aspect, (1.0 - design / aspect) * 0.5)
        } else {
            (1.0, 0.0)
        };
        (
            [x0 + x / HUD_REF_W * fw, y / HUD_REF_H],
            [w / HUD_REF_H, h / HUD_REF_H],
        )
    }

    /// A design-space quad in a flat colour.
    fn quad(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Sprite {
        let (pos, size) = Self::place(aspect, x, y, w, h);
        Sprite {
            texture: self.white,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color,
            rotation: 0.0,
            pivot: [0.5, 0.5],
        }
    }

    /// A design-space textured quad.
    fn image(&self, aspect: f32, tex: TextureId, x: f32, y: f32, w: f32, h: f32) -> Sprite {
        let (pos, size) = Self::place(aspect, x, y, w, h);
        Sprite {
            texture: tex,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: 0.0,
            pivot: [0.5, 0.5],
        }
    }

    /// One line of text at a design-space position, `h` pixels tall, laid out glyph by glyph in the
    /// same space so the spacing is the original's whatever the window's shape.
    fn text(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        s: &str,
        x: f32,
        y: f32,
        h: f32,
        color: [f32; 4],
    ) {
        let Some(f) = &self.font else { return };
        let scale = h / f.line_px();
        let (glyphs, _) = f.place(s);
        for g in &glyphs {
            let (pos, size) = Self::place(
                aspect,
                x + g.x * scale,
                y + g.y * scale,
                g.w * scale,
                g.h * scale,
            );
            out.push(Sprite {
                texture: f.texture(),
                pos,
                size,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                color,
                rotation: 0.0,
                pivot: [0.0, 0.0],
            });
        }
    }

    /// A dial: the 128 px face centred on `centre` (design pixels) with its needle on top.
    fn dial_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        face: TextureId,
        needle: (TextureId, f32, f32, f32),
        centre: [f32; 2],
        spec: &nfs_formats::fedata::DialSpec,
        value: i32,
    ) {
        out.push(self.image(
            aspect,
            face,
            centre[0] - 64.0,
            centre[1] - 64.0,
            128.0,
            128.0,
        ));
        let (tex, nw, nh, pivot) = needle;
        let piv_y = 1.0 - pivot / nh;
        let (pos, size) = Self::place(aspect, centre[0] - nw * 0.5, centre[1] - nh * piv_y, nw, nh);
        out.push(Sprite {
            texture: tex,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: Self::needle_turns(spec, value),
            pivot: [0.5, piv_y],
        });
        if let Some(hub) = self.hub {
            out.push(self.image(aspect, hub, centre[0] - 10.0, centre[1] - 10.0, 20.0, 20.0));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sprites(
        &self,
        aspect: f32,
        speed_kmh: f32,
        rpm: f32,
        tick: u64,
        gear: usize,
        race: Option<&RaceProgress>,
        num_laps: u32,
        position: Option<(usize, usize)>,
    ) -> Vec<Sprite> {
        let mut out = Vec::new();
        // dials: speedometer tray (392,352) and tach (512,307) at 640×480, 128 px faces (hud.md §2/§6)
        self.dial_sprites(
            &mut out,
            aspect,
            self.dial,
            self.needle,
            [392.0 + 64.0, 352.0 + 64.0],
            &self.kmh,
            speed_kmh as i32,
        );
        if let Some((td, tn)) = self.tach {
            self.dial_sprites(
                &mut out,
                aspect,
                td,
                tn,
                [512.0 + 64.0, 307.0 + 64.0],
                &self.tach_spec,
                (rpm / 10.0) as i32,
            );
        }
        if self.font.is_some() {
            // trays: digital speed 73×35 @ (460,444), gear 85×35 @ (555,444), time 73×35 @ (427,33), position 138×15 @ (502,1)
            self.tray(&mut out, aspect, 460.0, 444.0, 73.0, 35.0);
            self.tray(&mut out, aspect, 555.0, 444.0, 85.0, 35.0);
            if race.is_some_and(|r| !r.finished) {
                self.tray(&mut out, aspect, 427.0, 33.0, 73.0, 35.0);
                self.tray(&mut out, aspect, 502.0, 1.0, 138.0, 15.0);
            }
            // digital speed / gear: value top-right orange, label bottom-left grey
            self.text(
                &mut out,
                aspect,
                &format!("{:03}", speed_kmh as i32),
                500.0,
                446.0,
                16.0,
                HUD_ORANGE,
            );
            self.text(&mut out, aspect, "km/h", 462.0, 464.0, 10.0, HUD_GREY);
            let gear_s = match gear {
                0 => "R".to_string(),
                1 => "N".to_string(),
                g => (g - 1).to_string(),
            };
            self.text(&mut out, aspect, &gear_s, 615.0, 446.0, 16.0, HUD_ORANGE);
            self.text(&mut out, aspect, "gear", 557.0, 464.0, 10.0, HUD_GREY);
            if let Some(r) = race {
                let race_t = tick.saturating_sub(RACE_START_TICK);
                let lap_t = tick.saturating_sub(r.lap_start_tick.max(RACE_START_TICK));
                // race time tray 73×35 @ (427,33); lap counter / position 138×15 @ (502,1)
                if !r.finished {
                    let shown = if num_laps >= 99 { race_t } else { lap_t };
                    self.text(
                        &mut out,
                        aspect,
                        &format_ticks(shown),
                        440.0,
                        36.0,
                        16.0,
                        HUD_ORANGE,
                    );
                    self.text(
                        &mut out,
                        aspect,
                        if num_laps >= 99 { "time" } else { "lap time" },
                        429.0,
                        54.0,
                        10.0,
                        HUD_GREY,
                    );
                    let mut top = String::new();
                    if num_laps < 99 {
                        top += &format!("LAP {}/{}", (r.laps + 1).min(num_laps), num_laps);
                    }
                    if let Some((p, n)) = position {
                        top += &format!("   POS {}/{}", p + 1, n);
                    }
                    self.text(&mut out, aspect, &top, 504.0, 3.0, 12.0, HUD_ORANGE);
                }
                // wrong way: centred, orange, blinking 64 frames on / 32 off (hud.md §5)
                if r.wrong_way_ticks > 0 && tick % 96 < 64 {
                    self.text(
                        &mut out,
                        aspect,
                        "WRONG WAY",
                        256.0,
                        230.0,
                        24.0,
                        HUD_ORANGE,
                    );
                }
            }
        }
        // countdown: digit and rings centred (hud.md §5)
        if tick < RACE_START_TICK {
            let idx = COUNTDOWN_TICKS.iter().rposition(|&t| tick >= t);
            if let Some(i) = idx {
                if let Some((full, outline)) = self.rings {
                    let mut ring =
                        self.image(aspect, full, 320.0 - 64.0, 240.0 - 64.0, 128.0, 128.0);
                    ring.color = [1.0, 1.0, 1.0, 0.5];
                    out.push(ring);
                    let mut ring =
                        self.image(aspect, outline, 320.0 - 64.0, 240.0 - 64.0, 128.0, 128.0);
                    ring.color = [1.0, 1.0, 1.0, 0.25];
                    out.push(ring);
                }
                if let Some(tex) = self.digits[i] {
                    let h = 106.0;
                    out.push(self.image(aspect, tex, 320.0 - h * 0.4, 240.0 - h * 0.5, h * 0.8, h));
                }
            }
        }
        out
    }
}

/// `0x5B8B18` — the model-group letter the chronicle's file names use, indexed by the group at
/// `[0x5DBA44]`. Five groups, and the five `icons*.fsh` packs in `FEData/Chron` match them.
const CHRONICLE_GROUP_LETTERS: &str = "CBADE";
/// `[0x5DBA44]` starts at 2, so the chronicle opens on `'A'` — the 356.
const CHRONICLE_GROUP: usize = 2;

/// `chronicle.lay`'s five model-group buttons, in the order the jump table at **0x507894** puts
/// them: 0x507839 `ID_RACEMODEL`, 0x50784B `ID_911BUTTON`, 0x50785D `ID_356BUTTON`, 0x50786F
/// `ID_924BUTTON`, 0x507881 `ID_914BUTTON`. The index is `[0x5DBA44]`, which is also what
/// [`CHRONICLE_GROUP_LETTERS`] is indexed by — so group 2 is `ID_356BUTTON` and letter `'A'`,
/// which is what the port already had. Each arm sets the flag on its own button and 0x5077D0
/// clears it on all five first. §4.111.
const CHRONICLE_GROUP_BUTTONS: [&str; 5] = ["ID_RACEMODEL", "ID_911BUTTON", "ID_356BUTTON", "ID_924BUTTON", "ID_914BUTTON"];

/// The front end's **multiple-choice** accessors: the name a layout writes, the `fe.txt` key that
/// holds the chosen index, and the `FEStrings` ids of the choices in order.
///
/// All three columns are read from the binary. The registration call is
/// `0x4BB680(name, slot, options, 0, 0, 0)` — for example 0x4E4FC6 pushes `0x5C`,
/// `0x5B6D08` and `"Display.Speedometer"` — where `options` is a zero-terminated list of
/// `FEStrings` ids (0x4BB6B7 counts it) and `slot` indexes the registry table the front end builds
/// at 0x4B1000…0x4B18xx as 12-byte records `{ int slot, void *storage, const char *key }`
/// (0x4B1504 is `{0x5C, 0x657DBC, "SPEEDOMETER"}`). Joining the two on the slot gives this table,
/// and the twelve `Display.*` rows line up with `fe.txt`'s own key names exactly. CONFIRMED.
///
/// The rows whose slot is not a registry slot keep their choice in screen state:
/// `Track<n>.Laps`, `Track<n>.Traffic`, `Track<n>.Direction`, `OwnedCar.PackageNum`,
/// `Car.Transmission`, `Game.Difficutly` (the original's own spelling), `Comms.Positional` and
/// `Comms.Collision`. `CDPlayer.View` is one of them and is listed here with an **empty key**,
/// because a list accessor's index starts at 0 (0x4BC1F0 writes `[this+0x18] = 0`) — so the
/// original opens on `FEStrings` 1851, "track time remaining", and the slider is not blank.
///
/// Read again 2026-09-19 by walking every `0x4BB680` call: the registry record is
/// `{ int slot, void *storage, const char *key }`, so a key's slot is the dword **three** before
/// it, not the one after — and joining the two that way puts every row above on its own `fe.txt`
/// name. The only row that was missing is `Display.RecordTime`, slot 0x68 = `RECORDTIME`.
/// How often a held scrollbar arrow steps — `0x529220(this, 0x96, 1)` at 0x4D0A97, 150 ms.
const SCROLL_REPEAT_MS: u32 = 0x96;

/// The two rows of the front end's boolean accessor, in index order — 0x4B8770 answers `FEStrings`
/// **705** "off" for every index but 1, and **704** "on" for 1.
const BOOL_OPTIONS: [u32; 2] = [705, 704];

/// `Comms.Positional`'s two rows, in index order — the option list 0x5B9674 that 0x50BB03 hands
/// 0x4BB680: `FEStrings` 281 "input exchange" and 282 "positional". 0x50BB1C starts it on **1**.
const POSITIONAL_OPTIONS: [u32; 2] = [281, 282];

const CHOICE_ACCESSORS: &[(&str, &str, &[u32])] = &[
    ("Audio.Mode", "audio3d", &[1500, 1501]),
    ("Audio.Quality", "audioquality", &[1504, 1503, 1502, 1500]),
    // the three the settings screens carry that this port had no answer for. Each is built by
    // **0x4BB680**, the choice accessor, with a NUL-terminated list of `FEStrings` ids:
    // `Car.ABS` at 0x4F25F1 and `Car.Traction` at 0x4F262E both take the list at 0x5DAA20 /
    // 0x5DAA0C — **1500, 1501**, "off" and "on" — and `Car.Roof` at 0x4F266B takes 0x5DA9F0,
    // **1509, 1510**, "up" and "down". The `fe.txt` this installation ships carries `car[0].abs`
    // and `car[0].traction`; it has no key for the roof, so that one starts on the slot's own
    // zero, as `CDPlayer.View` does.
    //
    // Each one's sixth argument is a predicate — 0x4F1DA0, 0x4F1DF0 and 0x4F1E40 — that looks the
    // current car up through `CARSETTINGS.CAR` and, when the byte at `car+0x2E2` (ABS), `car+0x2E3`
    // (traction) or `car+0x1BF` (roof) is clear, forces the value to 0 and locks it. The roof's is
    // applied where the race reads the value (`.CABRIODOWN`, the player's car upload); the other
    // two, and the lock, are not.
    // Three more the same walk turned up, each a `0x4BB680` call whose list this port had no row
    // for: `Car.Transmission` at 0x4F22B8 takes 0x5DAB5C, **560, 561** — "manual" and "automatic"
    // — and `fe.txt` names it `car[0].transmission`; `OwnedCar.PackageNum` at 0x4F1FD1 takes
    // 0x5B7644, **620…627**, "none" and "package 1" through "package 7"; `Comms.Collision` at
    // 0x50BB47 takes 0x5B95F4, **1500, 1501**. Neither of the last two has a `fe.txt` key, so both
    // open on index 0, which is where 0x4BC1F0 starts a list accessor. (`Comms.Collision`'s call
    // passes a **2** in the argument the others leave 0; what 0x4BC1F0 does with that slot was not
    // traced, so it is not read as a default here.)
    ("Car.ABS", "car[0].abs", &[1500, 1501]),
    ("Car.Roof", "", &[1509, 1510]),
    ("Car.Traction", "car[0].traction", &[1500, 1501]),
    ("Car.Transmission", "car[0].transmission", &[560, 561]),
    ("Comms.Collision", "", &[1500, 1501]),
    ("OwnedCar.PackageNum", "", &[620, 621, 622, 623, 624, 625, 626, 627]),
    // `Tournament.EventType` is the **two-list** choice accessor 0x4BB6F0: 0x4FD13E builds it
    // through 0x506A70 with `[this+0x24] = 0x5B81A4` — `FEStrings` 470, 471, 472, 473 — and
    // `[this+0x28] = 0x5B81B8` — 470, 476, 477, 478, the short forms. The drop-down's rows and
    // the combo's caption both come from slot 12 (0x4BB750, `[this+0x24]`), reached through
    // 0x4BC350 at 0x4BC38B and through 0x4BC4E0, so the long list is the one listed here; the
    // short one answers `(SHORT)`. It has no `fe.txt` key. §4.105.
    ("Tournament.EventType", "", &[470, 471, 472, 473]),
    ("CDPlayer.Options", "randomplaylist", &[1856, 1855]),
    ("CDPlayer.View", "", &[1851, 1850]),
    ("Display.Damage", "damagedisplay", &[1500, 1805, 1806, 1807]),
    ("Display.DispCols", "displaycolors", &[1821, 1822, 1823]),
    ("Display.Gear", "gear", &[1500, 1501]),
    ("Display.Map", "map", &[1500, 1808, 1809, 1830]),
    ("Display.MapScale", "mapscale", &[1810, 1811, 1812, 1813, 1814]),
    ("Display.OppList", "opponentlist", &[1500, 1832, 1833]),
    ("Display.RearView", "rearviewmirrror", &[1500, 1501]),
    ("Display.RecordTime", "recordtime", &[1500, 1501]),
    ("Display.ScreenLayout", "screenlayout", &[1824, 1825, 1826, 1827]),
    ("Display.SpeedUnits", "speedunits", &[1803, 1804]),
    ("Display.Speedometer", "speedometer", &[1500, 1801, 1802]),
    ("Display.SplitTimes", "splittimes", &[1500, 1816, 1817, 1819]),
    ("Display.Tach", "tachometer", &[1500, 1801, 1802]),
    ("Display.Translucency", "translucency", &[1501, 1500]),
    ("Graphics.CarChrome", "carchrome", &[1502, 1503, 1504]),
    ("Graphics.CarDetail", "cardetail", &[1502, 1503, 1504]),
    ("Graphics.DetailedShadows", "detailedshadows", &[1500, 1501]),
    ("Graphics.Headlights", "headlights", &[1508, 1505]),
    ("Graphics.LensFlare", "lensflare", &[1500, 1501]),
    ("Graphics.Reflections", "reflections", &[1502, 1503, 1504]),
    ("Graphics.ShadowMapping", "shadowmapping", &[1500, 1501]),
    ("Graphics.SkyRendering", "skyrendering", &[1500, 1511, 1504]),
    ("Graphics.SpecialFX", "specialfx", &[1502, 1503, 1504, 1507]),
    ("Graphics.TextureDetail", "texturedetail", &[1506, 1502, 401, 1504, 1507]),
    ("Graphics.TexturedDamage", "textureddamage", &[1500, 1501]),
    ("Graphics.TripleBuffer", "triplebuffer", &[1500, 1501]),
    ("Graphics.ViewDistance", "adjustableviewdistance", &[1502, 1503, 1504, 1507]),
    ("Graphics.WideScreen", "widescreen", &[1500, 1501]),
];

impl App {
    /// `Showcase.Text(<n>)` — entry **`n / 2 - 1`** of the selected car's own
    /// `FEData/Locale/<key>.loc`, the file its catalogue record names at +0x4D
    /// (`nfs_formats::loc`). The showcase screens (`chassis.lay`, `general.lay`,
    /// `glancestats.lay`) are nothing but these.
    ///
    /// The halving is not a guess. Every car `.loc` holds 45 entries, while `chassis.lay` asks for
    /// `Showcase.Text` up to 90, and every index any screen uses is even. `general.lay` settles it
    /// by pairing each watch with a label: `FEStrings` 12702 "weight" sits beside
    /// `Showcase.Text(24)`, 12700 "length" beside (26), 12708 "width" beside (28), 12707 "height"
    /// beside (30) and 12706 "wheel base" beside (32) — and the '96 993 Carrera's own entries 11 to
    /// 15 are "1390Kg", "4260 mm", "1735 mm", "1315 mm", "2272 mm", in that order. Five rows, five
    /// matches. CONFIRMED.
    fn showcase_text(&self, index: usize) -> Option<String> {
        let index = (index / 2).checked_sub(1)?;
        let key = self.logo_car(Some("zMergedList.Car"), None)?.locale_key.clone();
        if key.is_empty() {
            return None;
        }
        let dir = self.paths.fe_data().join("Locale");
        let path = GamePaths::find_file(&dir, &format!("{key}.loc"))?;
        let d = std::fs::read(path).ok()?;
        // language 0 is English, the column `FEStrings.csv` also leads with
        nfs_formats::loc::LocFile::parse(&d, 0).ok()?.get(index).map(str::to_string)
    }

    /// A range accessor's value: what the player has changed this session, else `fe.txt`, else the
    /// default the original registers. Returns the value with its minimum and maximum.
    fn range_value(&self, name: &str) -> Option<(i32, i32, i32)> {
        let (_, key, default, lo, hi) = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == name)?;
        // the five audio levels are live state the mixers read every tick, so they come from there
        let a = &self.snd.audio_settings;
        let v = match name {
            "Audio.MusicLevel" => a.music_level as i32,
            "Audio.SfxLevel" => a.sfx_level as i32,
            "Audio.EngineLevel" => a.engine_level as i32,
            "Audio.SpeechLevel" => a.speech_level as i32,
            "Audio.AmbientLevel" => a.ambient_level as i32,
            _ => match self.choices.get(*key) {
                Some(v) => *v as i32,
                None => self.registry.u32(key, *default as u32) as i32,
            },
        };
        Some((v.clamp(*lo, *hi), *lo, *hi))
    }

    /// The index a multiple-choice accessor is on: what the player has changed this session, else
    /// the `fe.txt` value, else 0 — the slot's own static initialiser, which is zero for every one
    /// of these (their storage in `.bss` at 0x657D5C…0x657DC4 is all zero in the image, and
    /// `fe.txt` carries only the keys that have been saved).
    /// The rows a combo slider's drop-down would list: every option of the choice accessor it
    /// edits, and the one it is on. An accessor that is not one of [`CHOICE_ACCESSORS`] — a
    /// numeric range, say — has no enumerable options here, and that combo simply does not open.
    /// The rows a combo drops down when its accessor is **not** one of the multiple-choice ones.
    ///
    /// `FeComboBox` does not keep a list of its own: 0x4C66E8's loop walks the accessor, asking it
    /// for each entry in turn, so the drop-down is exactly the accessor's own enumeration with the
    /// current index marked. For an **int range** that enumeration is `max - min + 1` entries
    /// whose index is the value less the minimum — 0x4BA329 builds precisely that many and
    /// 0x4BA34A / 0x4BA362 index them that way (§4.81) — and for a list accessor it is the list.
    ///
    /// So each family here is enumerated from the very source this port already answers the
    /// combo's **caption** from, which is what makes it a reading rather than a guess: the car
    /// combos from the car table the caption's `(*m.car()?)` indexes, the track combos from
    /// `m.tracks`, the paint combos from the selected car's paint sets, and the four counted
    /// settings from the clamps their own steppers carry. §4.91.
    /// Every car on the used-car lot — the vector at `[0x691B88]`. It holds **both** the cars the
    /// game generates (0x4F80C0) and the player's own listings, which `0x4F91C0` appends to the
    /// same vector with `car+0x6A0 = 1`. This port keeps the two apart — generated cars in
    /// `Career::market`, listings in the garage under an `asking_price` — so it chains them, which
    /// matches the original's contents but not the order it happens to have appended them in.
    fn used_cars(&self) -> Vec<&nfs_game::career::OwnedCar> {
        let Some(c) = self.career.as_ref() else { return Vec::new() };
        c.career.market.iter().chain(c.career.garage.iter().filter(|o| o.asking_price.is_some())).collect()
    }

    /// The car the used-car screens are on — `UsedCar.Car`'s current row.
    fn used_car(&self) -> Option<&nfs_game::career::OwnedCar> {
        let all = self.used_cars();
        all.get(self.fe.usedcar_sel.min(all.len().saturating_sub(1))).copied()
    }

    /// The catalogue records behind the player's garage, in save order — `OwnedCars.Car`'s own
    /// list. Its accessor (`FEOwnedCarAccessor`, vtable 0x5B7868) counts
    /// `(end − begin) / 4` of the vector its `[this+0x2C]` thunk hands back (0x4F5CBD) and reads
    /// row `i` as `begin[i] + 6` (0x4F5C5F) — the FE car object's `+4` record plus its `+2` name.
    fn garage_cars(&self) -> Vec<&nfs_formats::fedata::CarCatalogEntry> {
        let Some(c) = self.career.as_ref() else { return Vec::new() };
        c.career.garage.iter().filter_map(|o| c.tables.car(o.car_id)).collect()
    }

    /// The garage half of a car list, with `FEOwnedCarAccessor`'s headings — §4.144. Pushes a
    /// `GroupHeading` for the era **and** the class when the era changes, one for the class when
    /// only the class does, then the car's own row; returns the row index of `select`.
    fn garage_rows(&self, rows: &mut Vec<ListRow>, short: bool, select: usize) -> Option<usize> {
        let mut at = None;
        let mut prev: Option<(u32, u32)> = None;
        for (i, e) in self.garage_cars().into_iter().enumerate() {
            let key = (e.era_string_id, e.class_string_id);
            let heading = |id: u32| ListRow::GroupHeading(self.fe.strings.get(id).unwrap_or_default().to_uppercase());
            match prev {
                None => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.0 != key.0 => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.1 != key.1 => rows.push(heading(key.1)),
                _ => {}
            }
            prev = Some(key);
            if i == select {
                at = Some(rows.len());
            }
            rows.push(ListRow::Text(if short { e.family.to_uppercase() } else { e.display_name.to_uppercase() }));
        }
        at
    }

    fn combo_rows(&self, name: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let (base, modifier) = match name.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')').trim()),
            None => (name, ""),
        };
        let m = self.fe.menu.as_ref()?;
        let num = |lo: i64, hi: i64, cur: i64| {
            let rows = (lo..=hi).map(|v| ListRow::Text(v.to_string())).collect();
            Some((rows, Some((cur - lo).max(0) as usize)))
        };
        match base {
            // **`zMergedList.Car` is two lists, not one.** `FEMergedCarListAccessor` (vtable
            // 0x5B773C) holds list A at `[this+0x28]` and list B at `[this+0x2C]`. Its rebuild
            // 0x4F4DB0 rebuilds both and then sets its own count to `A.count + B.count`
            // (0x4F4DD6 `add edi, eax` into 0x4BC310), and its row text 0x4F4A90 answers
            // `i < A.count ? A.row(i) : B.row(i − A.count)`. Its one construction, 0x4F2278,
            // passes the owning object's `[esi+0x14]` and `[esi+4]`, which the same registration
            // built as **`OwnedCars.Car`** (0x4F1ED2) and **`GenericCar.Car`** (0x4F20C5). Both
            // sub-lists' row text is the car record's name field — `owned+6` at 0x4F5C5F and
            // `record+2` at 0x4F300F, which is the same `display_name`. So the drop-down is the
            // player's garage first, then the whole catalogue. `RE/notes/controls.md` §4.133.
            //
            // **And its garage half is grouped.** Slot 26 (0x4F4D40) routes the row height to
            // `A->slot26(i)` below `A.count` and `B->slot26(i − A.count)` above it, and slot 25
            // (0x4F4D20) is `A->slot25() + B->slot25()` — the two lists' heights added. List A is
            // `OwnedCars.Car`, whose rows are 42 / 28 / 14 with era and class headings (§4.144),
            // so the merged drop-down carries those headings over its first half and stays flat
            // over the catalogue. §4.148.
            "zMergedList.Car" => {
                let short = modifier == "SHORT";
                let mut rows: Vec<ListRow> = Vec::new();
                self.garage_rows(&mut rows, short, usize::MAX);
                let a = rows.len();
                rows.extend(m.cars.iter().map(|c| {
                    ListRow::Text(if short { c.family.to_uppercase() } else { c.display_name.to_uppercase() })
                }));
                // the port's selection is an index into the catalogue (list B), so it sits at
                // `A.count + car_idx` in the merged numbering
                Some((rows, Some(a + m.car_idx)))
            }
            // the car tables — `(SHORT)` is the family name, the plain form the display name,
            // exactly as the caption resolves them
            // `usedcars.lay`'s `ID_CHOOSEUSEDBOX` is a combo on `UsedCar.Car(SHORT)` — the market,
            // one row per generated car, not the catalogue
            "UsedCar.Car" => {
                let c = self.career.as_ref()?;
                let rows = self
                    .used_cars()
                    .into_iter()
                    .filter_map(|o| c.tables.car(o.car_id))
                    .map(|e| ListRow::Text(if modifier == "SHORT" { e.family.to_uppercase() } else { e.display_name.to_uppercase() }))
                    .collect::<Vec<_>>();
                let cur = self.fe.usedcar_sel.min(rows.len().saturating_sub(1));
                Some((rows, Some(cur)))
            }
            // **`OpponentCar.Car` is not a car list.** `FEOpponentCarAccessor`'s row text
            // (0x4F5720) is a jump table over five `FEStrings` ids — 640 `same car`, 642
            // `same era`, 643 `same class`, 644 `same era/class`, 641 `random car`, in that row
            // order — and its own `slot25` answers **70**, five rows of fourteen. The value the
            // widget carries is the row index, which is what `[accessor+0x33C4]` holds and what
            // `nfs_game::opponents::OpponentChoice::from_index` reads (`modes.md` §1.1a).
            "OpponentCar.Car" => {
                let rows = nfs_game::opponents::OpponentChoice::ROWS
                    .iter()
                    .map(|c| ListRow::Text(self.fe.strings.get(c.string_id()).unwrap_or_default().to_uppercase()))
                    .collect();
                Some((rows, Some((m.opp_car as usize).min(4))))
            }
            // **The garage picker is the garage, and it is grouped.** `garage.lay`'s
            // `ID_CHOOSECARBOX` and `paintshop.lay`'s `ID_COMBOBOX` are `OwnedCars.Car(SHORT)`,
            // and `FEOwnedCarAccessor`'s row heights (slot 26, **0x4F60F0**) are **42** for the
            // first row and for any row whose **era** differs from the one above it, **28** when
            // only its **class** differs, and **14** otherwise. The painter 0x4F5D48 draws the
            // heading block for exactly those changes — two rules 13 apart with
            // `FEStrings[the id]` between them — and the ids are the FE car object's `+0x1B4` and
            // `+0x1B0`, which are the record's `+0x1B0` **era** and `+0x1AC` **class** (the
            // object holds the record at +4, `savegame.md` §4.7a). §4.144.
            "OwnedCars.Car" => {
                let c = self.career.as_ref()?;
                let mut rows: Vec<ListRow> = Vec::new();
                let at = self.garage_rows(&mut rows, modifier == "SHORT", c.car_sel.min(c.career.garage.len().saturating_sub(1)));
                Some((rows, at))
            }
            "PrizeCars.Car"
            | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" | "GenericCar2.Car" => {
                let rows = m
                    .cars
                    .iter()
                    .map(|c| {
                        ListRow::Text(if modifier == "SHORT" { c.family.to_uppercase() } else { c.display_name.to_uppercase() })
                    })
                    .collect();
                let cur = if base == "GenericCar2.Car" { m.car2_idx } else { m.car_idx };
                Some((rows, Some(cur)))
            }
            "Track1.Track" | "LoopedTrack.Track" | "Knockout.Track" | "Track.Track" | "Record.Track" => {
                Some((m.tracks.iter().map(|t| ListRow::Text(t.to_uppercase())).collect(), Some(m.track_idx)))
            }
            // a paint combo counts the selected car's own sets from 1, which is what its caption
            // shows (`m.paint_idx + 1`)
            "OwnedCar.ExtColour" | "OwnedCar.IntColour" | "OwnedCar.S1Colour" | "OwnedCar.S2Colour"
            | "GenericCar.Colour" | "DealerCar.Colour" | "DealerCar.IntColour" => {
                let n = m.cars.get(m.car_idx).map(|c| c.paint_sets.len()).unwrap_or(0).max(1) as i64;
                num(1, n, m.paint_idx as i64 + 1)
            }
            // the two two-way ones: the direction's rows are `DIRECTION_OPTIONS`, the same pair
            // its caption reads, and the traffic's `TRAFFIC_OPTIONS`' four
            "Track1.Direction" | "Knockout.Direction" => {
                let b = if base == "Knockout.Direction" { m.ko.get(m.ko_sel).map_or(m.backwards, |k| k.backwards) } else { m.backwards };
                let rows = DIRECTION_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some(usize::from(b))))
            }
            "Knockout.Traffic" => {
                let t = m.ko.get(m.ko_sel).map_or(m.traffic, |k| k.traffic);
                let rows = TRAFFIC_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some((t as usize).min(3))))
            }
            "Knockout.Laps" => num(1, 10, m.ko.get(m.ko_sel).map_or(m.laps, |k| k.laps) as i64),
            // `Game.Difficutly` — the file's own spelling — is the three skills its caption reads
            "Game.Difficutly" => {
                let rows = ["EASY", "MEDIUM", "HARD"].iter().map(|t| ListRow::Text((*t).to_string())).collect();
                Some((rows, Some((self.race_skill as usize).min(2))))
            }
            // `Tournament.Tournament` — class 0x5B88F0, whose `[this+0x24]` is **0x5053C0**, the
            // function that picks the tournament vector, and whose row text (slot 12, 0x506560) is
            // `FEStrings[rec+0x24]`, the port's `title_string_id`. 0x5053C0's own choice is the
            // jump table at 0x505450 over `Tournament.EventType`'s index: **0** falls to 0x505404,
            // which switches on the era id `[0x691E50]` — 0x209, 0x20A, 0x20B are 521, 522, 523,
            // the three eras — and **1**, **2**, **3** take the classic, golden and modern club
            // lists at 0x505447, 0x505441 and 0x50543B. §4.105.
            "Tournament.Tournament" => {
                let c = self.career.as_ref()?;
                let list = match self.choice_index("Tournament.EventType").map(|(i, _)| i) {
                    Some(1) => c.tables.club_tournaments_of(nfs_formats::career::Era::Classic),
                    Some(2) => c.tables.club_tournaments_of(nfs_formats::career::Era::Golden),
                    Some(3) => c.tables.club_tournaments_of(nfs_formats::career::Era::Modern),
                    _ => c.tables.era_tournaments(c.career.era),
                };
                let cur = c.current().and_then(|t| list.iter().position(|x| x.id == t.id));
                let rows = list
                    .iter()
                    .map(|t| ListRow::Text(self.fe.strings.get(if modifier == "SHORT" { t.name_string_id } else { t.title_string_id }).unwrap_or_default().to_uppercase()))
                    .collect::<Vec<_>>();
                (!rows.is_empty()).then(|| (rows, cur.or(Some(0))))
            }
            // `Tournament.RemainingTracks` — class 0x5B7E80, `[this+0x24] = 0x505170`, and its row
            // text (slot 12, 0x4FA560) is `FEStrings[rec+4]` of the i-th record in that vector.
            // `+4` is a `.trk` record's own name id (the port's `Track::name_string_id`), which is
            // what makes these rows the remaining events' **track names**. 0x4FA575 wraps an index
            // at or past the count back to 0.
            "Tournament.RemainingTracks" => {
                let c = self.career.as_ref()?;
                let t = c.current()?;
                let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                let rows: Vec<ListRow> =
                    t.events.iter().skip(done).map(|e| ListRow::Text(c.tables.track_name(e.track_id).to_uppercase())).collect();
                (!rows.is_empty()).then(|| (rows, Some(0)))
            }
            // `Controller.AllControllers` and `Controller.AllPads` — two empty string lists
            // (0x4B8990 at 0x51E4AE and 0x51E4E1) that 0x511809 fills from the input layer's own
            // enumeration: the count `0x51F050()`, each device's name `0x51F060(i)` appended to
            // the controllers' list, and the same name appended to the pads' list only when
            // `0x51F080(i)` says so. §4.103.
            "Controller.AllControllers" | "Controller.AllPads" => {
                let pads_only = base == "Controller.AllPads";
                let rows: Vec<ListRow> = self
                    .gamepads
                    .devices()
                    .into_iter()
                    .filter(|(_, is_pad)| !pads_only || *is_pad)
                    .map(|(n, _)| ListRow::Text(n.to_uppercase()))
                    .collect();
                (!rows.is_empty()).then(|| (rows, Some(0)))
            }
            // `Graphics.Resolutions` — 0x4BC710 formats one row per display mode as
            // `"%d x %d x %d"` (0x5D7574) from the width, height and depth `0x44E720(i, &out)`
            // writes, over the modes `0x44E780(i)` passes. This port has no Direct3D mode list, so
            // it takes the arm 0x44E72A takes when there is no hardware device: **one** mode,
            // 0x280 x 0x1E0 x 0x10. §4.104.
            "Graphics.Resolutions" => Some((vec![ListRow::Text("640 x 480 x 16".to_string())], Some(0))),
            // `Game.Damage` is the front end's one **boolean** accessor a layout names: class
            // 0x5B4C64, two rows, the row text from 0x4B8770 — index 1 is `FEStrings` 0x2C0 = 704
            // "on" and anything else 0x2C1 = 705 "off". §4.102.
            "Game.Damage" => {
                let rows = BOOL_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some(usize::from(m.damage))))
            }
            // and the four counted settings, at the ranges their own steppers clamp to
            "Game.NumOpponents" => num(0, 7, m.opponents as i64),
            "Game.NumKOOpponents" => num(1, 7, m.ko_opponents as i64),
            "Track1.Laps" => num(1, 10, m.laps as i64),
            "Track1.Traffic" => num(0, 3, m.traffic as i64),
            _ => None,
        }
    }

    fn choice_rows(&self, name: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let base = name.split_once('(').map(|(b, _)| b).unwrap_or(name);
        let (index, options) = self.choice_index(base)?;
        let rows = options
            .iter()
            .map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string()))
            .collect();
        Some((rows, Some(index)))
    }

    fn choice_index(&self, name: &str) -> Option<(usize, &'static [u32])> {
        let (_, key, options) = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == name)?;
        let v = match self.choices.get(Self::choice_slot(name, key)) {
            Some(v) => *v,
            None => self.registry.u32(key, 0) as usize,
        };
        Some((v.min(options.len() - 1), options))
    }

    /// Where this port keeps a choice accessor's index. The original keeps it in the accessor
    /// object itself, `[this+0x18]` (0x4BC460 / 0x4BC480); the `fe.txt` key is only where the
    /// *saved* ones are written back. Several accessors have **no** `fe.txt` key — `Car.Roof`,
    /// `Comms.Collision`, `OwnedCar.PackageNum`, `CDPlayer.View`, `Tournament.EventType` — so
    /// those are kept under their own name instead of all sharing the empty one.
    fn choice_slot<'a>(name: &'a str, key: &'a str) -> &'a str {
        if key.is_empty() { name } else { key }
    }
}

/// The repair screens' per-category condition accessors and the `nfs5.prt` category (`part+0x26`)
/// each one measures.
///
/// Every one is a getter of the same shape — 0x4EF640 is `ENGINEDAMAGE`'s: walk the car's
/// installed-parts vector at `car+0x758`, add `part+0x164` (the damage that part carries) for each
/// part whose `part+0x26` matches, and divide by how many matched when that is more than one. The
/// accessor then shows **`100 - that`** (`fsubr [0x5B242C]`, which is 100.0, formatted `"%2.0f"`
/// at 0x4F63B4) — a condition, not a damage. CONFIRMED, each getter read:
/// 1 → 0x4EF640, 2 → 0x4EF690, 3 → 0x4EF6E0, 4 → 0x4EF730, 5 → 0x4EF780, 6 → 0x4EF7D0,
/// 8 → 0x4EF870, 9 → 0x4EF8C0, 10 → 0x4EF910.
///
/// `OVERALLDAMAGE` is not here: 0x4EF960 combines the per-category means itself (it calls the
/// parameterised mean 0x4EF580 for 2, 3 and 7 and inlines the others), and the port answers it
/// from the car's own accumulated figure instead.
const PART_DAMAGE_CATEGORY: &[(&str, u16)] = &[
    ("ENGINEDAMAGE", 1),
    ("BRAKESDAMAGE", 2),
    ("TRANSMISSIONDAMAGE", 3),
    ("SUSPENSIONDAMAGE", 4),
    ("SHOCKSDAMAGE", 5),
    ("BODYWORKDAMAGE", 6),
    ("SWAYBARSDAMAGE", 8),
    ("TIRESDAMAGE", 9),
    ("COSMETICDAMAGE", 10),
];

/// The front end's **range** accessors: a name, its `fe.txt` key, and the default, minimum and
/// maximum the original registers for it.
///
/// The registration is `0x4BA310(name, slot, default, min, max, step, callback)` — 0x4E4937 pushes
/// `"Audio.MusicLevel"`, slot `0x37` (`MUSICLEVEL`), `0x7F`, `0`, `0x7F` — and the ctor builds the
/// accessor over `max - min + 1` entries (0x4BA329 computes exactly that, then hands it to the
/// same base 0x4BC1F0 the option lists use). The slots are the registry table of
/// [`CHOICE_ACCESSORS`]. CONFIRMED: the five audio defaults are the 127 `fe.txt` ships, and the
/// gamma default is its 100.
///
/// `Chronicle.Year` is registered the same way, with 1948, 1948 and 1996 — which is where the
/// chronicle's range comes from.
const RANGE_ACCESSORS: &[(&str, &str, i32, i32, i32)] = &[
    ("Audio.MusicLevel", "musiclevel", 127, 0, 127),
    ("Audio.SfxLevel", "sfxlevel", 127, 0, 127),
    ("Audio.EngineLevel", "enginelevel", 127, 0, 127),
    ("Audio.SpeechLevel", "speechlevel", 127, 0, 127),
    ("Audio.AmbientLevel", "ambientlevel", 127, 0, 127),
    ("FF.TotalVolume", "fftotalvolume0", 50, 0, 100),
    ("FF.RoadVolume", "ffroadvolume0", 50, 0, 100),
    ("FF.EngineVolume", "ffenginevolume0", 50, 0, 100),
    ("FF.CollisionVolume", "ffcollisionvolume0", 50, 0, 100),
    ("FF.RoadGripVolume", "ffroadgripvolume0", 50, 0, 100),
    ("Graphics.GammaCorrection", "gammacorrection", 100, 0, 400),
];

/// One row of the knockout location table (`locationsetup.lay`).
#[derive(Debug, Clone, Default, PartialEq)]
struct KoSlot {
    track_idx: usize,
    laps: u32,
    /// `TRAFFIC_DENSITY` 0..3, in the order the screen's own option list gives them
    traffic: u32,
    backwards: bool,
}

/// `Track<n>.Traffic`'s option list, from the registration at 0x4BB680: `FEStrings` 403, 402, 401
/// and 400 — "none", "light", "medium", "heavy". CONFIRMED.
const TRAFFIC_OPTIONS: [u32; 4] = [403, 402, 401, 400];
/// `Track<n>.Direction`'s option list: `FEStrings` 410…413 — "forward", "backward",
/// "forward/mirrored", "backward/mirrored". CONFIRMED. The port has no mirrored tracks, so it
/// uses the first two.
const DIRECTION_OPTIONS: [u32; 4] = [410, 411, 412, 413];

/// `TRAFFIC_DENSITY` 0..3 → number of traffic cars for a single race, quick knockout and knockout:
/// `_ftol(index · 2.5)` (0x4FC13B, `traffic.md` §1). Career events use `{0, 2, 4, 8}` instead.
const TRAFFIC_COUNT: [u32; 4] = [0, 2, 5, 7];

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
    /// `locationsetup.lay`'s table of knockout rounds: one row per opponent, each with its own
    /// track, laps, direction and traffic. `Knockout.TrackNumber`'s accessor (0x4BA760) asks
    /// `Game.NumKOOpponents` for the row count and answers the empty string for a row past it
    /// (0x4BA7CF), so the list is as long as the opponent count.
    ko: Vec<KoSlot>,
    /// which row of that table the screen's own `Knockout.Track` / `.Laps` / `.Traffic` /
    /// `.Direction` combo boxes edit
    ko_sel: usize,
    /// `GenericCar2.Car` — the **second** car of the compare screen, the one its
    /// `NFS5.ComboBoxSlider` cycles. `carcompare.lay` gives the left column `CarCompare1` (the car
    /// the screen was entered with) and the right column `GenericCar2.Car`, so the two are separate
    /// selections and need separate indices.
    car2_idx: usize,
    /// `Game.NumOpponents` — registered at 0x4E5543 as
    /// `0x4BA310(name, 0, 4, 0, 7, 2, 0)`: **default 4**, minimum **0**, maximum 7. `QuickRace.lay`
    /// is the one layout that names it.
    opponents: u32,
    /// `Game.NumKOOpponents` — a **separate** accessor, registered at 0x4E5576 with the same
    /// shape but a minimum of **1** (0x4E5570): default 4, 1…7. `knockout.lay`,
    /// `locationsetup.lay` and `quickknockout.lay` are the three layouts that name it, and it is
    /// what `Knockout.TrackNumber` measures its table by (0x4BA786).
    ko_opponents: u32,
    /// `Game.Damage` — the **boolean** accessor 0x4B8720 builds at 0x4E55DD from
    /// `("Game.Damage", 0, 1, 0)`. Its third argument is the initial value, fed to the setter
    /// 0x4BC240 as `!= 0`, so the setting starts **on**. Two rows, `FEStrings` 705 "off" at index
    /// 0 and 704 "on" at index 1 — 0x4B8770, the class's row-text slot (§4.102).
    damage: bool,
    /// `OpponentCar.Car` — which cars the opponents drive (`nfs-game/src/opponents.rs`,
    /// `modes.md` §1.1a). The original's accessor is built with this at 0, "same car".
    opp_car: u8,
    paint_idx: usize,
    /// quick-race laps (circuits; point-to-point tracks always run one)
    laps: u32,
    /// `TRAFFIC_DENSITY` 0..3 — none / light / medium / heavy (`traffic.md` §1)
    traffic: u32,
    /// `COPS` — four police cars in a single race (`traffic.md` §17.1)
    cops: bool,
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
    /// **The selected track, with the original's own out-of-range rule.** `FETrackAccessor`'s row
    /// getters all guard the index the same way — 0x4FA5A0, 0x4FA5E0 and 0x4FA620 each take the
    /// list's length as `(end − begin)/4` and, when the index is **not below** it, use **0**
    /// instead (`cmp ; jb ; xor edi, edi`). Slot 28 does it to the accessor's own current index,
    /// which is this. So a stale selection shows the first track rather than nothing — and rather
    /// than the panic a bare `self.tracks[self.track_idx]` would be.
    fn track(&self) -> &str {
        let i = if self.track_idx < self.tracks.len() { self.track_idx } else { 0 };
        self.tracks.get(i).map(String::as_str).unwrap_or("")
    }

    /// **The selected car, with the original's rule.** The car lists clamp in the *setter*, not the
    /// getter: `FEGenericCarAccessor`'s slot 11, 0x4F3060, takes the count from its own slot 9,
    /// compares the new index with `n − 1` and zeroes it when it is greater (0x4F3070/0x4F3072),
    /// zeroes it again when it is negative (0x4F3078 → 0x4F308B), and then **re-measures the vector
    /// and clamps a third time** (0x4F3087). Because the index can never be out of range, every
    /// getter indexes freely. This port keeps its index in a field nothing clamps, so the clamp has
    /// to live here — same effect for a read, and it is the eight `cars[car_idx]` panics that were
    /// the alternative.
    fn car(&self) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let i = if self.car_idx < self.cars.len() { self.car_idx } else { 0 };
        self.cars.get(i)
    }

    /// Keep the knockout location table as long as the opponent count, which is what
    /// `Knockout.TrackNumber`'s accessor measures it by (0x4BA786 reads `Game.NumKOOpponents`, and
    /// 0x4BA7CF answers the empty string for any row at or past it). A row that appears takes the
    /// screen's single-race settings, which is what it shows before the player edits it.
    fn fit_ko(&mut self) {
        let want = self.ko_opponents as usize;
        while self.ko.len() < want {
            let i = self.ko.len();
            self.ko.push(KoSlot { track_idx: (self.track_idx + i) % self.tracks.len().max(1), laps: self.laps, traffic: self.traffic, backwards: self.backwards });
        }
        self.ko.truncate(want);
        self.ko_sel = self.ko_sel.min(want.saturating_sub(1));
    }

    fn rows(&self) -> u8 {
        match self.mode {
            0 | 4 | 5 => 10,
            3 => 2,
            _ => 4,
        }
    }

    fn sprites(&self, career: Option<&CareerUi>) -> Vec<Sprite> {
        let Some(f) = &self.font else {
            return Vec::new();
        };
        let mut out = f.sprites(
            "PORSCHE UNLEASHED  -  NATIVE",
            [0.05, 0.08],
            0.05,
            [1.0, 1.0, 1.0, 1.0],
        );
        let sel = |r: u8| {
            if self.row == r {
                [1.0, 0.75, 0.1, 1.0]
            } else {
                [0.85, 0.85, 0.85, 1.0]
            }
        };
        out.extend(f.sprites(
            &format!(
                "MODE    < {} >",
                [
                    "QUICK RACE",
                    "EVOLUTION",
                    "TOURNAMENT",
                    "FACTORY DRIVER",
                    "QUICK KNOCKOUT",
                    "KNOCKOUT"
                ][self.mode.min(5) as usize]
            ),
            [0.08, 0.15],
            0.04,
            sel(0),
        ));
        if (1..=3).contains(&self.mode) {
            match career {
                // career rows share the cursor: menu row 1 = tournament, row 2 = car
                Some(c) => out.extend(c.sprites(f, (self.row > 0).then(|| self.row - 1))),
                None => out.extend(f.sprites(
                    "CAREER TABLES UNAVAILABLE",
                    [0.08, 0.30],
                    0.045,
                    [0.85, 0.85, 0.85, 1.0],
                )),
            }
            out.extend(f.sprites(
                "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER BUY / RACE   ESC QUIT",
                [0.08, 0.85],
                0.028,
                [0.6, 0.6, 0.6, 1.0],
            ));
            return out;
        }
        if self.mode == 5 {
            out.extend(f.sprites(&self.knockout_line, [0.08, 0.30], 0.045, sel(1)));
        } else {
            out.extend(f.sprites(
                &format!("TRACK   < {} >", self.track().to_uppercase()),
                [0.08, 0.30],
                0.045,
                sel(1),
            ));
        }
        let Some(c) = self.car() else { return Vec::new() };
        out.extend(f.sprites(
            &format!("CAR     < {} >", c.display_name.to_uppercase()),
            [0.08, 0.40],
            0.045,
            sel(2),
        ));
        let paint = c
            .paint_sets
            .get(self.paint_idx % c.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        out.extend(f.sprites(
            &format!(
                "PAINT   < {} / {} >",
                self.paint_idx % c.paint_sets.len().max(1) + 1,
                c.paint_sets.len()
            ),
            [0.08, 0.48],
            0.045,
            sel(3),
        ));
        out.extend(f.sprites(
            "####",
            [0.42, 0.48],
            0.045,
            [
                paint[0] as f32 / 255.0,
                paint[1] as f32 / 255.0,
                paint[2] as f32 / 255.0,
                1.0,
            ],
        ));
        out.extend(f.sprites(
            &format!("OPPONENTS  < {} >", if self.mode >= 4 { self.ko_opponents } else { self.opponents }),
            [0.08, 0.545],
            0.045,
            sel(4),
        ));
        // `OpponentCar.Car`: FE strings 640..644, default "same car" (`modes.md` §1.1a)
        out.extend(f.sprites(
            &format!(
                "THEY DRIVE < {} >",
                nfs_game::opponents::OpponentChoice::from_index(self.opp_car as u32).label()
            ),
            [0.08, 0.602],
            0.04,
            sel(9),
        ));
        if self.mode == 4 {
            out.extend(f.sprites(
                &format!("LAPS    {}  (ONE CAR OUT PER LAP)", self.ko_opponents.max(1)),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        } else if self.mode == 5 {
            out.extend(f.sprites(
                &format!(
                    "RACES   {}   LAPS < {} >   (LAST CAR OUT EACH RACE)",
                    self.ko_opponents.max(1),
                    self.laps
                ),
                [0.08, 0.66],
                0.04,
                sel(5),
            ));
        } else {
            out.extend(f.sprites(
                &format!("LAPS    < {} >", self.laps),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        }
        // Track1.Direction ids 0x19A..0x19D: forward / backward / forward mirrored / backward mirrored
        let dir = match (self.backwards, self.mirror) {
            (false, false) => "FORWARD",
            (true, false) => "BACKWARD",
            (false, true) => "FORWARD MIRRORED",
            (true, true) => "BACKWARD MIRRORED",
        };
        out.extend(f.sprites(
            &format!("DIRECTION  < {dir} >"),
            [0.08, 0.74],
            0.045,
            sel(6),
        ));
        // FE strings 403/402/401/400; the count is `_ftol(index · 2.5)` = 0/2/5/7 (`traffic.md` §1)
        let density = ["NONE", "LIGHT", "MEDIUM", "HEAVY"][self.traffic.min(3) as usize];
        out.extend(f.sprites(
            &format!(
                "TRAFFIC    < {density} >   ({} CARS)",
                TRAFFIC_COUNT[self.traffic.min(3) as usize]
            ),
            [0.08, 0.80],
            0.04,
            sel(7),
        ));
        out.extend(f.sprites(
            &format!(
                "POLICE     < {} >",
                if self.cops { "ON  (4 CARS)" } else { "OFF" }
            ),
            [0.08, 0.845],
            0.04,
            sel(8),
        ));
        out.extend(f.sprites(
            "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER START   ESC QUIT",
            [0.08, 0.91],
            0.026,
            [0.6, 0.6, 0.6, 1.0],
        ));
        out
    }
}

struct App {
    paths: GamePaths,
    /// the front end's screens and per-screen state — see [`FrontEnd`]
    fe: FrontEnd,
    career: Option<CareerUi>,
    /// opponents of the current career event (None = quick-race opponents)
    career_opponents: Option<Vec<CareerOpponent>>,
    /// `OpponentCar.Car`'s selection for the race being set up (`modes.md` §1.1a)
    opp_choice: u32,
    /// the `nfs5.car` id of the car the player picked, so the opponent rule can match era/class
    player_car_id: Option<u16>,
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
    /// recording and playback of the controller stream — see [`Playback`]
    replay: Playback,
    /// Seed of the shared RNG for this race; travels with a replay so playback re-simulates the
    /// same traffic, police and AI.
    traffic_seed: u32,
    /// `[0x65B940]`, the name-box cheat bits (`nfs_game::cheats`); read through 0x4D8790
    cheat_bits: u32,
    /// The profile name the original stores in a track record (`player.lst` → `LoadInfo`).
    player_name: String,
    /// `nfs5.car` id of the car being driven, for the replay snapshot's car slot.
    car_id: u16,
    /// headless run (`--autotest`): the bot may reset itself when stuck
    scripted: bool,
    stuck_ticks: u32,
    /// detail level (`[0x657D70]` → 0.5 / 0.67 / 0.83 / 1.0) scaling the `.key` draw distance
    detail: f32,
    /// `SPLITTIMES` (`[0x657DD8]`): 1 record lap, 2 best lap, 3 last lap — circuits only; the
    /// record time is always shown on point-to-point tracks (`HRecordTime` 0x42A200, hud.md §6.3).
    split_times: u8,
    records_path: std::path::PathBuf,
    record_done: bool,
    track: TrackAssets,
    car: CarAssets,
    screenshot: Option<String>,
    /// `--layout-sweep`: load every `.lay` in the game, draw it once, and report every widget that
    /// produced no sprite at all. The port's own measure of what the front end does not draw.
    layout_sweep: bool,
    /// `fe.txt`, for the multiple-choice accessors' saved indices.
    registry: nfs_formats::fetxt::Registry,
    /// The choices the player has changed on the options screens this session, by `fe.txt` key.
    /// The original writes them straight back into the registry slot; the port keeps them here
    /// rather than rewriting the read-only game directory.
    choices: std::collections::HashMap<String, usize>,
    /// scripted drive before the screenshot: seconds of full throttle (steer right for the last third)
    autotest: Option<f32>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<TrackWorld>,
    car_model: Option<CarModel>,
    /// the cockpit view's eye point from the player car's `.tpg`, when it has one
    driver_cam: Option<nfs_assets::car::DriverCam>,
    sky: Option<Sky>,
    scene: Scene,
    camera: Camera,
    input: InputState,
    gamepads: Gamepads,
    car_pos: Vec3,
    car_rot: Quat,
    specs: CarSpecs,
    vehicle: Option<Vehicle>,
    /// Index into `camera_cycle`.
    camera_mode: u8,
    /// The views the player cycles through, from the registry's `car[0].camera`
    /// (`nfs_game::camera::cycle_from_registry`): the shipped list is BumperCam2, BumperCam,
    /// TailCam0, HeliCam2.
    camera_cycle: Vec<u16>,
    /// tyre dust and smoke (`RE/notes/particles` — `nfs_game::particles`)
    particles: nfs_game::particles::Particles,
    /// the `SKDP` quad and its material, uploaded with the track
    particle_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the `SHAD` blob every car casts on the ground (0x43B820)
    shadow_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// One dynamic four-vertex mesh per shadow on screen — the quad 0x43B5B0 rebuilds every frame.
    shadow_meshes: Vec<nfs_renderer::MeshId>,
    /// `CARG`, the glare slot 0 of every light kind in the table at 0x5CD73C
    glare_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// `GameData/Render/flare.dat` (0x4522B0) and a sprite for every texture its records name
    flare_defs: Vec<nfs_formats::flare::FlareDef>,
    flare_sprites: std::collections::HashMap<String, (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the track's particle systems (`nfs_game::track_particles`) and their textures out of
    /// `Render/particle.fsh`
    track_particles: Option<nfs_game::track_particles::TrackParticles>,
    track_particle_defs: Vec<nfs_formats::particle_dat::ParticleDef>,
    track_particle_sprites: std::collections::HashMap<String, (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// trackside cameras and the triggers that switch them (`RE/notes/race-logic.md` §3.3)
    track_cameras: Option<nfs_game::cameras::TrackCameras>,
    /// environment map currently bound on the car materials
    env_bound: [Option<nfs_renderer::TextureId>; 3],
    slices: Option<SliceTable>,
    race: Option<RaceProgress>,
    /// the other cars on the road — opponents, traffic, police — see [`RoadCars`]
    road: RoadCars,
    racing_line: Option<nfs_formats::ais::RacingLine>,
    /// network play — see [`Network`]
    net: Network,
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
    resets: u32,
    hud: Option<Hud>,
    /// current car's HUD dial records (km/h, tach) from `nfs5.car`
    dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>,
    /// player body paint (catalogue set 0 by default)
    paint: [u8; 3],
    /// everything that makes a sound — see [`AudioState`]
    snd: AudioState,
    /// `.sim` name — also the name of the car's `.spl` engine-audio parameters
    sim_name: String,
    /// `SKILL [0x6573F4]` 0..2: `fe.txt skill` for a quick race, the event's `Game.Difficulty` in career
    race_skill: u8,
    last_frame: Instant,
    accumulator: Duration,
    tick: u64,
    frames: u32,
    fps_timer: Instant,
}

/// The sound half of [`App`]: the output device and every voice that plays through it — the
/// player's engine, the AI and traffic engine loops, the collision and squeal sounds, the
/// ambience, the reverb, music and speech — with the settings they are mixed by. Split out of
/// `App` on 2026-09-21; every field kept its name, so `self.x` became `self.snd.x`.
struct AudioState {
    audio: Option<nfs_audio::AudioOutput>,
    engine_sound: Option<nfs_audio::engine::EngineSound>,
    sfx: Option<nfs_audio::sfx::CollisionSfx>,
    ai_engines: Option<nfs_audio::ai_engine::AiEngines>,
    ambience: Option<nfs_audio::ambience::Ambience>,
    /// tunnel reverb level (`engine-audio.md` §4.7)
    reverb: nfs_audio::reverb::Reverb,
    music: Option<nfs_audio::music::MusicPlayer>,
    /// The song names in `GameData/Music`, read once at start-up. The CD-player screen needs them
    /// in the **front end**, where there is no `MusicPlayer` yet — the original plays menu music
    /// there (every screen block carries `musicfile=menu*`) and that screen edits the playlist, so
    /// the lists must fill without a race having been started.
    music_names: Vec<String>,
    /// each song's length in milliseconds — `track + 0x20` in the original's own music table
    music_lengths: Vec<u32>,
    /// `fe.txt` audio settings (playlist, shuffle, the five level sliders)
    audio_settings: nfs_audio::music::Settings,
    speech: Option<nfs_audio::speech::Speech>,
    countdown_said: u8,
    /// the catalogue sound set of the car being driven (`nfs5.car +0x179`)
    sound_set: String,
    /// player gear on the previous tick (shift click)
    last_gear: usize,
    /// the roll-over crash sound fires once per roll-over
    crash_played: bool,
    /// the front end's one-shots, `fesfx.bnk` (0x4AADB0) — loaded on the first one played
    fe_sfx: Option<nfs_audio::fesfx::FeSfx>,
}

/// Everyone else on the road, split out of [`App`] on 2026-09-21: the opponents, the traffic and
/// the police, each with its models, bodies, previous poses and per-car sound set and paint.
/// The fields kept their names, so `self.x` became `self.road.x`.
struct RoadCars {
    /// opponents (parked until the AI rules are implemented) with their own models
    opponents: Vec<RaceCar>,
    opponent_models: Vec<CarModel>,
    /// opponent poses before the last physics tick, as `App::render_prev` is for the player
    opp_prev: Vec<(Vec3, Quat)>,
    /// kinematic traffic (`nfs_game::traffic`), rebuilt with the scene
    traffic: Option<TrafficSet>,
    traffic_models: Vec<CarModel>,
    /// engine sound set per opponent (`nfs5.car +0x179`), for the AI engine loops
    opponent_sets: Vec<String>,
    /// engine sound set per traffic car, same source
    traffic_sets: Vec<String>,
    /// paint colour per opponent, for the HUD map blips
    opponent_paints: Vec<[u8; 3]>,
    /// paint colour per traffic car, same use
    traffic_paints: Vec<[u8; 3]>,
    /// police: the spawn manager, the cop cars and their models (`traffic.md` §17, §25, §27)
    police: Option<nfs_game::police::Police>,
    police_models: Vec<CarModel>,
    police_prev: Vec<(Vec3, Quat)>,
    police_bodies: Vec<Vehicle>,
    /// whether the race was set up with police at all (`COPS`)
    cops_enabled: bool,
    /// `RACE_TYPE 1` (0x4EB4FD): a single race launched with the Fuzzyfuz code, cheat bit 5
    cops_race: bool,
    traffic_prev: Vec<(Vec3, Quat)>,
    /// one rigid body per traffic car, re-synced from the kinematic state every AI tick so the shared
    /// car-vs-car solver can be used unchanged (`traffic.md` §21.5)
    traffic_bodies: Vec<Vehicle>,
    /// `default.sim` — what `Car_loadSim` 0x49C6C0 falls back to for a car with no sim name (§21.1)
    traffic_specs: Option<CarSpecs>,
    /// wheel roll angle per traffic car (radians), integrated from the distance driven
    traffic_spin: Vec<f32>,
    /// number of traffic cars this race was set up with
    traffic_count: u32,
}

/// Replay, split out of [`App`] on 2026-09-21. The fields dropped their redundant `replay_`
/// prefix: `self.replay` is `self.replay.stream`, `self.replay_speed` is `self.replay.speed`,
/// and `replay_loop` became `loop_count`.
struct Playback {
    /// Input recording / playback (`RE/notes/replay.md`): the original records the four-byte
    /// controller packet per 32 Hz frame and re-simulates everything else.
    stream: Option<nfs_game::replay::Replay>,
    speed: nfs_game::replay::Speed,
    setup: nfs_game::replay::ReplaySetup,
    path: std::path::PathBuf,
    saved: bool,
    /// Loop-iteration counter the playback-speed decimation of `replay.md` §5.1 indexes.
    loop_count: u64,
    /// Ticks since a played-back stream ran out.
    end_ticks: u64,
    /// `[0x60686C]` — the index into `replay::CAMERA_IDS` playback cycles through (`replay.md` §8).
    camera: u8,
}

/// Network play, split out of [`App`] on 2026-09-21: the running session and the front end's
/// lobby. `self.net` became `self.net.session`; `self.lobby` became `self.net.lobby`.
struct Network {
    /// `--net host[:PORT]` / `--net join:HOST[:PORT]` — network play (`netplay`, `network.md` §7).
    session: Option<netplay::NetPlay>,
    /// The front end's side of it, alive while one of the four network layouts is open.
    lobby: Option<netplay::Lobby>,
}

/// The front end's half of [`App`]: the menu and the layout screens it shows, the 3D room and
/// the cars standing in it, the pointer, and every piece of per-screen state the original keeps
/// in its screen and widget objects — the open combo, a dragged or held scrollbar, the chronicle
/// column, the used-car and parts rows, the repair basket. Split out of `App` on 2026-09-21;
/// the fields kept their names apart from dropping a redundant `fe_` (`fe_room` is `room`),
/// so `self.x` became `self.fe.x`.
struct FrontEnd {
    menu: Option<Menu>,
    /// menu kept for returning after a race
    menu_template: Option<Menu>,
    /// `Factory.ProfileNum` — which of the seven factory-team profiles `teamprofile.lay` shows.
    /// The original registers the accessor with a default of 6 (0x4BA310); the port starts at 0 and
    /// the screen has no control of its own to move it, so this only moves if something else sets
    /// it. MEDIUM.
    factory_profile: usize,
    /// where the pointer is, in **design** coordinates (640x480), or `None` when it is outside
    /// the window. The front end is mouse-driven (`RE/notes/controls.md` §4.39); this is what
    /// feeds `LayoutScreen::set_cursor`.
    cursor: Option<(f32, f32)>,
    /// the front end's 3D room and the car standing in it (`RE/notes/frontend-3d.md`), each keyed
    /// by the model name so a screen change only reloads what actually changed
    room: Vec<(String, CarModel)>,
    /// the cars standing in the front end — the room's own (`usize::MAX`) and one per
    /// `NFS5.3DCAR` widget with a `positiontype` of its own — each with the model file, **body
    /// style** and paint it was built with, and the uploaded model. The last three are all part of
    /// the reload key — see `fe_room_models`.
    cars: Vec<(usize, String, i16, [u8; 3], CarModel)>,
    /// what `cars` was loaded for, with each car's style overlay (the widget's 0x4C0340…0x4C0486)
    cars_key: Vec<(usize, String, i16, [u8; 3], Vec<[i16; 3]>)>,
    /// milliseconds the front end has been up, for the turntable
    ms: u64,
    /// `--layout <name>`: draw a `FEData/Layouts/<name>.lay` screen instead of the menu
    layout_preview: Option<nfs_game::layout::LayoutScreen>,
    /// `bottombar.lay`, which the front end loads once and draws over every screen (0x4D39D3).
    layout_bottom: Option<nfs_game::layout::LayoutScreen>,
    /// `FEData/Locale/FEStrings.csv`, for the layout string ids
    strings: nfs_formats::locale::Strings,
    /// `Parts.AllParts`'s two collapsible sections, `[this+0x2C]` and `[this+0x2D]` — both
    /// **false** out of the constructor (0x515190, 0x515193), so the list opens as two header
    /// rows and a click on one flips it (0x51548B / 0x5154B2)
    parts_open: [bool; 2],
    layout_name: Option<String>,
    /// selected button on the previewed screen, and the screens we came from
    layout_sel: usize,
    /// the combo whose list is open, by the flattened index `combo_sliders` uses
    layout_open_combo: Option<usize>,
    /// a list whose scrollbar knob is being dragged: which list, where the grab was, and the
    /// position it was on — `[this+0x104]` and `[this+0x108]` in the original (0x4D0973)
    list_drag: Option<(usize, f32, usize)>,
    /// a held scrollbar arrow: which list, which way, and how long since the last step
    list_repeat: Option<(usize, i32, u32)>,
    layout_stack: Vec<String>,
    /// the `NFS5.GouraudIndicatorBar` the pointer took — `FeGouraudIndicatorBar`'s slot 8 asks
    /// 0x4C7DD0 for focus before it drags, and the press is what grants it (§4.81)
    bar_grab: Option<String>,
    /// `[0x5DBA44]` — which model group the chronicle is on, 0…4, switched by the five buttons of
    /// [`CHRONICLE_GROUP_BUTTONS`] (§4.111).
    chronicle_group: usize,
    /// `[FEChronicleMainBoxControl + 0x154]` — the timeline column a click has put the chronicle
    /// on, `None` while nothing has been clicked (§4.120).
    chronicle_col: Option<usize>,
    /// `[FEUsedCarsScreen + 0x290]` — whether `usedcars.lay` is on the parts panel rather than
    /// the damage one. The constructor leaves it clear (§4.113).
    usedcars_showparts: bool,
    /// Which market car `UsedCar.Car` is on — the used-car list's current row.
    usedcar_sel: usize,
    /// Which row of `Parts.AllParts` the player has clicked — the accessor's own current index in
    /// the original, which its refresh reads back through `slot27` (§4.114).
    parts_sel: Option<usize>,
    /// A held `NFS5.CARSETTINGBUTTON`: the accessor it steps, its `data.step`, and how long since
    /// the last tick. Its press 0x4C1F40 starts `0x529220(this, 0x96, 1)` — the same 150 ms
    /// repeating timer the scrollbar arrows use — keeps the handle in `[this+0x14C]` and steps
    /// once immediately (0x4C1F67); 0x4C1F80 steps again on every firing whose `[ev+8]` matches
    /// that handle, and the release 0x4C1FB0 kills it with 0x5292D0 and clears the slot. §4.108.
    setting_repeat: Option<(String, i32, u32)>,
    /// whether the left button is held, for the scrollbar arrows' pressed frame (§4.101)
    mouse_down: bool,
    /// which of `repair.lay`'s ten `QUICKFIX.*` boxes are ticked — the ten bool accessors
    /// registered at 0x4F252A…0x4F258B, all of which start **false** (§4.84, §4.95)
    repair_ticks: std::collections::BTreeSet<&'static str>,
    /// the pads as they were when a binding row took focus — the baseline 0x51F310 copies into
    /// `[[0x69C9E8]]`, which the capture 0x51F3B0 compares against every frame (§4.117)
    binding_base: Option<Vec<nfs_input::bindings::PadRecord>>,
    /// `NFS_FE_PARTS` has been applied (see `fe_car_parts_step`)
    parts_forced: bool,
    /// the widget the pointer was over, for `FEControl::onMouseEnter`'s sound (0x4C7B90)
    last_hover: Option<usize>,
    /// the pop-up that is up or fading, and what its answer is for (§4.147)
    dialog: Option<(nfs_game::dialog::Dialog, DialogFor)>,
    /// `NFS_FE_DIALOG` has been applied
    dialog_forced: bool,
    /// an answer the tick took, waiting for `fe_dialog_answer`
    dialog_answer: Option<(bool, DialogFor)>,
    /// `Replay.List`'s row — the file `KILLREPLAY` and `STARTREPLAY` act on
    replay_sel: usize,
    /// `UserList.UserList`'s selected row when a click has moved it; otherwise the row
    /// `RebuildUserList` 0x4DEF60 selects — the profile `player.lst` names
    user_sel: Option<usize>,
    /// whether the end-of-era show's `MusicPlay("vic*", 2)` (0x4FD538) has been started for this
    /// opening of the screen
    era_music: bool,
    /// the CD player's two list selections, `CDPlayer.AvailableList` / `SelectedList` rows
    cd_available: Option<usize>,
    cd_selected: Option<usize>,
    /// `CDPlayer.Title`'s index `[this+0x18]` and the current song `[0x5D7938]` (−1 at start)
    cd_title: usize,
    cd_current: i32,
    /// the race running is a playback `STARTREPLAY` started (`[0x699FC9]`)
    after_replay: bool,
    /// The test drive running: `Game.GameType` 6 dealer, 7 used, 8 owned (`[0x5DC1C0]`, armed by
    /// `TESTDRIVENEW` / `TESTDRIVEUSED` / `TESTDRIVECARSETUP`), and the car record it carries
    test_drive: Option<(u8, Option<nfs_game::career::OwnedCar>)>,
}

/// What a front-end dialog's answer is for — each is one of the raise sites §4.147 tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogFor {
    /// `QUIT` (0x50CC10) → `FEStrings` 811; "yes" is 0x4D56E0(0), out to the system (0x50CC53)
    Quit,
    /// the start check's 858 (0x4D5AE8): "yes" clears the other part-way tournaments and runs the
    /// check again (0x4D2C94…0x4D2CA3)
    TournamentReset,
    /// a one-button notice whose answer nothing polls (813, 857)
    Notice,
    /// the pre-race screen's offer, 807 (0x505E79): "yes" sells (0x502190), "no" declines
    /// (0x4F9720); the garage index of the car
    Offer(usize),
    /// `FORFEITTOURNY` → 824 (0x506130): "yes" forfeits the tournament (0x503E40) and goes back
    /// one screen (0x4D23F0(0) → 0x4D5FB0)
    Forfeit,
    /// `KILLREPLAY` → 808 (0x50D3A0): "yes" deletes the selected `.rpl` and rebuilds the list
    /// (0x50CC7E → 0x50D3C0)
    KillReplay,
    /// `REMOVEPLAYER` → 803 (0x510770 raises the dialog the screen built at 0x51015A): "yes"
    /// deletes the selected profile (0x5103F0 → 0x4DE8A0), 804 when that fails
    RemovePlayer,
}

/// The layouts that speak to the network. Their `callbacklist=` ids are `ID_COMMSSERVERSCREEN`,
/// `ID_COMMSWEBGAMESSCREEN`, `ID_PEERTOPEERIP` and the modem one — see `netplay::Lobby`.
/// Matched case-insensitively, because the screens name each other in lower case
/// (`destination=peertopeerip`) while the files are mixed case.
const NETWORK_LAYOUTS: [&str; 7] = [
    "serverlist",
    "udpip",
    "local",
    "peertopeerip",
    "modem",
    "creategame",
    "joingame",
];

/// A lap or record time in 64 Hz ticks, the way 0x4356E0 prints it: seconds are `ticks/64`, the
/// minutes `secs/60` and the seconds `secs % 60`, and the hundredths `(ticks & 63)·100/64`
/// (0x43573C…0x43579D). Minutes below ten get a leading zero (0x4357AF).
/// 0x40F654…0x40F691: the slice a reset puts a car on — its own, or the first one after it along
/// the race (0x4817D0 with `[0x657430] ? −1 : 1`, which is +1 along this port's lap) that
/// 0x40F480 calls free: the slice centre is over a drivable patch (`Tracker::updateZone` then
/// `car+0x2C & 0xF`, 0x40F4BF…0x40F4C8) and no other live racer stands on it (0x40F4D3…0x40F500).
/// The smackables' test (0x40F502…0x40F523) is not carried — this port's props keep no slice.
/// The original searches without end; the port stops after one lap (a guard, not a rule).
fn free_reset_slice(t: &SliceTable, grid: &nfs_physics::collision::TriangleGrid, start: usize, occupied: &[usize]) -> usize {
    let mut s = start;
    for _ in 0..t.lap_len.max(1) {
        if grid.find_patch(t.slices[s].centre) & 0xF != 0 && !occupied.contains(&s) {
            return s;
        }
        s = t.add(s, 1);
    }
    start
}

fn format_lap_time(ticks: u32) -> String {
    let secs = ticks / 64;
    format!("{:02}:{:02}.{:02}", secs / 60, secs % 60, (ticks & 63) * 100 / 64)
}

/// A number with thousands separators, the way 0x4DBF80 prints one: it splits off `n % 1000` and
/// `n / 1000 % 1000` (the 0x3E8 and 0xF4240 divisors at 0x4DBFA3 and 0x4DBFAD) and joins the groups
/// with commas.
fn with_thousands(n: i64) -> String {
    let neg = n < 0;
    let mut d = n.unsigned_abs().to_string();
    let mut out = String::new();
    while d.len() > 3 {
        let at = d.len() - 3;
        out.insert_str(0, &format!(",{}", &d[at..]));
        d.truncate(at);
    }
    out.insert_str(0, &d);
    if neg {
        out.insert(0, '-');
    }
    out
}

/// The row index of a `Knockout.TrackNumber(<n><FIELD>)` reference, if that is what this is.
///
/// `locationsetup.lay` names the one accessor seven times with the row written into the argument
/// (`controls.md` §4.x), and `FEKnockoutTrackNumberAccessor`'s getter compares that row against
/// `Game.NumKOOpponents` at 0x4BA7CF before it looks anything up.
fn ko_row_of(src: &str) -> Option<u32> {
    let arg = src.split_once("Knockout.TrackNumber(")?.1;
    let digits: String = arg.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn is_network_layout(name: &str) -> bool {
    NETWORK_LAYOUTS.iter().any(|n| n.eq_ignore_ascii_case(name))
}

impl App {
    /// Keep the `.sav`'s knockout and CD-player modules in step with the live state, so a career
    /// save carries them (`RE/notes/savegame.md` §4.6/§4.8).
    fn sync_save_side_state(&mut self) {
        let ko = self
            .knockout
            .as_ref()
            .map(|k| nfs_game::save::KnockoutProgress {
                active: k.active,
                race_index: k.race_idx,
                opponents: k.n,
                alive: std::iter::once(true)
                    .chain(k.alive.iter().copied())
                    .collect(),
                start_pos: k.start_pos.clone(),
                personalities: k.opponents.iter().map(|o| o.personality).collect(),
            });
        let playlist: Vec<u32> = self
            .snd
            .audio_settings
            .playlist
            .iter()
            .map(|i| *i as u32)
            .collect();
        // what the settings screens hold, for the save's accessor block
        let a = &self.snd.audio_settings;
        let settings = nfs_game::save::SavedSettings {
            music: a.music_level,
            sfx: a.sfx_level,
            engine: a.engine_level,
            speech: a.speech_level,
            ambient: a.ambient_level,
            audio_quality: a.audio_quality,
            view_distance: [0.5f32, 0.67, 0.83, 1.0]
                .iter()
                .position(|v| (v - self.detail).abs() < 0.01)
                .unwrap_or(3) as u8,
            laps: self.fe.menu.as_ref().map(|m| m.laps).unwrap_or(self.num_laps),
            opponents: self
                .fe.menu
                .as_ref()
                .map(|m| m.opponents)
                .unwrap_or(self.num_opponents),
            traffic: self.fe.menu.as_ref().map(|m| m.traffic).unwrap_or(0),
            ko_opponents: self.fe.menu.as_ref().map(|m| m.ko_opponents).unwrap_or(4),
            // the sixteen dead-zone values as the screen has them, for pad 0
            deadzone: self
                .fe.layout_preview
                .as_ref()
                .map(|l| l.deadzone_values().map(|p| [p[0] as u32, p[1] as u32]))
                .unwrap_or([[0, 0x80]; 8]),
            controls: self.saved_control_fields(),
        };
        if let Some(c) = self.career.as_mut() {
            c.knockout = ko;
            c.playlist = playlist;
            c.settings = settings;
        }
    }

    /// The `Controller.*` accessors this port has a binding behind, in the order they are saved.
    const CONTROL_ACCESSORS: [&'static str; 11] = [
        "Controller.Accel",
        "Controller.Brake",
        "Controller.SteerLeft",
        "Controller.SteerRight",
        "Controller.HandBrake",
        "Controller.ShiftUp",
        "Controller.ShiftDown",
        "Controller.ResetCar",
        "Controller.Horn",
        "Controller.CameraView",
        "Controller.LookBehind",
    ];

    /// The player's bindings as the profile's `FEControllerInputAccessor`s save them (0x51D060).
    fn saved_control_fields(&self) -> [Option<(&'static str, [u32; 6])>; 12] {
        let mut out = [None; 12];
        for (k, name) in Self::CONTROL_ACCESSORS.iter().enumerate() {
            out[k] = accessors::controller_action(name)
                .and_then(nfs_input::binding_id)
                .and_then(|id| self.input.bindings.get(id))
                .map(|b| (*name, b.to_fields()));
        }
        out
    }

    /// Load the profile's saved `Controller.*` bindings over `fe.txt`'s defaults — the accessors'
    /// Load 0x51D080 restores the six fields, which the race's option stream then carries into
    /// `[0x5E9130 + id·4]` (`RE/notes/controls.md` §4.117). A profile with none keeps the defaults.
    fn apply_saved_controls(&mut self) {
        let Some(c) = self.career.as_ref() else { return };
        let Some(s) = std::fs::read(c.profile.path(&c.save_dir)).ok().and_then(|d| nfs_formats::sav::SaveFile::parse(&d).ok()) else { return };
        for (name, fields) in nfs_game::save::saved_controls(&s) {
            if let Some(id) = accessors::controller_action(&name).and_then(nfs_input::binding_id) {
                self.input.bindings.by_id.insert(id, nfs_input::bindings::Binding::from_fields(fields));
            }
        }
    }

    /// Which accessor `CARSETTINGS.CAR` forwards to — 0x4B88F0's switch on `Game.GameType`. The
    /// port has no numeric game type of its own, so it derives the three cases 0x4B88F0
    /// distinguishes and nothing finer: a factory-driver mission is **9**, a career event pending or
    /// a career loaded at all is the Evolution **4**, and everything else is **0**.
    fn car_settings_delegate(&self) -> &'static str {
        if self.mission.is_some() {
            "FactoryDriver.Car"
        } else if self.career.is_some() {
            "OwnedCars.Car"
        } else {
            "zMergedList.Car"
        }
    }

    /// The chronicle picture for the year the career is in.
    ///
    /// `0x50757F` builds the name as `"%s%c%c%02d%d%c.fsh"`: the directory, `'M'` for a picture,
    /// the model-group letter from the table at **`0x5B8B18` = `"CBADE"`** indexed by the group at
    /// `[0x5DBA44]` — which starts at **2**, and group 2 is therefore `'A'`, the 356, the line the
    /// chronicle opens on — then the year mod 100, a 1-based index within the year and a variant
    /// `'a' + n`.
    ///
    /// **What the port does not have is the screen's own state**: which group button is down and
    /// which entry of the year the reader is on. So it takes the default group and the newest entry
    /// **at or before** the career's chronicle year, which is the one that screen would open on —
    /// an APPROXIMATION of the browsing position, not of the naming rule.
    /// The two cars the car-compare screen shows, left then right.
    ///
    /// `carcompare.lay` names the left column `CarCompare1` and the right `GenericCar2.Car`. The
    /// left one comes out of the global `[0x699FE0]`, whose only writer is the plain setter at
    /// 0x50E6A0 — and that setter has **no direct caller**: it is reached through the front end's
    /// accessor table, which is built at run time, so what fills it cannot be read statically.
    /// The screen is entered from `dealership`, `QuickRace`, `knockout` and `quickknockout`
    /// (a bare `NFS5.GOTOBUTTON` in each), every one of which has exactly one car selected, so
    /// the port uses that selection. **MEDIUM** — the naming and the right-hand column are read
    /// from the binary; which car the opening screen hands over is inferred from where the button
    /// is.
    fn compare_cars(&self) -> Option<(&nfs_formats::fedata::CarCatalogEntry, &nfs_formats::fedata::CarCatalogEntry)> {
        let m = self.fe.menu.as_ref()?;
        Some((m.cars.get(m.car_idx)?, m.cars.get(m.car2_idx)?))
    }

    /// One bar or number of the car-compare screen. The source name is `<n>CarCompareCar<m>`:
    /// `FEGouraudIndicatorBar`'s accessor (0x4CA790) takes the **leading digit** as the rating
    /// index (0x4CA838 subtracts `'0'` and rejects anything outside 0..9), then compares the rest
    /// against `CarCompareCar1` — which resolves to `[0x699FE0]`, the left car — or
    /// `CarCompareCar2`, which falls through to the `GenericCar2.Car` accessor. The five ratings
    /// are the `i16`s at +0x324 … +0x32C of the catalogue record.
    fn compare_rating(&self, source: &str) -> Option<i32> {
        let (d, rest) = source.split_at_checked(1)?;
        let index = d.parse::<usize>().ok().filter(|i| *i < 5)?;
        let (car1, car2) = self.compare_cars()?;
        // 0x50E8BA tests only for `CarCompareCar1`; **anything else** falls through to
        // `GenericCar2.Car` (0x50E8F1), so the second column is the default, not a second name
        let car = if rest == "CarCompareCar1" { car1 } else { car2 };
        Some(car.compare[index] as i32)
    }

    fn chronicle_picture(&self) -> Option<String> {
        let year = self.career.as_ref()?.career.year;
        let letter = CHRONICLE_GROUP_LETTERS.as_bytes()[self.fe.chronicle_group.min(4)] as char;
        let dir = self.paths.fe_data().join("Chron");
        let mut best: Option<(u32, String)> = None;
        let mut earliest: Option<(u32, String)> = None;
        for e in std::fs::read_dir(&dir).ok()? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            // M<letter><yy><index><variant>.fsh
            let Some(stem) = name.strip_suffix(".fsh") else {
                continue;
            };
            let b = stem.as_bytes();
            if b.len() != 6 || b[0] != b'M' || b[1] as char != letter {
                continue;
            }
            let Ok(yy) = stem[2..4].parse::<u32>() else {
                continue;
            };
            // the chronicle runs 1948..2000, so a two-digit year below 48 is the next century
            let full = if yy < 48 { 2000 + yy } else { 1900 + yy };
            if earliest
                .as_ref()
                .is_none_or(|(y, n)| full < *y || (full == *y && name < *n))
            {
                earliest = Some((full, name.clone()));
            }
            if full > year as u32 {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(y, n)| full > *y || (full == *y && name < *n))
            {
                best = Some((full, name));
            }
        }
        // a fresh career starts in 1950 and the 356's own entries begin in 1960, so before the
        // first one the screen shows the earliest there is rather than nothing
        best.or(earliest).map(|(_, n)| n)
    }

    /// The car logo a chronicle pack names, as `(pack, pair)` — the pack without its extension,
    /// the way [`LayoutScreen::set_car_logo`] wants it.
    ///
    /// `FECarLogo::update` with `othersource=Chron` (0x4C1677) loads the chronicle pack, takes
    /// **entry 1** — the `ci` icon — and reads the string at `+0x24`, which is the entry's `0x6F`
    /// attachment. It wants at least three characters (0x4C1763); the first two are the logo pair
    /// as ASCII digits (0x4C17F6 computes `2·(10·s[0] + s[1]) − 0x420`, twice the number they
    /// spell) and the rest is the logo pack's name (0x4C17F9). CONFIRMED: `MA601a.fsh` carries
    /// `04finallogos2`, and `FEData/Art/finallogos2.fsh` is on disk.
    fn chronicle_logo(&self, file: &str) -> Option<(String, i16)> {
        let dir = self.paths.fe_data().join("Chron");
        let path = GamePaths::find_file(&dir, file)?;
        let data = std::fs::read(&path).ok()?;
        let entries = nfs_assets::texture::decode_pack(&path, &data).ok()?;
        let text = entries.get(1)?.text.as_deref()?;
        if text.len() < 3 {
            return None;
        }
        let pair = text[..2].parse::<i16>().ok()?;
        Some((text[2..].to_string(), pair))
    }

    /// One field of a car the player owns, for the accessors that ask a car for a number rather
    /// than its name. `None` means this port has nothing to answer with, and the widget stays blank
    /// rather than showing something else — which is what it used to do, printing the car's name
    /// where a percentage belongs.
    fn car_field(&self, c: &CareerUi, owned: &nfs_game::career::OwnedCar, modifier: &str) -> Option<String> {
        Some(match modifier {
            // 0x4F633E and 0x4F62E1, the same shape again: `"%s"` of 0x4DBF80's commas, no sign
            "CURRENTVALUE" => with_thousands(c.career.value_of(&c.tables, owned)?),
            "PURCHASEPRICE" => with_thousands(owned.purchase_price as i64),
            // and two more of the same shape, on the used-car screens: 0x4F61D0 reads
            // **`car+0x68C`**, the asking price while the car is listed, and 0x4F6281
            // **`car+0x698`**, the mileage. Both go through 0x4DBF80 and `"%s"`. With no car at
            // all 0x4F61C6 answers the empty buffer at [0x5E8E50], which a watch draws as nothing
            // (§4.10) — but with a car and no listing the field is simply 0, so it prints `0`.
            "ASKINGPRICE" => with_thousands(owned.asking_price.unwrap_or(0) as i64),
            "MILEAGE" => with_thousands(owned.mileage as i64),
            // what the repair would cost: the aged price of each damaged part times its damage
            // doubled, over a hundred (`part_repair_cost`, 0x518880), summed over one category —
            // 0x4EFBC0 for the engine and its neighbours — or over all of them for `OVERALL`
            // (0x4EFF80), printed with 0x4DBF80's thousands commas
            m if m.ends_with("DAMAGECOST") => {
                let cat = PART_DAMAGE_CATEGORY.iter().find(|(n, _)| n.trim_end_matches("DAMAGE") == m.trim_end_matches("DAMAGECOST")).map(|(_, c)| *c);
                if cat.is_none() && m != "OVERALLDAMAGECOST" {
                    return None;
                }
                let year = c.career.year;
                let total: i64 = owned
                    .part_state
                    .iter()
                    .filter_map(|(id, state)| {
                        let part = c.tables.parts.iter().find(|p| p.id == *id)?;
                        (cat.is_none_or(|k| part.kind == k)).then(|| nfs_game::career::part_repair_cost(part, year, *state))
                    })
                    .sum();
                with_thousands(total)
            }
            // the ten damage modifiers. Slot 5's arms all read the same way — 0x4F63A3 calls the
            // category's mean, 0x4F63A8 `fsubr [0x5B242C]` turns it into `100 − mean`, and
            // 0x4F63B4 prints it with **`"%2.0f"`**: no per-cent sign, and a leading space under
            // ten. `RE/notes/controls.md` §4.134.
            m if m == "OVERALLDAMAGE" || PART_DAMAGE_CATEGORY.iter().any(|(n, _)| *n == m) => {
                format!("{:>2.0}", self.car_damage(c, owned, m)?)
            }
            _ => return None,
        })
    }

    /// The **integer** an accessor's slot 6 hands back, where this port has one. The front end's
    /// only bars that ask for one are the damage family's (`FEOwnedCarAccessor`'s slot 6 is
    /// 0x4F6B50); everything else has no slot-6 override and falls back to the text.
    fn accessor_number(&self, src: &str) -> Option<i32> {
        let (base, modifier) = match src.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')').trim()),
            None => (src, ""),
        };
        if modifier != "OVERALLDAMAGE" && !PART_DAMAGE_CATEGORY.iter().any(|(n, _)| *n == modifier) {
            return None;
        }
        let c = self.career.as_ref()?;
        let car = match base {
            "UsedCar.Car" => self.used_car()?,
            "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                c.career.garage.get(c.car_sel.min(c.career.garage.len().saturating_sub(1)))?
            }
            _ => return None,
        };
        Some(self.car_damage(c, car, modifier)? as i32)
    }

    /// `100 − damage` for one of `FEOwnedCarAccessor`'s ten damage modifiers — the condition the
    /// screens draw, despite the names. It is the number both
    /// slot 5 (the text, `"%2.0f"`) and slot 6 (the integer a bar draws, `_ftol` at 0x4F6BDD, so
    /// **truncated**) are built from.
    ///
    /// One category's damage is 0x4EF580: the sum of `part+0x164` over the installed parts
    /// whose `part+0x26` is that category, divided by the count **only when the count is above
    /// one** (0x4EF5C2 `cmp esi, 1; jle`), and 0.0 when the category holds none. The categories
    /// are 1 engine, 2 brakes, 3 transmission, 4 suspension, 5 shocks, 6 and 7 bodywork,
    /// 8 sway bars, 9 tyres, 10 cosmetic. `RE/notes/controls.md` §4.134.
    fn car_damage(&self, c: &CareerUi, owned: &nfs_game::career::OwnedCar, modifier: &str) -> Option<f32> {
        let mean = |cat: u16| {
            let mut sum = 0.0f32;
            let mut n = 0u32;
            for (id, state) in &owned.part_state {
                if c.tables.parts.iter().any(|p| p.id == *id && p.kind == cat) {
                    sum += state;
                    n += 1;
                }
            }
            if n > 1 { sum / n as f32 } else { sum }
        };
        // 0x4EF7D0 sums category **6 and category 7** and halves them (0x4EF857 `faddp`,
        // 0x4EF85B `fmul [0x5B23F0]` = 0.5) — bodywork is two categories, not one
        let bodywork = (mean(6) + mean(7)) * 0.5;
        let v = match modifier {
            "BODYWORKDAMAGE" => bodywork,
            // 0x4EF960: nine terms — bodywork and the eight single categories — scaled by
            // `[0x5B74B8]` = 1/9 at 0x4EFBB1. The adds run 0x4EFB93…0x4EFBAD in the order
            // engine, bodywork, cosmetic, tyres, sway bars, shocks, suspension, transmission,
            // brakes.
            "OVERALLDAMAGE" => {
                (mean(1) + bodywork + mean(10) + mean(9) + mean(8) + mean(5) + mean(4) + mean(3) + mean(2)) / 9.0
            }
            m => mean(PART_DAMAGE_CATEGORY.iter().find(|(n, _)| *n == m)?.1),
        };
        Some(100.0 - v)
    }

    /// The catalogue record an `NFS5.CarLogo`'s source names. The original resolves the source in
    /// the order at 0x4C14E0: `othersource` `FactoryDriver` → the `FactoryDriver.Car` accessor,
    /// `EndOfEra` → `PrizeCars.Car`, `FactoryDriverPrize` → the factory-drive prize car, then the
    /// widget's own `accessor`, then `CarCompare1` → the compare screen's left car. `FactoryDriverPrize` and
    /// `CarCompare1` are code-side objects this port does not keep, so those widgets stay
    /// unrendered; `Chron` names a car logo indirectly, through the chronicle pack — see [`App::chronicle_logo`].
    fn logo_car(
        &self,
        accessor: Option<&str>,
        othersource: Option<&str>,
    ) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let name = match othersource {
            Some("FactoryDriver") => "FactoryDriver.Car",
            Some("EndOfEra") => "PrizeCars.Car",
            // `0x51B050` reads the current factory record's `+0x10`, the prize car of the stage the
            // player has just finished, and 0x4F4360 looks it up in the master list.
            //
            // `FactoryVictory.lay`'s `NFS5.3DCAR` asks for the same thing under an accessor name —
            // `FDPrizeCars.Car`, which 0x4F2053 registers and 0x51A3E0 builds as a car accessor of
            // the factory-driver module's own. Its list method is a bare `ret` (0x514990), so what
            // fills it was not traced; pointing it at the prize car of the stage just finished is
            // an inference from the screen it is on and the name it carries. **MEDIUM.**
            Some("FactoryDriverPrize") => {
                let c = self.career.as_ref()?;
                let m = c.career.promotion_due(&c.tables).or_else(|| {
                    c.career
                        .current_mission(&c.tables)
                        .map(|i| &c.tables.missions[i])
                })?;
                return c.tables.car(m.prize_car as u16);
            }
            // `0x50E6B0` hands back whichever car list the compare screen was opened from — the five
            // setters at 0x50DA68… pass `GenericCar.Car`, `OwnedCars.Car`, `UsedCar.Car`,
            // `DealerCar.Car` and `zMergedList.Car`. The port has no "screen I came from", so the
            // garage car stands in when a career is loaded (APPROXIMATION of which list it was).
            Some("CarCompare1") => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                return c.tables.car(c.career.garage.get(idx)?.car_id);
            }
            // handled by `bind_car_logos`, which loads a whole picture out of `FEData/Chron`
            Some("Chron") => return None,
            _ => match accessor {
                Some("FDPrizeCars.Car") => return self.logo_car(None, Some("FactoryDriverPrize")),
                a => a?,
            },
        };
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        match base {
            // the car the current factory-driver mission is run in (`nfs5.fac` `+0x…` car id, the
            // same one `career_ui`'s mission line names)
            "FactoryDriver.Car" => {
                let c = self.career.as_ref()?;
                let i = c.career.current_mission(&c.tables)?;
                c.tables.car(c.tables.missions[i].car_id as u16)
            }
            "OwnedCars.Car" => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                c.tables.car(c.career.garage.get(idx)?.car_id)
            }
            // the used-car lot's own selection — the car its logo, its 3D view and its bars are on
            "UsedCar.Car" => {
                let c = self.career.as_ref()?;
                c.tables.car(self.used_car()?.car_id)
            }
            // the same list the port's other car accessors answer from
            "zMergedList.Car" | "OpponentCar.Car" | "PrizeCars.Car"
            | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                let m = self.fe.menu.as_ref()?;
                m.cars.get(m.car_idx)
            }
            // the compare screen's two columns: `CarCompare1` is the car it was entered with and
            // `GenericCar2.Car` the one its combo box cycles (`fn compare_cars`)
            "CarCompare1" | "CarCompare.Data" => self.compare_cars().map(|(a, _)| a),
            "GenericCar2.Car" => self.compare_cars().map(|(_, b)| b),
            _ => None,
        }
    }

    /// The rows an `NFS5.ListBox`'s accessor hands out, and the row it has selected. The original
    /// asks the accessor for its count (vtable +0x24), its current index (+0x28) and each row's
    /// string (+0x30), and the base accessor draws that string as the whole row
    /// (`RE/notes/controls.md` §4). Only the lists this port keeps the contents of are answered;
    /// the ones whose accessor draws columns of its own (`Track.Records`, the `Parts.*` family) and
    /// the ones the port has no state for (`CDPlayer.*`, `Comms.*`) stay empty rather than invented.
    fn list_rows(&self, accessor: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let base = accessor
            .split_once('(')
            .map(|(b, _)| b.trim())
            .unwrap_or(accessor);
        match base {
            // the four network layouts, filled from the `'d'` answers a `'w'` probe collects
            "Comms.ServerList" | "Comms.GamesList" => {
                let l = self.net.lobby.as_ref()?;
                Some((
                    l.games().into_iter().map(ListRow::Text).collect(),
                    Some(l.sel),
                ))
            }
            "Comms.IPList" => {
                let l = self.net.lobby.as_ref()?;
                Some((
                    l.addresses().into_iter().map(ListRow::Text).collect(),
                    Some(l.sel),
                ))
            }
            "Comms.LobbyPlayerList" | "Comms.PlayerList" => {
                let rows = match self.net.session.as_ref() {
                    Some(n) => n.players(),
                    None => self.net.lobby.as_ref()?.players(),
                };
                Some((rows.into_iter().map(ListRow::Text).collect(), None))
            }
            "Comms.Positional" => {
                // with no session yet the answer is the accessor's own starting index, 1
                let on = match self.net.session.as_ref() {
                    Some(n) => n.positional,
                    None => self.net.lobby.as_ref().map_or(true, |l| l.positional),
                };
                let rows = POSITIONAL_OPTIONS
                    .iter()
                    .map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_uppercase()))
                    .collect();
                Some((rows, Some(usize::from(on))))
            }
            // no modem in this port
            "Comms.ModemList" | "Comms.PhoneList" => Some((Vec::new(), None)),
            "Track1.Track" | "LoopedTrack.Track" | "Knockout.Track" | "Track.Track" => {
                let m = self.fe.menu.as_ref()?;
                Some((
                    m.tracks
                        .iter()
                        .map(|t| ListRow::Text(t.to_uppercase()))
                        .collect(),
                    Some(m.track_idx),
                ))
            }
            // `Track.Records` draws its row in four columns, each at the x of another widget of the
            // same screen (0x4FB2A0): the position number 10 px left of `ID_RECORDPLAYERPOS`, the
            // driver 4 px right of it, the car 4 px right of `ID_RECORDCARPOS` and the time 4 px
            // right of `ID_RECORDTIMEPOS`. The rows are the `nfs5.trk` slots of the selected track.
            "Track.Records" | "Record.Track" => {
                let t = self
                    .fe.menu
                    .as_ref()
                    .map(|m| m.track().to_string())
                    .unwrap_or_else(|| self.track.name.clone());
                let rows = self
                    .records
                    .track(&t)?
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        ListRow::Columns(vec![
                            ListCell {
                                anchor: "ID_RECORDPLAYERPOS".into(),
                                dx: -10.0,
                                text: format!("{}", i + 1),
                            },
                            ListCell {
                                anchor: "ID_RECORDPLAYERPOS".into(),
                                dx: 4.0,
                                text: r.driver.clone(),
                            },
                            ListCell {
                                anchor: "ID_RECORDCARPOS".into(),
                                dx: 4.0,
                                text: r.car.clone(),
                            },
                            ListCell {
                                anchor: "ID_RECORDTIMEPOS".into(),
                                dx: 4.0,
                                text: format_ticks(r.time as u64),
                            },
                        ])
                    })
                    .collect();
                Some((rows, None))
            }
            // the CD player (`cdplayer.lay`). `CDPlayer.AvailableList` is the whole song table
            // `[0x65B628]` — 0x4ADE20 builds it from the streams matching `game*`, each record's
            // `+0x24` its stream index — row `i` its title `rec+0xC` (0x4BEBB0), all of them, the
            // ones already in the playlist too. `CDPlayer.SelectedList` is the playlist
            // `[0x65B62C]`, row `i` the title of table record `playlist[i]` (0x4BEC30).
            "CDPlayer.AvailableList" | "CDPlayer.SelectedList" => {
                let names = self.cd_names();
                let table = Self::cd_table(&names);
                let list = self.cd_playlist();
                let title = |n: &String| ListRow::Text(nfs_audio::music::split_title(n).0.to_uppercase());
                let (rows, sel): (Vec<ListRow>, Option<usize>) = match base {
                    "CDPlayer.SelectedList" => (list.iter().filter_map(|i| table.get(*i)).map(|(_, n)| title(n)).collect(), self.fe.cd_selected.filter(|r| *r < list.len())),
                    _ => (table.iter().map(|(_, n)| title(n)).collect(), self.fe.cd_available.filter(|r| *r < table.len())),
                };
                Some((rows, sel))
            }
            "UserList.UserList" => {
                let profiles = nfs_game::save::list_profiles(&nfs_game::save::save_dir());
                let current = self.user_row(&profiles);
                Some((
                    profiles
                        .into_iter()
                        .map(|p| ListRow::Text(p.name))
                        .collect(),
                    current,
                ))
            }
            "Replay.List" => {
                let names = self.replay_names();
                let sel = (!names.is_empty()).then(|| self.fe.replay_sel.min(names.len() - 1));
                Some((names.into_iter().map(ListRow::Text).collect(), sel))
            }
            // the parts screens. Each row is the part's name and its price right-aligned four
            // pixels left of the widget named `ID_PRICEPOS`, and the first row of each category
            // carries the category's own heading (0x515F50 with `drawPartsHeading` 0x518DE0).
            // `Parts.AllParts` is everything that fits the selected car, `Parts.InstallList` is
            // what is on it, and `Parts.ShoppingList` is the basket (its `(TOTAL)` watch is the
            // price sum). `Parts.UsedDamageList` / `Parts.OwnedDamageRepairList` are the installed
            // parts again with their damage percentage, and `Parts.RepairShoppingList` the damaged
            // ones the repair screen charges for.
            // The five `Parts.*` lists differ only in which parts they hold and what the right-hand
            // column says (`controls.md` §…, row draws 0x515620 / 0x515F50 / 0x516960 / 0x516D70 /
            // 0x5170B0). The two damage lists hold **every installed part** — 0x516C60 counts
            // `feCar+0x758` outright — and print the part's own damage as `"%d %"` at
            // `ID_DAMAGEPOS + 3` (0x516EF6).
            "Parts.AllParts"
            | "Parts.InstallList"
            | "Parts.ShoppingList"
            | "Parts.UsedDamageList"
            | "Parts.OwnedDamageRepairList"
            | "Parts.RepairShoppingList" => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let car = c.tables.car(c.career.garage.get(idx)?.car_id)?;
                let damage_list = matches!(
                    base,
                    "Parts.UsedDamageList"
                        | "Parts.OwnedDamageRepairList"
                        | "Parts.RepairShoppingList"
                );
                // `Parts.AllParts` is not a flat list: 0x5152E0 decodes a row as **0** a section
                // header, **1** a part of section one or **2** a part of section two, and
                // 0x5153A0 counts the list as **2** plus whichever section is open. Both flags
                // start **false** (0x515190, 0x515193), so the list opens as two header rows, and
                // clicking one flips it (0x51548B / 0x5154B2, a `sete`).
                //
                // The headers' text is 0x5157A1 / 0x5157C2: `FEStrings` **2550**,
                // "PARTS AVAILABLE FOR %s", with the car, and **2551**, "PARTS INVENTORY", with
                // nothing. The second section is the player's own inventory (0x518B80), which this
                // port has no model for, so it opens empty.
                if base == "Parts.AllParts" {
                    let name = self.fe.strings.get(2550).unwrap_or_default();
                    let title = car.display_name.to_uppercase();
                    let mut rows = vec![ListRow::Header(name.replacen("%s", &title, 1))];
                    if self.fe.parts_open[0] {
                        rows.extend(Self::parts_rows(c, idx, c.career.parts_for(&c.tables, car), &self.fe.strings, false));
                    }
                    rows.push(ListRow::Header(self.fe.strings.get(2551).unwrap_or_default().to_string()));
                    if self.fe.parts_open[1] {
                        // the player's own inventory — no model here, so nothing under it
                    }
                    return Some((rows, None));
                }
                let parts: Vec<&nfs_formats::career::Part> = match base {
                    "Parts.InstallList"
                    | "Parts.UsedDamageList"
                    | "Parts.OwnedDamageRepairList" => c.career.installed_parts(&c.tables, idx),
                    // the basket the shop fills before it charges for it
                    "Parts.ShoppingList" => c.basket_parts(),
                    // the repair basket: the damaged parts of the car, which is what the repair
                    // screen charges for (`career.md` §11.3)
                    "Parts.RepairShoppingList" => c
                        .career
                        .installed_parts(&c.tables, idx)
                        .into_iter()
                        .filter(|p| c.career.part_damage(idx, p) > 0.0)
                        .collect(),
                    _ => c.career.parts_for(&c.tables, car),
                };
                Some((Self::parts_rows(c, idx, parts, &self.fe.strings, damage_list), None))
            }
            _ => None,
        }
    }

    /// Which row `Parts.AllParts`'s **second** section header sits on: 1 while the first section
    /// is closed, and `1 + <the first section's rows>` while it is open (0x5152E0's walk).
    fn parts_header_b(&self) -> usize {
        if !self.fe.parts_open[0] {
            return 1;
        }
        let Some(c) = self.career.as_ref() else { return 1 };
        let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
        let Some(car) = c.career.garage.get(idx).and_then(|g| c.tables.car(g.car_id)) else { return 1 };
        1 + Self::parts_rows(c, idx, c.career.parts_for(&c.tables, car), &self.fe.strings, false).len()
    }

    /// One `Parts.*` row per part, grouped under the category headings every one of the five
    /// lists carries (`partCategoryString` 0x518D60, `drawPartsHeading` 0x518DE0). The left cell
    /// is `"%s %s"` of the part's localised name and its type name — the same string the
    /// accessors' own `slot 12` builds (0x515D3C, 0x516B7E) — and the right one is the price, or
    /// the damage for the two damage lists.
    fn parts_rows(
        c: &career_ui::CareerUi,
        idx: usize,
        parts: Vec<&nfs_formats::career::Part>,
        fe_strings: &nfs_formats::locale::Strings,
        damage_list: bool,
    ) -> Vec<ListRow> {
        let mut rows = Vec::new();
        let mut group = None;
        for p in parts {
            if group != Some(p.kind) {
                group = Some(p.kind);
                let heading = nfs_formats::career::part_category_string(p.kind)
                    .and_then(|id| fe_strings.get(id))
                    .unwrap_or("")
                    .to_string();
                rows.push(ListRow::Header(heading));
            }
            let name = fe_strings.get(p.name_string_id).unwrap_or("").to_string();
            let right = if damage_list {
                ListCell {
                    anchor: "ID_DAMAGEPOS".into(),
                    dx: 3.0,
                    text: format!("{} %", c.career.part_damage(idx, p) as i64),
                }
            } else {
                ListCell { anchor: "ID_PRICEPOS".into(), dx: -4.0, text: format!("{}", p.price) }
            };
            rows.push(ListRow::Columns(vec![
                ListCell { anchor: String::new(), dx: 3.0, text: format!("{name} {}", p.kind_name) },
                right,
            ]));
        }
        rows
    }

    /// Fill every `NFS5.ListBox` on the previewed screen from its accessor.
    /// Put the ten `QUICKFIX.*` ticks on the screen's check boxes. `FECheckBox::refresh`
    /// 0x4C3730 asks the `boolAccessor` and picks `onIndex` or `offIndex` by the answer; this is
    /// that, with the answer coming from `repair_ticks`. §4.96.
    fn bind_checks(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let wanted: Vec<(usize, Option<bool>)> = l
            .check_boxes()
            .into_iter()
            .map(|(i, a)| {
                let on = a.strip_prefix("QUICKFIX.").map(|n| self.fe.repair_ticks.contains(n));
                (i, on)
            })
            .collect();
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (i, on) in wanted {
                l.set_check(i, on);
            }
        }
    }

    fn bind_lists(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let wanted: Vec<(usize, Option<(Vec<ListRow>, Option<usize>)>)> = l
            .list_boxes()
            .into_iter()
            .map(|w| {
                (
                    w.index,
                    w.accessor.as_deref().and_then(|a| self.list_rows(a)),
                )
            })
            // a combo slider's drop-down is drawn by the very same code (§4.43), so it takes the
            // same binding — the options of the choice accessor it edits, and which one it is on
            .chain(l.combo_sliders().into_iter().map(|w| {
                (
                    w.index,
                    // …and when it is neither a choice nor one of the families `combo_rows`
                    // enumerates, the accessor is still an accessor: 0x4C66E8's loop asks it for
                    // its count and its rows exactly as a list box's does (§4.91), so the combo
                    // falls through to the same `list_rows` a list box would get. That is what
                    // puts the `Comms.*` lists into the combo on `netgamesetup.lay`.
                    w.accessor.as_deref().and_then(|a| self.choice_rows(a).or_else(|| self.combo_rows(a)).or_else(|| self.list_rows(a))),
                )
            }))
            .collect();
        let Some(l) = self.fe.layout_preview.as_mut() else {
            return;
        };
        for (index, rows) in wanted {
            match rows {
                Some((rows, current)) => l.set_list_rows(index, rows, current),
                None => l.set_list_rows(index, Vec::new(), None),
            }
        }
    }

    /// Point every `NFS5.CarLogo` on the previewed screen at the car its source names. The pack and
    /// the entry pair are the car record's own +0x147 and +0x1F6 (`RE/notes/layouts.md` §4).
    fn bind_car_logos(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let wanted: Vec<(usize, Option<(String, i16)>)> = l
            .car_logos()
            .into_iter()
            .map(|w| {
                // `othersource=Chron` is a chronicle picture, not a car logo
                if w.othersource.as_deref() == Some("Chron") {
                    return (w.index, None);
                }
                let car = self.logo_car(w.accessor.as_deref(), w.othersource.as_deref());
                (w.index, car.map(|c| (c.logo.clone(), c.logo_pair)))
            })
            .collect();
        // the `NFS5.ChroniclePicture` widgets of `chroniclepicture.lay` and `chroniclemedia.lay`
        let chron: Vec<usize> = l.numbered_widgets("NFS5.ChroniclePicture").into_iter().map(|(i, _)| i).collect();
        // a `NFS5.CarLogo` with `othersource=Chron` is **still a car logo** — 0x4C1747 takes entry
        // 1 of the chronicle pack (its `ci` icon) and reads the string its `0x6F` attachment
        // carries, which is a two-digit logo pair followed by the logo pack's name
        // (`MA601a.fsh` says `04finallogos2`). It draws nothing when that string is shorter than
        // three characters (0x4C1763).
        let chron_logo: Vec<usize> = l.car_logos().into_iter().filter(|w| w.othersource.as_deref() == Some("Chron")).map(|w| w.index).collect();
        let picture = (!chron.is_empty() || !chron_logo.is_empty())
            .then(|| self.chronicle_picture())
            .flatten();
        let chron_pair = picture.as_deref().filter(|_| !chron_logo.is_empty()).and_then(|f| self.chronicle_logo(f));
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else {
            return;
        };
        for (index, car) in wanted {
            match car {
                Some((pack, pair)) => l.set_car_logo(r, &self.paths, index, &pack, pair),
                None => l.set_car_logo(r, &self.paths, index, "", -1),
            }
        }
        for index in chron {
            l.set_chronicle_picture(r, &self.paths, index, picture.as_deref().unwrap_or(""));
        }
        for index in chron_logo {
            match &chron_pair {
                Some((pack, pair)) => l.set_car_logo(r, &self.paths, index, pack, *pair),
                None => l.set_car_logo(r, &self.paths, index, "", -1),
            }
        }
    }

    /// `--layout-sweep`: load every shipped `.lay`, draw it once with the port's real accessors and
    /// print every widget class that produced no sprite at all, with the source it names. A widget
    /// lands here when the original places its content from code and the file carries neither a
    /// shape nor a string for it — which is exactly the list of front-end things still to port.
    fn run_layout_sweep(&mut self) {
        // The career is put into the states the blank count is otherwise waiting on — a car with
        // damage on every part, and a used-car market — so the widgets that need them are
        // measured instead of assumed. It only ever touches the in-memory career of this one
        // diagnostic run, never the save.
        //
        // This is **on by default** since 2026-09-21, and the reason is the same one that made
        // `coverage.py`'s phantom functions worth recording: a diagnostic that answers with the
        // starting state of a synthetic career is measuring the harness, not the port. With it
        // off the headline was 61, of which 42 were `UsedCar.Car` and damage widgets that are
        // blank only because nothing had been bought, damaged or listed — a parity loop reading
        // that number chases widgets that are already ported. `NFS_SWEEP_STATE=0` restores the
        // bare-career count for anyone who wants to see it.
        if std::env::var("NFS_SWEEP_STATE").as_deref() != Ok("0")
            && let Some(c) = self.career.as_mut()
        {
            let parts: Vec<(i16, f32)> = {
                let car = c.career.garage.first().map(|o| o.car_id);
                let family = car.and_then(|id| c.tables.car(id)).and_then(nfs_game::career::part_family).map(str::to_string);
                c.tables
                    .parts
                    .iter()
                    .filter(|p| Some(p.family.as_str()) == family.as_deref())
                    .map(|p| (p.id, 40.0))
                    .collect()
            };
            if let Some(o) = c.career.garage.first_mut() {
                o.damage_pct = 37.0;
                o.damage = [37.0; 10];
                o.parts = parts.iter().map(|(id, _)| *id).collect();
                o.part_state = parts;
                o.mileage = 12_345;
                o.asking_price = Some(9_500);
            }
            // …and a used-car market, which the original only has after the first career race
            // (0x4F7DB0 → 0x4F80C0). Without it the used-car screens have nothing to read.
            if c.career.market.is_empty() {
                let mut seed = 0x1234_5678u32;
                let mut rand = move || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed >> 8
                };
                let CareerUi { career, tables, .. } = c;
                career.refresh_market(tables, &mut rand);
            }
        }
        let dir = self.paths.fe_data().join("Layouts");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.strip_suffix(".lay").or_else(|| n.strip_suffix(".LAY")).map(str::to_string)
            })
            .collect();
        names.sort();
        let mut total: std::collections::BTreeMap<(String, String), usize> = Default::default();
        let mut where_of: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let mut expr_fail: std::collections::BTreeMap<(String, String, String, String), usize> = Default::default();
        let mut empty_combos: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mute_combos: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mute_bars: std::collections::BTreeMap<String, usize> = Default::default();
        // `NFS5.3DCAR`s, counted out of the blank total and reported on their own
        let mut cars3d = 0usize;
        let mut ko_past = 0usize;
        let mut screens = 0usize;
        for name in &names {
            self.open_layout(name);
            if self.fe.layout_preview.is_none() {
                continue;
            }
            screens += 1;
            // the same probe fills the browser with one session, so the four `Comms.*` lists and
            // the watches beside them can be measured too. The record is what a `'d'` answer
            // carries (`nfs_net::comm::Describe`), filled as a two-player game would fill it.
            if std::env::var("NFS_SWEEP_STATE").as_deref() != Ok("0")
                && let Some(lobby) = self.net.lobby.as_mut()
                && lobby.found.is_empty()
            {
                let mut name = [0u8; 32];
                name[..8].copy_from_slice(b"HANS'S \0");
                lobby.found.push((
                    std::net::SocketAddr::from(([127, 0, 0, 1], 1106)),
                    nfs_net::comm::Describe {
                        request: 0,
                        from: nfs_net::wire::Addr::ip([127, 0, 0, 1], 1106),
                        conn_dword: 0,
                        conn_words: [0, 0],
                        conn_raw: [0, 0],
                        name,
                        bytes: [0; 4],
                        words: [0, 0],
                        dwords: [0, 0],
                        player_limit: 2,
                        slot_mask: 1,
                    },
                ));
            }
            self.bind_car_logos();
            self.bind_lists();
            self.bind_checks();
            self.bind_indicator_bars();
            self.bind_chronicle_icons();
            self.bind_chronicle_group();
            self.bind_repair_bars();
            self.bind_usedcars_panels();
            self.bind_garage();
            self.bind_player_buttons();
            self.bind_parts_buttons();
            self.bind_mergedlist_panel();
            self.bind_showcase_pictures();
            self.bind_track_maps();
            self.bind_player_pictures();
            // `NFS_SWEEP_DUMP=<layout>` prints what every accessor of that one screen resolves to,
            // and `NFS_SWEEP_DUMP=*` every screen's. The blank count cannot see a widget that draws
            // the **wrong** thing — `UsedCar.Car` answered every modifier with the car's name for a
            // long time and counted as drawn — so this is the other half of the measurement.
            if std::env::var("NFS_SWEEP_DUMP").as_deref().is_ok_and(|v| v == "*" || v == name.as_str())
                && let Some(l) = self.fe.layout_preview.as_ref()
            {
                for e in &l.layout.elements {
                    if let Some(a) = e.get("accessor").filter(|a| !a.is_empty()) {
                        println!("  {name:<20} {:<26} {a:<30} -> {:?}", e.class, self.accessor_value(a));
                    }
                }
                // the lists too: a row that says the wrong thing is as invisible to the blank
                // count as an accessor that does
                for w in l.list_boxes() {
                    let Some(a) = w.accessor.as_deref() else { continue };
                    let rows = self.list_rows(a).map(|(r, cur)| (r.len(), cur, r.into_iter().take(3).map(|r| format!("{r:?}")).collect::<Vec<_>>()));
                    println!("  {name:<20} {:<26} {a:<30} => {rows:?}", "list");
                }
                // and the 3D cars: the sweep has no 3D pass, so they always count as blank. What
                // can be measured here is whether the port would have a model to put in each one.
                for w in l.car_3d() {
                    let model = self.logo_car(w.accessor.as_deref(), w.othersource.as_deref()).map(|c| c.model.clone());
                    println!("  {name:<20} {:<26} {:<30} ~> {model:?}", "3dcar", w.accessor.clone().unwrap_or_default());
                }
            }
            // Every part expression the port cannot evaluate. `reference` answers `None` for a
            // name it does not know and every caller falls back silently, so a template rule the
            // port misses leaves no trace in the blank count.
            if std::env::var("NFS_EXPR_SWEEP").is_ok()
                && let Some(l) = self.fe.layout_preview.as_ref()
            {
                for (class, part, key, expr) in l.expr_failures() {
                    *expr_fail.entry((class, part, key, expr)).or_insert(0usize) += 1;
                }
            }
            if std::env::var("NFS_BOX_SWEEP").is_ok()
                && let Some(l) = self.fe.layout_preview.as_ref()
            {
                for (class, b, u) in l.box_disagreements() {
                    println!("  box {name:<20} {class:<28} size.value=({:.0}x{:.0}) parts=({:.0}x{:.0})", b.0, b.1, u.0, u.1);
                }
            }
            if let Some(l) = self.fe.layout_preview.as_ref() {
                cars3d += l.car_3d().len();
            }
            // **The blank count cannot see an empty drop-down.** A combo always draws its own
            // box, so however empty its list is it never counts as a widget that drew nothing —
            // which is how 61 of the 97 went unnoticed until §4.91. Counted here as its own axis.
            if let Some(l) = self.fe.layout_preview.as_ref() {
                for w in l.combo_sliders() {
                    let a = w.accessor.clone().unwrap_or_default();
                    let n = self
                        .choice_rows(&a)
                        .or_else(|| self.combo_rows(&a))
                        .or_else(|| self.list_rows(&a))
                        .map(|(r, _)| r.len())
                        .unwrap_or(0);
                    if n == 0 {
                        *empty_combos.entry(a.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                    // and the caption is blind in the same way: a combo whose accessor answers
                    // nothing still draws its box, so the blank count never sees it either
                    if self.accessor_value(&a).is_none() {
                        *mute_combos.entry(a.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                }
                // the same blindness, one class along: a `NFS5.GouraudIndicatorBar` paints its two
                // bands whatever its accessor says, so a bar the port cannot answer sits at zero
                // and the blank count never sees it (§4.94)
                for (_, src, is_other) in l.indicator_bars() {
                    let v = if is_other {
                        self.compare_rating(&src)
                    } else {
                        let name = src.split_once('(').map_or(src.as_str(), |(b, _)| b.trim());
                        match self.range_value(name) {
                            Some((_, lo, hi)) if hi > lo => Some(0),
                            _ => self
                                .accessor_value(&src)
                                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                                .map(|v| v as i32),
                        }
                    };
                    if v.is_none() {
                        *mute_bars.entry(src.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                }
            }
            let Some(l) = self.fe.layout_preview.as_ref() else { continue };
            for (class, src) in l.blank_widgets(nfs_game::layout::DESIGN.0 / nfs_game::layout::DESIGN.1, &self.fe.strings, &|a| self.accessor_value(a)) {
                // **A knockout row past the opponent count draws nothing in the original too.**
                // `locationsetup.lay` carries seven fixed rows of watches, rows 0…6, but
                // `FEKnockoutTrackNumberAccessor`'s getter reads `Game.NumKOOpponents`
                // (0x4BA786) and, at 0x4BA7CF, `cmp ebp, [esp+0x44]; jl …` — a row at or past
                // that count falls straight through to `mov eax, 0x5E8E50`, the shared empty
                // string. With the default count of 4 that is rows 4, 5 and 6, five fields
                // each. They are not blanks the port owes anyone; counting them made the sweep
                // chase its own harness, the same way `NFS5.3DCAR` did.
                if let Some(row) = ko_row_of(&src)
                    && row >= self.fe.menu.as_ref().map(|m| m.ko_opponents).unwrap_or(4)
                {
                    ko_past += 1;
                    continue;
                }
                where_of.entry(class.clone()).or_default().push(name.clone());
                *total.entry((class, src)).or_default() += 1;
            }
        }
        if !expr_fail.is_empty() {
            // Each of these was decomposed on 2026-09-21 (`RE/AUDIT.md` §J) and every one is
            // either a fault in the shipped data — a name no template declares — or a place the
            // port follows the **code** rather than the template expression, with the addresses
            // in the note. They are separated from anything new so this axis reports a small
            // number the way the others do; a row that is not on this list is the interesting
            // one. The key is `(class, part.key)`, not the expression, so a template edit cannot
            // slip past it unnoticed.
            const EXPLAINED: [(&str, &str); 12] = [
                // the port sets alignment as an anchor in the box, not as this part offset
                ("NFS5.AccessorWatch", "label.pos"),
                ("REAL.LABEL", "label.pos"),
                // `data.area` is not among `BottomBarButton.ctr`'s declared attributes, so the
                // original cannot resolve it either — and `bg` is `0x00000000`
                ("NFS5.BOTTOMBARBUTTON", "bg.size"),
                // `ChatDisplayBox.ctr` has no `backrect` part at all
                ("NFS5.CHATDISPLAYBOX", "sblinebottom.pos"),
                ("NFS5.CHATDISPLAYBOX", "sblineleft.pos"),
                ("NFS5.CHATDISPLAYBOX", "sblineright.pos"),
                // the caption runs from 0x4C6054 / 0x4C6237, not from the template (§4.14)
                ("NFS5.ComboBoxSlider", "label.pos"),
                // the text comes from an accessor and is drawn from the code path
                ("NFS5.INPUTBOX", "untext.pos"),
                ("NFS5.CHARVECTACCESSORINPUTBOX", "untext.pos"),
                // a button whose own string is empty — AUDIT §F's five
                ("NFS5.GOTOBUTTON", "mediumlabel.pos"),
                ("NFS5.GOTOBUTTON", "smalllabel.pos"),
                // the `.lay` block carries a malformed `size=150,`
                ("NFS5.Rectangle", "data.size"),
            ];
            let mut ks: Vec<_> = expr_fail.iter().collect();
            ks.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            let mut news = 0usize;
            let mut known = 0usize;
            for ((class, part, key, expr), n) in &ks {
                let site = format!("{part}.{key}");
                if EXPLAINED.iter().any(|(c, s)| c == class && *s == site) {
                    known += **n;
                    continue;
                }
                if news == 0 {
                    println!("part expressions that do not resolve and are NOT accounted for:");
                }
                news += **n;
                println!("  {n:5}  {class:<28} {site} = {expr}");
            }
            println!("part expressions that do not resolve: {news} unaccounted, {known} decomposed in AUDIT.md §J");
        }
        // which screen each blank is on — `NFS_SWEEP_WHERE=1` prints it. A count by class says
        // what is missing, not where to look for it, and every blank that turned out to be a real
        // gap was found by opening the screen it is on.
        // **A state a template declares and this port never returns is a look the original has
        // and the port does not** — and no blank count can see it, because the widget draws its
        // other states perfectly well. `NFS_SWEEP_STATES=1` lists them. The set the port can
        // return is `state_for`'s own: the `.lay`'s `state=`, `normal`, `hover`, `focused`,
        // `focusedhover`, `selected`, `selectedhover` and `NFS5.CAMERASELECT`'s `highlighty`.
        // §4.112.
        //
        // **`hidden` and `disabled` belong in that set and were missing from it until
        // 2026-09-21**, which made this axis report 58 classes "never hidden" and 11 "never
        // disabled" — 69 phantom gaps. The port does both: a hidden widget is skipped at the
        // draw (`layout.rs`'s `hidden_flags`, set by name the way 0x50D910 does at its fifty-one
        // call sites) and `state_for`'s **first** arm returns `disabled` off `disabled_flags`.
        // A list of gaps that names things already done is the same failure as the blank count
        // reading an empty career, and it hid a real one underneath: `state_hidden_or_disabled`
        // was reading only the `.lay` attribute, so a widget hidden at run time stayed pressable.
        if std::env::var("NFS_SWEEP_STATES").is_ok() {
            const KNOWN: [&str; 9] =
                ["normal", "hover", "focused", "focusedhover", "selected", "selectedhover", "highlighty", "hidden", "disabled"];
            let mut miss: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
            for name in &names {
                self.open_layout(name);
                let Some(l) = self.fe.layout_preview.as_ref() else { continue };
                for (class, states) in l.declared_states() {
                    for st in states {
                        if !KNOWN.contains(&st.as_str()) {
                            miss.entry(class.clone()).or_default().insert(st);
                        }
                    }
                }
            }
            // and the parts no state lists — only a class's own draw can reach one (§4.130)
            let mut unlisted: std::collections::BTreeMap<String, Vec<String>> = Default::default();
            for name in &names {
                self.open_layout(name);
                if let Some(l) = self.fe.layout_preview.as_ref() {
                    unlisted.extend(l.unlisted_parts());
                }
            }
            println!("template parts no state lists:");
            for (class, parts) in &unlisted {
                println!("  {class:<32} {}", parts.join(", "));
            }
            println!("the chronicle index for group {} (§4.118): {:?}", self.fe.chronicle_group, self.chronicle_index());
            println!("template states this port never returns:");
            for (class, states) in &miss {
                println!("  {class:<32} {}", states.iter().cloned().collect::<Vec<_>>().join(", "));
            }
        }
        if std::env::var("NFS_SWEEP_WHERE").is_ok() {
            println!("blank widgets by screen:");
            let mut ks: Vec<&String> = where_of.keys().collect();
            ks.sort();
            for c in ks {
                let mut per: std::collections::BTreeMap<&str, usize> = Default::default();
                for s in &where_of[c] {
                    *per.entry(s.as_str()).or_default() += 1;
                }
                let list: Vec<String> = per.iter().map(|(s, n)| if *n > 1 { format!("{s} x{n}") } else { (*s).to_string() }).collect();
                println!("  {c:<32} {}", list.join(", "));
            }
        }
        let mut by_class: std::collections::BTreeMap<String, usize> = Default::default();
        for ((c, _), n) in &total {
            *by_class.entry(c.clone()).or_default() += n;
        }
        let mut rows: Vec<(usize, String)> = by_class.into_iter().map(|(c, n)| (n, c)).collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        println!("layouts swept: {screens} of {}", names.len());
        println!("widgets that drew nothing, by class:");
        for (n, c) in &rows {
            let srcs: Vec<String> = total.iter().filter(|((k, _), _)| k == c).map(|((_, s), n)| format!("{s} x{n}")).take(4).collect();
            println!("  {n:5}  {c:<32} {}", srcs.join(", "));
        }
        println!("total: {}", rows.iter().map(|(n, _)| n).sum::<usize>());
        // the second axis: containers the blank count is blind to (§4.92)
        let ec: usize = empty_combos.values().sum();
        println!("\ncombos with an empty drop-down: {ec}");
        let mut ecs: Vec<(usize, String)> = empty_combos.into_iter().map(|(a, n)| (n, a)).collect();
        ecs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &ecs {
            println!("  {n:4}  {a}");
        }
        let mc: usize = mute_combos.values().sum();
        println!("\ncombos whose caption answers nothing: {mc}");
        let mut mcs: Vec<(usize, String)> = mute_combos.into_iter().map(|(a, n)| (n, a)).collect();
        mcs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &mcs {
            println!("  {n:4}  {a}");
        }
        // `NFS5.3DCAR` is not in the blank count — it is never a 2D sprite in either build
        // (`FECar3DControl`'s draw 0x4C0990 calls no painter at all) — so it is reported here
        // instead of going missing.
        println!("\n3D car widgets (never a sprite, drawn by the 3D pass — 0x4C0990): {cars3d}");
        println!("knockout rows past Game.NumKOOpponents (empty in the original — 0x4BA7CF): {ko_past}");
        let mb: usize = mute_bars.values().sum();
        println!("\nindicator bars whose accessor answers nothing: {mb}");
        let mut mbs: Vec<(usize, String)> = mute_bars.into_iter().map(|(a, n)| (n, a)).collect();
        mbs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &mbs {
            println!("  {n:4}  {a}");
        }
        println!("\nby source:");
        let mut srcs: Vec<(usize, String, String)> = total.iter().map(|((c, s), n)| (*n, c.clone(), s.clone())).collect();
        srcs.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        for (n, c, s) in &srcs {
            println!("  {n:4}  {c:<30} {s}");
        }
    }

    /// Fill the chronicle's thumbnail column.
    ///
    /// `FEChroniclePictureIconControl`'s activate, **0x5087A0**, is the whole binding:
    ///
    /// ```text
    /// 0x5087BD  n = data.number                       ; [this+0x140]
    /// 0x5087CF  if (n >= [0x699D00]) nothing is drawn ; past the media count
    /// 0x5087DA  rec = 0x507480(n)                     ; [0x699C84][n], a file name
    /// 0x5087EB  [this+0x148] = 0x4D7A10(rec, 0)       ; entry 0 of that pack
    /// 0x5087F9  [this+0x144] = 0x4D7A10(rec, 1)       ; entry 1
    /// ```
    ///
    /// and the draw 0x508810 picks `[this+0x144]` when 0x4C7C90 says the widget is **hovered**
    /// (§4.109) and `[this+0x148]` otherwise, returning at 0x508832 when that pointer is null. 0x4D7A10's
    /// second argument really is an entry index — its tail, 0x4D7B09…0x4D7B16, is
    /// `pack[+0x1C][+8][k]`.
    ///
    /// The list itself is built by **0x5074A0(kind, year, variant)**, which formats
    /// `"%s%c%c%02d%d%c.fsh"` (0x50757F) and keeps every name that exists:
    ///
    /// ```text
    /// 0x507508  kind 0 -> 'p', kind 2 -> 'v', anything else -> 'm'
    /// 0x50756A  the model-group letter, "CBADE"[[0x5DBA44]]
    /// 0x50754D  the year, mod 100
    /// 0x507567  a 1-based index, and the variant 'a' + `variant`
    /// 0x507627  stop at the first name that is not on disk, or at four (0x507639)
    /// ```
    ///
    /// so a screen shows **at most four** pictures of one year and variant, and 0x507649 adds the
    /// matching `"%s%c%c%02d%d%c.mad"` for kind 2 — the movie box's file. 0x5080F8 passes kind 0
    /// for `chroniclepicture` and 0x508137 kind 1 for `chroniclemedia`.
    ///
    /// **What the port does not have is the screen's own state**: 0x5080E0 takes the year and the
    /// variant from `[this + row*4 + 0x160]` and `[this + row*4 + 0x174]` of the timeline screen,
    /// whose row table is the untraced 0x507B4F. So the port takes them from the entry
    /// [`App::chronicle_picture`] already picks — the newest at or before the career's year.
    /// APPROXIMATION of the browsing position only; the naming, the count and the two frames are
    /// read. Which of the two frames shows is **not** approximate: 0x508819's test is 0x4C7C90,
    /// `[this+0x10C] & 4`, bit 2 of the state mask — **hover** (§4.109) — so entry 1 is the
    /// hovered frame and the draw picks it.
    ///
    /// The draw also puts `[[this+0x148]+0x24]`, the entry's own name, under the icon in the
    /// class's `Univers7b.ffn` (0x5089AD), but only when that name is non-empty (0x508965) — and
    /// the `Chron` packs' entries carry none, so nothing is drawn there. Read, not invented.
    fn bind_chronicle_icons(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let icons = l.numbered_widgets("NFS5.ChroniclePictureIcon");
        if icons.is_empty() {
            return;
        }
        let files = self.chronicle_media();
        let wanted: Vec<(usize, String)> = icons
            .into_iter()
            .map(|(i, number)| (i, files.get(number.max(0) as usize).cloned().unwrap_or_default()))
            .collect();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, file) in wanted {
            // entry 0 is the resting frame; the draw takes the next one when the pointer is on
            // the icon, which is what 0x508819's 0x4C7C90 tests (§4.109)
            l.set_pack_picture(r, &self.paths, "Chron", index, &file, 1);
            l.set_pack_picture(r, &self.paths, "Chron", index, &file, 0);
        }
    }

    /// The car-picking screens show **one of two** widgets at one place: the garage button or the
    /// colour box. 0x50CC85:
    ///
    /// ```text
    /// 0x50CC8C  owned = 0x4F4DF0([this+0x280])                       ; the merged list accessor
    /// 0x50CCA5  0x50D910("ID_MERGEDLISTCOLOR",    owned,  0)         ; hide the paint box
    /// 0x50CCB9  0x50D910("ID_MERGEDLISTCARSETUP", !owned, 0)         ; hide the garage button
    /// ```
    ///
    /// and 0x4F4DF0 is three instructions of meaning:
    ///
    /// ```text
    /// 0x4F4DF9  n   = [acc+0x28]->slot9()   ; the sub-accessor's row count
    /// 0x4F4E02  cur = acc->slot10()         ; the merged list's own index
    /// 0x4F4E08  setl                        ; cur < n
    /// ```
    ///
    /// `FEMergedCarListAccessor` puts the player's **owned** cars first, and 0x4EB88F proves what
    /// the test means: when it is true 0x4EB89C takes an existing record through `[vtable+0x8C]`,
    /// and when it is false 0x4EB8B4 **allocates a fresh 0x760-byte car**. So the two widgets are
    /// "this is a car you own — take it to the garage" against "this is one you do not — here is
    /// its paint". Five screens carry the pair, at the same `pos` in each: `QuickRace`,
    /// `creategame`, `joingame`, `knockout` and `quickknockout`. §4.115.
    ///
    /// This port's merged list is the catalogue, one row per record, so it cannot compare indices
    /// the way 0x4F4DF0 does; it asks the same question of the selection instead — is a car with
    /// this record's id in the garage. Recorded as the one difference.
    fn bind_mergedlist_panel(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = ["ID_MERGEDLISTCARSETUP", "ID_MERGEDLISTCOLOR"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let owned = self
            .fe.menu
            .as_ref()
            .and_then(|m| m.cars.get(m.car_idx))
            .zip(self.career.as_ref())
            .is_some_and(|(car, c)| c.career.garage.iter().any(|g| g.car_id == car.id));
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_hidden_flag(index, if which == 0 { !owned } else { owned });
            }
        }
    }

    /// `parts.lay`'s install and uninstall buttons are **one of two**, and both go away when no
    /// part is picked. The screen's refresh 0x4F1AC0:
    ///
    /// ```text
    /// 0x4F1ADE  [this+0x288]->slot27(&picked, &installed)     ; two out-parameters
    /// 0x4F1AED  0x50D880("ID_PARTSDESC", picked != 0, 0)      ; enable the description
    /// 0x4F1AFD  picked == 0 -> 0x4F1B47
    ///           set: 0x50D880("ID_UNINSTALLBUTTON", 1)        ; enable it
    /// 0x4F1B29       0x50D910("ID_INSTALLBUTTON",   installed)  ; hide when it is fitted
    /// 0x4F1B40       0x50D910("ID_UNINSTALLBUTTON", !installed) ; hide when it is not
    /// 0x4F1B47  clear: 0x50D910("ID_INSTALLBUTTON", 1)        ; hide install
    /// 0x4F1B6C         0x50D910("ID_UNINSTALLBUTTON", 0)      ; show uninstall
    /// 0x4F1B7B         0x50D880("ID_UNINSTALLBUTTON", 0)      ; and disable it
    /// ```
    ///
    /// so the three states are: **nothing picked** — description disabled, install hidden,
    /// uninstall shown but dead; **a part that is not fitted** — install shown, uninstall hidden;
    /// **a part that is fitted** — install hidden, uninstall shown and live. §4.114.
    ///
    /// Which section of `Parts.AllParts` the picked row is in *is* whether it is fitted: 0x5152E0
    /// decodes a row as a header, as the car's own parts or as the shop's (§4.x), and this port
    /// builds the same two sections.
    fn bind_parts_buttons(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = ["ID_PARTSDESC", "ID_INSTALLBUTTON", "ID_UNINSTALLBUTTON"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let picked = self.parts_picked();
        // (hidden, enabled) for the three, in the order of `names`
        let state: [(bool, Option<bool>); 3] = match picked {
            None => [(false, Some(false)), (true, None), (false, Some(false))],
            Some(false) => [(false, Some(true)), (false, None), (true, Some(true))],
            Some(true) => [(false, Some(true)), (true, None), (false, Some(true))],
        };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                let (hide, enable) = state[which];
                l.set_hidden_flag(index, hide);
                if let Some(en) = enable {
                    l.set_disabled_flag(index, !en);
                }
            }
        }
    }

    /// The part `Parts.AllParts` is on, and whether it is **fitted** — `None` when the row is one
    /// of the two section headers or nothing has been picked (§4.114).
    fn parts_picked(&self) -> Option<bool> {
        let row = self.fe.parts_sel?;
        let b = self.parts_header_b();
        if row == 0 || row == b {
            return None;
        }
        // section 0 is the shop's list, section 1 the car's own — `combo_rows` builds them in
        // that order, which is the order 0x5152E0 walks
        Some(row > b)
    }

    /// `usedcars.lay` shows **one of two panels**, and disables its three buttons when the market
    /// is empty. `FEUsedCarsScreen`'s refresh, 0x4F7384 onwards:
    ///
    /// ```text
    /// 0x4F7394  n = the used-car list accessor's count (slot 9)
    /// 0x4F7399  n != 0 -> 0x4F73E3
    ///           n == 0: hide ID_SHOWDAMAGEBUTTON, hide ID_SHOWPARTSBUTTON,
    ///                   0x50D880 on ID_SHOWPARTSBUTTON and ID_TESTDRIVEUSEDBUTTON,
    ///                   then `jmp 0x4F74BA` — disable ID_BUYUSEDBUTTON
    /// 0x4F73E3  showparts = [this+0x290]
    ///           set: hide ID_SHOWPARTSBUTTON, hide ID_USEDCARDAMAGEPANEL,
    ///                show ID_PARTSPANEL, show ID_PARTSBOX, show ID_SHOWDAMAGEBUTTON,
    ///                then `jmp 0x4F749F` — enable ID_SHOWDAMAGEBUTTON
    /// 0x4F7448  clear: show ID_SHOWPARTSBUTTON, show ID_USEDCARDAMAGEPANEL,
    ///                hide ID_PARTSPANEL, hide ID_PARTSBOX, hide ID_SHOWDAMAGEBUTTON,
    ///                enable ID_SHOWPARTSBUTTON
    /// 0x4F74A6  and both arms end: enable ID_TESTDRIVEUSEDBUTTON, enable ID_BUYUSEDBUTTON
    /// ```
    ///
    /// `[this+0x290]` is flipped by the two callbacks the constructor registers at 0x4F71C0 and
    /// 0x4F71D1 — `SHOWPARTS` (0x4F76A0) and `SHOWDAMAGE` (0x4F76B0) — which are the `callback`
    /// lines on `ID_SHOWPARTSBUTTON` and `ID_SHOWDAMAGEBUTTON`. The constructor itself
    /// (0x4F71E2…0x4F726D) leaves the screen on the **damage** panel with `ID_Used3DCar` and
    /// `ID_GREYRECT` hidden. §4.113.
    fn bind_usedcars_panels(&mut self) {
        const SHOW: &str = "ID_SHOWPARTSBUTTON";
        const DAMAGE: &str = "ID_SHOWDAMAGEBUTTON";
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = [SHOW, DAMAGE, "ID_USEDCARDAMAGEPANEL", "ID_PARTSPANEL", "ID_PARTSBOX", "ID_Used3DCar", "ID_GREYRECT", "ID_TESTDRIVEUSEDBUTTON", "ID_BUYUSEDBUTTON"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let n = self.used_cars().len();
        let parts = self.fe.usedcars_showparts;
        // (hidden, enabled) per name, in the order of `names`
        let state: [(bool, Option<bool>); 9] = if n == 0 {
            [(true, Some(false)), (true, None), (false, None), (true, None), (true, None), (true, None), (true, None), (false, Some(false)), (false, Some(false))]
        } else if parts {
            [(true, None), (false, Some(true)), (true, None), (false, None), (false, None), (true, None), (true, None), (false, Some(true)), (false, Some(true))]
        } else {
            [(false, Some(true)), (true, None), (false, None), (true, None), (true, None), (true, None), (true, None), (false, Some(true)), (false, Some(true))]
        };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                let (hide, enable) = state[which];
                l.set_hidden_flag(index, hide);
                if let Some(en) = enable {
                    l.set_disabled_flag(index, !en);
                }
            }
        }
    }

    /// **`garage.lay`'s buttons, from the garage screen's tick 0x50F970** (§4.112): which are
    /// enabled (0x50D880) and which of the two that share a place is shown (0x50D910).
    ///
    /// ```text
    /// 0x50F98B  car = [this+0x2D4]->slot35()       ; the garage car on show, or null
    /// 0x50F997  listed = car[+0x691]               ; on the used-car market
    /// 0x50F9A7  ID_CHOOSECARBOX, ID_CARSETUP, ID_PAINTBUTTON, ID_SETTINGSBUTTON  enabled = car
    /// 0x50FA1A  career = Game.GameType == 4
    /// 0x50FA2B  ID_USEDCARSBUTTON, ID_NEWCARSBUTTON                              enabled = career
    /// 0x50FA65  ID_PARTSBUTTON, ID_REPAIRBUTTONGARAGE                            enabled = car && career
    /// 0x50FA9A  career:  ID_REMOVEFMBUTTON hidden = !(car && listed)
    ///                    ID_SELLCARBUTTON  enabled = car, hidden = car && listed
    /// 0x50FAF9  else:    ID_REMOVEFMBUTTON hidden; ID_SELLCARBUTTON disabled, shown
    /// ```
    ///
    /// "Game type 4" is the port's career (`car_settings_delegate` reads it the same way), and the
    /// car on show is the career's selected garage car.
    fn bind_garage(&mut self) {
        const NAMES: [&str; 10] = [
            "ID_CHOOSECARBOX",
            "ID_CARSETUP",
            "ID_PAINTBUTTON",
            "ID_SETTINGSBUTTON",
            "ID_USEDCARSBUTTON",
            "ID_NEWCARSBUTTON",
            "ID_PARTSBUTTON",
            "ID_REPAIRBUTTONGARAGE",
            "ID_REMOVEFMBUTTON",
            "ID_SELLCARBUTTON",
        ];
        if self.fe.layout_name.as_deref() != Some("garage") {
            return;
        }
        let career = self.career.is_some() && self.mission.is_none();
        let owned = self.career.as_ref().and_then(|c| c.career.garage.get(c.car_sel));
        let car = owned.is_some();
        let listed = owned.is_some_and(|c| c.asking_price.is_some());
        // (hidden, enabled) per name, in the order of `NAMES`
        let state: [(Option<bool>, bool); 10] = [
            (None, car),
            (None, car),
            (None, car),
            (None, car),
            (None, career),
            (None, career),
            (None, car && career),
            (None, car && career),
            (Some(!(career && car && listed)), true),
            (Some(career && car && listed), career && car),
        ];
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        for (index, which) in l.widgets_named(&NAMES) {
            let (hide, enable) = state[which];
            if let Some(h) = hide {
                l.set_hidden_flag(index, h);
            }
            l.set_disabled_flag(index, !enable);
        }
    }

    /// `repair.lay`'s ten damage bars are **hidden** when their category has nothing in it.
    ///
    /// 0x50F4AD…0x50F609 is ten copies of one block:
    ///
    /// ```text
    /// 0x50F4BC  ok = [this + 0x284 + 4k]->slot27(&[this + 0x2AC + 4k])   ; the category accessor
    /// 0x50F4C1  sete al                                                  ; hide = !ok
    /// 0x50F4CC  0x50D910(screen, "ID_BAR<K>", hide)
    /// ```
    ///
    /// in the order ENGINE, BRAKES, TRANSMISSION, SUSPENSION, SHOCKS, BODYWORK, SWAYBARS, TIRES,
    /// COSMETIC, OVERALL — the same order as [`QUICKFIX_NAMES`]. So a category the car has no
    /// part of does not draw an empty bar in the original: **the bar is not there at all**. This
    /// port had been drawing a full-length one for `BODYWORK` on a 356, which has no part of
    /// category 6 — the one row `RE/AUDIT.md` §10 recorded as "blank with state". §4.112.
    ///
    /// `OVERALL` is the tenth and is never hidden here: 0x4EF960 combines the other categories
    /// itself and always answers.
    fn bind_repair_bars(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names: Vec<String> = QUICKFIX_NAMES.iter().map(|k| format!("ID_BAR{k}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let want = l.widgets_named(&refs);
        if want.is_empty() {
            return;
        }
        let has: Vec<bool> = QUICKFIX_NAMES
            .iter()
            .map(|k| *k == "OVERALL" || self.category_has_parts(*k))
            .collect();
        tracing::debug!(hidden = ?QUICKFIX_NAMES.iter().zip(&has).filter(|(_, h)| !**h).map(|(k, _)| *k).collect::<Vec<_>>(), "repair bars");
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_hidden_flag(index, !has[which]);
            }
        }
    }

    /// Whether the garage car has any installed part of the category `<K>DAMAGE` names — what
    /// decides whether 0x50F4BC's accessor answers at all (§4.112).
    fn category_has_parts(&self, key: &str) -> bool {
        let Some(c) = self.career.as_ref() else { return true };
        let name = format!("{key}DAMAGE");
        let Some(cat) = PART_DAMAGE_CATEGORY.iter().find(|(n, _)| *n == name).map(|(_, c)| *c) else { return true };
        let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
        let Some(owned) = c.career.garage.get(idx) else { return true };
        owned.part_state.iter().any(|(id, _)| c.tables.parts.iter().any(|p| p.id == *id && p.kind == cat))
    }

    /// `chronicle.lay`'s five model-group buttons carry the **`selected`** flag, one at a time.
    /// 0x5077D0 clears it on all five through the by-name helper 0x50D9A0 and then, on
    /// `[0x5DBA44]` through the jump table at 0x507894, sets it on exactly one (§4.111).
    fn bind_chronicle_group(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let want = l.widgets_named(&CHRONICLE_GROUP_BUTTONS);
        if want.is_empty() {
            return;
        }
        // and the timeline's own five columns, which the same screen carries (§4.122)
        let cols: Vec<(u32, usize)> = match self.chronicle_column() {
            Some(c) => self.chronicle_columns().into_iter().skip_while(|x| *x != c).take(5).collect(),
            None => Vec::new(),
        };
        // the icons of the ten cells: `0x507340(year, row, variant, hot)` for rows 0 and 1
        // (0x507DDB loops `ebx < 2`), each answering the resting frame and the hovered one
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let pack = format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)]);
        let entries = self.chronicle_entries();
        let mut cells: std::collections::BTreeMap<(usize, usize), (usize, usize)> = Default::default();
        for (col, (year, var)) in cols.iter().enumerate() {
            for row in 0..2 {
                if let Some((_, _, _, f)) = entries.iter().find(|(y, k, v, _)| y == year && *k == row && v == var) {
                    cells.insert((col, row), (f[0], if f[1] == usize::MAX { f[0] } else { f[1] }));
                }
            }
        }
        let frames: Vec<usize> = cells.values().flat_map(|(a, b)| [*a, *b]).collect();
        if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
            for n in frames {
                l.load_pack_entry(r, &self.paths, "Chron", &pack, n);
            }
        }
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_chronicle_icons(pack, cells);
            l.set_chronicle_columns(cols);
        }
        let group = self.fe.chronicle_group;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_selected_flag(index, which == group);
            }
        }
    }

    /// **The chronicle's entry index**, as 0x506FE0 builds it.
    ///
    /// The index is a flat table of 5 × 53 × 3 × 5 × 2 dwords at **0x692038** — group, year − 48,
    /// kind, variant, and a flag — and 0x507340 reads it with
    /// `[0x692038 + (2*(5*(3*(53*group + year − 48) + kind) + variant) + (flag != 0)) * 4]`.
    /// 0x507065's `rep stosd` of **0x1F0E** dwords is 7950, which is that product exactly.
    ///
    /// It is built from the **five icon packs**, not from the picture files: 0x50701E formats
    /// `"%s%s.fsh"` over the names at 0x5B8AB4, stepping 0x14 up to 0x5B8B18 — `iconsrace`,
    /// `icons911`, `icons356`, `icons924`, `icons914`, which is group order and agrees with both
    /// `"CBADE"` (§4.107) and the five buttons (§4.111). For each entry 0x537F60 hands back the
    /// four-character directory tag and 0x5070CF…0x5070FB parses it:
    ///
    /// ```text
    /// 0x5070FB  year = 10*tag[0] + tag[1] - 0x210     ; two ASCII digits; "00" becomes 100
    /// 0x5070CA  kind = table[tag[2] - 0x4D]           ; 0x507234 -> the jump table 0x507224
    ///                'p' -> 0, 'm' -> 1, 'v' -> 2, anything else -> 0
    /// 0x507132  the slot is found by **linear probing along the flat table** from
    ///           (group, year, kind, 0, 0): 0x50714C walks `eax += 4` while the dword is
    ///           taken, counting in `ebx`, and gives up at ten — which is the product of the
    ///           two fastest dimensions, 5 variants x 2 flags. So the k-th entry with the same
    ///           (group, year, kind) lands at `variant = k / 2`, `flag = k % 2`, and the pairs
    ///           are exactly §4.107's two frames, the resting one and the hovered one.
    /// ```
    ///
    /// so `icons356.fsh`'s `"48p "` is the 1948 picture of the 356 line. §4.118.
    fn chronicle_index(&self) -> Vec<(u32, usize, usize)> {
        self.chronicle_entries().into_iter().map(|(y, k, v, _)| (y, k, v)).collect()
    }

    /// The same walk, keeping the **pack entry number** each slot holds — which is what
    /// 0x507340 returns and what the timeline's icons are (§4.123). The two slots of a
    /// (year, kind, variant) are the resting frame and the hovered one (§4.121).
    fn chronicle_entries(&self) -> Vec<(u32, usize, usize, [usize; 2])> {
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let dir = self.paths.fe_data().join("Chron");
        let Some(path) = GamePaths::find_file(&dir, &format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)])) else {
            return Vec::new();
        };
        let Some(pack) = std::fs::read(&path).ok().and_then(|d| nfs_formats::fsh::Fsh::parse(&d).ok()) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, usize, usize, [usize; 2])> = Vec::new();
        for (n, e) in pack.entries.iter().enumerate() {
            let t = e.name.as_bytes();
            if t.len() < 3 || !t[0].is_ascii_digit() || !t[1].is_ascii_digit() {
                continue;
            }
            let year = match 10 * u32::from(t[0]) + u32::from(t[1]) - 0x210 {
                0 => 100,
                y => y,
            };
            let kind = match t[2] {
                b'p' | b'P' => 0,
                b'm' | b'M' => 1,
                b'v' => 2,
                _ => 0,
            };
            // 0x50714C's probe: the k-th entry of a (year, kind) lands at variant k/2 and
            // frame k%2, so a group with a free second frame takes this one, and otherwise a
            // fresh variant opens
            match out.iter_mut().find(|(y, kk, _, f)| *y == year && *kk == kind && f[1] == usize::MAX) {
                Some(slot) => slot.3[1] = n,
                None => {
                    let v = out.iter().filter(|(y, kk, _, _)| *y == year && *kk == kind).count();
                    out.push((year, kind, v, [n, usize::MAX]));
                }
            }
        }
        out.sort();
        out
    }

    /// `Chronicle.Year` on the chronicle screen. The accessor is a range one of its own (0x508A24:
    /// 1948…1996, not the career's calendar year `[0x5DB764]`), and both the screen's constructor
    /// (0x506E30…0x506E98) and every group button (0x507690…) set it the same way: its range to
    /// `1948 … cursor[g]` through 0x4BA4F0 with 0x79C, then its value to the group's **earliest**
    /// year through slot 11, 0x4BC240, which clamps the index to `0 … count − 1`.
    ///
    /// Both years come out of 0x506FE0's walk over the group's icon pack: the earliest is the
    /// smallest `1900 + yy` of any tag, `yy = 0` being 2000 (0x507157…0x507180); the cursor starts
    /// at 1996 (0x50708B) and gains one for every `p` tag of a year at or past it that is the third,
    /// fifth… of its (year, kind) — an even count of at least two slots already filled, under ten
    /// (0x507186…0x5071B0).
    fn chronicle_year(&self) -> i32 {
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let tags: Vec<String> = GamePaths::find_file(&self.paths.fe_data().join("Chron"), &format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)]))
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|d| nfs_formats::fsh::Fsh::parse(&d).ok())
            .map(|pack| pack.entries.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();
        let (mut cursor, mut earliest) = (1996i32, None::<i32>);
        let mut filled: std::collections::HashMap<(i32, usize), u32> = std::collections::HashMap::new();
        for t in tags.iter().map(|t| t.as_bytes()) {
            if t.len() < 3 {
                continue;
            }
            let yy = match 10 * i32::from(t[0] as i8) + i32::from(t[1] as i8) - 0x210 {
                0 => 100,
                y => y,
            };
            let kind = match t[2] {
                b'p' | b'P' => 0,
                b'm' | b'M' => 1,
                b'v' => 2,
                _ => 0,
            };
            let year = 1900 + yy;
            earliest = Some(earliest.map_or(year, |e: i32| e.min(year)));
            let n = filled.entry((yy, kind)).or_insert(0);
            if *n < 10 {
                if year >= cursor && *n >= 2 && *n % 2 == 0 && kind == 0 {
                    cursor += 1;
                }
                *n += 1;
            }
        }
        let count = cursor - 1948 + 1;
        1948 + (earliest.unwrap_or(1948) - 1948).clamp(0, (count - 1).max(0))
    }

    /// 0x5074A0's list for the screen the port is on: `<kind><group><yy><i><variant>.fsh` for
    /// `i` = 1…4, stopping at the first one that is not on disk.
    /// The timeline's columns: the index's distinct **(year, variant)** pairs, which is what
    /// 0x507B4F's walk settles on — it asks 0x507340 for all three kinds and moves to the next
    /// year only when the variant it is on has none of them (0x507B93). §4.120.
    fn chronicle_columns(&self) -> Vec<(u32, usize)> {
        let mut v: Vec<(u32, usize)> = self.chronicle_index().into_iter().map(|(y, _, var)| (y, var)).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The (year, variant) the chronicle is on: the column a click has picked, else the one the
    /// timeline opens at — **0x507999** asks the `Chronicle.Year` accessor the bind 0x507E65 put
    /// in `[this+0x148]`, takes 0x76C (1900) off it and keeps the remainder mod 100 (0x5079AF),
    /// and 0x507B4F's walk starts there at variant 0 and only ever moves forward. §4.121.
    fn chronicle_column(&self) -> Option<(u32, usize)> {
        let cols = self.chronicle_columns();
        if let Some(c) = self.fe.chronicle_col.and_then(|c| cols.get(c).copied()) {
            return Some(c);
        }
        let start = ((self.chronicle_year() - 1900) % 100) as u32;
        cols.iter().copied().find(|(y, _)| *y >= start).or_else(|| cols.first().copied())
    }

    fn chronicle_media(&self) -> Vec<String> {
        // 0x507508's three letters, by the kind each screen passes (0x5080F8, 0x508137) — and
        // the same three the index's tags carry (§4.118)
        self.chronicle_media_for(match self.fe.layout_name.as_deref().unwrap_or("").to_ascii_lowercase().as_str() {
            "chroniclemovie" => 2usize,
            "chroniclemedia" => 1,
            _ => 0,
        })
    }

    /// 0x5074A0 for one kind — what a timeline cell's row asks for before it opens a screen
    /// (0x5080F8, 0x508137).
    fn chronicle_media_for(&self, k: usize) -> Vec<String> {
        let kind = [b'p', b'm', b'v'][k.min(2)] as char;
        let letter = CHRONICLE_GROUP_LETTERS.as_bytes()[self.fe.chronicle_group.min(4)] as char;
        // **the year and the variant come from the index**, which is what 0x5080E0 reads them
        // from — its row table `[this + row*4 + 0x160]` and `[+0x174]` is filled by the timeline
        // draw out of the very entries 0x506FE0 put in the table (§4.118). This port has no
        // timeline row, so it takes the newest entry of this kind at or before the career's year,
        // which is the browsing approximation §4.107 already records.
        let want = self.career.as_ref().map(|c| c.career.year as u32).unwrap_or(0);
        let want = if want >= 1900 { want - 1900 } else { want };
        // **Which** column the screen is on is 0x5080E0's `[this+0x154]`, a timeline row this port
        // has not got — so the year and the variant still come from the entry
        // [`App::chronicle_picture`] picks, the browsing approximation §4.107 records. The index
        // §4.118 reads is the source the original uses for the same two numbers, and the port's
        // own reading of it is checkable with `NFS_SWEEP_STATES=1`; rewiring the browsing
        // position onto it wants the timeline row, which is still unported.
        let _ = want;
        // the column the timeline is on — 0x5080E0 reads the year and the variant out of
        // `[this+0x160 + 4*col]` and `[this+0x174 + 4*col]`, and a click on a cell moves the
        // column (§4.120)
        let Some((year, var)) = self.chronicle_column() else { return Vec::new() };
        let yy = format!("{:02}", year % 100);
        let variant = (b'a' + var.min(4) as u8) as char;
        let dir = self.paths.fe_data().join("Chron");
        let mut out = Vec::new();
        for i in 1..=4 {
            let name = format!("{kind}{letter}{yy}{i}{variant}.fsh");
            if GamePaths::find_file(&dir, &name).is_none() {
                break;
            }
            out.push(name);
        }
        out
    }

    /// Fill the showcase screens' picture mosaic. `NFS5.ShowcasePicture` is only a place — its
    /// template has no drawable part — and the `number` its `.lay` block carries is the entry of
    /// the selected car's own pack, `FEData/Showcase/<key>.fsh`, where `key` is the catalogue
    /// record's +0xE3 string (86 of the 87 racing records name a pack that is on disk). The match
    /// is exact: `general.lay` and `glancestats.lay` give `number` 0…7, and each widget's `size`
    /// is entry `number`'s own width and height. CONFIRMED.
    fn bind_showcase_pictures(&mut self) {
        // `NFS5.CAMERAPICTURE`'s `shp` part is in no state list, so nothing preloads it (§4.129)
        let cams: Vec<i64> = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.numbered_widgets("NFS5.CAMERAPICTURE").into_iter().filter_map(|(i, _)| l.layout.elements.get(i).and_then(|e| e.int("picturenum"))).collect())
            .unwrap_or_default();
        if !cams.is_empty()
            && let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut())
        {
            for n in cams {
                l.load_pack_entry(r, &self.paths, "Art", nfs_game::layout::CAMERA_PICTURES, n.max(0) as usize);
            }
        }
        // `NFS5.ANIMSHOWCASEPICTURE` needs all eight entries of `show.fsh` — four pictures and
        // four labels — which its own bind 0x4BDB90 loads up front (§4.127)
        if self.fe.layout_preview.as_ref().is_some_and(|l| !l.numbered_widgets("NFS5.ANIMSHOWCASEPICTURE").is_empty())
            && let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut())
        {
            for n in 0..8 {
                l.load_pack_entry(r, &self.paths, "Art", nfs_game::layout::SHOWCASE_ANIM, n);
            }
        }
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let want = l.numbered_widgets("NFS5.ShowcasePicture");
        if want.is_empty() {
            return;
        }
        let pack = self
            .logo_car(Some("zMergedList.Car"), None)
            .map(|c| format!("{}.fsh", c.showcase))
            .unwrap_or_default();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, number) in want {
            l.set_pack_picture(r, &self.paths, "Showcase", index, &pack, number.max(0) as usize);
        }
    }

    /// Fill the three map widgets.
    ///
    /// `NFS5.TrackSmall2dSpline` (`tournament.lay`) shows the tournament's events one after the
    /// other; the art is each track's own `nfs5.trk` `+0x58` name in `FEData/TrackArt`, and the
    /// marker is entry 1 or entry 2 by the event's `backwards` byte — see [`TrackMapEvent`].
    ///
    /// `NFS5.PRIZERACEMAP` (`VictoryMessage.lay`) and `NFS5.FACTORYMAP` (`missionmap.lay`) are one
    /// picture each. The prize map's pack is the era's (0x506430: `mclass`, `mgold`, `mmod` for the
    /// `+4` values 0x209, 0x20A, 0x20B; 0x504800 picks the first era that still has an unfinished
    /// tournament, which is what the port's own `career.era` tracks). The factory map's is the
    /// current mission's code at `fac+0x3F` — `1m01.fsh` and the rest, in `FEData/Factory` — from
    /// the first mission whose result is still 0 (0x51AE80).
    /// `ID_ENDOFERASCREEN` (`endofera.lay`): build `FEEndOfEraCallBackList`'s show from the career
    /// the way its constructor 0x4FD4F0 reads it (`RE/notes/controls.md` §4.150a). The tables are
    /// the original's globals, so without a career the screen still has them and a fresh career's
    /// state.
    fn bind_end_of_era(&mut self) {
        use nfs_formats::career::{Era, TournamentKind};
        use nfs_game::end_of_era::{EndOfEra, EraInput, TITLE_TEXT};
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let is_show = l.layout.elements.first().and_then(|e| e.get("callbacklist")).is_some_and(|c| c.eq_ignore_ascii_case("ID_ENDOFERASCREEN"));
        if !is_show {
            self.fe.era_music = false;
            return;
        }
        // The show's constructor 0x4FD4F0 starts the victory music before anything else:
        // 0x4FD531 / 0x4FD538 push **2** and `"vic*"` into `MusicPlay` 0x4AE4D0, which is mode 2 —
        // the playlist becomes that one pick (`engine-audio.md` §8.3). Once per opening.
        if !self.fe.era_music {
            self.fe.era_music = true;
            self.fe_music(|m, a| m.play_pattern(a, "vic*", 2));
        }
        let loaded;
        let fresh;
        let (tables, state) = match self.career.as_ref() {
            Some(c) => (&c.tables, &c.career),
            None => {
                let Ok(t) = nfs_game::career::CareerTables::load(&self.paths) else { return };
                loaded = t;
                fresh = nfs_game::career::Career::default();
                (&loaded, &fresh)
            }
        };
        let idx = |e: Era| match e {
            Era::Classic => 0usize,
            Era::Golden => 1,
            Era::Modern => 2,
        };
        // 0x503ED0: every era record not hidden (`+0xE4 != 2`) completed
        let done = |e: Era| tables.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(e) && !t.is_hidden()).all(|t| state.state(t.id).completed);
        let current = state.era;
        // 0x5032E0: Classic and Golden answer Classic, Modern answers Golden
        let previous = if current == Era::Modern { Era::Golden } else { Era::Classic };
        // 0x504800: the highest era found done; with none, a stack word the movie table maps to 0
        // (MEDIUM — `+0x30C` stays 0 unless that word happens to be an era id)
        let completed = [Era::Modern, Era::Golden, Era::Classic].into_iter().find(|e| done(*e)).map(idx).unwrap_or(0);
        // 0x4E06D0 walks the loaded tables in order and the front end's own is first, so an id the
        // file also gives the HUD reads the front end's row
        let text = |id: u32| self.fe.strings.get(id).unwrap_or_default().to_string();
        let tournaments = [Era::Classic, Era::Golden, Era::Modern].map(|e| {
            tables.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(e) && t.visibility == 0).map(|t| text(t.title_string_id)).collect::<Vec<_>>()
        });
        let input = EraInput {
            previous: idx(previous),
            current: idx(current),
            completed,
            current_done: done(current),
            era_names: [text(0x209), text(0x20A), text(0x20B)],
            cars: tables.catalog.iter().map(|c| (c.year as i32, c.display_name.clone())).collect(),
            tournaments,
            title: text(TITLE_TEXT),
            centre: (320, 240),
        };
        // APPROXIMATION: the front end's generator is seeded from the clock (0x4E3182); a fixed
        // seed keeps a still reproducible, as the movie circle's overlay does
        let show = EndOfEra::build(&input, &mut nfs_game::opponents::FeRng::new(0));
        let (Some(r), Some(l)) = (self.renderer.as_mut(), self.fe.layout_preview.as_mut()) else { return };
        l.set_end_of_era(r, &self.paths, show);
    }

    fn bind_track_maps(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let splines = l.numbered_widgets("NFS5.TrackSmall2dSpline");
        let prize = l.numbered_widgets("NFS5.PRIZERACEMAP");
        let factory = l.numbered_widgets("NFS5.FACTORYMAP");
        if splines.is_empty() && prize.is_empty() && factory.is_empty() {
            return;
        }
        let events: Vec<TrackMapEvent> = self
            .career
            .as_ref()
            .and_then(|c| {
                let t = c.current()?;
                Some(
                    t.events
                        .iter()
                        .filter_map(|ev| {
                            let trk = c.tables.track(ev.track_id)?;
                            let name = self
                                .fe.strings
                                .get(trk.name_string_id + 20)
                                .map(str::to_string)
                                .unwrap_or_else(|| c.tables.track_name(ev.track_id).to_uppercase());
                            Some(TrackMapEvent { art: trk.art_small.clone(), backwards: ev.backwards, mirror: ev.direction_flag != 0, name })
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        let era_pack = self
            .career
            .as_ref()
            .map(|c| match c.career.era {
                nfs_formats::career::Era::Classic => "mclass.fsh",
                nfs_formats::career::Era::Golden => "mgold.fsh",
                nfs_formats::career::Era::Modern => "mmod.fsh",
            })
            .unwrap_or("mclass.fsh")
            .to_string();
        let mission_pack = self
            .career
            .as_ref()
            .and_then(|c| Some(format!("{}.fsh", c.tables.missions[c.career.current_mission(&c.tables)?].code)))
            .unwrap_or_default();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, _) in splines {
            l.set_track_map(r, &self.paths, index, &events);
        }
        for (index, _) in prize {
            l.set_pack_picture(r, &self.paths, "TrackArt", index, &era_pack, 0);
        }
        for (index, _) in factory {
            l.set_pack_picture(r, &self.paths, "Factory", index, &mission_pack, 0);
        }
    }

    /// Fill the player's portrait and the Factory Driver licence card.
    ///
    /// **The portrait index is the leading digit of the profile's file name.**
    /// `FEPlayerPicture::update` (0x4CFB70) asks its `UserList.UserList` accessor (0x4CFAE6) for
    /// the selected option's text and takes `text[0] - '0'`, clamped to 0..8 (0x4CFBA0…0x4CFBCC) —
    /// and that text is `sprintf("%d%s", picture, name)`, the same `.sav` stem 0x4DEDA0 builds. The
    /// picture is entry `n` of `FEData/Art/ALLdrivr.fsh` (0x4DBCEC), whose nine 54×64 entries are
    /// `M01`…`M04`, `F01`…`F04` and `driv`. This closes the UNKNOWN `RE/notes/layouts.md` left on
    /// where the chosen index lives.
    ///
    /// The licence card is `license`, `license2`, `license3` or `license4` (the table at 0x5D8E50)
    /// by **the stage of the mission the career is on, minus one** — 0x51AE00 finds the first
    /// mission whose `+0xE8` result is still 0, answers −1 when that is mission 0 (nothing passed
    /// yet, so no licence), reads `stage` at `+8` and subtracts 1, and answers 3 when every mission
    /// is done. The name under it is printed in the colour at 0x5D8E60 for that stage — white,
    /// 0xD20000, black, black — and the rank line is `FEStrings[3496 + stage]`, or `3485 + stage`
    /// when the portrait is one of the four female ones (0x4CF56E: `-11` when 0x5024E0 answers
    /// 4…7). 3496 is `IDS_JUNIORTESTDRIVER` and 3485 `IDS_FEMALEJUNIORTESTDRIVER`. CONFIRMED.
    fn bind_player_pictures(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let pictures: Vec<usize> = ["NFS5.PLAYERPICTURE", "NFS5.MINIPLAYERPICTURE"].iter().flat_map(|c| l.numbered_widgets(c)).map(|(i, _)| i).collect();
        let badges: Vec<usize> = ["NFS5.BADGE", "NFS5.MINIBADGE"].iter().flat_map(|c| l.numbered_widgets(c)).map(|(i, _)| i).collect();
        if pictures.is_empty()
            && badges.is_empty()
            && ["NFS5.FACTORYPIC", "NFS5.MOVIEBOX", "NFS5.MOVIECIRCLE", "NFS5.HUD"].iter().all(|c| l.numbered_widgets(c).is_empty())
        {
            return;
        }
        let profile = self.career.as_ref().map(|c| c.profile.clone());
        let picture = profile.as_ref().map_or(0, |p| p.picture.min(8) as usize);
        let name = profile.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        // 0x51AE00
        let stage = self.career.as_ref().and_then(|c| {
            let i = c.career.current_mission(&c.tables)?;
            if i == 0 {
                return None;
            }
            Some((c.tables.missions[i].stage.max(1) - 1).clamp(0, 3) as usize)
        });
        let rank = stage
            .and_then(|st| {
                let base = if (4..=7).contains(&picture) { 3485 } else { 3496 };
                self.fe.strings.get(base + st as u32).map(str::to_string)
            })
            .unwrap_or_default();
        let colour = match stage.unwrap_or(0) {
            0 => [1.0, 1.0, 1.0, 1.0],
            1 => [0xD2 as f32 / 255.0, 0.0, 0.0, 1.0],
            _ => [0.0, 0.0, 0.0, 1.0],
        };
        let pack = stage.map(|st| if st == 0 { "license.fsh".to_string() } else { format!("license{}.fsh", st + 1) });
        // `NFS5.FACTORYPIC` (0x51A080): one of the six 54×64 faces of `FactoryPeople.fsh` —
        // `bill`, `step`, `diet`, `fran`, `klau`, `rolp`, in the narrator order the missions use.
        // With `source=profile` (`teamprofile.lay`) the entry is `Factory.ProfileNum` through the
        // jump table at 0x51A1A8, which permutes 0…5 to **2, 0, 3, 1, 4, 5** — profile 0 is Dieter,
        // which is what `FEStrings[3501]` says too. With anything else (`missionbrief.lay` says
        // `source=nothingmuch`) it is the current mission's `+0x1E`, the narrator (0x51A184).
        let faces: Vec<(usize, usize)> = l
            .numbered_widgets("NFS5.FACTORYPIC")
            .into_iter()
            .map(|(i, _)| {
                const PROFILE_FACE: [usize; 6] = [2, 0, 3, 1, 4, 5];
                let by_profile = l.layout.elements[i].get("source").is_some_and(|s| s.eq_ignore_ascii_case("profile"));
                let entry = if by_profile {
                    PROFILE_FACE[(self.fe.factory_profile as usize).min(5)]
                } else {
                    self.career
                        .as_ref()
                        .and_then(|c| Some(c.tables.missions[c.career.current_mission(&c.tables)?].narrator))
                        .unwrap_or(0)
                        .clamp(0, 5) as usize
                };
                (i, entry)
            })
            .collect();
        // `NFS5.MOVIEBOX`. 0x4CD190 reads the widget's `data.othersource`: `chroniclemain` plays
        // `<Movies>/<0x507390()>.mad`, and 0x507390 maps the chronicle's model group — the same
        // `[0x5DBA44]` the picture names use — to `ch5`, `Ch3`, `Ch1`, `Ch2`, `Ch4` for groups 0…4
        // and `Ch0` past them. `chronicle` instead plays the per-entry clip 0x507662 builds with
        // `"%s%c%c%02d%d%c.mad"`, the picture's own name with the other extension; this
        // installation ships none of those, so that widget finds no file, as the original would.
        //
        // `NFS5.MOVIECIRCLE` plays one as well, and its activate picks the file by the same
        // `othersource` key (0x4CD1C1, 0x4CD20B): **`demoloop`** goes through 0x4CD090, which
        // builds `"%s%s.mad"` from a rotation — `[this+0x1B8]` picks `er1pc-av`, `er2pc-av`,
        // `er3pc-av` or `er4pc-av` for 1…4 through the jump table at 0x4CD17C and **`ch6`** for
        // anything else (0x4CD142) — and the activate sets that index to the `strcmp` result,
        // **0**, so the first clip is `ch6`. **`careerfirst`** takes `er1pc-av` outright
        // (0x4CD247). All five ship in `FEData/Movies`.
        //
        // The port plays the first clip and does not rotate: what advances `[this+0x1B8]` is the
        // player's own end-of-clip, which this port's loop does not signal.
        let movies: Vec<(usize, String)> = l
            .numbered_widgets("NFS5.MOVIEBOX")
            .into_iter()
            .filter_map(|(i, _)| {
                const CHRONICLE_MOVIE: [&str; 5] = ["ch5", "Ch3", "Ch1", "Ch2", "Ch4"];
                let src = l.layout.elements[i].get("othersource")?;
                src.eq_ignore_ascii_case("chroniclemain").then(|| (i, format!("{}.mad", CHRONICLE_MOVIE[self.fe.chronicle_group.min(4)])))
            })
            .chain(l.numbered_widgets("NFS5.MOVIECIRCLE").into_iter().filter_map(|(i, _)| {
                let file = match l.layout.elements[i].get("othersource")? {
                    s if s.eq_ignore_ascii_case("demoloop") => "ch6.mad",
                    s if s.eq_ignore_ascii_case("careerfirst") => "er1pc-av.mad",
                    _ => return None,
                };
                Some((i, file.to_string()))
            }))
            .collect();
        let huds = l.numbered_widgets("NFS5.HUD");
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for index in pictures {
            l.set_pack_picture(r, &self.paths, "Art", index, "ALLdrivr.fsh", picture);
        }
        for (index, entry) in faces {
            l.set_pack_picture(r, &self.paths, "Art", index, "FactoryPeople.fsh", entry);
        }
        for (index, file) in movies {
            l.set_movie(r, &self.paths, index, "Movies", &file);
        }
        // `NFS5.HUD`'s own art: 0x4CB65D builds `<Art>hud.fsh` and 0x4CB685 keeps all **fifteen**
        // of its entries at `widget + 0x178 + 4i`. Entry 0 is `disp`, 240×180 — the size
        // `display.lay` gives the widget — and the draw blits it first (0x4CBB93).
        for (index, _) in huds {
            l.set_pack_picture(r, &self.paths, "Art", index, "hud.fsh", 0);
        }
        if let Some(pack) = pack {
            for index in badges {
                l.set_badge(r, &self.paths, index, &pack, &name, &rank, colour);
            }
        }
    }

    /// Fill the `NFS5.GouraudIndicatorBar`s of whatever screen is up. Only the car-compare screen
    /// has any, and its bars read the same `<n>CarCompareCar<m>` sources as its numbers.
    fn bind_indicator_bars(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let wanted: Vec<(usize, Option<i32>)> = l
            .indicator_bars()
            .into_iter()
            .map(|(i, src, is_other)| {
                let v = if is_other {
                    self.compare_rating(&src)
                } else {
                    // an accessor-driven bar: a range setting scaled onto the bar's own 0..100
                    // (the widget's stops are `c0`, `c50` and `c100`, and its draw compares against
                    // the literal 0x64 at 0x4CA724), otherwise whatever number the accessor's text
                    // carries — the damage accessors already answer in per cent
                    let name = src.split_once('(').map_or(src.as_str(), |(b, _)| b.trim());
                    // a bar asks slot **6**, the integer, not slot 5's text — for the damage
                    // family that is `_ftol(100 − condition)`, truncated (§4.134)
                    match self.accessor_number(&src) {
                        Some(v) => Some(v),
                        None => match self.range_value(name) {
                            Some((v, lo, hi)) if hi > lo => Some((v - lo) * 100 / (hi - lo)),
                            _ => self
                                .accessor_value(&src)
                                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                                .map(|v| v.round() as i32),
                        },
                    }
                };
                (i, v)
            })
            .collect();
        // `NFS5.KNOCKOUTTRACKLIST` is the strip beside those bars: the opponent count and the row
        // `Knockout.TrackNumber` is on (`LayoutScreen`'s own arm has the geometry)
        let strips: Vec<(usize, usize, usize)> = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.numbered_widgets("NFS5.KNOCKOUTTRACKLIST"))
            .unwrap_or_default()
            .into_iter()
            .map(|(i, _)| (i, self.fe.menu.as_ref().map_or(0, |m| m.ko_opponents as usize), self.fe.menu.as_ref().map_or(0, |m| m.ko_sel)))
            .collect();
        // the car-setting sliders: the knob's place along its track is the accessor's own index
        // in its own list (`RE/notes/layouts.md`, the car-setup section)
        let knobs: Vec<(usize, Option<(i32, i32)>)> = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_setting_sliders())
            .unwrap_or_default()
            .into_iter()
            .map(|(i, a)| {
                let base = a.split_once('(').map_or(a.as_str(), |(b, _)| b.trim());
                let v = nfs_game::career::CAR_SETUP_NAMES
                    .iter()
                    .position(|n| *n == base)
                    .and_then(|k| {
                        let c = self.career.as_ref()?;
                        let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                        Some(nfs_game::career::car_setup_index(k, c.career.garage.get(idx)?.setup[k]))
                    });
                (i, v)
            })
            .collect();
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        for (index, value) in wanted {
            l.set_bar(index, value);
        }
        for (index, value) in knobs {
            l.set_knob(index, value);
        }
        for (index, rows, current) in strips {
            l.set_ko_strip(index, rows, current);
        }
    }

    /// Abandon the running race and go back to the menu; `false` when there is no menu to go back
    /// to (the app was started straight into a race).
    fn leave_race(&mut self) -> bool {
        let Some(menu) = self.fe.menu_template.take() else {
            return false;
        };
        self.save_replay();
        // back in the front end the playback flag `[0x699FC9]` is clear again — the controller's
        // constructor zeroes it (0x50C16F) — so the next race records
        if self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)) {
            self.replay.stream = Some(nfs_game::replay::Replay::new());
        }
        if let Some(c) = self.career.as_mut() {
            c.pending = None;
        }
        self.sync_save_side_state();
        if let Some(k) = self.knockout.as_mut() {
            k.active = false;
            k.message = "SERIES CANCELLED".into();
        }
        self.player_grid_override = None;
        self.career_opponents = None;
        self.skill = None;
        self.mission = None;
        self.stop_race_audio();
        self.fe.menu = Some(menu);
        self.vehicle = None;
        self.race = None;
        self.world = None;
        self.car_model = None;
        self.sky = None;
        self.scene = Scene::default();
        // back from a replay the front end opens `gamesetup` in place and pushes `loadreplay` over
        // it — 0x4D3EB2 tests the playback flag, then 0x4D2100("gamesetup", 0, 1) and
        // 0x4D2100("loadreplay", 1, 1)
        if std::mem::take(&mut self.fe.after_replay) {
            self.fe.layout_stack.push("gamesetup".to_string());
            self.open_layout("loadreplay");
        }
        // back from a test drive (0x4D4347…0x4D4736) the stack is rebuilt over the front end's
        // entry screen `gamesetup` (0x4B6C5B's argument) — `tournament`, `garage`, then the screen
        // the drive was taken from: `dealership` (6), `usedcars` (7) or `carsetup` (8).
        // APPROXIMATION: the order 0x4D6520 / 0x4D5BE0 / 0x4D5DF0 build the manager's vector in is
        // taken as bottom-to-top, which is how this port's ancestor stack reads
        if let Some((t, _)) = self.fe.test_drive.take() {
            self.fe.layout_stack = vec!["gamesetup".to_string(), "tournament".to_string(), "garage".to_string()];
            self.open_layout(match t {
                6 => "dealership",
                7 => "usedcars",
                _ => "carsetup",
            });
        }
        true
    }
    /// Drain the network and put every remote player on its seat. Returns the seats the network
    /// owns this tick, so the AI/physics below leaves them alone — a remote car is driven by what
    /// arrived, exactly as the original's receiver drives it (`network.md` §5.2).
    fn net_step(&mut self) -> Vec<usize> {
        if self.net.session.is_none() {
            return Vec::new();
        }
        let poses: Vec<(Vec3, Quat)> = self
            .road.opponents
            .iter()
            .map(|o| {
                (
                    o.vehicle.position,
                    Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
                )
            })
            .collect();
        let tick = self.tick;
        // taken out for the duration so the seats and the player's car can be borrowed too
        let mut net = self.net.session.take();
        let seats = {
            let n = net.as_mut().expect("checked");
            for line in n.poll(tick, |s| {
                poses
                    .get(s)
                    .copied()
                    .unwrap_or((Vec3::ZERO, Quat::IDENTITY))
            }) {
                tracing::info!(target: "netplay", "{line}");
            }
            n.remotes.values().map(|r| r.seat).collect::<Vec<_>>()
        };
        if let Some(n) = net.as_ref() {
            for &s in &seats {
                if let Some(o) = self.road.opponents.get_mut(s) {
                    let specs = o.specs.clone();
                    n.drive(s, &mut o.vehicle, &specs, tick);
                }
            }
        }
        // `NfsNet_tick` 0x48EDF0 hangs off the **64 Hz** world tick and its gate is
        // `(gGameTick & 7) == 7`, which is always an odd tick — so this cannot sit inside the
        // port's nested 32 Hz force step, or it would never fire.
        if let (Some(n), Some(v)) = (net.as_mut(), self.vehicle.as_ref()) {
            n.send(tick, v);
        }
        self.net.session = net;
        seats
    }

    fn simulate(&mut self) {
        self.tick += 1;
        // the track's particle systems run on the 64 Hz clock (0x4689C0 → 0x4551E0)
        if self.track_particles.is_none()
            && !self.track_particle_defs.is_empty()
            && let Some(w) = self.world.as_ref()
        {
            let p = nfs_game::track_particles::TrackParticles::new(std::mem::take(&mut self.track_particle_defs), &w.emitters, self.traffic_seed, 256);
            tracing::info!(emitters = w.emitters.len(), systems = p.systems(), flares = w.flares.len(), "track effects");
            self.track_particles = Some(p);
            // diagnostic: `NFS_EMITTER_VIEW=k` runs the systems 300 updates ahead so a still of
            // emitter k (see `rebuild_scene`) shows them going
            if std::env::var("NFS_EMITTER_VIEW").is_ok()
                && let Some(p) = self.track_particles.as_mut()
            {
                for _ in 0..300 {
                    p.step();
                }
                let sp = p.sprites();
                tracing::info!(sprites = sp.len(), first = ?sp.first(), "emitter view");
            }
        }
        if let Some(p) = self.track_particles.as_mut() {
            p.step();
        }
        self.render_prev = Some((self.car_pos, self.car_rot));
        self.road.opp_prev = self
            .road.opponents
            .iter()
            .map(|o| {
                (
                    o.vehicle.position,
                    Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
                )
            })
            .collect();
        let net_seats = self.net_step();
        let view = self.camera_view();
        // whether the camera key steps the replay's own nine ids (`replay.md` §8) — taken before
        // the car is borrowed
        let replay_camera = self.replay_view().is_some();
        let Some(v) = self.vehicle.as_mut() else {
            return;
        };
        let Some(w) = self.world.as_ref() else { return };
        if self.tick % 2 == 0 {
            // 32 Hz: latch controller, run the force step (Sc32)
            // right = +x in the body frame; rounded, so a pad's packet byte (a multiple of 2/126)
            // comes back whole
            let mut steer = (self.input.steer() * 126.0).round() as i32;
            if self.mirror {
                steer = -steer;
            }
            if self.follow_road
                && let (Some(t), Some(r)) = (&self.slices, &self.race)
            {
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
                if v.speed() > 36.0 || steer.abs() > 80 {
                    throttle = throttle.min(0.35);
                }
            }
            // before the start tick the original forces full brake and no throttle (0x49DD20)
            v.race_started = self.tick >= RACE_START_TICK;
            v.game_tick = self.tick;
            v.rally = self.cheat_bits & 1 != 0;
            v.tiny = self.cheat_bits & (1 << 1) != 0;
            v.race_state = if self.race.as_ref().is_some_and(|r| r.finished) { 2 } else { 0 };
            // countdown speech (Phrase_kCountdown: SOR-a-3/2/1/Go at ticks 256/320/384/448)
            let phase = COUNTDOWN_TICKS
                .iter()
                .chain(std::iter::once(&RACE_START_TICK))
                .filter(|&&t| self.tick >= t)
                .count() as u8;
            if phase > self.snd.countdown_said
                && let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech)
            {
                self.snd.countdown_said = phase;
                sp.say(
                    a,
                    ["SOR-a-3.bnk", "SOR-a-2.bnk", "SOR-a-1.bnk", "SOR-a-Go.bnk"]
                        [(phase - 1) as usize],
                    0.8,
                );
            }
            // The shape the original's controller packet has (`replay.md` §2): every axis is at
            // most 0x7E and the car doubles it (0x4992CF), so the values the physics sees are even
            // and never exceed 252 / ±126.
            let controls = Controls {
                throttle: ((throttle * 126.0) as u8).min(0x7E) * 2,
                brake: ((self.input.brake() * 126.0) as u8).min(0x7E) * 2,
                steer: (steer / 2).clamp(-63, 63) * 2,
                handbrake: self.input.held(Action::Handbrake),
                // the packer's plain group fires a command on its press (0x41312F…): code 2
                // `C1_SHIFTUP`, 3 `C1_SHIFTDOWN`
                command: if self.input.take_pressed(Action::ShiftUp) {
                    2
                } else if self.input.take_pressed(Action::ShiftDown) {
                    3
                } else {
                    0
                },
            };
            // replay.md §4: the hook sits inside the 32 Hz input step, before the car reads the
            // packet, so recording and playback are in phase with the simulation by construction
            let controls = match self.replay.stream.as_mut() {
                // `.RAMPS` / `.RAMPG` / `.RAMPB` are `kind != 1` of the `C1_RIGHT`, `C1_GAS` and
                // `C1_BRAKE` bindings (0x51CF54…0x51CFFC) — set for keys, clear for an axis
                Some(r) => r.step(
                    controls,
                    {
                        let (steer, gas, brake) = self.input.ramps();
                        nfs_game::replay::Ramp { steer, gas, brake }
                    },
                    self.tick,
                ),
                None => controls,
            };
            // `DraftFactor` 0x49D9B0: the racer array only — traffic and police do not draft
            let racers: Vec<Vec3> = self.road.opponents.iter().map(|o| o.vehicle.position).collect();
            v.draft =
                nfs_physics::vehicle::draft_factor(v.position, v.forward(), v.v_long(), &racers);
            // the misfire draw comes from the shared LCG, like every other draw in the game
            v.misfire_roll = nfs_game::rng::rand01();
            v.step32(&controls, &self.specs, &w.collision);
            if let Some(rec) = self.record.as_mut() {
                use std::io::Write;
                let _ = writeln!(
                    rec,
                    "{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{},{}",
                    self.tick,
                    controls.throttle,
                    controls.brake,
                    controls.steer,
                    controls.handbrake as u8,
                    v.position.x,
                    v.position.y,
                    v.position.z,
                    v.velocity.x,
                    v.velocity.y,
                    v.velocity.z,
                    v.rpm,
                    v.gear,
                    self.race.as_ref().map(|r| r.slice).unwrap_or(0)
                );
            }
            // `Car_wallCollide` 0x495020's corner skip: the drivable patch that contains the
            // corner in plan and its height, which the collider compares with the car's own y
            // (§6.1). It used to be a downward ray with a 6 m window, which is not the rule.
            let patch_height = |p: Vec3| w.collision.find_patch_height(p).and_then(|(word, h)| (word & 0xF != 0).then_some(h));
            let landing = v.impact; // set by a hard landing during step32 (closing speed)
            v.impact = 0.0;
            if v.collide_walls(&w.walls, &patch_height, 0.0, 0.0, 0.0) {
                self.wall_hits += 1;
                if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                    let j = v.impact;
                    if j >= v.speed() / 8.0 {
                        s.wall_hit(a, j, v.last_wall_class, 0.8)
                    } else {
                        s.wall_scrape(a, v.speed(), v.last_wall_class, 0.6)
                    }
                }
                v.impact = 0.0;
            } else if landing > 6.0
                && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
            {
                s.landing(a, landing, 0.8);
            }
            // the track's props right after the walls (0x498FF0: 0x495020, then 0x498DE0)
            if let Some((impact, class)) = v.collide_props(&w.props, self.tick as u32)
                && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
            {
                s.prop_hit(a, impact, class, 0.8);
            }
            // crash / roll-over one-shot (Damage 0x494500 zone rule; roll-over 0x49BC80 at level 127)
            let rolled = v.orientation.row(1).y < -0.86;
            if (v.crash_sound || rolled)
                && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
            {
                if !self.snd.crash_played {
                    s.crash(a, 0.9);
                }
                self.snd.crash_played = rolled;
            } else {
                self.snd.crash_played = false;
            }
            v.crash_sound = false;
            // tyre dust and smoke, per wheel (0x4A2880); they live on the simulation clock so a
            // replay draws the same spray
            self.particles.step(GAME_TICK.as_secs_f32());
            if self.particle_draw.is_some() {
                let ground = self
                    .world
                    .as_ref()
                    .map(|w| w.light_at(v.position))
                    .unwrap_or([1.0; 3]);
                for (i, w) in v.wheels.iter().enumerate() {
                    if !w.contact {
                        continue;
                    }
                    self.particles.wheel(
                        i,
                        i < 2,
                        w.surface & 0xF,
                        w.wet,
                        w.slip,
                        w.world_pos,
                        v.velocity,
                        ground,
                    );
                }
            }
            // trackside cameras: the trigger sweep 0x413B80 for the watched car
            if let Some(tc) = self.track_cameras.as_mut() {
                if let Some(i) = tc.update(v.position, self.prev_car_pos) {
                    tracing::debug!(camera = i, "trackside camera");
                }
            }
            // Factory Driver skill test: checkpoints / finish / cones (even ticks, 0x416170)
            if let Some(sk) = self.skill.as_mut() {
                let sides = [
                    (v.wheels[0].world_pos, v.wheels[2].world_pos),
                    (v.wheels[1].world_pos, v.wheels[3].world_pos),
                ];
                // 0x416170: `car+0xDB0 == 0 && car+0x1100 < 4`
                let about_to_reset = v.engine_alive == 0 && v.reset_countdown < 4;
                let mut events = sk.tick(
                    self.tick,
                    v.position,
                    self.prev_car_pos,
                    &sides,
                    about_to_reset,
                );
                events.extend(sk.knock_cones(
                    v.position,
                    &v.orientation,
                    v.half_extents,
                    v.velocity,
                ));
                for e in events {
                    match e {
                        SkillEvent::Checkpoint { sequence, .. } => {
                            let shown: Vec<usize> = sk.shown_arrow.into_iter().collect();
                            self.world_actions
                                .push(WorldAction::Show(sk.arrows.clone(), shown));
                            tracing::info!(sequence, "checkpoint");
                        }
                        SkillEvent::Finish { time_ticks } => {
                            if let Some(r) = self.race.as_mut() {
                                r.finished = true;
                                r.lap_times.push(time_ticks);
                            }
                            tracing::info!(
                                time_s = time_ticks as f32 / 64.0,
                                cones = sk.cones_knocked(),
                                "skill test finished"
                            );
                        }
                        SkillEvent::ConeHit {
                            element,
                            away,
                            launch,
                        } => {
                            self.world_actions
                                .push(WorldAction::Knock(element, away, launch));
                            if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
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
            }
            // both trigger sweeps above compare the position with the previous tick's
            self.prev_car_pos = v.position;
            // traffic AI (priority 6 on the 32 Hz list, before ground contact and the collision pass)
            if let (Some(tr), Some(t)) = (self.road.traffic.as_mut(), self.slices.as_ref()) {
                let slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
                // the player's own `car+0xEB0` (0x417800): −1 below −3 m/s, +1 from 0 up, otherwise
                // the sign of the heading against the slice
                let vl = v.v_long();
                let dir = if vl <= -3.0 {
                    -1.0
                } else if vl >= 0.0 {
                    1.0
                } else {
                    v.forward().dot(t.slices[slice].forward).signum()
                };
                let laps = self.race.as_ref().map(|r| r.laps).unwrap_or(0);
                // `0x408360` is inserted into the 32 Hz list twice (priorities 6 and 30) and the LOD
                // gate `0x414B10` is stateless, so a due traffic car really does run its whole AI and
                // drive step twice per tick while `dt` still says 1/32 (`traffic.md` §21.5)
                let reference = Reference {
                    position: v.position,
                    slice,
                    laps,
                    speed: vl,
                    dir,
                };
                // the cars the lane scorers must see besides the traffic set itself
                let lane_car = |pos_slice: usize,
                                lateral_of: f32,
                                hw: f32,
                                hl: f32,
                                speed: f32,
                                est: f32,
                                human: bool| {
                    let sl = &t.slices[pos_slice];
                    LaneCar {
                        slice: pos_slice,
                        lane: lane_of(sl, lateral_of),
                        lane_mask: lane_span(sl, lateral_of, hw),
                        speed,
                        speed_est: est,
                        half_len: hl,
                        lateral: lateral_of,
                        half_span: hw,
                        is_human: human,
                        // every car this closure builds is in the race — the player or an
                        // opponent — and 0x401C8D's shoulder bonus asks for `52C & 1`, which is
                        // exactly that (`traffic.md`)
                        is_racer: true,
                    }
                };
                let lateral_on = |pos: Vec3, s: usize| {
                    (pos - t.slices[s].centre).dot(t.slices[s].lateral.normalize_or_zero())
                };
                let mut others = vec![lane_car(
                    slice,
                    lateral_on(v.position, slice),
                    self.car_half_width,
                    v.half_extents.z,
                    vl,
                    v.speed(),
                    true,
                )];
                others.extend(self.road.opponents.iter().map(|o| {
                    let s = o.progress.slice;
                    lane_car(
                        s,
                        lateral_on(o.vehicle.position, s),
                        o.vehicle.half_extents.x,
                        o.vehicle.half_extents.z,
                        o.vehicle.v_long(),
                        o.vehicle.speed(),
                        false,
                    )
                }));
                // **Once**, not twice. `traffic.md` §21.5 used to say 0x408360 was inserted into
                // two scheduler slots for a traffic car and therefore ran twice per due tick; it
                // is not. `Car_installHandlers` seeds `car+0xE0C = 0x49AF70` at 0x410DDD and only
                // overwrites it with 0x408360 at 0x410E6D **when `car+0x52C & 1`** — the racer
                // bit, which traffic does not have (`test bl, al; je` at 0x410E64/0x410E6B skips
                // the store). `+0xE1C` gets 0x408360 unconditionally at 0x410E83, so traffic has
                // it in one slot. A racer has it in both and 0x408366 makes it return at once for
                // exactly that case, so it never does work twice for anybody.
                tr.update(t, self.tick, reference, &others);
            }
            // police (`traffic.md` §17.3 violation, §25 spawn, §27 chase)
            if let (Some(pol), Some(t)) = (self.road.police.as_mut(), self.slices.as_ref()) {
                let slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
                let vl = v.v_long();
                let one_way = t.slices.get(1).map(|s| s.lanes_a == 0).unwrap_or(false);
                // the speed limit is a flat 22 m/s and wrong way is only an offence on a one-way road
                pol.violation =
                    nfs_game::police::check_violation(pol.violation, vl, 1.0, one_way, false, 0.0);
                let clean = pol.violation.is_clean();
                // a pursuit may only start once a cop is in the world and the race is under way
                // §25.5 runs for every on-duty cop whatever the perp is doing: while the perp is
                // clean the probability doubles and the cop comes out on *patrol* (§17.4 states
                // 2/3); a wanted perp gets chasers. `set_wanted` is the order that turns the patrol
                // cars into chasers when the perp offends.
                pol.set_wanted(!clean);
                if self.tick >= nfs_game::police::PURSUIT_START_TICK {
                    let idle = pol.cars.iter().position(|c| !c.active);
                    if let Some(i) = idle {
                        let has_cop = pol.active_count() > 0;
                        let roll = nfs_game::rng::rand01();
                        if let Some(rec) = pol.manager.find_spawn(
                            t,
                            slice,
                            vl,
                            self.tick as i64,
                            roll,
                            clean,
                            self.race_skill,
                            &[slice],
                            has_cop,
                        ) {
                            pol.deploy_as(t, i, rec, self.tick, !clean);
                        }
                    }
                }
                if pol.active_count() > 0 {
                    let sl = &t.slices[slice];
                    let lat = (v.position - sl.centre).dot(sl.lateral.normalize_or_zero());
                    let target = nfs_game::police::ChaseTarget {
                        position: v.position,
                        slice,
                        lateral: lat,
                        lane: lane_of(sl, lat),
                        speed: vl,
                        // `car+0x3A8` / `car+0x3B0`, the windows the §27.8 sector tests use
                        half_width: self.car_half_width,
                        half_length: v.half_extents.z,
                        finished: self.race.as_ref().map(|r| r.finished).unwrap_or(false),
                    };
                    let others = vec![LaneCar {
                        slice,
                        lane: target.lane,
                        lane_mask: lane_span(sl, lat, self.car_half_width),
                        speed: vl,
                        speed_est: v.speed(),
                        half_len: v.half_extents.z,
                        lateral: lat,
                        half_span: self.car_half_width,
                        is_human: true,
                        is_racer: true,
                    }];
                    pol.update(t, self.tick, &target, &others);
                }
            }
            // opponents (parked controller until the AI rules land)
            let mut ko_laps: Vec<(usize, u32)> = Vec::new();
            if let Some(t) = &self.slices {
                // race order (ascending progress) for the avoidance rule: cars directly ahead of each opponent
                let mut order: Vec<(f32, Vec3, usize, f32, f32, bool)> = self
                    .road.opponents
                    .iter()
                    .map(|o| {
                        (
                            o.progress.progress,
                            o.vehicle.position,
                            o.progress.slice,
                            o.vehicle.v_long(),
                            0.9,
                            false,
                        )
                    })
                    .collect();
                if let Some(r) = &self.race {
                    order.push((
                        r.progress,
                        v.position,
                        r.slice,
                        v.v_long(),
                        self.car_half_width,
                        true,
                    ));
                }
                // traffic is in the race-order array too (`traffic.md` §8), so the opponents' "two cars
                // directly ahead" rule sees it and steers around it
                if let Some(tr) = &self.road.traffic {
                    order.extend(tr.cars.iter().filter(|c| c.active).map(|c| {
                        (
                            c.progress,
                            c.position,
                            c.slice,
                            c.speed * c.dir,
                            c.half_extents.x,
                            false,
                        )
                    }));
                }
                order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                // catch-up band (`ai.md` §5.2 rule 1): the human's progress and average pace
                let (player_progress, player_pace) = match &self.race {
                    Some(r) => (
                        r.progress,
                        if self.tick > 0 {
                            (r.progress - r.start_progress) * 640_000.0 / self.tick as f32
                        } else {
                            0.0
                        },
                    ),
                    None => (0.0, 0.0),
                };
                // every racer's position: index 0 is the human, then the opponents in order
                let racer_positions: Vec<Vec3> = std::iter::once(self.car_pos)
                    .chain(self.road.opponents.iter().map(|o| o.vehicle.position))
                    .collect();
                for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                    o.vehicle.race_started = self.tick >= RACE_START_TICK;
                    o.vehicle.game_tick = self.tick;
                    o.vehicle.rally = self.cheat_bits & 1 != 0;
                    o.vehicle.tiny = self.cheat_bits & (1 << 1) != 0;
                    if net_seats.contains(&oi) {
                        // a remote player: the pose is already set, so only the lap tracker runs
                        o.progress.update_tracked(
                            t,
                            o.vehicle.position,
                            o.vehicle.forward(),
                            self.tick,
                            self.num_laps,
                            None,
                        );
                        continue;
                    }
                    if let Controller::Ai(d) = &mut o.controller {
                        d.set_catchup(self.race_skill, player_progress, player_pace);
                    }
                    let ahead: Vec<(Vec3, usize, f32, f32, bool)> = order
                        .iter()
                        .filter(|e| e.0 > o.progress.progress)
                        .take(2)
                        .map(|e| (e.1, e.2, e.3, e.4, e.5))
                        .collect();
                    let c =
                        o.controls_for(&controls, t, self.racing_line.as_ref(), self.tick, &ahead);
                    o.vehicle.misfire_roll = nfs_game::rng::rand01();
                    o.vehicle.draft = nfs_physics::vehicle::draft_factor(
                        o.vehicle.position,
                        o.vehicle.forward(),
                        o.vehicle.v_long(),
                        &racer_positions[..]
                            .iter()
                            .enumerate()
                            .filter(|(k, _)| *k != oi + 1)
                            .map(|(_, p)| *p)
                            .collect::<Vec<_>>(),
                    );
                    o.vehicle.step32(&c, &o.specs, &w.collision);
                    if o.vehicle.collide_walls(&w.walls, &patch_height, 0.0, 0.0, 0.0) {
                        o.wall_hits += 1;
                    }
                    o.vehicle.collide_props(&w.props, self.tick as u32);
                    let before = o.progress.progress;
                    // the tracker's `preferredSection`: the branch this opponent chose (§1.4)
                    let pref = match &o.controller {
                        Controller::Ai(d) => d.preferred_segment,
                        _ => None,
                    };
                    let o_lap_done = o.progress.update_tracked(
                        t,
                        o.vehicle.position,
                        o.vehicle.forward(),
                        self.tick,
                        self.num_laps,
                        pref,
                    );
                    if o.progress.progress > before {
                        o.last_progress_tick = self.tick;
                    }
                    if o_lap_done && !o.eliminated && o.progress.laps <= self.num_laps {
                        ko_laps.push((oi, o.progress.laps));
                    }
                }
            }
            // car–car contact: 0x4980D0's first round per pair (`nfs_physics::vehicle::collide_cars`),
            // after every car's own step — the collision pass is priority 0x28 on the 32 Hz list,
            // behind the drive (6) and the ground contact (0x1E) of all cars (`traffic.md` §21.5).
            // APPROXIMATION of the pass itself: 0x498FF0 runs, car by car, the car's walls and props
            // and then its pairs with the cars already done this tick; this port collides the walls
            // in each car's own step and then walks the pairs below
            let no_carcol = std::env::var_os("NFS_NO_CARCOL").is_some();
            // `car+0x354`, the collision mass: `HEAVY` `[0x6573FC]` (the fetherw8 code, cheat bit 8)
            // multiplies a human car's by 12 (0x49B5FB…0x49B613, `[0x5B240C]`)
            // …and otherwise `.TINYCAR` divides any car's by 12 (0x49B615…0x49B62F, `[0x5B49B0]`) —
            // every car but traffic carries it under the Gulliver code
            let tiny_k = if self.cheat_bits & (1 << 1) != 0 { 1.0 / 12.0 } else { 1.0 };
            let player_mass = self.specs.mass * if self.cheat_bits & (1 << 8) != 0 { 12.0 } else { tiny_k };
            // 0x4980D0's rounds collide each racer's walls again between them (0x495020)
            // (traffic has no wall collider in this port, so its side is left alone)
            let tick = self.tick as u32;
            let mut racer_walls = |c: &mut nfs_physics::vehicle::Vehicle| {
                if !c.is_traffic {
                    c.collide_walls(&w.walls, &patch_height, 0.0, 0.0, 0.0);
                    c.collide_props(&w.props, tick);
                }
            };
            for o in self.road.opponents.iter_mut() {
                if !no_carcol
                    && let Some(effect) = nfs_physics::vehicle::collide_cars_with(
                        v,
                        player_mass,
                        &mut o.vehicle,
                        o.specs.mass * tiny_k,
                        &mut racer_walls,
                    )
                    && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
                {
                    s.car_car(a, effect, 0.8);
                }
            }
            // traffic vs everyone (`traffic.md` §21.5): the broad phase walks every car pair, so a
            // traffic car is a full rigid body for the solver. The impulse it takes lives until the
            // next AI tick overwrites its velocity, exactly as in the original.
            if !no_carcol && let Some(tr) = self.road.traffic.as_mut() {
                for (i, c) in tr.cars.iter_mut().enumerate().filter(|(_, c)| c.active) {
                    let Some(b) = self.road.traffic_bodies.get_mut(i) else {
                        continue;
                    };
                    b.position = c.position;
                    b.orientation = c.orientation;
                    b.velocity = c.velocity;
                    b.angular_velocity = Vec3::ZERO;
                    b.half_extents = c.half_extents;
                    if let Some(effect) =
                        nfs_physics::vehicle::collide_cars_with(v, player_mass, b, c.mass, &mut racer_walls)
                    {
                        // the collision script trigger (§12.9 trigger 0)
                        c.hit_by_human_tick = self.tick;
                        // `car+0x418`, the counter that disables the "on its roof" exit (§21.7)
                        c.collisions += 1;
                        tracing::debug!(
                            car = i,
                            effect,
                            mass = c.mass,
                            speed = v.speed(),
                            "player hit traffic"
                        );
                        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                            s.car_car(a, effect, 0.8);
                        }
                    }
                    for o in self.road.opponents.iter_mut() {
                        if let Some(effect) = nfs_physics::vehicle::collide_cars_with(
                            &mut o.vehicle,
                            o.specs.mass * tiny_k,
                            b,
                            c.mass,
                            &mut racer_walls,
                        ) {
                            c.collisions += 1;
                            tracing::debug!(
                                car = i,
                                opponent = o.name,
                                effect,
                                "opponent hit traffic"
                            );
                        }
                    }
                    c.position = b.position;
                    c.velocity = b.velocity;
                }
            }
            // the player and the opponents against the police
            if !no_carcol && let Some(pol) = self.road.police.as_mut() {
                for (i, c) in pol.cars.iter_mut().enumerate().filter(|(_, c)| c.active) {
                    let Some(b) = self.road.police_bodies.get_mut(i) else {
                        continue;
                    };
                    b.position = c.position;
                    b.orientation = c.orientation;
                    b.velocity = c.velocity;
                    b.angular_velocity = c.angular_velocity;
                    b.half_extents = c.half_extents;
                    if let Some(effect) =
                        nfs_physics::vehicle::collide_cars_with(v, player_mass, b, c.mass * tiny_k, &mut racer_walls)
                    {
                        tracing::debug!(cop = i, effect, "player hit a cop");
                        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                            s.car_car(a, effect, 0.8);
                        }
                    }
                    for o in self.road.opponents.iter_mut() {
                        nfs_physics::vehicle::collide_cars_with(&mut o.vehicle, o.specs.mass * tiny_k, b, c.mass * tiny_k, &mut racer_walls);
                    }
                    c.position = b.position;
                    c.velocity = b.velocity;
                    c.angular_velocity = b.angular_velocity;
                }
            }
            for i in 0..self.road.opponents.len() {
                if no_carcol {
                    break;
                }
                let (l, r) = self.road.opponents.split_at_mut(i + 1);
                for o in r.iter_mut() {
                    let (ma, mb) = (l[i].specs.mass * tiny_k, o.specs.mass * tiny_k);
                    nfs_physics::vehicle::collide_cars_with(&mut l[i].vehicle, ma, &mut o.vehicle, mb, &mut racer_walls);
                }
            }
            // the post-impact pass (`Car_postImpact` 0x410280, priority 0x32, after the collisions):
            // reset to the current slice when 0x40F740's crash countdown runs out
            // (`Vehicle::reset_countdown_tick`), which runs eight times faster off the road
            // surface — `car+0x464 == 0`, the body tracker's word (0x49AFA1…0x49AFDB), which
            // `find_patch` answers. The placement is the port's own (APPROXIMATION of 0x40F540).
            // diagnostic: `NFS_ESCAPE_LOG=x0,x1` prints the player's state while its x is in that
            // band, for chasing a car that gets past the wall line
            if let Ok(band) = std::env::var("NFS_ESCAPE_LOG") {
                let mut it = band.split(',').filter_map(|v| v.parse::<f32>().ok());
                if let (Some(a), Some(b)) = (it.next(), it.next())
                    && v.position.x >= a.min(b)
                    && v.position.x <= a.max(b)
                {
                    tracing::info!(t = self.tick, pos = ?v.position, kmh = v.speed() * 3.6, patch = w.collision.find_patch(v.position) & 0xF, wall = ?w.walls.nearest(v.position, 20.0).map(|(n, d)| (n.x, n.y, d)), "escape watch");
                }
            }
            let off_surface = w.collision.find_patch(v.position) & 0xF == 0;
            let crashed = v.reset_countdown_tick(off_surface);
            // headless runs stand in for a human pressing reset: a scripted bot that is stuck below
            // 2 m/s for 5 s resets itself (the original's AI rule `car+0xD8B = 5`, ai.md §3.2) so a
            // wedged car cannot block a regression run. Interactive play uses the key only.
            if self.scripted {
                self.stuck_ticks = if v.speed() < 2.0 && v.race_started {
                    self.stuck_ticks + 1
                } else {
                    0
                };
            }
            let bot_stuck = self.stuck_ticks >= 160;
            if bot_stuck {
                self.stuck_ticks = 0;
            }
            // 0x40F7EB: a reset request (the player's key, or the bot's stand-in for it) goes through
            // `Vehicle::reset_request`'s gates; 0x40F540 itself does nothing in a skill test on the
            // skidpad (`[0x6573EC] == 4` and the track "skidpad", 0x40F540…0x40F565)
            let asked = self.input.take_pressed(Action::Reset) || bot_stuck;
            let requested = v.reset_request(asked, self.tick);
            let exempt = self.skill.is_some() && self.track.name.eq_ignore_ascii_case("skidpad");
            if (crashed || requested)
                && !exempt
                && let (Some(t), Some(r)) = (&self.slices, &self.race)
            {
                self.resets += 1;
                let occupied: Vec<usize> =
                    self.road.opponents.iter().filter(|o| !o.eliminated).map(|o| o.progress.slice).collect();
                let slice = free_reset_slice(t, &w.collision, r.slice, &occupied);
                let sl = &t.slices[slice];
                let fwd = sl.forward.normalize();
                let up = sl.up.normalize();
                let right = up.cross(fwd).normalize();
                v.position = sl.centre
                    + up * (0.5
                        + self
                            .car_model
                            .as_ref()
                            .map(|c| c.wheel_radius)
                            .unwrap_or(0.3));
                v.orientation = glam::Mat3::from_cols(right, up, fwd).transpose();
                v.velocity = Vec3::ZERO;
                v.angular_velocity = Vec3::ZERO;
                v.rpm = 0.0;
                v.gear = 2;
                v.rearm_after_reset();
                // 0x40F59D: every reset clears the countdown
                v.reset_countdown = 0;
                tracing::info!(slice, "car reset to track");
            }
            if let (Some(t), Some(r)) = (&self.slices, &self.race) {
                let off_surface = w.collision.find_patch(v.position) & 0xF == 0;
                v.reset_countdown_start(t.slices[r.slice].up.normalize(), off_surface);
            }
            // the opponents' post-impact pass, after the player's (each car's 0x410280 slot, in the
            // cars' order): the AI reset request and 0x40F740's crash countdown
            if let Some(t) = &self.slices {
                let racer_slices: Vec<Option<usize>> = std::iter::once(self.race.as_ref().map(|r| r.slice))
                    .chain(self.road.opponents.iter().map(|o| (!o.eliminated).then_some(o.progress.slice)))
                    .collect();
                for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                    // stuck: reset onto the slice (AI reset request car+0xD8B = 5)
                    // the two triggers are kept apart for the log: `ai.md` §3.2's five seconds
                    // under 2 m/s, and facing the wrong way at over 10 m/s
                    let (was_stuck, was_wrong, facing) = match &o.controller {
                        Controller::Ai(ai) => (ai.stuck_ticks >= 0xA0, ai.wrong_way, ai.dbg_facing),
                        _ => (false, false, 0.0),
                    };
                    let stuck = match &mut o.controller {
                        Controller::Ai(ai) => ai.wants_reset(),
                        _ => false,
                    };
                    // diagnostic: `NFS_SLOW_LOG=1` prints every opponent that is barely moving,
                    // which is how a car that cannot get going is told from one that is merely slow
                    if self.tick % 32 == 0
                        && o.vehicle.speed() < 3.0
                        && std::env::var_os("NFS_SLOW_LOG").is_some()
                    {
                        let surf: Vec<u32> = o.vehicle.wheels.iter().map(|wh| w.collision.find_patch(wh.world_pos) & 0xF).collect();
                        tracing::info!(
                            opponent = oi,
                            t = self.tick / 64,
                            kmh = o.vehicle.speed() * 3.6,
                            rpm = o.vehicle.rpm,
                            gear = o.vehicle.gear,
                            engine = o.vehicle.engine_alive,
                            thr = o.vehicle.throttle,
                            arm = o.vehicle.engine_arm,
                            alive_ticks = o.vehicle.ticks_alive,
                            brk = o.vehicle.brake,
                            contact = o.vehicle.wheels.iter().filter(|wh| wh.contact).count(),
                            surf = ?surf,
                            slip = ?o.vehicle.wheels.iter().map(|wh| wh.slip as i32).collect::<Vec<_>>(),
                            pos = ?o.vehicle.position,
                            "slow opponent"
                        );
                    }
                    // 0x40F740's crash countdown runs for the AI cars too, with the same
                    // off-surface rule (`car+0x464 == 0`)
                    let off_surface = w.collision.find_patch(o.vehicle.position) & 0xF == 0;
                    let crashed = o.vehicle.reset_countdown_tick(off_surface);
                    let stuck = o.vehicle.reset_request(stuck, self.tick);
                    if stuck || crashed {
                        let occupied: Vec<usize> = racer_slices
                            .iter()
                            .enumerate()
                            .filter(|&(k, _)| k != oi + 1)
                            .filter_map(|(_, s)| *s)
                            .collect();
                        let slice = free_reset_slice(t, &w.collision, o.progress.slice, &occupied);
                        let sl = &t.slices[slice];
                        let fwd = sl.forward.normalize();
                        let up = sl.up.normalize();
                        let right = up.cross(fwd).normalize();
                        tracing::debug!(
                            opponent = oi,
                            why = if !stuck { "crash countdown" } else if was_wrong { "wrong way" } else if was_stuck { "stuck 5 s" } else { "reset request" },
                            from_slice = o.progress.slice,
                            to_slice = slice,
                            pos = ?o.vehicle.position,
                            kmh = o.vehicle.speed() * 3.6,
                            engine = o.vehicle.engine_alive,
                            killed_by = o.vehicle.engine_kill,
                            wheels_down = o.vehicle.wheels.iter().filter(|w| w.contact).count(),
                            gear = o.vehicle.gear,
                            facing,
                            "opponent reset"
                        );
                        o.vehicle.reset_to(
                            sl.centre + up * 0.8,
                            glam::Mat3::from_cols(right, fwd.cross(right).normalize(), fwd)
                                .transpose(),
                        );
                        o.resets += 1;
                    }
                    let off_surface = w.collision.find_patch(o.vehicle.position) & 0xF == 0;
                    o.vehicle.reset_countdown_start(t.slices[o.progress.slice].up.normalize(), off_surface);
                }
            }
            // Quick Knockout cut for opponents that completed a lap this tick (modes.md §4)
            if self.quick_knockout && !ko_laps.is_empty() {
                let mut all: Vec<(usize, f32)> = self
                    .road.opponents
                    .iter()
                    .enumerate()
                    .map(|(i, o)| (i + 1, o.progress.progress))
                    .collect();
                if let Some(r) = &self.race {
                    all.push((0, r.progress));
                }
                all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                let pos_of = |d: usize| {
                    all.iter()
                        .position(|(i, _)| *i == d)
                        .map(|p| p as i64 + 1)
                        .unwrap_or(99)
                };
                for &(oi, lap) in &ko_laps {
                    let cut = self.num_laps as i64 - lap as i64 + 2;
                    if pos_of(oi + 1) == cut {
                        for &(d, _) in all.iter().filter(|(d, _)| pos_of(*d) >= cut) {
                            if d == 0 {
                                if let Some(r) = self.race.as_mut()
                                    && !r.finished
                                {
                                    self.player_eliminated = true;
                                    r.finished = true;
                                    r.progress -= 0.01;
                                    if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                                        sp.say(a, "KO-eliminated.bnk", 0.9);
                                    }
                                }
                            } else if let Some(o) = self.road.opponents.get_mut(d - 1)
                                && !o.eliminated
                            {
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
                if r.progress > before {
                    self.player_progress_tick = self.tick;
                }
                let n_cars = self.road.opponents.len() + 1;
                let place = 1 + self
                    .road.opponents
                    .iter()
                    .filter(|o| o.progress.progress > r.progress)
                    .count();
                if lap_done {
                    tracing::info!(
                        lap = r.laps,
                        time_s = r.lap_times.last().map(|t| *t as f32 / 64.0),
                        "lap completed"
                    );
                    if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                        let rnd = (self.tick as usize) ^ (r.laps as usize * 7919);
                        let ord = ["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th"];
                        if r.finished && !was_finished {
                            // results: kRacePlacing, then kLostRace when last of several
                            let name = if place == 1 {
                                if rnd & 1 == 0 {
                                    "Place-youwonrace.bnk"
                                } else {
                                    "Place-youplace1st.bnk"
                                }
                                .to_string()
                            } else if place >= 8 {
                                if rnd & 1 == 0 {
                                    "Place-youplace8th.bnk"
                                } else {
                                    "Place-youplacelast.bnk"
                                }
                                .into()
                            } else {
                                format!("Place-youplace{}.bnk", ord[(place - 1).min(7)])
                            };
                            sp.say(a, &name, 0.9);
                            if place == n_cars && n_cars > 1 {
                                sp.say(
                                    a,
                                    if rnd & 2 == 0 {
                                        "PG-youlostrace.bnk"
                                    } else {
                                        "PG-Betterlucknext.bnk"
                                    },
                                    0.9,
                                );
                            }
                        } else if self.num_laps < 99 {
                            // Race_lapSpeech 0x4277D0 bundle (RE/notes/sfx.md §7.2)
                            let n = r.laps.saturating_sub(1) as usize; // lapsDone − 1
                            sp.say(
                                a,
                                &if n == 0 {
                                    "Lap-your1st.bnk".to_string()
                                } else {
                                    format!("Lap-{}.bnk", ord[n.min(7)])
                                },
                                0.9,
                            );
                            let lap_m = t.lap_len as f32 * nfs_game::race::SLICE_SPACING;
                            let leader = self
                                .road.opponents
                                .iter()
                                .map(|o| o.progress.progress)
                                .fold(r.progress, f32::max);
                            let speed = v.speed();
                            if n_cars == 2 {
                                // kPositionRel: seconds ahead (+) / behind (−) of the rival
                                if let Some(o) = self.road.opponents.first()
                                    && speed > 1.0
                                {
                                    let g =
                                        ((r.progress - o.progress.progress) * lap_m / speed) as i32;
                                    let bucket = if g > 30 {
                                        0
                                    } else if g >= 13 {
                                        1
                                    } else if g >= 7 {
                                        2
                                    } else if g == 6 {
                                        3
                                    } else if g == 5 {
                                        4
                                    } else if g == 4 {
                                        5
                                    } else if g == 3 {
                                        6
                                    } else if g == 2 {
                                        7
                                    } else if g == 1 {
                                        8
                                    } else if g >= -4 {
                                        9
                                    } else {
                                        10
                                    };
                                    sp.say(
                                        a,
                                        [
                                            "Ahead-youreway.bnk",
                                            "Lead-yougotgood.bnk",
                                            "Lead-youreinthe.bnk",
                                            "Lead-yougotsmall.bnk",
                                            "Ahead-yourejust.bnk",
                                            "Lead-barelyholding.bnk",
                                            "Race-itsclose.bnk",
                                            "Race-itstight.bnk",
                                            "Neck-youreneck.bnk",
                                            "Behind-yourejust.bnk",
                                            "Behind-Hurryup.bnk",
                                        ][bucket],
                                        0.9,
                                    );
                                }
                            } else if n_cars >= 3 {
                                // kLapPosition (two phrasings) + kLapTimeBehind (whole seconds to the leader, speed > 13.4 m/s)
                                let p = (place - 1).min(7);
                                let name = if p == 7 {
                                    if rnd & 1 == 0 {
                                        "Place-8th.bnk".to_string()
                                    } else {
                                        "Place-Last.bnk".into()
                                    }
                                } else if rnd & 2 == 0 {
                                    format!("Place-{}.bnk", ord[p])
                                } else {
                                    format!("Place-youin{}.bnk", ord[p])
                                };
                                sp.say(a, &name, 0.9);
                                if speed > 13.4 {
                                    let sec =
                                        ((leader - r.progress) * lap_m / speed).floor() as i32;
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
                            if r.lap_times.len() >= 2
                                && let Some(last) = r.lap_times.last()
                                && r.lap_times[..r.lap_times.len() - 1]
                                    .iter()
                                    .all(|t| last < t)
                            {
                                sp.say(
                                    a,
                                    ["Lap-best.bnk", "Lap-Pthebest.bnk", "Lap-Pyourbest.bnk"]
                                        [rnd % 3],
                                    0.9,
                                );
                            }
                            if r.laps + 1 == self.num_laps {
                                sp.say(
                                    a,
                                    [
                                        "Lap-Final.bnk",
                                        "Lap-yourfinal.bnk",
                                        "Lap-itsthefinal.bnk",
                                        "Lap-Last.bnk",
                                        "Lap-onemore.bnk",
                                        "Lap-thelast.bnk",
                                    ][rnd % 6],
                                    0.9,
                                );
                            }
                        }
                    }
                }
                if r.finished && !was_finished {
                    tracing::info!(place, n_cars, "race finished");
                    // `MusicPlay("win*"|"lose*", 1)` — only in a race with opponents, `win*` iff first
                    // (`engine-audio.md` §8.3); the jingle is appended to the playlist and plays loud
                    if n_cars > 1
                        && let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut())
                    {
                        m.play_pattern(a, if place == 1 { "win*" } else { "lose*" }, 1);
                    }
                }
                // track records (modes.md §6): player's total time on point-to-point tracks, best lap on circuits
                if r.finished
                    && !self.record_done
                    && !self.player_eliminated
                    && self.mission.is_none()
                {
                    self.record_done = true;
                    let circuit = self
                        .records
                        .track(&self.track.name)
                        .map(|t| t.circuit)
                        .unwrap_or(false);
                    let time = if circuit {
                        r.lap_times.iter().copied().min().unwrap_or(0)
                    } else {
                        r.lap_times.iter().sum::<u64>()
                    } as u32;
                    let car_name = self.specs.name.clone();
                    let driver = self.player_name.clone();
                    if let Some(rank) =
                        self.records
                            .insert(&self.track.name, &driver, &car_name, time)
                    {
                        tracing::info!(
                            rank,
                            time = format_ticks(time as u64),
                            circuit,
                            "track record entered"
                        );
                        if let Err(e) = self.records.save(&self.records_path) {
                            tracing::warn!(error = %e, "records save failed");
                        }
                        if rank == 0
                            && let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech)
                        {
                            let rnd = self.tick as usize;
                            sp.say(
                                a,
                                if circuit {
                                    [
                                        "Record-Lap.bnk",
                                        "Record-Newlap.bnk",
                                        "Record-Youbeatlap.bnk",
                                    ][rnd % 3]
                                } else {
                                    [
                                        "PG-beattrackrec.bnk",
                                        "PG-setnewtrackrec.bnk",
                                        "PG-setnewrecord4trk.bnk",
                                    ][rnd % 3]
                                },
                                0.9,
                            );
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
                        for o in self
                            .road.opponents
                            .iter_mut()
                            .filter(|o| o.progress.progress < r.progress + 0.01)
                        {
                            if !o.progress.finished {
                                o.eliminated = true;
                                o.progress.finished = true;
                                o.progress.progress -= 0.01;
                                o.controller = Controller::Parked;
                            }
                        }
                        if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                            sp.say(a, "KO-eliminated.bnk", 0.9);
                        }
                        tracing::info!(lap, place, "player knocked out");
                    }
                }
            }
            if self.input.take_pressed(Action::CameraNext) && replay_camera {
                // A playing replay steps its own nine ids instead (`replay.md` §8). 0x414953 skips
                // **0x0B `SplineCam`** unless the track is `skidpad` (the `strcmp` at 0x414A28) and
                // retries with the next entry; APPROXIMATION: which key the original cycles them
                // with is UNKNOWN (§15), so this port reuses `I_CYCLECAMERA`, the same choice it
                // made for the four playback speeds on the digits.
                let skidpad = self.track.name.eq_ignore_ascii_case("skidpad");
                self.replay.camera = nfs_game::replay::next_camera(self.replay.camera as usize, skidpad) as u8;
            } else if self.input.take_pressed(Action::CameraNext) {
                // `I_CYCLECAMERA`: step through the car's own camera list (`car[0].camera`)
                let n = self.camera_cycle.len().max(1) as u8;
                let driver = nfs_game::camera::VIEW_DRIVER as u16;
                let has_driver = self.driver_cam.is_some();
                self.camera_mode = (self.camera_mode + 1) % n;
                if !has_driver && self.camera_cycle.get(self.camera_mode as usize) == Some(&driver)
                {
                    self.camera_mode = (self.camera_mode + 1) % n;
                }
            }
            if self.input.take_pressed(Action::SplitTimes) {
                self.split_times = self.split_times % 3 + 1;
            }
            self.input.end_tick();
        }
        // police move on the same 64 Hz integrator as traffic
        if let (Some(pol), Some(t)) = (self.road.police.as_mut(), self.slices.as_ref()) {
            self.road.police_prev = pol
                .cars
                .iter()
                .map(|c| {
                    (
                        c.position,
                        Quat::from_mat3(&c.orientation.transpose()).normalize(),
                    )
                })
                .collect();
            let grid = self.world.as_ref().map(|w| &w.collision);
            pol.integrate_on(t, 1.0 / 64.0, grid);
        }
        // traffic is integrated at 64 Hz from the velocity the AI wrote on the last 32 Hz tick
        if let (Some(tr), Some(t)) = (self.road.traffic.as_mut(), self.slices.as_ref()) {
            self.road.traffic_prev = tr
                .cars
                .iter()
                .map(|c| {
                    (
                        c.position,
                        Quat::from_mat3(&c.orientation.transpose()).normalize(),
                    )
                })
                .collect();
            // `[0x5B2828]`, the integrator's fixed dt; the collision grid gives the cars the ground
            // contact `Car_installHandlers` leaves them with (`car+0xE20`, §14.1)
            let grid = self.world.as_ref().map(|w| &w.collision);
            tr.integrate_on(t, 1.0 / 64.0, grid);
            self.road.traffic_spin.resize(tr.cars.len(), 0.0);
            for (sp, c) in self.road.traffic_spin.iter_mut().zip(tr.cars.iter()) {
                *sp = if c.active {
                    *sp + c.speed / 64.0 / 0.32
                } else {
                    0.0
                };
            }
        }
        v.step64(&self.specs);
        for o in self.road.opponents.iter_mut() {
            o.vehicle.step64(&o.specs);
            if self.tick % 32 == 0 {
                o.vehicle.renormalise();
            }
        }
        if self.tick % 32 == 0 {
            v.renormalise();
        }
        self.car_pos = v.position;
        self.car_rot = Quat::from_mat3(&v.orientation.transpose()).normalize();
        if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
            sp.update(a);
        }
        let mirror = self.mirror;
        // AI engine loops (`engine-audio.md` §5): two crossfaded samples plus a road loop per opponent.
        // Traffic cars are left silent: the original drives their loops from `car+0xDB4`, and the port's
        // kinematic traffic has no engine speed to drive them with.
        if let (Some(a), Some(ai)) = (&self.snd.audio, self.snd.ai_engines.as_mut()) {
            let cam = self.camera.position;
            // `MIRROR` negates the pan angle before the atan2 that produces it (0x4A704D, 0x4A78C1
            // and the other four sites of `modes.md` §2.1), so flip the right vector with the world
            let cam_right = (self.camera.target - cam)
                .normalize_or_zero()
                .cross(Vec3::Y)
                .normalize_or_zero()
                * if mirror { -1.0 } else { 1.0 };
            let level = self.snd.audio_settings.engine_level as f32 / 255.0;
            for (i, o) in self.road.opponents.iter().enumerate() {
                let set = self.road.opponent_sets.get(i).cloned().unwrap_or_default();
                if set.is_empty() {
                    continue;
                }
                let rel = o.vehicle.position - cam;
                let d = rel.length();
                let (cos, pan) = if d > 0.1 {
                    ((rel / d).dot(o.vehicle.forward()), (rel / d).dot(cam_right))
                } else {
                    (0.0, 0.0)
                };
                let frac = if o.specs.redline > 0.0 {
                    o.vehicle.rpm / o.specs.redline
                } else {
                    0.0
                };
                ai.update(
                    a,
                    i,
                    &set,
                    frac,
                    d,
                    cos,
                    pan,
                    o.vehicle.v_long(),
                    1.0,
                    self.snd.audio_settings.audio_quality,
                    64,
                    level,
                );
            }
            // traffic uses the same cheap path. Its engine speed is real: `0x408360` computes
            // `car+0xDB4 = carSpecs[0x60 + 4·gear] · speed` at 0x4084AD with the gear from
            // `Traffic_pickGear` 0x408130 — 1 below 2 m/s, otherwise 0 while the car has not been hit
            // (`car+0x104C == −1`), and the highest gear whose top speed is under the current speed
            // while it is recovering from one.
            if let (Some(tr), Some(specs)) = (&self.road.traffic, &self.road.traffic_specs) {
                let base = self.road.opponents.len();
                for (i, c) in tr.cars.iter().enumerate() {
                    let set = self.road.traffic_sets.get(i).cloned().unwrap_or_default();
                    if !c.active || set.is_empty() {
                        ai.stop(a, base + i);
                        continue;
                    }
                    let rel = c.position - cam;
                    let d = rel.length();
                    let (cos, pan) = if d > 0.1 {
                        (
                            (rel / d).dot(c.orientation.row(2) * c.dir),
                            (rel / d).dot(cam_right),
                        )
                    } else {
                        (0.0, 0.0)
                    };
                    let gear = if c.speed < 2.0 { 1 } else { 0 };
                    let rpm = specs.ratio_rpm_per_mps.get(gear).copied().unwrap_or(0.0) * c.speed;
                    let frac = if specs.redline > 0.0 {
                        rpm / specs.redline
                    } else {
                        0.0
                    };
                    ai.update(
                        a,
                        base + i,
                        &set,
                        frac,
                        d,
                        cos,
                        pan,
                        c.speed,
                        1.0,
                        self.snd.audio_settings.audio_quality,
                        64,
                        level,
                    );
                    // `traffic.md` §12.3: a car whose `scripts.bin` HONK_HORN action is running
                    // sounds its horn (`Honk(car, 1)` on mask 8). Traffic runs personality row 0,
                    // whose collision script is `HONK_HORN(2) END HONK_HORN(2) END` — so a car the
                    // player runs into leans on the horn.
                    let honking = c
                        .script
                        .query(nfs_formats::scripts::mask::HONK_HORN)
                        .is_some()
                        || c.script
                            .query(nfs_formats::scripts::mask::CRAZY_HORN)
                            .is_some();
                    ai.honk(a, base + i, honking, d, pan, level);
                }
            }
        }
        // track ambience: a quarter of the elements per update (0x4AEF30)
        if let (Some(a), Some(amb)) = (&self.snd.audio, self.snd.ambience.as_mut()) {
            let cam = self.camera.position;
            let fwd = (self.camera.target - cam).normalize_or_zero();
            let right = fwd.cross(Vec3::Y).normalize_or_zero();
            amb.update(a, self.tick, cam.into(), right.into(), fwd.into());
        }
        // end-of-stream advance through the playlist (0x4ADD6D)
        if let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut()) {
            m.update(a);
        }
        // tunnel reverb (`engine-audio.md` §4.7): the enclosure test every audio update, then the
        // level that snaps to 100 under a road deck and decays 100 → 68 → 36 → 8 on the way out
        if let Some(a) = &self.snd.audio {
            let in_tunnel = w.in_tunnel(v.position);
            if self.snd.reverb.update(in_tunnel, false, false).is_some() {
                a.set_reverb(self.snd.reverb.wet());
            }
        }
        if let (Some(a), Some(e)) = (&self.snd.audio, self.snd.engine_sound.as_mut()) {
            // view mode: the audio mode follows the view id the camera stands for
            // (`nfs_game::camera::audio_mode`, `engine-audio.md` §4.2) — 1 for the bumper view,
            // 2 for the chase views, 0 (the `.cct`/`.clt` cockpit layers) for the driver view and
            // 3 for the detached trackside camera.
            let mode = nfs_game::camera::audio_mode(
                view,
                !matches!(view, 0x0C | 0x14),
                view == nfs_game::camera::VIEW_DRIVER as u16,
            );
            let to_car = self.car_pos - self.camera.position;
            let dist = to_car.length();
            let cos = if dist > 0.1 {
                (to_car / dist).dot(v.forward())
            } else {
                0.0
            };
            // mirrored races negate the pan (see the AI engines above)
            let cam_right = (self.camera.target - self.camera.position)
                .normalize_or_zero()
                .cross(Vec3::Y)
                .normalize_or_zero()
                * if self.mirror { -1.0 } else { 1.0 };
            let pan = if dist > 0.1 {
                (to_car / dist).dot(cam_right)
            } else {
                0.0
            };
            e.update(
                a,
                nfs_audio::engine::EngineInput {
                    rpm: v.rpm.max(600.0),
                    redline: self.specs.redline,
                    throttle: v.throttle_raw,
                    brake: v.brake_raw,
                    reversing: v.gear == 0,
                    running: v.engine_alive != 0, // `car+0xDB0`: a heavy crash or a roll kills it until the reset
                    // `car+0x548`, the rev limiter's ignition-cut counter (`physics.md` §4.1) — the
                    // engine carries it, so the loud-load branch of `engine-audio.md` §3 gets the real
                    // thing rather than "at or past the redline"
                    rev_cut: v.ignition_cut > 0,
                    mode,
                    cos,
                    pan,
                    distance: dist,
                    // `.TINYCAR` doubles the pitch base it hands the engine queue (0x4AA8F9…0x4AA906),
                    // after the road and turbo layers have taken theirs
                    doppler: if self.cheat_bits & (1 << 1) != 0 { 2.0 } else { 1.0 },
                    detail: self.snd.audio_settings.audio_quality,
                    crowding: 0,
                    engine_level: self.snd.audio_settings.engine_level,
                    shift: (v.shift_timer > 0 && v.gear > self.snd.last_gear)
                        .then_some((v.shift_timer, self.specs.shift_ticks)),
                },
            );
            // the click fires on the last tick of the shift, not at the gear change (§4.3)
            if v.shift_timer == 1 && v.gear > self.snd.last_gear {
                e.play_shift(a, 0.6);
            }
        }
        self.snd.last_gear = v.gear;
        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
            s.update();
            // tyre squeal producer 0x40FBC0: per-axle slip sums, upright and near the ground
            let upright = v.orientation.row(1).y > 0.5;
            let front = if upright {
                v.wheels[0].slip + v.wheels[1].slip
            } else {
                0.0
            };
            let rear = if upright {
                v.wheels[2].slip + v.wheels[3].slip
            } else {
                0.0
            };
            let sfx_level = self.snd.audio_settings.sfx_level as f32 / 255.0;
            s.squeal(
                a,
                self.specs.class.min(2),
                v.wheels[0].surface,
                front,
                rear,
                sfx_level,
            );
            // road roll (§4.4), wind (§4.5) and the turbo layer (§4.6); the wind only plays from
            // the attached exterior view
            let cam_to_car = self.car_pos - self.camera.position;
            let dist = cam_to_car.length();
            let cam_right = (self.camera.target - self.camera.position)
                .normalize_or_zero()
                .cross(Vec3::Y)
                .normalize_or_zero();
            let pan = if dist > 0.1 {
                (cam_to_car / dist).dot(cam_right)
            } else {
                0.0
            };
            let gain = (2048 / (dist as i32 + 2)) * 127 / 128;
            s.road_roll(
                a,
                v.v_long(),
                v.wheels[0].surface,
                v.handbrake,
                gain,
                1.0,
                pan,
                sfx_level,
            );
            // the wind is heard from a camera riding the car in the open (`BumperCam`)
            s.wind(a, view == 0x00, v.v_long(), sfx_level);
            // §4.6: the boost state the physics now keeps (`car+0x54C`), at the aux base level of
            // `Engine_auxVolume(mode)` scaled by the engine gain
            if let Some(e) = &self.snd.engine_sound {
                let mode = nfs_game::camera::audio_mode(
                    view,
                    !matches!(view, 0x0C | 0x14),
                    view == nfs_game::camera::VIEW_DRIVER as u16,
                );
                s.turbo(
                    a,
                    v.boost,
                    v.throttle_raw,
                    v.rpm,
                    self.specs.redline,
                    e.aux_volume(mode),
                    e.gain(),
                    1.0,
                    pan,
                    sfx_level,
                );
            }
        }
        self.replay_tick();
    }

    /// The replay's per-tick housekeeping: save the recording when the race ends the way
    /// `Replay_saveDefault` 0x413FD0 does from the de-init chain, and leave the race one second
    /// after a played-back stream runs out (0x415CB3, `replay.md` §5.2).
    fn replay_tick(&mut self) {
        use nfs_game::replay::Mode;
        let Some(r) = self.replay.stream.as_ref() else {
            return;
        };
        match r.mode {
            Mode::Recording | Mode::Full => {
                let done = self.race.as_ref().map(|r| r.finished).unwrap_or(false)
                    || self.player_eliminated;
                if done && !self.replay.saved {
                    self.save_replay();
                }
            }
            Mode::Exhausted => {
                self.replay.end_ticks += 1;
                if self.replay.end_ticks >= nfs_game::replay::END_GRACE_TICKS {
                    self.replay.end_ticks = 0;
                    tracing::info!("replay finished");
                    if !self.leave_race() {
                        self.replay.stream = None;
                    }
                }
            }
            Mode::Playing => {}
        }
    }

    /// Write the port's replay file (`replay.md` §6.2 saves `replay.rpl` after every race; §14.5
    /// explains why this is the port's own format and not the original's 118 312-byte dump).
    fn save_replay(&mut self) {
        self.replay.saved = true;
        let Some(r) = self.replay.stream.as_mut() else {
            return;
        };
        if r.written() == 0 {
            return;
        }
        if r.mode == nfs_game::replay::Mode::Recording {
            r.final_tick = self.tick;
        }
        let file = nfs_game::replay::ReplayFile {
            setup: self.replay.setup.clone(),
            final_tick: r.final_tick,
            stream: r.stream().to_vec(),
        };
        let path = self.replay.path.clone();
        let bytes = file.write();
        let res = std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|_| std::fs::write(&path, bytes));
        // …and the original's own format beside it (`replay.md` §6.1 + §16): the registry snapshot
        // with the fields the port has values for, the final tick and the same stream
        let rpl = nfs_game::replay::original_rpl(&file.setup, file.final_tick, &file.stream);
        if let Err(e) = std::fs::write(path.with_extension("rpl"), rpl) {
            tracing::debug!(error = %e, "writing the original-format .rpl failed");
        }
        match res {
            Ok(()) => {
                tracing::info!(path = %path.display(), ticks = file.final_tick, bytes = r.written(), "replay saved")
            }
            Err(e) => tracing::warn!(error = %e, "replay save failed"),
        }
    }

    /// The room the previewed screen names in its `room=` (`RE/notes/frontend-3d.md` §1).
    fn fe_room_of(&self) -> Option<&'static nfs_game::room::Room> {
        let l = self.fe.layout_preview.as_ref()?;
        nfs_game::room::room(l.layout.screen().and_then(|s| s.get("room")).unwrap_or(""))
    }

    /// Load the room's models and the car that stands in it, keeping whatever is already loaded.
    fn load_fe_room(&mut self) {
        let Some(room) = self.fe_room_of() else {
            self.fe.room.clear();
            return;
        };
        // the car the screen's own `NFS5.3DCAR` names — with `positiontype` 0 the room owns it
        // the cars the screen's `NFS5.3DCAR` widgets name. `positiontype` 0 hands the car to the
        // room; 1, 2 and 3 put one of their own at a fixed spot (`RE/notes/frontend-3d.md` §4). A
        // source the port cannot resolve falls back to the car the front end has selected, as the
        // accessor answers elsewhere.
        // the model **and its body style** — `nfs5.car +0x1C0`, which is what picks the `[styleN]`
        // section (`car-model.md`, `applyStyle` 0x437B00)
        let fallback = self.fe.menu.as_ref().and_then(|m| m.car()).map(|c| (c.model.clone(), c.variant));
        let widgets = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        let mut wanted: Vec<(usize, String, i16, Vec<[i16; 3]>)> = Vec::new();
        for w in &widgets {
            let model = self
                .logo_car(w.accessor.as_deref(), w.othersource.as_deref())
                .map(|c| (c.model.clone(), c.variant, self.fe_car_swaps(w.accessor.as_deref(), c)))
                .or_else(|| fallback.clone().map(|(m, v)| (m, v, Vec::new())));
            if let Some((model, variant, swaps)) = model {
                wanted.push((
                    if w.position_type == 0 {
                        usize::MAX
                    } else {
                        w.index
                    },
                    model,
                    variant,
                    swaps,
                ));
            }
        }
        // a real room with no `NFS5.3DCAR` on the screen still stands the selected car in it; an
        // empty room (`ID_NONE`) without one shows nothing
        if wanted.is_empty()
            && !room.models.is_empty()
            && let Some((model, variant)) = fallback
        {
            wanted.push((usize::MAX, model, variant, Vec::new()));
        }
        if self
            .fe.room
            .iter()
            .map(|(n, _)| n.as_str())
            .ne(room.models.iter().copied())
        {
            self.fe.room.clear();
            let dir = self.paths.fe_data().join("Models");
            for name in room.models {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load_in(&dir, name)
                    .map_err(anyhow::Error::from)
                    .and_then(|a| CarModel::upload(r, &a, &self.paths, 1, [0xC0, 0xC0, 0xC0]));
                match loaded {
                    Ok(m) => self.fe.room.push((name.to_string(), m)),
                    Err(e) => {
                        tracing::warn!(room = room.id, model = name, error = %e, "front-end room model unavailable")
                    }
                }
            }
            tracing::info!(
                room = room.id,
                models = self.fe.room.len(),
                "front-end room loaded"
            );
        }
        // **The key is the model file, the body style and the paint — not the file alone.** 87
        // playable cars share **21** model files: a family is one `.crp` and its cars differ by
        // the `[styleN]` section `nfs5.car +0x1C0` picks (`applyStyle` 0x437B00) — `356A` is one
        // file for twelve cars and twelve styles, `964` one for eight. Keyed on the file alone,
        // stepping through a family never reloaded, so the showroom kept the first car's body
        // and paint while the name under it changed. Found in play, 2026-09-21.
        let paint = self.paint;
        let key: Vec<(usize, String, i16, [u8; 3], Vec<[i16; 3]>)> = wanted.iter().map(|(i, n, v, sw)| (*i, n.clone(), *v, paint, sw.clone())).collect();
        if self.fe.cars_key != key || self.fe.cars.len() != key.len() {
            self.fe.cars.clear();
            self.fe.cars_key = key;
            for (slot, model, variant, swaps) in wanted {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load(&self.paths, &model)
                    .map(|c| c.with_style(variant).with_style_overlay(swaps))
                    .map_err(anyhow::Error::from)
                    .and_then(|a| CarModel::upload(r, &a, &self.paths, 1, paint));
                match loaded {
                    Ok(m) => self.fe.cars.push((slot, model, variant, paint, m)),
                    Err(e) => tracing::warn!(model, error = %e, "front-end car model unavailable"),
                }
            }
        }
    }

    /// The 3D car widget's style overlay (0x4C0340…0x4C0486): the 120 dwords start at −1, then a
    /// car the accessor's slot 0x8C answers — a garage record — has its installed parts' swaps
    /// written in (`+0x758`, 0x4C0420…0x4C0486), and otherwise the catalogue record its slot 0x90
    /// answers has its **stock** parts' (`+0x3BC` / `+0x3BE`, 0x4C036A…0x4C0405, each through the
    /// part lookup 0x49C470).
    fn fe_car_swaps(&self, accessor: Option<&str>, car: &nfs_formats::fedata::CarCatalogEntry) -> Vec<[i16; 3]> {
        let Some(cu) = self.career.as_ref() else { return Vec::new() };
        let base = accessor.map(|a| a.split_once('(').map_or(a, |(b, _)| b));
        let ids: Vec<i16> = match base {
            Some("OwnedCars.Car") => cu.career.garage.get(cu.car_sel).map(|o| o.parts.clone()).unwrap_or_default(),
            Some("UsedCar.Car") => self.used_car().map(|o| o.parts.clone()).unwrap_or_default(),
            _ => car.stock_parts.clone(),
        };
        // 0x4D1318…0x4D133E copy the array over `carinfo+0x32C` with index 10 forced to −1
        ids.iter()
            .filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id))
            .flat_map(|p| p.geometry_swaps.iter().copied())
            .filter(|s| !(s[0] == 0 && s[1] == 10))
            .collect()
    }

    /// The car a `NFS5.3DCAR` widget shows: the one loaded for its own slot, or — for the widget
    /// with no `positiontype` of its own — the room's car (`usize::MAX`).
    fn fe_car_slot_of(&self, widget: usize) -> Option<usize> {
        let l = self.fe.layout_preview.as_ref()?;
        let w = l.car_3d().into_iter().find(|w| w.index == widget)?;
        let slot = if w.position().is_some() { widget } else { usize::MAX };
        self.fe.cars.iter().position(|c| c.0 == slot)
    }

    /// `RebuildReplayList` — `%s*.rpl` from the replay directory (0x4E5E1B, and again at
    /// 0x50D45A after a delete), by name.
    fn replay_names(&self) -> Vec<String> {
        let Some(dir) = self.replay.path.parent() else { return Vec::new() };
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("rpl")))
            .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect();
        names.sort();
        names
    }

    /// **`STARTREPLAY`** (slot 51, 0x50D390): `[0x699FC9] = 1` — "this is a replay playback" —
    /// then `PLAYGAME`. The launch (0x4D5372…0x4D5425) sends option 0x97 = 2 and 0x110 =
    /// `"%s.rpl"` over `Replay.List`'s current name, and the start check 0x4D5730 skips the
    /// game-type switch and the car checks for it (0x4D5786). Here: that file read, the stream set
    /// playing, the race set up from its snapshot, and the race started.
    fn start_replay(&mut self, event_loop: &ActiveEventLoop) {
        let names = self.replay_names();
        let Some(name) = names.get(self.fe.replay_sel.min(names.len().saturating_sub(1))).cloned() else { return };
        let Some(dir) = self.replay.path.parent().map(|d| d.to_path_buf()) else { return };
        let path = dir.join(format!("{name}.rpl"));
        let Some(f) = std::fs::read(&path).ok().and_then(|d| read_replay_file(&d)) else {
            tracing::warn!(path = %path.display(), "not a replay file");
            return;
        };
        let mut setup = f.setup.clone();
        resolve_replay_car(&self.paths, &mut setup);
        let (track, car) = match (TrackAssets::load(&self.paths, &setup.track), CarAssets::load(&self.paths, &setup.car), CarAssets::load_sim(&self.paths, if setup.sim.is_empty() { "default" } else { &setup.sim })) {
            (Ok(t), Ok(c), Ok(sim)) => (t, (c, sim)),
            (t, c, s) => {
                tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "replay load failed");
                return;
            }
        };
        tracing::info!(path = %path.display(), track = setup.track, car = setup.car, "front end: start replay");
        let race_class = setup.sim.to_lowercase().starts_with("gt") || setup.sim.starts_with("935");
        self.specs = CarSpecs::from_sim(&car.1, race_class);
        // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
        if self.cheat_bits & (1 << 1) != 0 {
            self.specs.apply_tinycar();
        }
        self.track = track;
        self.car = car.0;
        self.sim_name = setup.sim.clone();
        self.num_opponents = setup.opponents;
        self.num_laps = setup.laps.max(1);
        self.backwards = setup.backwards;
        self.mirror = setup.mirror;
        self.paint = setup.paint;
        self.road.traffic_count = TRAFFIC_COUNT[(setup.traffic as usize).min(3)];
        self.road.cops_enabled = setup.cops;
        self.road.cops_race = false;
        self.traffic_seed = setup.seed;
        self.career_opponents = None;
        self.replay.stream = Some(nfs_game::replay::Replay::playing(f.stream, f.final_tick));
        self.replay.setup = setup;
        // the menu comes back when the playback ends (`leave_race`)
        self.fe.menu_template = self.fe.menu.take();
        self.fe.after_replay = true;
        self.fe.layout_preview = None;
        self.fe.layout_name = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return };
        self.tick = 0;
        self.start_race(renderer, window, event_loop);
    }

    /// **0x50D3C0**, the "yes" to 808: `"%s%s.rpl"` over the replay directory and `Replay.List`'s
    /// current name (0x50D402), removed when it exists (0x50D412…0x50D425), then the list rebuilt.
    /// The directory is this port's own save directory, never the game's.
    fn kill_selected_replay(&mut self) {
        let names = self.replay_names();
        let Some(name) = names.get(self.fe.replay_sel.min(names.len().saturating_sub(1))) else { return };
        if let Some(dir) = self.replay.path.parent() {
            let path = dir.join(format!("{name}.rpl"));
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "replay deleted"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "replay delete failed"),
            }
        }
        let n = self.replay_names().len();
        self.fe.replay_sel = self.fe.replay_sel.min(n.saturating_sub(1));
        self.bind_lists();
    }

    /// Raise a dialog (slot 13, 0x5128A0) with its text from `FEStrings` — unless the one for the
    /// same purpose is still up, which is 0x50CC1B's "gone" test.
    fn fe_raise_dialog(&mut self, kind: nfs_game::dialog::DialogKind, text_id: u32, labels: [u32; 2], purpose: DialogFor) {
        if self.fe.dialog.as_ref().is_some_and(|(d, p)| *p == purpose && !d.gone()) {
            return;
        }
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let text = self.fe.strings.get(text_id).unwrap_or_default().to_string();
        let mut d = nfs_game::dialog::Dialog::new(kind, text, labels);
        d.raise(now);
        self.fe.dialog = Some((d, purpose));
    }

    /// Act on a dialog's answer (the callers' polls of slots 18/19). Returns whether the front
    /// end is leaving.
    fn fe_dialog_answer(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.fe.dialog_answer.take() {
            // 0x50CC4F: "yes" to 811 is 0x4D56E0(0) — the front end is over
            Some((true, DialogFor::Quit)) => {
                event_loop.exit();
                true
            }
            Some((yes, DialogFor::Offer(i))) => {
                if let Some(c) = self.career.as_mut() {
                    if yes {
                        let _ = c.career.accept_offer(i);
                        c.car_sel = 0;
                    } else {
                        c.career.decline_offer(i);
                    }
                    c.save();
                }
                false
            }
            Some((true, DialogFor::Forfeit)) => {
                if let Some(c) = self.career.as_mut()
                    && let Some(t) = c.current().cloned()
                {
                    c.career.forfeit(t.id);
                    c.save();
                }
                self.menu_key(MenuKey::Back, event_loop);
                false
            }
            Some((true, DialogFor::KillReplay)) => {
                self.kill_selected_replay();
                false
            }
            // 0x5103DA…0x51042F: delete the selected stem; the list is rebuilt (0x4DEF60) when it
            // went, 804 says it did not
            Some((true, DialogFor::RemovePlayer)) => {
                let profiles = nfs_game::save::list_profiles(&nfs_game::save::save_dir());
                if let Some(p) = self.user_row(&profiles).and_then(|i| profiles.get(i)) {
                    match nfs_game::save::delete_profile(&nfs_game::save::save_dir(), p) {
                        Ok(()) => {
                            self.fe.user_sel = None;
                            self.bind_lists();
                        }
                        Err(e) => {
                            tracing::info!(error = %e, "profile not deleted");
                            self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 804, [0, 0], DialogFor::Notice);
                        }
                    }
                }
                self.bind_player_buttons();
                false
            }
            Some((true, DialogFor::TournamentReset)) => {
                if let Some(c) = self.career.as_mut() {
                    c.confirm_reset();
                }
                self.menu_key(MenuKey::Enter, event_loop);
                self.fe.menu.is_none()
            }
            _ => false,
        }
    }

    /// A dialog the career screen asked for (`CareerUi::dialog`, §4.147): 858 two-button with
    /// its reset answer, the rest one-button notices — 802, 809, 810, 813, 857.
    fn fe_career_dialog(&mut self) {
        if self.fe.dialog.is_some() {
            return;
        }
        if let Some((id, two)) = self.career.as_mut().and_then(|c| c.dialog.take()) {
            use nfs_game::dialog::DialogKind;
            let (kind, purpose) = if two { (DialogKind::Two, DialogFor::TournamentReset) } else { (DialogKind::One, DialogFor::Notice) };
            self.fe_raise_dialog(kind, id, [0, 0], purpose);
        }
    }

    /// The pre-race screen's tick (0x505E58…0x505ED3): with no offer dialog up and a garage car
    /// holding an offer, ask about it — `FEStrings` 807 over the offer, the car's name and its
    /// asking price — with yes / no.
    fn fe_offer_check(&mut self) {
        let on_career = self.fe.layout_name.as_deref() == Some("prerace")
            || self.fe.menu.as_ref().is_some_and(|m| (1..=3).contains(&m.mode));
        if !on_career || self.fe.dialog.is_some() {
            return;
        }
        let Some(ui) = self.career.as_ref() else { return };
        let Some(i) = ui.career.pending_offer() else { return };
        let car = &ui.career.garage[i];
        let name = ui.tables.car(car.car_id).map(|c| c.display_name.clone()).unwrap_or_default();
        let fmt = self.fe.strings.get(807).unwrap_or_default();
        // "%d … %s … %d": the offer, the name, the asking price (0x505E95…0x505EB1)
        let text = fmt
            .replacen("%d", &car.offer.unwrap_or(0).to_string(), 1)
            .replacen("%s", &name, 1)
            .replacen("%d", &car.asking_price.unwrap_or(0).to_string(), 1);
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::Two, text, [700, 701]);
        d.raise(now);
        self.fe.dialog = Some((d, DialogFor::Offer(i)));
    }

    /// One frame of the dialog: the tick, the answer kept for `fe_dialog_answer`, the screens
    /// covered while it is up.
    fn fe_dialog_step(&mut self) {
        // `NFS_FE_DIALOG=<FEStrings id>` raises a two-button dialog with that text at tick 0, for
        // a still — the way `NFS_FE_PARTS` opens the car's parts
        if self.fe.dialog.is_none()
            && !self.fe.dialog_forced
            && let Some(id) = std::env::var("NFS_FE_DIALOG").ok().and_then(|v| v.parse::<u32>().ok())
        {
            self.fe.dialog_forced = true;
            let text = self.fe.strings.get(id).unwrap_or_default().to_string();
            let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::Two, text, [700, 701]);
            d.raise(0);
            self.fe.dialog = Some((d, DialogFor::Quit));
        }
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut answer = None;
        let mut up = false;
        if let Some((d, purpose)) = self.fe.dialog.as_mut() {
            d.tick(now);
            if let Some(a) = d.take_answer() {
                answer = Some((a, *purpose));
            }
            up = d.up;
            if d.gone() {
                self.fe.dialog = None;
            }
        }
        // a 3D car zoomed into a door covers every screen each tick too (0x4C052F → 0x4D2000),
        // and lets them go through 0x4D20F0 (= 0x4D1E70) when it zooms back out
        let zoomed = self.fe.layout_preview.as_ref().is_some_and(|l| l.car_3d().iter().any(|w| l.car_zoom(w.index).1));
        let up = up || zoomed;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_covered(up);
        }
        if let Some(b) = self.fe.layout_bottom.as_mut() {
            b.set_covered(up);
        }
        if answer.is_some() {
            self.fe.dialog_answer = answer;
        }
    }

    /// A release on the bottom bar — `NFS5.BOTTOMBARBUTTON`'s slot 9 is `NFS5.GOTOBUTTON`'s
    /// **0x4C9470**: unless disabled, sound 5, then the callback (0x50D6D0) and the destination.
    /// `QUIT` raises the exit dialog; `GOBACK` and `PLAYGAME` are the port's own back and race
    /// paths; `OPTIONS`' handler is not read and is logged.
    fn fe_bottom_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(b) = self.fe.layout_bottom.as_ref() else { return false };
        let Some(hit) = b.hovered() else { return false };
        if b.widget_disabled(hit) {
            return true;
        }
        let Some(c) = b.controls().into_iter().find(|c| c.index == hit) else { return false };
        self.fe_sound(5);
        match c.callback.as_deref() {
            Some("QUIT") => self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 811, [0, 0], DialogFor::Quit),
            Some("GOBACK") => self.menu_key(MenuKey::Back, event_loop),
            Some("PLAYGAME") => {
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.can_play()) {
                    self.start_from_layout(event_loop);
                }
            }
            other => tracing::info!(callback = ?other, "bottom bar callback not ported"),
        }
        true
    }

    /// **0x4AADB0(`slot`)**, the front end's one-shot: `fesfx.bnk`'s slot at the level
    /// `nfs_audio::fesfx::level` gives, silent at sound detail 3. The output opens and the bank
    /// loads on the first call; a screenshot run plays nothing.
    fn fe_sound(&mut self, slot: usize) {
        if self.screenshot.is_some() {
            return;
        }
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        let q = self.snd.audio_settings.audio_quality;
        if self.snd.fe_sfx.is_none() {
            self.snd.fe_sfx = nfs_audio::fesfx::FeSfx::load(&self.paths.game_data().join("Sounds"), q)
                .map_err(|e| tracing::warn!(error = %e, "front-end sounds unavailable"))
                .ok();
        }
        if let (Some(out), Some(fx)) = (self.snd.audio.as_ref(), self.snd.fe_sfx.as_mut()) {
            fx.play(out, slot, self.snd.audio_settings.sfx_level, q);
        }
    }

    /// `carinfo+0xC` for the player's car: `Car.Transmission` at index 1, 561 "automatic"
    /// (`fe.txt`'s `car[0].transmission = 1`); 0, 560 "manual", otherwise.
    ///
    /// A Factory Driver mission overrides it: 0x4EB9B7…0x4EB9D7 set the per-car routine's seventh
    /// argument when the current mission (0x51AEC0, the first not yet passed) is index **0x1D**,
    /// and 0x4EEF8D then sends `.TRANSMISSION` 0 — a manual gearbox — whatever the screen says.
    fn player_automatic(&self) -> bool {
        if self.mission.as_ref().is_some_and(|m| m.index == 0x1D) {
            return false;
        }
        self.choice_index("Car.Transmission").is_none_or(|(i, _)| i == 1)
    }

    /// A click's part events: the lids go to the car's own easers (0x439A00…0x439BD0), the sounds
    /// through 0x4AADB0.
    fn fe_car_part_events(&mut self, widget: usize, events: &[nfs_game::carparts::PartEvent]) {
        use nfs_game::carparts::PartEvent;
        for e in events {
            match *e {
                PartEvent::Hood(open) => {
                    if let Some(k) = self.fe_car_slot_of(widget) {
                        self.fe.cars[k].4.set_lid(true, open);
                    }
                }
                PartEvent::Trunk(open) => {
                    if let Some(k) = self.fe_car_slot_of(widget) {
                        self.fe.cars[k].4.set_lid(false, open);
                    }
                }
                PartEvent::Sfx(n) => self.fe_sound(n as usize),
                // the roof click's `Car.Roof` write, index 1 "down" / 0 "up" (0x4C0F9E / 0x4C0FA7)
                PartEvent::Roof(down) => {
                    self.choices.insert("Car.Roof".to_string(), usize::from(down));
                }
            }
        }
    }

    /// One frame of the parts: the step (0x4C0696…0x4C07CF), then each widget's doors onto its
    /// car's channels 6 and 7 and the lids' easers onto 8 and 9.
    fn fe_car_parts_step(&mut self) {
        // `NFS_FE_PARTS=1,3` clicks those zones of every 3D car at tick 0, once, so a still taken
        // with `NFS_FE_CLOCK` shows them open — the way `NFS_FE_DZ` stands in for a dragged bar
        if !self.fe.parts_forced
            && let Ok(v) = std::env::var("NFS_FE_PARTS")
        {
            self.fe.parts_forced = true;
            let widgets: Vec<usize> = self.fe.layout_preview.as_ref().map(|l| l.car_3d().iter().map(|w| w.index).collect()).unwrap_or_default();
            for w in widgets {
                for z in v.split(',').filter_map(|z| z.trim().parse::<u32>().ok()) {
                    let ev = self.fe.layout_preview.as_mut().map(|l| l.car_click_zone(w, z, 0)).unwrap_or_default();
                    self.fe_car_part_events(w, &ev);
                    if let Some(k) = self.fe_car_slot_of(w) {
                        self.fe.cars[k].4.update_lids(1000);
                    }
                }
            }
        }
        // the widget's tick first looks at `Car.Roof` (0x4C05E7…0x4C063F) — and the roof zone is
        // there only when the car's roof animation has a length (0x438190 at 0x4C0D10)
        let roof_down = self.choice_index("Car.Roof").is_some_and(|(i, _)| i == 1);
        let widgets: Vec<usize> = self.fe.layout_preview.as_ref().map(|l| l.car_3d().iter().map(|w| w.index).collect()).unwrap_or_default();
        for w in widgets {
            let has_roof = self.fe_car_slot_of(w).is_some_and(|k| self.fe.cars[k].4.anim_length(1) > 0);
            let ev = match self.fe.layout_preview.as_mut() {
                Some(l) => {
                    l.set_car_roof(w, has_roof);
                    l.sync_car_roof(w, roof_down)
                }
                None => Vec::new(),
            };
            self.fe_car_part_events(w, &ev);
        }
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        // the step's closing sounds, 0x31 / 0x33 / 0x35 (0x4C0778…0x4C07A6)
        let closing = l.step_car_parts();
        for e in closing {
            if let nfs_game::carparts::PartEvent::Sfx(n) = e {
                self.fe_sound(n as usize);
            }
        }
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        let widgets: Vec<usize> = l.car_3d().iter().map(|w| w.index).collect();
        // `data.damage` (0x4BFCEE → `[+0x1AD]`): every tick the widget hands the car its ten zone
        // damages — the owned car's `FECar+0x6A8` floats when the key is set and the accessor has
        // a car, 0 otherwise (0x4C0662…0x4C068B, through 0x4D1A00 into `car+0x4E4 + 4z`)
        let damages: Vec<(usize, [f32; 10])> = l.car_3d().iter().map(|w| (w.index, if w.damage { self.fe_car_damage(w.accessor.as_deref()) } else { [0.0; 10] })).collect();
        for (w, d) in damages {
            if let Some(k) = self.fe_car_slot_of(w)
                && let Some(r) = self.renderer.as_mut()
            {
                self.fe.cars[k].4.update_damage(r, &d);
            }
        }
        let dt = (self.frame_dt * 1000.0).round().clamp(0.0, 250.0) as u32;
        for w in widgets {
            let doors = self.fe.layout_preview.as_ref().map(|l| l.car_doors(w)).unwrap_or((0.0, 0.0));
            let roof = self.fe.layout_preview.as_ref().map(|l| l.car_roof_state(w)).unwrap_or(0);
            if let Some(k) = self.fe_car_slot_of(w) {
                let car = &mut self.fe.cars[k].4;
                car.set_channel(6, doors.0);
                car.set_channel(7, doors.1);
                // 0x4C07E8: after the step, the roof animation follows zone 0's state
                if let Some(r) = self.renderer.as_mut() {
                    car.set_roof(r, roof);
                }
                // 0x438751: channel 16 follows the roof in the front end
                car.follow_roof();
            }
        }
        for c in self.fe.cars.iter_mut() {
            c.4.update_lids(dt);
        }
    }

    /// The room's meshes and the car turning in it, plus the camera and lighting the room's own rig
    /// gives them (`RE/notes/frontend-3d.md` §2, §3).
    fn fe_room_items(&mut self) {
        let Some(room) = self.fe_room_of() else {
            return;
        };
        // the screen's transition progress, 0…1 — the light block's fade (0x4E84DE)
        let fade = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.transition().progress() as f32 / nfs_game::layout::TRANS_FULL as f32)
            .unwrap_or(0.0);
        // **The room moves toward the camera when a door is zoomed into**, not with the screen:
        // `[room+0x2E8]` is written only by its slot 0xB8 (0x4E7800), which only the room car's
        // widget calls, each tick, with its zoom (0x4C05E1)
        let zoom = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d().iter().filter(|w| w.position().is_none()).map(|w| l.car_zoom(w.index).0).fold(0.0f32, f32::max))
            .unwrap_or(0.0);
        let p = room.zoomed_pos(zoom);
        let pos = Vec3::new(p[0], p[1], p[2]);
        let turn = |t: f32| {
            let rot = Quat::from_rotation_y((t + room.rot[1]) * std::f32::consts::TAU)
                * Quat::from_rotation_x(room.rot[0] * std::f32::consts::TAU)
                * Quat::from_rotation_z(room.rot[2] * std::f32::consts::TAU);
            Mat4::from_rotation_translation(rot, pos)
        };
        // the parts a click opened (`nfs_game::carparts`): the step, then the doors onto channels
        // 6/7 and the lids' easers onto 8/9 of the car each `NFS5.3DCAR` shows
        self.fe_car_parts_step();
        let spin = room.spin_turns(self.fe.ms);
        let mut items = Vec::new();
        let room_m = turn(room.models_turn.unwrap_or(spin));
        // **The light block** 0x4E84C0 fills for each model (`RE/notes/lighting.md` §6): the
        // room's own models (0x4EA07A, flag 0) take the rig's ambient and sun, the car
        // (0x4EA08A, flag 1) a fixed 0.3 / 0.7 grey from (0, −0.707, 0.707); both faded by
        // `1 − progress / 512`. The direction is carried into this port's frame the way the models
        // are, through `MODEL_TO_BODY`, so a surface meets the light at the angle it does there.
        let light_fade = 1.0 - fade;
        let sun_dir = nfs_game::world::MODEL_TO_BODY.transform_vector3(Vec3::from(room.sun_dir));
        let room_light = nfs_game::world::LightBlock::room(room.ambient, room.sun, sun_dir, light_fade, false);
        let mut car_light = nfs_game::world::LightBlock::room(room.ambient, room.sun, sun_dir, light_fade, true);
        car_light.direction = nfs_game::world::MODEL_TO_BODY.transform_vector3(car_light.direction);
        for (_, model) in &self.fe.room {
            items.extend(model.draw_items_block(room_m, [0.0; 4], [0.0; 2], &room_light));
        }
        let widgets = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        // the room's own car goes in with the room; a widget with a `positiontype` of its own is
        // drawn into an offscreen view instead, so it lands in that widget's place in the screen's
        // drawing order rather than behind all of it
        let mut views: Vec<(usize, Vec<nfs_renderer::DrawItem>)> = Vec::new();
        for (slot, _, _, _, car) in &self.fe.cars {
            match widgets
                .iter()
                .find(|w| w.index == *slot)
                .and_then(|w| w.position())
            {
                Some(p) => {
                    // the table at 0x4BFF28 is kept exactly as it is in the binary, and its `x` is
                    // **mirrored here**: `carcompare.lay` gives the widget at `pos=175` (the left
                    // half of the 640-wide screen) `positiontype=1`, which the table places at
                    // `x = +2.4`, and the one at `pos=350` `positiontype=2` at `x = -2.4`. So in
                    // the original's front-end camera space +x is toward screen *left*, and this
                    // port's camera has +x to the right. CONFIRMED by the two widgets' own boxes.
                    let m = Mat4::from_rotation_translation(
                        Quat::from_rotation_y(spin * std::f32::consts::TAU),
                        Vec3::new(-p[0], p[1], p[2]),
                    );
                    views.push((*slot, car.draw_items_block(m, [0.0; 4], [0.0; 2], &car_light)));
                }
                None => items.extend(car.draw_items_block(turn(spin), [0.0; 4], [0.0; 2], &car_light)),
            }
        }
        self.scene.items = items;
        self.scene.headlight = None;
        self.scene.ambient = room.ambient.iter().sum::<f32>() / (3.0 * 255.0);
        self.scene.sun_direction =
            -Vec3::new(room.sun_dir[0], room.sun_dir[1], room.sun_dir[2]).normalize();
        // the camera sits at the origin looking down +z — the rooms are placed in front of it —
        // with the rig's own vertical angle
        self.camera.position = Vec3::ZERO;
        self.camera.target = Vec3::Z;
        self.camera.up = Vec3::Y;
        // `V.A.` is the half-angle: with the full angle the two `carcompare` cars land far outside
        // the rectangles their widgets carry, and with twice it they land inside them
        // (`RE/notes/frontend-3d.md` §2, HIGH).
        self.camera.fov_y_radians = (room.view_angle * 2.0).to_radians();
        self.camera.near = 0.05;
        self.camera.far = 500.0;
        if views.is_empty() {
            return;
        }
        let (w, h) = self
            .renderer
            .as_ref()
            .map(|r| r.size())
            .unwrap_or((1280, 800));
        let (camera, ambient, sun) = (self.camera, self.scene.ambient, self.scene.sun_direction);
        let mut rendered: Vec<(usize, nfs_renderer::TextureId)> = Vec::new();
        for (slot, items) in views {
            let reuse = self.fe.layout_preview.as_ref().and_then(|l| l.car_view(slot));
            let scene = Scene {
                items,
                ambient,
                sun_direction: sun,
                ..Scene::default()
            };
            let Some(r) = self.renderer.as_mut() else {
                return;
            };
            rendered.push((slot, r.render_offscreen(&scene, &camera, w, h, reuse)));
        }
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (slot, tex) in rendered {
                l.set_car_view(slot, Some(tex));
            }
        }
    }

    /// The front end's own sprites: the previewed `.lay` screen over its background, the bottom bar
    /// and the port's key hint, or the text menu when no layout is loaded.
    fn fe_sprites(&self) -> Vec<Sprite> {
        match &self.fe.layout_preview {
            Some(l) => {
                let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                let controls = l.controls();
                let sel = controls
                    .get(self.fe.layout_sel.min(controls.len().saturating_sub(1)))
                    .map(|c| c.index);
                let mut v = l.letterbox_bars(aspect);
                v.extend(l.sprites(aspect, &self.fe.strings, sel, &|a| self.accessor_value(a)));
                // the bottom bar the front end draws on every screen (0x4D39D3)
                if let Some(b) = &self.fe.layout_bottom {
                    v.extend(
                        b.sprites(aspect, &self.fe.strings, None, &|a| self.accessor_value(a)),
                    );
                }
                // a dialog over everything (§4.147)
                if let Some((d, _)) = &self.fe.dialog {
                    v.extend(l.dialog_sprites(d, aspect, &self.fe.strings, self.fe.mouse_down));
                }
                if let Some(f) = self.fe.menu.as_ref().and_then(|m| m.font.as_ref()) {
                    let name = self.fe.layout_name.clone().unwrap_or_default();
                    let hint = if l.can_play() {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER RACE   ESC BACK"
                    } else {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER FOLLOW   ESC BACK"
                    };
                    // above the bottom bar, not on it: the bar's four buttons occupy the bottom
                    // 24 of the 480 design rows (`bottombar.lay` section 7), and since the draw
                    // order was corrected (§4.67) they are actually drawn there. This line is the
                    // port's own key hint, not the original's.
                    v.extend(f.sprites(
                        &format!("{}   {hint}", name.to_uppercase()),
                        [0.02, 0.915],
                        0.025,
                        [0.6, 0.6, 0.6, 0.9],
                    ));
                }
                v
            }
            None => {
                let mut v = self.fe.menu.as_ref().map(|m| m.sprites(self.career.as_ref())).unwrap_or_default();
                // a dialog over the port's own text menu, painted by the bottom bar's screen —
                // the dialog belongs to no screen of its own (§4.147)
                if let (Some((d, _)), Some(b)) = (&self.fe.dialog, &self.fe.layout_bottom) {
                    let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                    v.extend(b.dialog_sprites(d, aspect, &self.fe.strings, self.fe.mouse_down));
                }
                v
            }
        }
    }

    /// Re-bind the cars' environment map when the `.map` section under the player changes
    /// (`RE/notes/lighting.md` §3): inside a section the car reflects that section's map, in its
    /// covered variant while something drivable hangs over the car, and the track's default map
    /// everywhere else.
    /// Repaint the car's texture pages when its body damage changes (`RE/notes/car-model.md` §11):
    /// the `<model>D.fsh` / `<model>L.fsh` decals are blended over the pages with an alpha that
    /// follows each zone's damage. `CarModel::update_damage` does nothing until a zone moves by a
    /// whole per cent, so this is cheap to call every frame.
    fn update_damage_decals(&mut self) {
        let Some(mut damage) = self.vehicle.as_ref().map(|v| v.damage) else {
            return;
        };
        // diagnostic: `NFS_DAMAGE=<per cent>` puts that much damage on every zone
        if let Ok(d) = std::env::var("NFS_DAMAGE").map(|v| v.parse::<f32>().unwrap_or(0.0)) {
            damage = [d; 10];
        }
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_damage(r, &damage);
        }
        // slot 9's race animations (the wing off the speed gate, a tiny car's aerial) and slot 6's
        // pop-up headlights off the track's night flag (`car-model.md`)
        let speed = self.vehicle.as_ref().map(|v| v.speed()).unwrap_or(0.0);
        let reverse = self.vehicle.as_ref().is_some_and(|v| v.gear == 0);
        let night = self.track.is_night();
        let tiny = self.cheat_bits & (1 << 1) != 0;
        // slot 9's driver: `[0x6197C4]` is `CARDETAIL`, the `Graphics.CarDetail` choice (0x44EF80)
        let detail = self.choice_index("Graphics.CarDetail").map_or(0, |(i, _)| i as u8);
        let driver = self.vehicle.as_ref().map(|v| nfs_game::world::DriverInputs {
            detail,
            // channels 1 and 2's `.y`: each front wheel's `+0xB4` over 216, negated when mirrored
            wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / 216.0 * if self.mirror { -1.0 } else { 1.0 },
            shift_timer: v.shift_timer,
            racing: v.race_state == 0 && !self.paused,
            handbrake: v.handbrake,
            reverse: v.gear == 0,
            long_acc: v.long_filtered,
            lat_acc: v.lateral_filtered,
        });
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_animation(r, 16, speed, night, tiny, reverse);
            if let Some(d) = driver {
                c.update_driver(r, 16, &d);
            }
            // the shared traffic and police models take slot 9's clock channel (the siren)
            for m in self.road.police_models.iter_mut().chain(self.road.traffic_models.iter_mut()) {
                m.update_clock_channels(16);
            }
            // slot 9 runs for every car's model: the opponents' own too
            let k = if self.mirror { -1.0 } else { 1.0 };
            // 0x43846F: with `[0x657424] >= 2` (a replay) the steer channels take the smoothed
            // angle `car+0xAC8` / `+0xACC` instead of the wheels' own
            let replaying = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            let wheel_steer = |v: &nfs_physics::vehicle::Vehicle| if replaying { v.steer_smoothed } else { [v.wheels[0].steer, v.wheels[1].steer] };
            for o in &self.road.opponents {
                let Some(m) = self.road.opponent_models.get_mut(o.model) else { continue };
                let v = &o.vehicle;
                m.update_animation(r, 16, v.speed(), night, tiny, v.gear == 0);
                m.update_driver(
                    r,
                    16,
                    &nfs_game::world::DriverInputs {
                        detail,
                        wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / 216.0 * k,
                        shift_timer: v.shift_timer,
                        racing: v.race_state == 0 && !self.paused,
                        handbrake: v.handbrake,
                        reverse: v.gear == 0,
                        long_acc: v.long_filtered,
                        lat_acc: v.lateral_filtered,
                    },
                );
                let ws = wheel_steer(v);
                m.update_race_channels(
                    16,
                    [ws[0] / 216.0 * k, ws[1] / 216.0 * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    v.rpm,
                    v.speed(),
                );
            }
            if let Some(v) = self.vehicle.as_ref() {
                let ws = wheel_steer(v);
                c.update_race_channels(
                    16,
                    [ws[0] / 216.0 * k, ws[1] / 216.0 * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    v.rpm,
                    v.speed(),
                );
            }
        }
    }

    fn update_env_map(&mut self) {
        let Some(w) = self.world.as_ref() else { return };
        let slice = self.race.as_ref().map(|r| r.slice as u32).unwrap_or(0);
        let covered = w.in_tunnel(self.car_pos);
        let mut layers = w.env_layers_for(slice, covered);
        // outside every section the track's own default entry stands in, as it always has
        layers[0] = layers[0].or(w.env_map);
        if layers == self.env_bound {
            return;
        }
        self.env_bound = layers;
        if let Some(r) = self.renderer.as_mut() {
            r.set_env_textures(layers);
        }
    }

    /// The four corners of a car's shadow, world-space and draped on the ground — `R3DCar`'s
    /// slot 22, **0x43B220**, with the patch slot 23 (0x43B5B0) lays over them.
    ///
    /// 0x43B231…0x43B2F3 build the corners in body space out of nothing but the half extents at
    /// `car+0x3A8` and `car+0x3B0` — `(−ex, 0, −ez)`, `(+ex, 0, −ez)`, `(+ex, 0, +ez)`,
    /// `(−ex, 0, +ez)`, with no scale factor of any kind — and 0x532A80 turns them by the car's
    /// full basis `car+0x364` and moves them to `car+0x330`. The byte table at **0x5CD5E8** takes
    /// them in the order **3, 2, 0, 1** with the UVs at 0x5CD5F0, `(0,0) (1,0) (1,1) (0,1)`, and
    /// each corner's **y** is the ground under it: 0x43B6CF starts that query from the car's own
    /// `y` plus **`[0x5B36E8]` = 1.75**.
    ///
    /// APPROXIMATION, still: the original subdivides the quad into an n×n patch with
    /// `n = 6 − (int)(dist² × 0.01)`, floored at 1 and forced to 6 in a replay
    /// (0x43B8EE…0x43B91A), sampling every vertex of it; this draws the n = 1 case, four corners.
    /// The `0.04` lift is this port's own, against z-fighting.
    fn shadow_quad(&self, pos: Vec3, rot: Quat, half: Vec3) -> Option<[nfs_renderer::Vertex; 4]> {
        let w = self.world.as_ref()?;
        let body = [
            Vec3::new(-half.x, 0.0, -half.z),
            Vec3::new(half.x, 0.0, -half.z),
            Vec3::new(half.x, 0.0, half.z),
            Vec3::new(-half.x, 0.0, half.z),
        ];
        const ORDER: [usize; 4] = [3, 2, 0, 1];
        const UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut out = [nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] }; 4];
        for (k, &i) in ORDER.iter().enumerate() {
            let p = pos + rot * body[i];
            let h = w.collision.ground(p.x, pos.y + 1.75, p.z, 12.0)?;
            out[k].position = [p.x, h.point.y + 0.04, p.z];
            out[k].uv = UV[k];
        }
        Some(out)
    }

    /// The glare a car's lamps throw towards the camera (`RE/notes/particles.md` §4): each light
    /// kind's first sprite is `CARG` in every row of the table at 0x5CD73C, and 0x43D173 works the
    /// strength out as `min(−dot(lamp facing, view), 1)`, cut off below 0.1 and scaled by the lamp's
    /// own intensity (`[car+0x2D8]`, 1.0 for an undimmed lamp). APPROXIMATION: the sprite's size and
    /// the second, kind-dependent flare (`SPIK` / `GLNT`) are not read.
    fn glare_items(
        &self,
        out: &mut Vec<nfs_renderer::DrawItem>,
        model: &CarModel,
        pos: Vec3,
        rot: Quat,
        right: Vec3,
        up: Vec3,
    ) {
        let Some((mesh, material)) = self.glare_draw else {
            return;
        };
        for l in &model.lights {
            let world = pos + rot * l.position;
            let to_cam = self.camera.position - world;
            let d = to_cam.length();
            if d < 1.0 || d > 120.0 {
                continue;
            }
            let facing = rot * (Vec3::Z * l.facing);
            let b = facing.dot(to_cam / d).max(0.0);
            if b <= 0.1 {
                continue;
            }
            // APPROXIMATION: the size and the lamp's colour. A tail light glows red, a lamp facing
            // forward white — the two roles `car-model.md` §3 gives the articles.
            let size = 0.25 + 0.45 * b;
            let tint = if l.facing < 0.0 {
                [1.0, 0.25, 0.15]
            } else {
                [1.0, 0.95, 0.85]
            };
            let m = Mat4::from_cols(
                (right * size).extend(0.0),
                (up * size).extend(0.0),
                right.cross(up).extend(0.0),
                world.extend(1.0),
            );
            out.push(nfs_renderer::DrawItem {
                light: tint,
                alpha: b * 0.7,
                ..nfs_renderer::DrawItem::new(mesh, material, m)
            });
        }
    }

    /// The shadow projector a car at `pos` takes, 0x43A6F1 → 0x456160 (`crp-geometry.md` §8).
    /// APPROXIMATION: `[0x6197CA]`, the detail option that gates it, is taken as set, and every
    /// projector of the track is a candidate (the original's register as their articles draw).
    fn projector_planes(&self, pos: Vec3) -> Option<([f32; 4], [f32; 4])> {
        self.world.as_ref().and_then(|w| w.projector_for(pos)).map(|q| (q.u, q.v))
    }

    /// The view the camera is on, by the original's ids (`nfs_game::camera`).
    fn camera_view(&self) -> u16 {
        if let Some(v) = self.replay_view() {
            return v;
        }
        self.camera_cycle
            .get(self.camera_mode as usize)
            .copied()
            .unwrap_or(0x02)
    }

    /// The view a **playing** replay is on — its own nine-entry cycle (`replay::CAMERA_IDS`, the
    /// table 0x5CC654), not the car's camera list. 0x414953 applies the entry: `0x14` is the
    /// track-side "TV" camera only while the track has camera nodes and **id 3** (`TailCam1`) when
    /// it has none, and every other id goes through as it is.
    ///
    /// APPROXIMATION: `0x0A CircleCam` and `0x0B SplineCam` have no motion of their own here —
    /// their entries in `VIEW_OFFSETS` are zero and what moves them is not read — so on those two
    /// the scene falls into the trackside branch and shows the nearest camera element.
    fn replay_view(&self) -> Option<u16> {
        let playing = self.replay.stream.as_ref().is_some_and(|r| {
            matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)
        });
        if !playing {
            return None;
        }
        let id = *nfs_game::replay::CAMERA_IDS.get(self.replay.camera as usize)?;
        Some(nfs_game::replay::apply_camera(id as u8, self.track_has_camera_nodes()) as u16)
    }

    /// `0x46D960()` — whether the track carries any camera node at all, which is what decides
    /// between the "TV" camera and its fallback both at 0x413F3A and at 0x414960.
    fn track_has_camera_nodes(&self) -> bool {
        self.track_cameras.as_ref().is_some_and(|t| !t.cameras.is_empty())
    }

    fn rebuild_scene(&mut self) {
        // the port's own overlays lay their text out in screen units, so the fonts need to know
        // what shape the window is (the HUD and the front end place glyphs in 640×480 themselves)
        let a = self
            .renderer
            .as_ref()
            .map(|r| r.aspect())
            .unwrap_or(HUD_REF_W / HUD_REF_H);
        if let Some(f) = self.hud.as_mut().and_then(|h| h.font.as_mut()) {
            f.set_aspect(a);
        }
        if let Some(f) = self.fe.menu.as_mut().and_then(|m| m.font.as_mut()) {
            f.set_aspect(a);
        }
        if let Some(w) = self.world.as_mut() {
            for a in self.world_actions.drain(..) {
                match a {
                    WorldAction::Show(all, shown) => w.set_shown(&all, &shown),
                    WorldAction::Knock(element, away, launch) => w.knock(element, away, launch),
                }
            }
        }
        self.scene.items.clear();
        self.scene.sprites.clear();
        self.scene.mirror = self.mirror && self.vehicle.is_some();
        // the sky's own colour, from the track's `Sky/<Track>.bin` (`RE/RENDERING.md`): black on the
        // Monaco circuits, sunset on coastal, plain blue on forest. It shows wherever the sky layers
        // do not cover, and the layers are drawn over it.
        if self.vehicle.is_some()
            && let Some(c) = self.track.sky_bin.as_ref().and_then(|b| b.background())
        {
            self.scene.clear_color = [c[0] as f64, c[1] as f64, c[2] as f64, 1.0];
        }
        if self.vehicle.is_none() && self.fe.menu.is_some() {
            self.scene.clear_color = [0.0, 0.0, 0.0, 1.0];
            self.load_fe_room();
            self.fe_room_items();
            // a screen with a room behind it must not paint its own black fill over it
            let room = !self.fe.room.is_empty() || !self.fe.cars.is_empty();
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_overlay(room);
            }
            self.scene.sprites = self.fe_sprites();
            return;
        }
        // the HUD is a 640×480 layout; on a wider window it is pillarboxed, not stretched
        let hud_aspect = self
            .renderer
            .as_ref()
            .map(|r| r.aspect())
            .unwrap_or(HUD_REF_W / HUD_REF_H);
        if let (Some(h), Some(v)) = (&self.hud, &self.vehicle) {
            let position = if self.road.opponents.is_empty() {
                None
            } else {
                // rank the player among the opponents by progress (ties: earlier progress tick)
                let mut better = 0;
                if let Some(r) = &self.race {
                    for o in &self.road.opponents {
                        if o.progress.progress > r.progress
                            || (o.progress.progress == r.progress
                                && o.last_progress_tick < self.player_progress_tick)
                        {
                            better += 1;
                        }
                    }
                }
                Some((better, self.road.opponents.len() + 1))
            };
            self.scene.sprites.extend(h.sprites(
                hud_aspect,
                v.speed() * 3.6,
                v.rpm,
                self.tick,
                v.gear,
                self.race.as_ref(),
                self.num_laps,
                position,
            ));
            // `HReplay` is up for as long as the stream is playing (`hud.md` §5.2)
            if let Some(r) = &self.replay.stream
                && r.mode == nfs_game::replay::Mode::Playing
            {
                let mut bar = Vec::new();
                h.replay_bar(&mut bar, hud_aspect, self.paused, self.replay.speed);
                self.scene.sprites.extend(bar);
            }
            // the original's blip loop walks the whole racer list (0x433328), so traffic is on the map too
            let tick = self.tick;
            let mut others: Vec<(Vec3, [f32; 4])> = self
                .road.opponents
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    (
                        o.vehicle.position,
                        Hud::blip_color(
                            self.road.opponent_paints
                                .get(i)
                                .copied()
                                .unwrap_or([0x20, 0x40, 0xC0]),
                            false,
                            tick,
                        ),
                    )
                })
                .collect();
            others.extend(self.road.police.iter().flat_map(|p| {
                p.cars.iter().filter(|c| c.active).map(|c| {
                    (
                        c.position,
                        Hud::blip_color([0xE0, 0xE0, 0xE0], c.lights, tick),
                    )
                })
            }));
            others.extend(self.road.traffic.iter().flat_map(|tr| {
                tr.cars
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.active)
                    .map(|(i, c)| {
                        (
                            c.position,
                            Hud::blip_color(
                                self.road.traffic_paints
                                    .get(i)
                                    .copied()
                                    .unwrap_or([0x8C, 0x8C, 0x90]),
                                false,
                                tick,
                            ),
                        )
                    })
            }));
            let mut map = Vec::new();
            h.map_sprites(&mut map, hud_aspect, v.position, &others);
            self.scene.sprites.extend(map);
            if self.race.as_ref().map(|r| r.finished).unwrap_or(false) {
                let (order, _) = self.finishing_order();
                // cars still racing get the original's projected time (Race_finalize 0x416AC0);
                // eliminated cars (knockout) keep no time at all
                let circuit = self
                    .records
                    .track(&self.track.name)
                    .map(|t| t.circuit)
                    .unwrap_or(false);
                let elapsed = self.tick.saturating_sub(RACE_START_TICK);
                let row = |d: usize| -> (String, String, Option<u64>, Option<u64>) {
                    let (name, car, p, elim) = if d == 0 {
                        (
                            "YOU".to_string(),
                            self.specs.name.clone(),
                            self.race.as_ref(),
                            self.player_eliminated,
                        )
                    } else {
                        let o = &self.road.opponents[d - 1];
                        (
                            o.name.clone(),
                            o.specs.name.clone(),
                            Some(&o.progress),
                            o.eliminated,
                        )
                    };
                    let laps = p.map(|p| p.lap_times.clone()).unwrap_or_default();
                    let total = match p {
                        Some(p) if !elim => {
                            Some(p.projected_ticks(elapsed, self.num_laps, circuit))
                        }
                        _ => None,
                    };
                    (name, car, laps.iter().copied().min(), total)
                };
                let rows: Vec<_> = order.iter().map(|&d| row(d)).collect();
                let player = order.iter().position(|d| *d == 0).unwrap_or(0);
                let mut table = Vec::new();
                // the headers, the ordinals and the billboard's title come from `FEStrings`
                let fe = |id: u32| self.fe.strings.get(id).unwrap_or_default().to_string();
                let head = [fe(18111), fe(18112), fe(18113), fe(18114)];
                let ordinals: Vec<String> = (18116..18124).map(fe).collect();
                let title = fe(match self.fe.menu.as_ref().map(|m| m.mode) {
                    Some(1) => 18175,
                    Some(2) => 18169,
                    _ => 18129,
                });
                h.results_sprites(&mut table, hud_aspect, &rows, player, &title, &head, &ordinals);
                self.scene.sprites.extend(table);
            }
            if let Some(f) = &h.font {
                if self.player_eliminated {
                    self.scene.sprites.extend(f.sprites(
                        "KNOCKED OUT",
                        [0.38, 0.22],
                        0.06,
                        [1.0, 0.3, 0.2, 1.0],
                    ));
                } else if let Some(t) = self.records.track(&self.track.name)
                    && self.mission.is_none()
                {
                    // HRecordTime 0x42A200: point-to-point → the track record; circuits → the
                    // SPLITTIMES selection (record lap / best lap so far / last lap)
                    let record = t.entries.first().map(|e| e.time as u64);
                    let laps = self
                        .race
                        .as_ref()
                        .map(|r| r.lap_times.clone())
                        .unwrap_or_default();
                    let (label, value) = if !t.circuit {
                        ("RECORD", record)
                    } else {
                        match self.split_times {
                            2 => ("BEST LAP", laps.iter().copied().min()),
                            3 => ("LAST LAP", laps.last().copied()),
                            _ => ("RECORD LAP", record),
                        }
                    };
                    if let Some(v) = value {
                        self.scene.sprites.extend(f.sprites(
                            &format!("{label}  {}", format_ticks(v)),
                            [0.70, 0.03],
                            0.03,
                            [0.9, 0.9, 0.9, 0.8],
                        ));
                    }
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
                    if let Some(nfs_formats::career::ConeRule::PenaltySeconds(s)) = m.cone_rule
                        && cones > 0
                    {
                        line += &format!(" (+{} S)", s as usize * cones);
                    }
                    if let Some(d) = m.damage_limit_pct {
                        line += &format!("   DAMAGE {:.0}% / {d}%", v.damage_avg());
                    }
                    if sk.finished || sk.dnf {
                        let (pass, t) = sk.evaluate(m, 1, v.damage_avg());
                        let secs = t as f32 / 64.0;
                        line += &format!(
                            "   {}  {}:{:05.2}",
                            if pass {
                                "MISSION PASSED"
                            } else {
                                "MISSION FAILED"
                            },
                            (secs / 60.0) as u32,
                            secs % 60.0
                        );
                    }
                }
                self.scene.sprites.extend(f.sprites(
                    &line,
                    [0.02, 0.13],
                    0.035,
                    [1.0, 0.9, 0.5, 0.95],
                ));
            }
        }
        // render pose = previous tick → current tick, by the accumulator fraction
        let alpha = (self.accumulator.as_secs_f32() / GAME_TICK.as_secs_f32()).clamp(0.0, 1.0);
        let (car_pos, car_rot) = match self.render_prev {
            Some((p0, r0)) if p0.distance(self.car_pos) < 5.0 => (
                p0.lerp(self.car_pos, alpha),
                r0.slerp(self.car_rot, alpha).normalize(),
            ),
            _ => (self.car_pos, self.car_rot),
        };
        // **The headlight projector** 0x4526C0, rebuilt each frame from the player car: world into
        // the car's frame (0x466280 / 0x466220 from `car+0x364` / `+0x330`), its y shifted by
        // `[0x5CEC54]` = −4.8 (0x452749), then turned −17° about x (`[0x5CEC5C]` through 0x539BD0
        // at 0x452769, the row-vector product 0x539AD0). The car's lamp bit `car+0xD87 & 1` is the
        // night flag (`Car_init` 0x410C74), so it lights exactly on a night track.
        self.scene.headlight = (self.vehicle.is_some() && self.track.is_night()).then(|| {
            Mat4::from_rotation_x((-17.0f32).to_radians())
                * Mat4::from_translation(Vec3::new(0.0, -4.8, 0.0))
                * Mat4::from_rotation_translation(car_rot, car_pos).inverse()
        });
        let dt = self.frame_dt.clamp(0.001, 0.1);
        let fwd = car_rot * Vec3::Z;
        let rear_view = self.input.held(Action::RearView) && self.vehicle.is_some();
        // the horn is a loop on the car's own engine bank (`engine-audio.md` §6)
        if let (Some(a), Some(e)) = (&self.snd.audio, self.snd.engine_sound.as_mut()) {
            e.horn(
                a,
                self.input.held(Action::Horn) && !self.paused && self.vehicle.is_some(),
                0.6,
            );
        }
        if self.paused
            && let Some(h) = &self.hud
            && let Some(f) = &h.font
        {
            self.scene.sprites.extend(f.sprites(
                "PAUSED",
                [0.42, 0.40],
                0.07,
                [1.0, 1.0, 1.0, 1.0],
            ));
            self.scene.sprites.extend(f.sprites(
                "ENTER RESUME     ESC MENU",
                [0.33, 0.50],
                0.035,
                [0.8, 0.8, 0.8, 1.0],
            ));
        }
        // the driving views keep the port's own field of view; only a trackside camera element
        // brings its own (`+0x9C`, 37.5 or 45 degrees in the shipped scenes)
        self.camera.fov_y_radians = 60f32.to_radians();
        let view = self.camera_view();
        let off = nfs_game::camera::VIEW_OFFSETS
            .get(view as usize)
            .copied()
            .unwrap_or(nfs_game::camera::VIEW_OFFSETS[2]);
        // a view that rides the car and looks forward from it: the bumper and shotgun cameras
        let rides_car = matches!(view, 0x00 | 0x08 | 0x09);
        // the chase family — behind the car, heading and height low-passed
        let chases = matches!(view, 0x02..=0x07 | 0x0D);
        if rear_view && view != 0x0C && view != 0x14 {
            // `ReverseCam1` (0x0F): its own offset, looking back down the car's axis
            let r = nfs_game::camera::VIEW_OFFSETS[0x0F];
            self.camera.position = car_pos + car_rot * r;
            self.camera.target = self.camera.position - fwd * 20.0;
        } else if chases {
            // APPROXIMATION: the heading and height are low-passed so suspension pitch/roll and
            // small yaw wobble do not shake the view, where the original rides the car's matrix.
            let vel = self
                .vehicle
                .as_ref()
                .map(|v| v.velocity)
                .unwrap_or(Vec3::ZERO);
            let dir = if vel.length() > 3.0 {
                Vec3::new(vel.x, 0.0, vel.z).normalize_or_zero()
            } else {
                Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero()
            };
            // time-based low-pass (τ = 0.15 s heading, 0.08 s height) so the result does not depend on the frame rate
            let k_dir = 1.0 - (-dt / 0.15).exp();
            let k_y = 1.0 - (-dt / 0.08).exp();
            let smoothed = if self.cam_dir.length_squared() > 0.0 {
                (self.cam_dir * (1.0 - k_dir) + dir * k_dir).normalize_or_zero()
            } else {
                dir
            };
            self.cam_dir = smoothed;
            let ground_y = car_pos.y;
            self.cam_y = if self.cam_y == 0.0 {
                ground_y
            } else {
                self.cam_y * (1.0 - k_y) + ground_y * k_y
            };
            let base = Vec3::new(car_pos.x, self.cam_y, car_pos.z);
            self.camera.position =
                base + smoothed * off.z + Vec3::Y * off.y + smoothed.cross(Vec3::Y) * off.x;
            self.camera.target = base + Vec3::Y * 0.9 + smoothed * 2.0;
        } else if rides_car {
            // bumper / shotgun: the table's offset in the car's own frame, looking along it
            self.camera.position = car_pos + car_rot * off;
            let ahead = if off.z > 0.0 && view == 0x09 {
                -1.0
            } else {
                1.0
            };
            self.camera.target = self.camera.position + fwd * 20.0 * ahead;
        } else if view == nfs_game::camera::VIEW_DRIVER as u16
            && let Some(dc) = self.driver_cam
        {
            // driver (`BumperCam2`): the car's own `drivercampos` / `drivercamrot`, in the same body
            // frame as the table, aimed by the file's pitch and yaw in degrees
            let aim = Quat::from_rotation_y(dc.rotation.y.to_radians())
                * Quat::from_rotation_x(dc.rotation.x.to_radians());
            self.camera.position = car_pos + car_rot * dc.position;
            self.camera.target = self.camera.position + car_rot * (aim * Vec3::Z) * 20.0;
            self.camera.up = car_rot * Vec3::Y;
        } else {
            // `TVCam` / `DirectorsCam`: the camera the last camera trigger switched to
            // (0x413B80 → 0x413960), with its own field of view; before any trigger has fired, the
            // nearest element
            let cam = self
                .track_cameras
                .as_ref()
                .and_then(|t| {
                    t.current()
                        .filter(|c| c.has_position)
                        .map(|c| (c.position, c.fov))
                })
                .or_else(|| {
                    self.track.cameras.as_ref().and_then(|c| {
                        c.of_kind(&nfs_formats::scn::ElementKind::Camera)
                            .map(|e| Vec3::from(e.position))
                            .min_by(|a, b| {
                                a.distance_squared(self.car_pos)
                                    .partial_cmp(&b.distance_squared(self.car_pos))
                                    .unwrap()
                            })
                            .map(|p| (p, 45.0))
                    })
                });
            match cam {
                Some((p, fov)) if p.distance(car_pos) < 400.0 => {
                    self.camera.position = p;
                    self.camera.target = car_pos + Vec3::Y * 0.7;
                    self.camera.fov_y_radians = fov.to_radians();
                }
                _ => {
                    self.camera.position = car_pos - fwd * 7.5 + Vec3::Y * 2.3;
                    self.camera.target = car_pos + Vec3::Y * 0.9;
                }
            }
        }
        // **The police beacon's projector** 0x4526C0's second half: on a night track
        // (`word [0x628A28] != 0`) it walks the car list and keeps the live car whose `car+0xECC & 2`
        // (lights on) is set and whose octagonal distance to the camera in the xz plane,
        // `max(|dx|,|dz|) + 0.25·min(|dx|,|dz|)` (0x4527DB…0x45284D), is smallest — starting from
        // **180** (`0x43340000`, 0x4527AF), so a farther one lights nothing. Its frame is the car's
        // own (`car+0x330` / `car+0x364`) turned about the car's **y** by `TIMER_getTicks() × 0.02`
        // **turns** (`[0x5B3714]`, through 0x5564A0 whose sine takes turns, `[0x5BB2C0]` = 2π), and
        // 0x539CD0 stores world → that frame in `[0x61E184]`. The 128 Hz TIMER is two of this
        // port's 64 Hz game ticks.
        self.scene.beacon = self.track.is_night().then(|| {
            let cam = self.camera.position;
            // diagnostic: `NFS_BEACON_VIEW=1` spins the beacon on the player's own car, the only
            // way to watch the sweep without a pursuit
            if std::env::var("NFS_BEACON_VIEW").is_ok() {
                let turns = (self.tick * 2) as f32 * 0.02;
                let rot = car_rot * Quat::from_rotation_y(turns * std::f32::consts::TAU);
                return Some(Mat4::from_rotation_translation(rot, car_pos).inverse());
            }
            let pol = self.road.police.as_ref()?;
            let (car, _) = pol
                .cars
                .iter()
                .filter(|c| c.active && c.lights)
                .filter_map(|c| {
                    let (dx, dz) = ((c.position.x - cam.x).abs(), (c.position.z - cam.z).abs());
                    let d = dx.max(dz) + 0.25 * dx.min(dz);
                    (d < 180.0).then_some((c, d))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
            let turns = (self.tick * 2) as f32 * 0.02;
            let rot = Quat::from_mat3(&car.orientation) * Quat::from_rotation_y(turns * std::f32::consts::TAU);
            Some(Mat4::from_rotation_translation(rot, car.position).inverse())
        }).flatten();
        // draw distance from `<track>_{forward,backwards}.key` for the camera's slice, scaled by the
        // detail setting (0x44F1A0: `value × {0.5, 0.67, 0.83, 1.0}`, at least 50 m). The sky is drawn
        // around the camera, so distant terrain simply stops at this range as in the original.
        let keys = if self.backwards {
            &self.track.draw_keys_backward
        } else {
            &self.track.draw_keys_forward
        };
        let cam_slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
        self.camera.far = if self.vehicle.is_some() {
            keys.far_distance(cam_slice, self.detail)
        } else {
            20000.0
        };
        if let Some(s) = &self.sky {
            self.scene.items.extend(s.draw_items(
                self.camera.position,
                self.camera.far,
                self.tick as f32 / 64.0,
            ));
        }
        if let Some(w) = self.world.as_mut() {
            // knocked-over smackables keep tumbling (`RE/notes/skill-tests.md` §5.3)
            w.step_props(dt);
            // the looping props (`RE/notes/crp-geometry.md` §7) walk on the race clock
            w.animate(self.tick as f32 / 64.0);
            // the breakable props' pieces (`RE/notes/suspension-collision.md` §9)
            w.break_props(self.tick as u32);
        }
        if let Some(w) = &self.world {
            self.scene.items.extend(w.visible_items());
            self.scene.ambient = w.ambient;
        }
        if let Some(pol) = &self.road.police {
            for (i, c) in pol.cars.iter().enumerate().filter(|(_, c)| c.active) {
                let (p1, r1) = (
                    c.position,
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.road.police_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                let m = Mat4::from_rotation_translation(r, p);
                let spin = c.speed.abs() * self.tick as f32 / 64.0 / 0.32;
                let block = self.world.as_ref().map(|w| w.car_light(p));
                let model = &self.road.police_models[c.model];
                let mut items = match &block { Some(b) => model.draw_items_block(m, [spin; 4], [0.0; 2], b), None => model.draw_items(m, [spin; 4], [0.0; 2]) };
                model.apply_projector(&mut items, self.projector_planes(p));
                self.scene.items.extend(items);
            }
        }
        if let Some(tr) = &self.road.traffic {
            for (i, c) in tr.cars.iter().enumerate().filter(|(_, c)| c.active) {
                let (p1, r1) = (
                    c.position,
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.road.traffic_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                let m = Mat4::from_rotation_translation(r, p);
                // a kinematic car has no wheel state: the wheels are rolled from the distance driven
                let spin = self.road.traffic_spin.get(i).copied().unwrap_or(0.0);
                let block = self.world.as_ref().map(|w| w.car_light(p));
                let model = &self.road.traffic_models[c.model];
                let mut items = match &block { Some(b) => model.draw_items_block(m, [spin; 4], [0.0; 2], b), None => model.draw_items(m, [spin; 4], [0.0; 2]) };
                model.apply_projector(&mut items, self.projector_planes(p));
                self.scene.items.extend(items);
            }
        }
        for (i, o) in self.road.opponents.iter().enumerate() {
            let (p1, r1) = (
                o.vehicle.position,
                Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
            );
            let (p, r) = match self.road.opp_prev.get(i) {
                Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                    (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                }
                _ => (p1, r1),
            };
            let m = Mat4::from_rotation_translation(r, p);
            let w = &o.vehicle.wheels;
            let block = self.world.as_ref().map(|w| w.car_light(p));
            // the front wheels' `+0xB4` over 216 (slot 9's channels 1 and 2)
            let (spin, steer) = ([w[0].rotation, w[1].rotation, w[2].rotation, w[3].rotation], [w[0].steer / 216.0, w[1].steer / 216.0]);
            let model = &self.road.opponent_models[o.model];
            let mut items = match &block {
                Some(b) => model.draw_items_block(m, spin, steer, b),
                None => model.draw_items(m, spin, steer),
            };
            model.apply_projector(&mut items, self.projector_planes(p));
            self.scene.items.extend(items);
        }
        // every car drops a blob shadow on the ground under it (0x43B820)
        if let Some((_, material)) = self.shadow_draw {
            let half = self
                .vehicle
                .as_ref()
                .map(|v| v.half_extents)
                .unwrap_or(Vec3::new(0.9, 0.6, 2.1));
            let mut quads: Vec<[nfs_renderer::Vertex; 4]> = Vec::new();
            if self.vehicle.is_some() {
                quads.extend(self.shadow_quad(car_pos, car_rot, half));
            }
            for o in &self.road.opponents {
                let r = Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize();
                quads.extend(self.shadow_quad(o.vehicle.position, r, o.vehicle.half_extents));
            }
            for c in self
                .road.traffic
                .iter()
                .flat_map(|t| t.cars.iter())
                .filter(|c| c.active)
            {
                let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                quads.extend(self.shadow_quad(c.position, r, Vec3::new(0.9, 0.6, 2.1)));
            }
            for c in self
                .road.police
                .iter()
                .flat_map(|p| p.cars.iter())
                .filter(|c| c.active)
            {
                let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                quads.extend(self.shadow_quad(c.position, r, Vec3::new(0.9, 0.6, 2.1)));
            }
            // each quad gets its own dynamic mesh, rewritten in place every frame — the corners
            // move with the car and with the ground under it, so a transform cannot carry them
            if let Some(r) = self.renderer.as_mut() {
                while self.shadow_meshes.len() < quads.len() {
                    let mut m = nfs_renderer::MeshData::default();
                    m.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] }; 4];
                    m.indices = vec![0, 1, 2, 2, 1, 3];
                    self.shadow_meshes.push(r.upload_dynamic_mesh(&m));
                }
                for (mesh, q) in self.shadow_meshes.iter().zip(&quads) {
                    r.write_mesh_vertices(*mesh, q);
                }
            }
            for mesh in self.shadow_meshes.iter().take(quads.len()) {
                self.scene.items.push(nfs_renderer::DrawItem {
                    light: [0.0; 3],
                    alpha: 0.9,
                    ..nfs_renderer::DrawItem::new(*mesh, material, Mat4::IDENTITY)
                });
            }
        }
        // from inside the car the windscreen is drawn instead of the body: 0x43A490 puts the `Wndo`
        // quad up with the `INTGLASS` glass whenever the view id is 0 or 1 (`car-model.md` §12)
        if let Some(c) = &self.car_model
            && (!nfs_game::camera::draws_own_car(self.camera_view())
                || std::env::var("NFS_GLASS_ALWAYS").is_ok())
            && let Some((mesh, mat)) = c.window
        {
            let m = Mat4::from_rotation_translation(car_rot, car_pos);
            self.scene
                .items
                .push(nfs_renderer::DrawItem::new(mesh, mat, m));
        }
        // the camera's own car is not drawn in the bumper and driver views (0x4383FD)
        if let Some(c) = &self.car_model
            && nfs_game::camera::draws_own_car(self.camera_view())
        {
            let m = Mat4::from_rotation_translation(car_rot, car_pos);
            let (spin, steer) = match &self.vehicle {
                Some(v) => (
                    [
                        v.wheels[0].rotation,
                        v.wheels[1].rotation,
                        v.wheels[2].rotation,
                        v.wheels[3].rotation,
                    ],
                    [v.wheels[0].steer / 216.0, v.wheels[1].steer / 216.0],
                ),
                None => ([0.0; 4], [0.0; 2]),
            };
            // the car's light block — the ground under it times the `.map` colours, and the sun of
            // the track's sky (`RE/notes/lighting.md` §6)
            let block = self.world.as_ref().map(|w| w.car_light(car_pos));
            let mut items = match &block { Some(b) => c.draw_items_block(m, spin, steer, b), None => c.draw_items(m, spin, steer) };
            c.apply_projector(&mut items, self.projector_planes(car_pos));
            self.scene.items.extend(items);
        }
        // diagnostic: `NFS_EMITTER_VIEW=k` puts the camera 6 m off the track's k-th particle
        // emitter, looking at it
        if let Some(k) = std::env::var("NFS_EMITTER_VIEW").ok().and_then(|v| v.parse::<usize>().ok())
            && let Some(e) = self.world.as_ref().and_then(|w| w.emitters.get(k))
        {
            self.camera.target = e.position;
            self.camera.position = e.position + e.dir_a * 6.0 + Vec3::Y * 1.5;
        }
        {
            let fwd = (self.camera.target - self.camera.position).normalize_or_zero();
            let right = fwd.cross(self.camera.up).normalize_or_zero();
            let up = right.cross(fwd).normalize_or_zero();
            if let Some((mesh, material)) = self.particle_draw {
                self.scene
                    .items
                    .extend(self.particles.draw_items(mesh, material, right, up));
            }
            // the lamps' glare, for every car the camera can see
            if self.glare_draw.is_some() {
                let mut glares = Vec::new();
                if let Some(c) = &self.car_model
                    && nfs_game::camera::draws_own_car(self.camera_view())
                {
                    self.glare_items(&mut glares, c, car_pos, car_rot, right, up);
                }
                for o in &self.road.opponents {
                    let r = Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.road.opponent_models[o.model],
                        o.vehicle.position,
                        r,
                        right,
                        up,
                    );
                }
                for c in self
                    .road.traffic
                    .iter()
                    .flat_map(|t| t.cars.iter())
                    .filter(|c| c.active)
                {
                    let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.road.traffic_models[c.model],
                        c.position,
                        r,
                        right,
                        up,
                    );
                }
                for c in self
                    .road.police
                    .iter()
                    .flat_map(|p| p.cars.iter())
                    .filter(|c| c.active)
                {
                    let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.road.police_models[c.model],
                        c.position,
                        r,
                        right,
                        up,
                    );
                }
                self.scene.items.extend(glares);
            }
            let mut flares = Vec::new();
            self.flare_items(&mut flares, fwd, right, up);
            self.scene.items.extend(flares);
            if let Some(p) = self.track_particles.as_ref() {
                // 0x454430 → 0x44F6E0: a camera-facing billboard per particle, turned about the
                // view axis. APPROXIMATION: the blend mode (`+0x28`, 0x537A50) and the depth bias
                // are not read — the glare material stands in
                for sp in p.sprites() {
                    let Some(&(mesh, material)) = self.track_particle_sprites.get(&sp.material) else { continue };
                    let (s, c) = sp.degrees.to_radians().sin_cos();
                    let (r, u) = (right * c + up * s, up * c - right * s);
                    let m = Mat4::from_cols((r * sp.size).extend(0.0), (u * sp.size).extend(0.0), right.cross(up).extend(0.0), sp.position.extend(1.0));
                    let a = sp.argb;
                    self.scene.items.push(nfs_renderer::DrawItem {
                        light: [((a >> 16) & 0xFF) as f32 / 255.0, ((a >> 8) & 0xFF) as f32 / 255.0, (a & 0xFF) as f32 / 255.0],
                        alpha: (a >> 24) as f32 / 255.0,
                        ..nfs_renderer::DrawItem::new(mesh, material, m)
                    });
                }
            }
        }
    }

    /// The track's flares, `RDrawArticleBundle`'s slot 3 for kind 2 (0x451940,
    /// `RE/notes/crp-geometry.md` §8): the part's definition by its id (0x452510), shown while its
    /// blink says so (`FlareDef::lit` on the game tick), and for each of the two layers with a
    /// texture a camera-facing billboard (0x44F6E0, `RBillBoard`) at the part's point, a side of
    /// `size × 0.1`, the layer's ARGB colour, turned about the view axis by — flag 4 —
    /// `√d² · 3.6 + facing · 120` degrees, `d` from the camera and `facing = −dir · forward`
    /// (0x451A5A…0x451B62; the root is the image's own `(bits >> 1) + 0x1FC00000`). Every shipped
    /// definition carries flags 4 alone, so 0x4515F0's other fades (flags 1, 2, 8, 0x20, 0x40) have
    /// nothing to act on and are not carried; the fog's (`[0x61BCA8]`) has no fog here.
    /// APPROXIMATION: every flare of the track is tried — the original's come with the articles its
    /// renderer finds in view — and the depth bias 0x537A60 and the batch's blend are not read
    /// (the lamps' glare material stands in). MEDIUM: the row 0x451A57 dots is taken as the
    /// camera's forward and the turn's sense as the axis-angle build's (0x539BD0).
    fn flare_items(&self, out: &mut Vec<nfs_renderer::DrawItem>, fwd: Vec3, right: Vec3, up: Vec3) {
        let Some(w) = self.world.as_ref() else { return };
        if self.flare_defs.is_empty() {
            return;
        }
        let tick = self.tick as u32;
        let cam = self.camera.position;
        for f in &w.flares {
            let Some(def) = self.flare_defs.iter().find(|d| d.id == f.id) else { continue };
            if !def.lit(f.phase, tick) {
                continue;
            }
            let d2 = (f.position - cam).length_squared();
            let dist = f32::from_bits((d2.to_bits() >> 1).wrapping_add(0x1FC0_0000));
            let facing = -f.direction.dot(fwd);
            let angle = if def.flags & 4 != 0 { (dist * 3.6 + facing * 120.0).to_radians() } else { 0.0 };
            let (s, c) = angle.sin_cos();
            let (r, u) = (right * c + up * s, up * c - right * s);
            for k in 0..2 {
                let Some(name) = def.textures[k].as_ref() else { continue };
                let Some(&(mesh, material)) = self.flare_sprites.get(name) else { continue };
                let size = def.sizes[k] * 0.1;
                let argb = def.colours[k];
                let m = Mat4::from_cols((r * size).extend(0.0), (u * size).extend(0.0), right.cross(up).extend(0.0), f.position.extend(1.0));
                out.push(nfs_renderer::DrawItem {
                    light: [((argb >> 16) & 0xFF) as f32 / 255.0, ((argb >> 8) & 0xFF) as f32 / 255.0, (argb & 0xFF) as f32 / 255.0],
                    alpha: (argb >> 24) as f32 / 255.0,
                    ..nfs_renderer::DrawItem::new(mesh, material, m)
                });
            }
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
    /// (scene element, direction from the car, the velocity the prop leaves with)
    Knock(usize, Vec3, Vec3),
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
        // `--layout` browse mode: the screens navigate each other through their own `destination` fields
        if self.fe.layout_preview.is_some() {
            self.layout_key(key, event_loop);
            return;
        }
        let Some(m) = self.fe.menu.as_mut() else { return };
        let rows = m.rows();
        match key {
            MenuKey::Up => {
                m.row = (m.row + rows - 1) % rows;
                if (1..=3).contains(&m.mode)
                    && m.row > 0
                    && let Some(c) = self.career.as_mut()
                {
                    c.row = m.row - 1;
                }
            }
            MenuKey::Down => {
                m.row = (m.row + 1) % rows;
                if (1..=3).contains(&m.mode)
                    && m.row > 0
                    && let Some(c) = self.career.as_mut()
                {
                    c.row = m.row - 1;
                }
            }
            MenuKey::Left | MenuKey::Right => {
                let delta: i32 = if key == MenuKey::Left { -1 } else { 1 };
                if m.row == 0 {
                    m.mode = if self.career.is_some() {
                        ((m.mode as i32 + delta).rem_euclid(6)) as u8
                    } else {
                        [0u8, 4, 5][((match m.mode {
                            0 => 0,
                            4 => 1,
                            _ => 2,
                        } as i32
                            + delta)
                            .rem_euclid(3)) as usize]
                    };
                    if m.mode == 4
                        && !m
                            .circuits
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(m.track()))
                        && let Some(i) = m
                            .tracks
                            .iter()
                            .position(|t| m.circuits.iter().any(|c| c.eq_ignore_ascii_case(t)))
                    {
                        m.track_idx = i;
                    }
                    if let Some(c) = self.career.as_mut() {
                        c.set_mode(m.mode);
                    }
                } else if (1..=3).contains(&m.mode) {
                    if let Some(c) = self.career.as_mut() {
                        c.row = m.row - 1;
                        c.move_col(delta);
                    }
                } else {
                    let step = |i: usize, n: usize| {
                        ((i as i32 + delta).rem_euclid(n.max(1) as i32)) as usize
                    };
                    match m.row {
                        1 => {
                            m.track_idx = step(m.track_idx, m.tracks.len());
                            if m.mode == 4 {
                                // Quick Knockout: circuits only
                                for _ in 0..m.tracks.len() {
                                    if m.circuits
                                        .iter()
                                        .any(|c| c.eq_ignore_ascii_case(m.track()))
                                    {
                                        break;
                                    }
                                    m.track_idx = step(m.track_idx, m.tracks.len());
                                }
                            }
                        }
                        2 => {
                            m.car_idx = step(m.car_idx, m.cars.len());
                            m.paint_idx = 0
                        }
                        3 => {
                            m.paint_idx = if delta < 0 {
                                m.paint_idx.saturating_sub(1)
                            } else {
                                m.paint_idx + 1
                            }
                        }
                        // the knockout modes edit `Game.NumKOOpponents`, whose minimum is 1
                        // (0x4E5570); everything else edits `Game.NumOpponents`, minimum 0
                        4 => {
                            if m.mode >= 4 {
                                m.ko_opponents = (m.ko_opponents as i32 + delta).clamp(1, 7) as u32;
                                m.fit_ko();
                            } else {
                                m.opponents = (m.opponents as i32 + delta).clamp(0, 7) as u32;
                            }
                        }
                        9 => m.opp_car = ((m.opp_car as i32 + delta).rem_euclid(5)) as u8,
                        5 => m.laps = (m.laps as i32 + delta).clamp(1, 10) as u32,
                        7 => m.traffic = (m.traffic as i32 + delta).clamp(0, 3) as u32,
                        8 => m.cops = !m.cops,
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
                if (1..=3).contains(&m.mode)
                    && let Some(c) = self.career.as_mut()
                {
                    c.back();
                }
            }
            MenuKey::Enter if (1..=3).contains(&m.mode) => {
                let Some(req) = self.career.as_mut().and_then(|c| c.activate()) else {
                    // a dialog the career screen asked for is raised on the next frame
                    // (`fe_career_dialog`)
                    return;
                };
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    cars: m.cars.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
                self.fe.menu_template = Some(template);
                self.num_opponents = req.opponents.len() as u32;
                self.num_laps = req.laps;
                self.paint = req.paint;
                self.career_opponents = Some(req.opponents.clone());
                self.road.traffic_count = req.traffic;
                self.race_skill = req.skill;
                self.backwards = req.backwards;
                self.mirror = false;
                self.quick_knockout = false;
                self.road.cops_race = false;
                let car = self.cheat_993(req.car);
                self.mission = req.mission.clone();
                self.skill = None;
                match (
                    { let derby = self.derby_track(&req.track_file); TrackAssets::load(&self.paths, &derby) },
                    CarAssets::load(&self.paths, &car.model).map(|c| {
                        c.with_style(car.variant)
                            .with_cabrio(car.cabrio)
                            .with_style_overlay(req.parts.iter().flat_map(|p| p.geometry_swaps.iter().copied()).collect())
                    }),
                    CarAssets::load_sim(&self.paths, &car.sim),
                ) {
                    (Ok(mut t), Ok(c), Ok(sim)) => {
                        if let Some(ms) = &req.mission
                            && let Some(suffix) = &ms.scene_suffix
                        {
                            match t.load_scene(suffix) {
                                Ok(Some(scene)) => {
                                    self.skill = Some(SkillTest::from_scene(&scene, &t.name));
                                    t.extra_scenes.push(scene);
                                }
                                other => {
                                    tracing::error!(suffix, result = ?other.err(), "skill-test scene missing")
                                }
                            }
                        }
                        self.track = t;
                        self.car = c;
                        let race_class =
                            car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                        if self.cheat_bits & (1 << 1) != 0 {
                            self.specs.apply_tinycar();
                        }
                        // the purchased parts, then the setup sliders — the order 0x49CB50 runs
                        // them in, and both of them before anything reads the block
                        let grip_ratio = self
                            .specs
                            .apply_parts(&req.parts.iter().collect::<Vec<_>>(), &car.sim_family);
                        tracing::debug!(parts = req.parts.len(), grip_ratio, "parts applied");
                        if let Some(cu) = &self.career {
                            self.specs.zone_masks =
                                nfs_game::career::zone_masks_for(&cu.tables, &car);
                            // the parts bring the damage they carry from earlier races
                            if let Some(owned) = cu
                                .pending
                                .as_ref()
                                .and_then(|p| p.garage_idx)
                                .and_then(|i| cu.career.garage.get(i))
                            {
                                self.specs.wear =
                                    nfs_game::career::part_wear_for(&cu.tables, owned);
                                // …and the car-setup screen's seventeen sliders reach the block
                                // here, as 0x49D290 folds them in out of the per-player record.
                                // `automatic` is `carinfo+0xC`, the manual ×1.05 of 0x49D733
                                self.specs.apply_setup(&owned.setup, self.player_automatic());
                            }
                        }
                        self.snd.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.fe.menu = None;
                        let (renderer, window) =
                            (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => {
                        tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "career event load failed");
                        if let Some(cu) = self.career.as_mut() {
                            cu.pending = None;
                            cu.message = "EVENT LOAD FAILED".into();
                        }
                        self.career_opponents = None;
                        if let Some(mut mt) = self.fe.menu_template.take() {
                            mt.font = mt.font.take();
                            self.fe.menu = Some(mt);
                        }
                    }
                }
            }
            MenuKey::Enter if m.mode == 5 => {
                let Some(car) = m.car().cloned() else { return };
                // new series unless one is active
                if !self.knockout.as_ref().map(|k| k.active).unwrap_or(false) {
                    let n = m.ko_opponents.clamp(1, 7);
                    let mut seed = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(1))
                        | 1;
                    let mut next = move || {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        seed
                    };
                    // the series runs the locations the player set on `locationsetup.lay`, one
                    // row per opponent — `Knockout.TrackNumber`'s own table. A row the table does
                    // not have (it is kept the length of the opponent count, so this is only
                    // reachable if that count grew without the screen) falls back to the menu's
                    // single-race track.
                    let tracks: Vec<String> = (0..n as usize)
                        .map(|i| {
                            let idx = m.ko.get(i).map_or(m.track_idx, |k| k.track_idx);
                            m.tracks.get(idx).cloned().unwrap_or_else(|| m.track().to_string())
                        })
                        .collect();
                    let mut used: Vec<String> = vec![car.model.to_lowercase()];
                    let mut opponents = Vec::new();
                    // `0x4E3160`: one `nfs5.per` driver per opponent, drawn without replacement from
                    // the records whose era matches the difficulty selection (`modes.md` §5.2)
                    let drivers: Vec<i16> = match self.career.as_ref() {
                        Some(cu) => nfs_game::career::pick_personalities(
                            &cu.tables.personalities,
                            Some(self.race_skill as u32),
                            n as usize,
                            |len| (next() % len as u64) as usize,
                        ),
                        None => Vec::new(),
                    };
                    for i in 0..n {
                        let Some(entry) = m
                            .cars
                            .iter()
                            .filter(|c| {
                                !c.sim.is_empty()
                                    && c.year >= 1990
                                    && !used.iter().any(|u| u.eq_ignore_ascii_case(&c.model))
                            })
                            .nth((i as usize * 7 + (next() % 5) as usize) % 12)
                        else {
                            break;
                        };
                        used.push(entry.model.to_lowercase());
                        let paint = entry
                            .paint_sets
                            .get((i as usize + 1) % entry.paint_sets.len().max(1))
                            .map(|p| p.colours[0])
                            .unwrap_or([0x20, 0x40, 0xC0]);
                        let per = drivers.get(i as usize).copied().unwrap_or(0);
                        let (name, skill) = match self.career.as_ref().and_then(|cu| {
                            cu.tables
                                .personalities
                                .iter()
                                .find(|p| p.id == per)
                                .map(|p| (cu.tables.driver_name(per), p.skill))
                        }) {
                            Some((n, s)) => (n, s),
                            None => (entry.display_name.clone(), [[1.0, 0.0]; 4]),
                        };
                        opponents.push(CareerOpponent {
                            car: entry.clone(),
                            paint,
                            name,
                            difficulty: 1.0,
                            skill,
                            grid: None,
                            personality: per,
                            parts: Vec::new(),
                        });
                    }
                    let n = opponents.len() as u32;
                    let mut start_pos = vec![n + 1];
                    start_pos.extend(1..=n);
                    self.knockout = Some(KnockoutSeries {
                        n,
                        race_idx: 0,
                        tracks,
                        opponents,
                        alive: vec![true; n as usize],
                        start_pos,
                        active: true,
                        message: String::new(),
                    });
                }
                let Some(k) = self.knockout.as_ref() else {
                    return;
                };
                let track_name = k.tracks[k.race_idx as usize].clone();
                let opponents: Vec<CareerOpponent> = k
                    .opponents
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| k.alive[*i])
                    .map(|(i, o)| CareerOpponent {
                        grid: Some(k.start_pos[i + 1] - 1),
                        ..o.clone()
                    })
                    .collect();
                self.player_grid_override = Some(k.start_pos[0] - 1);
                self.num_opponents = opponents.len() as u32;
                self.career_opponents = Some(opponents);
                self.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                self.road.cops_enabled = m.cops;
                self.road.cops_race = false;
                self.quick_knockout = false;
                self.num_laps = m.laps;
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    cars: m.cars.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
                self.paint = car
                    .paint_sets
                    .get(m.paint_idx % car.paint_sets.len().max(1))
                    .map(|p| p.colours[0])
                    .unwrap_or([0xD4, 0x10, 0x10]);
                self.fe.menu_template = Some(template);
                let car = self.cheat_993(car);
                tracing::info!(
                    track = track_name,
                    race = k.race_idx + 1,
                    of = k.n,
                    "knockout race"
                );
                match (
                    { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
                    CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio)),
                    CarAssets::load_sim(
                        &self.paths,
                        if car.sim.is_empty() {
                            "default"
                        } else {
                            &car.sim
                        },
                    ),
                ) {
                    (Ok(t), Ok(c), Ok(sim)) => {
                        self.track = t;
                        self.car = c;
                        let race_class =
                            car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                        if self.cheat_bits & (1 << 1) != 0 {
                            self.specs.apply_tinycar();
                        }
                        self.snd.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.fe.menu = None;
                        let (renderer, window) =
                            (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => {
                        tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed")
                    }
                }
            }
            MenuKey::Enter => {
                let Some(car) = m.car().cloned() else { return };
                let track_name = m.track().to_string();
                self.player_grid_override = None;
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    cars: m.cars.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
                self.num_opponents = if m.mode >= 4 { m.ko_opponents } else { m.opponents };
                self.opp_choice = m.opp_car as u32;
                self.player_car_id = Some(car.id);
                self.quick_knockout = m.mode == 4;
                self.num_laps = if m.mode == 4 {
                    m.ko_opponents.max(1)
                } else {
                    m.laps
                };
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                self.career_opponents = None;
                self.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                // the single race (game type 0) with the Fuzzyfuz code is the cops race (0x4EB4FD)
                self.road.cops_race = m.mode == 0 && self.cheat_bits & (1 << 5) != 0;
                self.road.cops_enabled = m.cops || self.road.cops_race;
                self.paint = car
                    .paint_sets
                    .get(m.paint_idx % car.paint_sets.len().max(1))
                    .map(|p| p.colours[0])
                    .unwrap_or([0xD4, 0x10, 0x10]);
                self.fe.menu_template = Some(template);
                let car = self.cheat_993(car);
                tracing::info!(
                    track = track_name,
                    car = car.model,
                    sim = car.sim,
                    "menu: start race"
                );
                match (
                    { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
                    CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio)),
                    CarAssets::load_sim(
                        &self.paths,
                        if car.sim.is_empty() {
                            "default"
                        } else {
                            &car.sim
                        },
                    ),
                ) {
                    (Ok(t), Ok(c), Ok(sim)) => {
                        self.track = t;
                        self.car = c;
                        let race_class =
                            car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                        self.specs = CarSpecs::from_sim(&sim, race_class);
                        // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                        if self.cheat_bits & (1 << 1) != 0 {
                            self.specs.apply_tinycar();
                        }
                        self.snd.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.fe.menu = None;
                        let (renderer, window) =
                            (self.renderer.take().unwrap(), self.window.take().unwrap());
                        self.tick = 0;
                        self.start_race(renderer, window, event_loop);
                    }
                    (t, c, s) => {
                        tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed")
                    }
                }
            }
        }
    }

    /// Navigate the previewed front-end screen: the buttons carry the name of the screen they lead to.
    fn layout_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) {
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let controls = l.controls();
        if controls.is_empty() {
            if key == MenuKey::Back {
                match self.fe.layout_stack.pop() {
                    Some(prev) => self.open_layout(&prev),
                    None => event_loop.exit(),
                }
            }
            return;
        }
        let sel = self.fe.layout_sel.min(controls.len() - 1);
        match key {
            MenuKey::Up => self.fe.layout_sel = (sel + controls.len() - 1) % controls.len(),
            MenuKey::Down => self.fe.layout_sel = (sel + 1) % controls.len(),
            // a combo slider edits the accessor it names
            MenuKey::Left | MenuKey::Right => {
                let delta = if key == MenuKey::Left { -1 } else { 1 };
                if let Some(a) = controls[sel].accessor.clone() {
                    self.set_accessor(&a, delta);
                    // a slider that changed the car changes the logo with it (0x4C1A6A: the widget
                    // reloads when the car's pair differs from the one it cached)
                    self.bind_car_logos();
                    self.bind_indicator_bars();
                    self.bind_chronicle_icons();
            self.bind_chronicle_group();
            self.bind_repair_bars();
            self.bind_usedcars_panels();
            self.bind_garage();
            self.bind_player_buttons();
            self.bind_parts_buttons();
            self.bind_mergedlist_panel();
            self.bind_showcase_pictures();
            self.bind_track_maps();
            self.bind_player_pictures();
                    self.bind_lists();
                    self.bind_checks();
                }
            }
            MenuKey::Enter => {
                let c = controls[sel].clone();
                // the network screens' own callbacks. What the original's handlers behind
                // `ID_COMMSSERVERSCREEN` and friends do has not been read; these do what the
                // protocol of `network.md` §7 allows and nothing more.
                if self.net.lobby.is_some()
                    && let Some(cb) = c.callback.as_deref()
                    && matches!(
                        cb,
                        "CREATE" | "JOIN" | "CONNECT" | "DIAL" | "ENTER" | "EJECTPLAYER" | "READY"
                    )
                {
                    self.network_callback(cb, event_loop);
                    return;
                }
                // `usedcars.lay`'s `ID_BUYCARBUTTON` — `callback=BUYCAR`, which for the used-car
                // lot is 0x501F40: the money goes and the car moves across as it stands, mileage,
                // parts and all. The player's own listings are on the same lot and cannot be
                // bought back, so only a generated one is taken.
                if c.callback.as_deref() == Some("BUYCAR")
                    && self.fe.layout_name.as_deref().map(|n| n.eq_ignore_ascii_case("usedcars")).unwrap_or(false)
                {
                    let n = self.career.as_ref().map_or(0, |c| c.career.market.len());
                    let idx = self.fe.usedcar_sel;
                    if idx < n
                        && let Some(cu) = self.career.as_mut()
                    {
                        match cu.career.buy_used_car(idx) {
                            Ok(()) => {
                                self.fe.usedcar_sel = 0;
                                self.bind_lists();
                                self.bind_indicator_bars();
                                self.bind_usedcars_panels();
                            }
                            Err(e) => tracing::info!("used car: {e}"),
                        }
                    }
                    return;
                }
                // The bottom bar's PLAYGAME button, or any screen that says it can start a game.
                //
                // **The gate is this port's, and it is an approximation — §4.85.** `PLAYGAME`
                // resolves through the controller's callback table to **slot 53**, 0x50CE90
                // (0x50C1F2 stores 0x50E5B0, which is `jmp [vtbl+0xD4]`), and that handler asks
                // nothing: it opens with its own re-entry latch `[this+0x278]` and goes straight
                // into the network-session checks. `canplaygame`'s getter, slot 36 at 0x4E0BF0,
                // has no direct callers at all — in the original the **bottom bar** shows and
                // hides the button per screen, so on a screen that cannot race there is no
                // button to press. This port keeps the button and declines instead, which is the
                // same outcome by a different route, and it is what stops the RACE button
                // starting a race from `credits.lay` and `createplayer.lay`. The thirteen screens
                // with a non-zero `canplaygame` are exactly the race-launching ones.
                //
                // (An earlier reading paired `PLAYGAME` with 0x50E540 and hung this on slot 47's
                // "ask then act". That registration is **`QUIT`**, and its `[this+0x270]` is the
                // exit-confirm dialog, `FEStrings` 811.)
                // `FORFEITTOURNY` (0x506130) raises 824; it has no destination, so without this
                // it fell into the start below and raced instead of forfeiting
                if c.callback.as_deref() == Some("FORFEITTOURNY") {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 824, [700, 701], DialogFor::Forfeit);
                    return;
                }
                // `KILLREPLAY` (0x50D3A0) raises the prebuilt 808 with the default ok / cancel
                // (0x50C690 passes 0, 0); `STARTREPLAY` is not ported — both used to fall into the
                // race start below
                if c.callback.as_deref() == Some("KILLREPLAY") {
                    if !self.replay_names().is_empty() {
                        self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 808, [0, 0], DialogFor::KillReplay);
                    }
                    return;
                }
                let can_play = l.can_play();
                if let Some(cb) = c.callback.clone()
                    && (self.player_callback(&cb, event_loop) || self.cd_callback(&cb))
                {
                    return;
                }
                if c.callback.as_deref() == Some("STARTREPLAY") {
                    self.start_replay(event_loop);
                    return;
                }
                // 0x50D330 / 0x50D350 / 0x50D370 arm the `Game.GameType` override `[0x5DC1C0]`
                // with 7 / 6 / 8 and run PLAYGAME
                if let Some(t) = match c.callback.as_deref() {
                    Some("TESTDRIVEUSED") => Some(7),
                    Some("TESTDRIVENEW") => Some(6),
                    Some("TESTDRIVECARSETUP") => Some(8),
                    _ => None,
                } {
                    self.start_test_drive(t, event_loop);
                    return;
                }
                // `carsetup.lay`'s `TESTDRIVECHECK` (0x4F1A90): unless 0x502420 says no, 0x4F14B0,
                // then `0x4D2100("testdrivecarsetup", 1, 1)` — pushed over the setup screen.
                // Neither 0x502420 nor 0x4F14B0 is read, so the push is unconditional here.
                if c.callback.as_deref() == Some("TESTDRIVECHECK") {
                    if let Some(cur) = self.fe.layout_name.clone() {
                        self.fe.layout_stack.push(cur);
                    }
                    self.open_layout("testdrivecarsetup");
                    return;
                }
                if (c.callback.as_deref() == Some("PLAYGAME") || c.destination.is_none()) && can_play {
                    self.start_from_layout(event_loop);
                    return;
                }
                let Some(dest) = c.destination else { return };
                // **`data.offset` is what the button does to the screen stack.** Its slot 9 hands
                // the attribute's int to `0x4D2100(destination, offset, 1)` (0x4C94BD…0x4C94CF),
                // and that function branches on it three ways over the vector at `[mgr+0xD8]`:
                //
                // * **> 0** — *push*: a new entry is made for `destination` and appended
                //   (0x4D21A5…0x4D2229), so the screen it was clicked on stays underneath;
                // * **0** — *replace*: the **last** entry's own screen becomes `destination`
                //   (0x4D235F → `0x4D5DF0(end[-1], …)`), and the depth does not change;
                // * **< 0** — *pop*: only when the vector holds **at least two** entries
                //   (0x4D22DD), the last one is closed through its slot 31 (0x4D22FC) and the new
                //   last one takes `destination` (`0x4D5DF0(end[-2], …)`).
                //
                // The original's vector holds one entry per open screen, this port's stack holds
                // the ancestors only, so its "at least two" is this one's "not empty". Of the 153
                // shipped `NFS5.GOTOBUTTON`s, **109 replace** (the key absent or 0), 25 push and
                // 19 pop — so the port pushing on every one of them was wrong for all but 25, and
                // `ESC` walked back through screens the game does not keep.
                match c.offset {
                    n if n > 0 => {
                        if let Some(cur) = self.fe.layout_name.clone() {
                            self.fe.layout_stack.push(cur);
                        }
                    }
                    n if n < 0 => {
                        self.fe.layout_stack.pop();
                    }
                    _ => {}
                }
                self.open_layout(&dest);
            }
            MenuKey::Back => match self.fe.layout_stack.pop() {
                Some(prev) => self.open_layout(&prev),
                None => event_loop.exit(),
            },
        }
    }

    /// The song names, from the live player in a race and from the files in the front end.
    fn cd_names(&self) -> Vec<String> {
        match self.snd.music.as_ref() {
            Some(m) => m.names.clone(),
            None => self.snd.music_names.clone(),
        }
    }

    /// The song table `[0x65B628]` (0x4ADE20 with `game*`): each matching stream's index and name,
    /// in stream order.
    fn cd_table(names: &[String]) -> Vec<(usize, String)> {
        names.iter().enumerate().filter(|(_, n)| nfs_audio::music::pattern_match(n, "game*")).map(|(i, n)| (i, n.clone())).collect()
    }

    /// The playlist `[0x65B62C]` as stream indices.
    fn cd_playlist(&self) -> Vec<usize> {
        match self.snd.music.as_ref() {
            Some(m) => m.playlist().to_vec(),
            None => self.snd.audio_settings.playlist.iter().map(|i| *i as usize).collect(),
        }
    }

    fn set_cd_playlist(&mut self, list: Vec<usize>) {
        self.snd.audio_settings.playlist = list.clone();
        if let Some(m) = self.snd.music.as_mut() {
            m.set_saved(list);
        }
    }

    /// The CD player's editing callbacks (`cdplayer.lay`, the screen at vtable 0x5B9F58):
    ///
    /// * `ADDSONG` 0x511FF0: with an `AvailableList` row selected, **815** when the playlist
    ///   already holds 40 (`cmp eax, 0x28`, raised at 0x51202C), else that record's stream index
    ///   appended (0x4BEBE0 → 0x4BE670);
    /// * `REMOVESONG` 0x512070: with a `SelectedList` row selected, that entry erased (0x4BE790);
    /// * `CLEARALL` (slot 0xEC → 0x4BE9E0): the playlist emptied;
    /// * `RESETLIST` (slot 0xF0 → 0x4BE7E0): emptied, then every table record whose name's first
    ///   four characters, lower-cased, are `game` appended in table order.
    fn cd_callback(&mut self, cb: &str) -> bool {
        let names = self.cd_names();
        let table = Self::cd_table(&names);
        let mut list = self.cd_playlist();
        match cb {
            "ADDSONG" => {
                let Some(r) = self.fe.cd_available.filter(|r| *r < table.len()) else { return true };
                if list.len() >= 0x28 {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 815, [0, 0], DialogFor::Notice);
                    return true;
                }
                list.push(table[r].0);
            }
            "REMOVESONG" => {
                let Some(r) = self.fe.cd_selected.filter(|r| *r < list.len()) else { return true };
                list.remove(r);
            }
            // slots 55 / 56, 0x511F90 / 0x511FC0: one of `Audio.lay`'s two panels at a time — the
            // one going away hidden (0x4CEC50), the other loaded (its slot 0x6C) and shown
            // (0x4CEC60); `[+0x290]` remembers which
            "SHOWPLAYLISTPANEL" | "HIDEPLAYLISTPANEL" => {
                let show = cb == "SHOWPLAYLISTPANEL";
                let (off, on) = if show { ("ID_MAINSCREENPANEL", "ID_PLAYLISTPANEL") } else { ("ID_PLAYLISTPANEL", "ID_MAINSCREENPANEL") };
                if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                    l.set_panel(r, &self.paths, off, false);
                    l.set_panel(r, &self.paths, on, true);
                }
                self.bind_lists();
                return true;
            }
            // the transport (`audioreal.lay`)
            "PLAYSONG" => {
                // 0x5120D0: `MusicStop(0)`, then `MusicPlay(0x4BEB40(), 0)` — the current song's
                // name, or none (`MusicPlay(NULL, 0)`, a pick from the playlist) when `[0x5D7938]`
                // names no table record
                let name = table.iter().find(|(i, _)| *i as i32 == self.fe.cd_current).map(|(_, n)| n.clone());
                self.fe_music(|m, a| {
                    m.stop(a);
                    match &name {
                        Some(n) => m.play_pattern(a, n, 0),
                        None => m.start_race(a),
                    }
                });
                return true;
            }
            "STOPSONG" => {
                // 0x5120F0: `MusicStop(0)`
                self.fe_music(|m, a| m.stop(a));
                return true;
            }
            "SKIPFWD" | "SKIPBK" => {
                // 0x512100 / 0x512110: `CDPlayer.Title` stepped forward / back, wrapping
                // (0x4BC690 / 0x4BC660), through its setter 0x4BEDC0
                let n = table.len();
                if n > 0 {
                    let i = self.fe.cd_title.min(n - 1);
                    let next = if cb == "SKIPFWD" { (i + 1) % n } else { (i + n - 1) % n };
                    self.cd_set_title(next);
                }
                return true;
            }
            "CLEARALL" => list.clear(),
            "RESETLIST" => {
                list = table.iter().filter(|(_, n)| n.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("game"))).map(|(i, _)| *i).collect();
            }
            _ => return false,
        }
        self.set_cd_playlist(list);
        self.bind_lists();
        true
    }

    /// `CDPlayer.Title`'s setter **0x4BEDC0**: the index clamped (0x4BC240), then the record's
    /// stream index made the current song `[0x5D7938]` when it differs (and is not −1), and played
    /// at once — `MusicStop(0)`, `MusicPlay(name, 0)` — when the current screen answers its slot
    /// 0xA8 true, which only the CD player's does (0x510E90; every other screen's is 0x50DD60,
    /// false). APPROXIMATION: which layouts get that screen class is not read; the port takes the
    /// two that carry its callbacks, `audioreal` and `cdplayer`.
    fn cd_set_title(&mut self, index: usize) {
        let table = Self::cd_table(&self.cd_names());
        let Some((id, name)) = table.get(index.min(table.len().saturating_sub(1))).cloned() else { return };
        self.fe.cd_title = index.min(table.len() - 1);
        if id as i32 == self.fe.cd_current {
            return;
        }
        self.fe.cd_current = id as i32;
        let cd_screen = self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("audioreal") || n.eq_ignore_ascii_case("cdplayer"));
        if cd_screen {
            self.fe_music(|m, a| {
                m.stop(a);
                m.play_pattern(a, &name, 0);
            });
        }
        self.bind_lists();
    }

    /// The front end's music player, made on first use (the port has no menu music of its own yet)
    /// — nothing in a screenshot run.
    fn fe_music(&mut self, f: impl FnOnce(&mut nfs_audio::music::MusicPlayer, &nfs_audio::AudioOutput)) {
        if self.screenshot.is_some() {
            return;
        }
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        if self.snd.music.is_none() {
            self.snd.music = nfs_audio::music::MusicPlayer::new(&self.paths.game_data().join("Music"), &self.paths.game_data().join("Sounds"), &self.snd.audio_settings)
                .map_err(|e| tracing::warn!(error = %e, "music unavailable"))
                .ok();
        }
        if let (Some(a), Some(m)) = (self.snd.audio.as_ref(), self.snd.music.as_mut()) {
            f(m, a);
        }
    }

    /// The zone damages of the car a 3D widget's accessor holds (`FECar+0x6A8`, ten floats, the
    /// owned car's own) — the garage's selected car for `OwnedCars.Car`, the lot's for
    /// `UsedCar.Car`; nothing for a source without one.
    fn fe_car_damage(&self, accessor: Option<&str>) -> [f32; 10] {
        let Some(c) = self.career.as_ref() else { return [0.0; 10] };
        match accessor.map(|a| a.split('(').next().unwrap_or(a).trim()) {
            Some("OwnedCars.Car") => c.career.garage.get(c.car_sel).map(|o| o.damage).unwrap_or([0.0; 10]),
            Some("UsedCar.Car") => c.career.market.get(self.fe.usedcar_sel).map(|o| o.damage).unwrap_or([0.0; 10]),
            _ => [0.0; 10],
        }
    }

    /// The row of `UserList.UserList` that is selected: the clicked one, else the profile
    /// `player.lst` names — `RebuildUserList` 0x4DEF60 selects it — which is the loaded one.
    fn user_row(&self, profiles: &[nfs_game::save::Profile]) -> Option<usize> {
        match self.fe.user_sel {
            Some(i) if i < profiles.len() => Some(i),
            _ => self.career.as_ref().and_then(|c| profiles.iter().position(|p| *p == c.profile)),
        }
    }

    /// The player screens' callbacks — the front-end controller's slots 0xDC…0xE8 and 0x108
    /// (vtable 0x5B9D64), registered by name at 0x50FF40 (§4.147). Returns whether `cb` was one.
    ///
    /// * `REMOVEPLAYER` (0x510770) raises 803; its answer is `DialogFor::RemovePlayer`.
    /// * `SETNAMETHENBACK` (0x5106C0): with a row selected, `LoadInfo(stem, 0)`; loaded, the
    ///   knockout is cancelled (0x4DE040) and the screen goes back (slot 0xB4 = 0x4D23F0(0)),
    ///   otherwise 806.
    /// * `CREATEPLAYERDONE` (0x5107A0): the name box's text; nothing when empty (0x5107F9); 801
    ///   when the name is taken (0x4DE510); 805 when the profile cannot be made (0x4DE5B0 with
    ///   the picture `[0x5D8F2C]`); then `LoadInfo` of the new stem — 806 on failure — and the
    ///   whole layer closes, 0x4D23F0(−1), with sound 4 (0x4D252B).
    /// * `CANCELCREATEPLAYER` (0x510AE0): slot 0xB4, back one history entry (0x4D5FB0).
    ///
    /// Not ported: 0x4D8680, which first tries the name as one of the seven cheat codes of the
    /// table at 0x5D9618 (sound 0xC and a bit of `[0x65B940]` when it matches); 0x4F3E40, the
    /// `DealerCar` accessors' reset `SETNAMETHENBACK` makes before loading; 0x505570 each path
    /// ends in; and the driver-picture choice, so `[0x5D8F2C]` is its image value 8 — which
    /// `CREATEPLAYERENTER` (0x510780) also sets.
    fn player_callback(&mut self, cb: &str, event_loop: &ActiveEventLoop) -> bool {
        use nfs_game::dialog::DialogKind;
        let dir = nfs_game::save::save_dir();
        match cb {
            "REMOVEPLAYER" => {
                self.fe_raise_dialog(DialogKind::Two, 803, [0, 0], DialogFor::RemovePlayer);
            }
            "SETNAMETHENBACK" => {
                let profiles = nfs_game::save::list_profiles(&dir);
                let Some(p) = self.user_row(&profiles).and_then(|i| profiles.get(i)).cloned() else { return true };
                if self.career.as_mut().is_some_and(|c| c.switch_profile(&p)) {
                    self.fe.user_sel = None;
                    self.apply_saved_controls();
                    self.menu_key(MenuKey::Back, event_loop);
                } else {
                    self.fe_raise_dialog(DialogKind::One, 806, [0, 0], DialogFor::Notice);
                }
            }
            "CREATEPLAYERDONE" => {
                let name = self.fe.layout_preview.as_ref().map(|l| l.input_text("UserList.NameInput")).unwrap_or_default();
                if name.is_empty() {
                    return true;
                }
                // 0x510804: a cheat code first — sound 0xC, its bit of `[0x65B940]` set, and the
                // name box emptied (0x510818) instead of a profile made
                if let Some(id) = nfs_game::cheats::code_of(&name) {
                    self.fe_sound(0xC);
                    // id 4 would call 0x5012E0(−1) instead; no shipped code carries it
                    if id != 4 {
                        self.cheat_bits |= 1 << id;
                    }
                    tracing::info!(id, "cheat code");
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.clear_input("UserList.NameInput");
                    }
                    return true;
                }
                if nfs_game::save::name_taken(&dir, &name) {
                    self.fe_raise_dialog(DialogKind::One, 801, [0, 0], DialogFor::Notice);
                    return true;
                }
                let made = self.career.as_mut().is_some_and(|c| c.create_profile(&name, nfs_game::save::DEFAULT_PICTURE));
                if !made {
                    self.fe_raise_dialog(DialogKind::One, 805, [0, 0], DialogFor::Notice);
                    return true;
                }
                self.fe.user_sel = None;
                let p = nfs_game::save::Profile::new(name, nfs_game::save::DEFAULT_PICTURE);
                if self.career.as_mut().is_some_and(|c| c.switch_profile(&p)) {
                    self.fe_sound(4);
                    if let Some(prev) = self.fe.layout_stack.pop() {
                        self.open_layout(&prev);
                    }
                } else {
                    self.fe_raise_dialog(DialogKind::One, 806, [0, 0], DialogFor::Notice);
                }
            }
            // 0x4D5FB0 opens the screen before this one in the layer's history. This port keeps
            // no per-layer history, and the one screen that replaces itself with `createplayer`
            // is `selectname` (the only `destination=createplayer`, offset 0) — so that is it.
            "CANCELCREATEPLAYER" => self.open_layout("selectname"),
            _ => return false,
        }
        true
    }

    /// The player screens' enables from the tick 0x510210, through the by-name 0x50D880:
    /// `ID_CREATEPLAYEROKBUTTON` while the name box holds something (0x51034D) and
    /// `ID_REMOVEBUTTON` while the list is not empty (0x51036E: the accessor's slot 0x6C,
    /// 0x4B8B50, is `begin == end`). `ID_CONTINUEBUTTON`'s is gated on a local the slot-0x6C call
    /// never writes — it returns with a plain `ret` over the argument pushed for it — so what it
    /// sees is left to a running original (AUDIT.md §E) and the button is left as it stands.
    fn bind_player_buttons(&mut self) {
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        let named = l.widgets_named(&["ID_CREATEPLAYEROKBUTTON", "ID_REMOVEBUTTON"]);
        if named.is_empty() {
            return;
        }
        let empty = nfs_game::save::list_profiles(&nfs_game::save::save_dir()).is_empty();
        let typed = !l.input_text("UserList.NameInput").is_empty();
        for (index, which) in named {
            l.set_disabled_flag(index, if which == 0 { !typed } else { empty });
        }
    }

    /// `CREATE`, `JOIN`, `CONNECT`, `DIAL`, `ENTER`, `EJECTPLAYER` and `READY` — the callbacks the
    /// network layouts name.
    ///
    /// **`CREATE` and `JOIN` open a lobby screen, they do not start a race.** `0x509FA0`, the
    /// handler behind `ID_COMMSCREATESCREEN` / `ID_COMMSJOINSCREEN`, calls `0x4D2100(this,
    /// "creategame")` at 0x50A0DB and `0x4D2100(this, "joingame")` at 0x50AB31 — that helper opens
    /// the named layout. The data agrees: `creategame` carries `canplaygame=2`, so the host starts
    /// the race from it, and `joingame` carries `canplaygame=0`, so the joiner cannot — it presses
    /// `READY` (0x50A877 looks the control up by name and 0x48CAA0 tests it) and waits.
    ///
    /// `DIAL` has no modem behind it here, and `ENTER` on the server list only moves the cursor,
    /// since what the original's server list connects *to* is the lobby service of §1.
    fn network_callback(&mut self, cb: &str, _event_loop: &ActiveEventLoop) {
        let opened = match cb {
            "CREATE" => self.net.lobby.as_ref().map(|l| l.create()),
            "JOIN" | "CONNECT" => self.net.lobby.as_ref().map(|l| l.connect()),
            "DIAL" => {
                tracing::info!("the modem screen has nothing behind it in this port");
                return;
            }
            // `EJECTPLAYER` is message 0xC6, which only a host can mean — and only once a session
            // is actually running, since the lobby's socket has no peers of its own
            "EJECTPLAYER" => {
                let slot = self.net.lobby.as_ref().map(|l| l.sel).unwrap_or(0) as u8;
                match self.net.session.as_mut() {
                    Some(n) if n.is_host() => {
                        n.eject(slot + 1);
                        tracing::info!(slot, "eject sent");
                    }
                    _ => tracing::info!("nobody to eject: no session is running here"),
                }
                return;
            }
            // what the original's READY does has not been read
            "READY" => {
                tracing::info!("READY has no traced handler");
                return;
            }
            _ => {
                // ENTER on the server list: take the row under the cursor as the address to use
                if let Some(l) = self.net.lobby.as_mut() {
                    l.ip = l.target().map(|a| a.to_string()).unwrap_or_default();
                }
                self.bind_lists();
            self.bind_checks();
                return;
            }
        };
        match opened {
            Some(Ok(mut n)) => {
                let host = n.is_host();
                tracing::info!(cb, host, "network session open");
                n.positional = self.net.lobby.as_ref().is_none_or(|l| l.positional);
                self.net.session = Some(n);
                // one seat per player the session can hold, minus ourselves
                if self.num_opponents == 0 {
                    self.num_opponents = 1;
                }
                // 0x50A0DB / 0x50AB31: the lobby screen, not the race
                if let Some(cur) = self.fe.layout_name.clone() {
                    self.fe.layout_stack.push(cur);
                }
                self.open_layout(if host { "creategame" } else { "joingame" });
            }
            Some(Err(e)) => tracing::warn!(cb, error = %e, "cannot open the network session"),
            None => {}
        }
    }

    /// The analogue devices the front end can see. `NFS5.DEADZONE`'s draw returns before painting
    /// anything when there is none (0x51B60D), which is what an empty list gives — and that is
    /// what this port has on a machine with no pad. `NFS_FE_PAD=1` stands one in, all eight axes
    /// present and each a tenth of the way along, so `deadzone.lay` can be looked at without one.
    fn fe_pads(&self) -> Vec<nfs_game::layout::FePad> {
        if std::env::var("NFS_FE_PAD").is_ok() {
            return vec![nfs_game::layout::FePad { analogue: true, axes: 0xFF, values: [32767; 8] }];
        }
        // `NFS_FE_DZ=<min>,<max>` puts the sixteen values somewhere other than the factory's
        // 0 / 0x80, so a still shows what a trimmed bar looks like
        self.gamepads
            .axis_sets()
            .into_iter()
            .map(|a| nfs_game::layout::FePad { analogue: a.mask != 0, axes: a.mask, values: a.values })
            .collect()
    }

    /// `NFS_FE_DZ=<min>,<max>` — the sixteen dead-zone values for a still, in the accessors' own
    /// 0…0x80. Without it they are the factory's 0 and 0x80 (0x51E546, 0x51E550).
    fn fe_deadzone() -> Option<[[i32; 2]; 8]> {
        let v = std::env::var("NFS_FE_DZ").ok()?;
        let (a, b) = v.split_once(',')?;
        let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        Some([[a, b]; 8])
    }

    /// Advance the screens, rebuild the scene and present it. **Both** redraw arms end here: the
    /// menu arm used to handle its keys and then ask for another redraw without ever drawing, which
    /// left the window blank for the whole front end.
    fn draw_frame(&mut self) {
        // the front end's screens slide and fade in over half a second (`RE/notes/controls.md` §6)
        let dt_ms = (self.frame_dt * 1000.0).round().clamp(0.0, 250.0) as u32;
        self.fe.ms += dt_ms as u64;
        // the garage screen's tick re-decides its buttons every frame (0x50F970), and the player
        // screens' tick theirs (0x510210)
        self.bind_garage();
        self.bind_player_buttons();
        // the dialog's tick (0x512980 and the kind's own), and the covering the manager keeps
        // while it is up (0x4D2000 on the raise, 0x4D1E70 on the dismiss)
        self.fe_dialog_step();
        // the front end's music, when the CD player started some: the end-of-stream advance
        // (0x4ADD6D) the race's audio update runs
        if self.vehicle.is_none()
            && let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut())
        {
            m.update(a);
        }
        // the end-of-era show's sting, once, 128 ticks in (0x4FFA7C): `0x4A6270(k)` queues speech
        // id 4 + k — `Era-ClassictoGolden`, `Era-GoldentoModern`, `Era-CEvoModecomplet`
        // (`RE/notes/sfx.md` §7.1)
        if self.vehicle.is_none()
            && let Some(k) = self.fe.layout_preview.as_ref().and_then(|l| l.take_end_of_era_sting())
        {
            if self.snd.speech.is_none() {
                self.snd.speech = nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech"))
                    .map_err(|e| tracing::warn!(error = %e, "speech unavailable"))
                    .ok();
            }
            const ERA_STING: [&str; 3] = ["Era-ClassictoGolden.bnk", "Era-GoldentoModern.bnk", "Era-CEvoModecomplet.bnk"];
            if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                sp.say(a, ERA_STING[k.clamp(0, 2) as usize], 0.9);
            }
        }
        if self.vehicle.is_none()
            && let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech)
        {
            sp.update(a);
        }
        self.fe_career_dialog();
        self.fe_offer_check();
        // the steering band is the front end's own `XAxis.Min` / `XAxis.Max`, not a constant of
        // this port's (`nfs-input/src/gamepad.rs`)
        if let Some(l) = self.fe.layout_preview.as_ref() {
            let bands = l.deadzone_values();
            self.gamepads.set_axis_band(bands[0][0], bands[0][1]);
            self.input.axis_bands = bands;
        }
        // **A binding row waiting for a control polls the pads** — slot 1, 0x51C4E0, every frame
        // while it has focus: 0x51F3B0 against the baseline taken when it got it, and on a find
        // the same commit as a key's (`Bindings::assign`) and focus goes (0x51C5DE).
        match self.fe.layout_preview.as_ref().and_then(|l| l.focused_binding_row()) {
            Some((i, acc)) => {
                if let Some(base) = self.fe.binding_base.as_ref()
                    && let Some(b) = nfs_input::bindings::capture(base, &self.input.pads)
                {
                    if let Some(id) = accessors::controller_action(&acc).and_then(nfs_input::binding_id) {
                        self.input.bindings.assign(id, b);
                        tracing::info!(row = %acc, kind = b.kind, index = b.index, "control rebound");
                    }
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.release_focus(i);
                    }
                    self.fe.binding_base = None;
                }
            }
            None => self.fe.binding_base = None,
        }
        let pads = self.fe_pads();
        // `Camera.Current` — the slot `NFS5.CAMERASELECT` highlights (§4.89). This port keeps the
        // chosen view as an index into the registry's own cycle, which is the same thing.
        let cam_cur = self.camera_mode as usize;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.advance_transition(dt_ms);
            // hand the screen the pointer, so its widgets can pick their `hover` state (§4.39)
            l.set_cursor(self.fe.cursor);
            l.set_pads(pads, 0);
            l.set_camera_current(cam_cur);
        }
        // the bottom bar is a screen of its own (0x4D39D3) and its four buttons hover and animate
        // like any other (§4.53), so it needs the clock and the pointer too
        if let Some(b) = self.fe.layout_bottom.as_mut() {
            b.advance_transition(dt_ms);
            b.set_cursor(self.fe.cursor);
        }
        // A held scrollbar arrow repeats. 0x4D0A97: the press starts `0x529220(this, 0x96, 1)` —
        // a **150 ms** repeating timer, the same one `NFS5.CARSETTINGBUTTON` uses (§4.40) — and
        // slot 10 (0x4D0B40) steps the position each time it fires.
        if let Some((index, delta, ref mut acc)) = self.fe.list_repeat {
            *acc += dt_ms;
            let steps = *acc / SCROLL_REPEAT_MS;
            *acc %= SCROLL_REPEAT_MS;
            if steps > 0 && let Some(l) = self.fe.layout_preview.as_mut() {
                l.scroll_list(index, delta * steps as i32);
            }
        }
        // and so does a held car-setting arrow, off the very same timer (0x4C1F55, 0x4C1F80)
        if let Some((name, step, acc)) = self.fe.setting_repeat.take() {
            let acc = acc + dt_ms;
            let steps = acc / SCROLL_REPEAT_MS;
            self.fe.setting_repeat = Some((name.clone(), step, acc % SCROLL_REPEAT_MS));
            for _ in 0..steps {
                self.set_accessor(&name, step);
            }
            if steps > 0 {
                self.bind_car_logos();
                self.bind_indicator_bars();
                self.bind_lists();
                self.bind_checks();
            }
        }
        // and the movies on it, which need the renderer to put each frame in its texture
        if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
            l.advance_movies(r, dt_ms);
        }
        // the front end's own pointer (`controls.md` §4.152), and where the turntable stands,
        // which is what a hovered `NFS5.3DCAR` picks its cursor by (§4.153)
        let spin = self.fe_room_of().map(|r| r.spin_turns(self.fe.ms)).unwrap_or(0.0);
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.advance_cursor(dt_ms, self.fe.mouse_down);
            l.set_car_spin(spin);
        }
        // a network screen probes once a second and refills its lists from the answers
        if self.net.lobby.is_some() {
            let before = self.net.lobby.as_ref().map(|l| l.found.len());
            if let Some(l) = self.net.lobby.as_mut() {
                l.poll();
            }
            if before != self.net.lobby.as_ref().map(|l| l.found.len()) {
                self.bind_lists();
            self.bind_checks();
            }
        }
        if let Some(b) = self.fe.layout_bottom.as_mut() {
            b.advance_transition(dt_ms);
        }
        self.rebuild_scene();
        self.update_damage_decals();
        self.update_env_map();
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

    /// Start the race the front end's own settings describe (the same path the text menu takes).
    /// The "evenstvn" name-box code, cheat bit 3: the launch's per-car routine 0x4EEDA0 sends every
    /// car but a traffic one (kind 4) `.SIM` 0x13E = `"993coupe36"` and the model 0x13F = `"993"`
    /// in place of the record's own `+0xB5` / `+0x119` (0x4EEE9B…0x4EEEC6) — the player's car, the
    /// opponents and the cops all become 993 coupés; everything else of the record stays.
    fn cheat_993(&self, mut car: nfs_formats::fedata::CarCatalogEntry) -> nfs_formats::fedata::CarCatalogEntry {
        if self.cheat_bits & (1 << 3) != 0 {
            car.model = "993".to_string();
            car.sim = "993coupe36".to_string();
        }
        car
    }

    /// The "Smash Up" name-box code, cheat bit 7 (`nfs_game::cheats`): after every mode's own
    /// options the launch appends `DERBY` 0x1B = 1, `LINEAR_TRACK` 0x1D = 0, `RACE_TYPE` 0x10 = **3**
    /// (the lap knockout), `TRACK_NAME` 0x10F = `"Skidpad"`, `DAMAGE` 0x20 = 1 and `NOCOLLISION`
    /// 0x29 = 0 (0x4ED7D9…0x4ED841), so whatever was chosen runs as a lap knockout on the skid pad
    /// with the mode's own lap count. `DERBY` itself is read only by the network grid spacing
    /// (`modes.md` §1), and this port's races always take damage.
    fn derby_track(&mut self, track: &str) -> String {
        if self.cheat_bits & (1 << 7) == 0 {
            return track.to_string();
        }
        self.quick_knockout = true;
        "skidpad".to_string()
    }

    /// A test drive, `Game.GameType` 6 / 7 / 8. The car is 0x4EB710's per type — 6 a new
    /// `DealerCar.Car`, built fresh (0x4EB7C3…0x4EB859), 7 `UsedCar.Car` and 8 `OwnedCars.Car`,
    /// each the record as it stands (0x4EB7A6, 0x4EB85E → 0x4EB727); the race is 0x4FC0B0's for
    /// 6…8 — `Track1.Track`, one lap, forward, **five** traffic cars — with **no** opponents
    /// (0x4ECF00) and no cops.
    fn start_test_drive(&mut self, game_type: u8, event_loop: &ActiveEventLoop) {
        let Some(m) = self.fe.menu.as_ref() else { return };
        let track_name = m.track().to_string();
        let (car, owned) = match game_type {
            6 => {
                let Some(car) = m.car().cloned() else { return };
                (car, None)
            }
            7 => {
                let Some(o) = self.used_car().cloned() else { return };
                let Some(car) = self.career.as_ref().and_then(|c| c.tables.car(o.car_id)).cloned() else { return };
                (car, Some(o))
            }
            _ => {
                let Some(c) = self.career.as_ref() else { return };
                let Some(o) = c.career.garage.get(c.car_sel).cloned() else { return };
                let Some(car) = c.tables.car(o.car_id).cloned() else { return };
                (car, Some(o))
            }
        };
        let paint_set = owned.as_ref().map_or(m.paint_idx, |o| o.paint_set as usize);
        self.paint = car.paint_sets.get(paint_set % car.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
        self.num_opponents = 0;
        self.num_laps = 1;
        self.backwards = false;
        self.mirror = false;
        self.road.traffic_count = 5;
        self.road.cops_enabled = false;
        self.road.cops_race = false;
        self.career_opponents = None;
        self.quick_knockout = false;
        let car = self.cheat_993(car);
        // a record car's parts write the style overlay (0x49C5F7), as in a career race
        let swaps: Vec<[i16; 3]> = match (owned.as_ref(), self.career.as_ref()) {
            (Some(o), Some(cu)) => o.parts.iter().filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).flat_map(|p| p.geometry_swaps.iter().copied()).collect(),
            _ => Vec::new(),
        };
        tracing::info!(track = track_name, car = car.model, game_type, "front end: test drive");
        match (
            { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
            CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio).with_style_overlay(swaps.clone())),
            CarAssets::load_sim(&self.paths, if car.sim.is_empty() { "default" } else { &car.sim }),
        ) {
            (Ok(t), Ok(c), Ok(sim)) => {
                self.track = t;
                self.car = c;
                let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                self.specs = CarSpecs::from_sim(&sim, race_class);
                // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                if self.cheat_bits & (1 << 1) != 0 {
                    self.specs.apply_tinycar();
                }
                // a record car brings its parts, their wear and its setup, as a career race does
                if let (Some(o), Some(cu)) = (owned.as_ref(), self.career.as_ref()) {
                    let parts: Vec<&nfs_formats::career::Part> = o.parts.iter().filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).collect();
                    self.specs.apply_parts(&parts, &car.sim_family);
                    self.specs.zone_masks = nfs_game::career::zone_masks_for(&cu.tables, &car);
                    self.specs.wear = nfs_game::career::part_wear_for(&cu.tables, o);
                    self.specs.apply_setup(&o.setup, self.player_automatic());
                }
                self.snd.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_tach));
                self.fe.test_drive = Some((game_type, owned));
                self.fe.menu_template = self.fe.menu.take();
                self.fe.layout_preview = None;
                self.fe.layout_name = None;
                let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                self.tick = 0;
                self.start_race(renderer, window, event_loop);
            }
            (t, c, s) => tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "test drive load failed"),
        }
    }

    fn start_from_layout(&mut self, event_loop: &ActiveEventLoop) {
        let Some(m) = self.fe.menu.as_ref() else { return };
        let Some(car) = m.car().cloned() else { return };
        let track_name = m.track().to_string();
        self.num_opponents = m.opponents;
        self.num_laps = m.laps;
        self.backwards = m.backwards;
        self.mirror = m.mirror;
        self.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
        // a layout race is the single race, game type 0: the Fuzzyfuz code (bit 5) makes it the cops
        // race — `RACE_TYPE` 1 (0x4EB4FD) and four cop cars (0x4ED0BC…0x4ED282)
        self.road.cops_race = self.cheat_bits & (1 << 5) != 0;
        self.road.cops_enabled = m.cops || self.road.cops_race;
        self.paint = car
            .paint_sets
            .get(m.paint_idx % car.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        self.quick_knockout = false;
        let car = self.cheat_993(car);
        tracing::info!(track = track_name, car = car.model, "front end: start race");
        match (
            { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
            CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio)),
            CarAssets::load_sim(
                &self.paths,
                if car.sim.is_empty() {
                    "default"
                } else {
                    &car.sim
                },
            ),
        ) {
            (Ok(t), Ok(c), Ok(sim)) => {
                self.track = t;
                self.car = c;
                let race_class =
                    car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                self.specs = CarSpecs::from_sim(&sim, race_class);
                // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                if self.cheat_bits & (1 << 1) != 0 {
                    self.specs.apply_tinycar();
                }
                self.snd.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_tach));
                self.fe.layout_preview = None;
                self.fe.layout_name = None;
                self.fe.menu = None;
                let (renderer, window) =
                    (self.renderer.take().unwrap(), self.window.take().unwrap());
                self.tick = 0;
                self.start_race(renderer, window, event_loop);
            }
            (t, c, s) => {
                tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed")
            }
        }
    }

    /// The write half of `accessor_value`: a combo slider moves the port's own setting the accessor
    /// stands for. Only the names the port has state for do anything.
    /// Set a range accessor to an absolute value — what `NFS5.GouraudIndicatorBar`'s pointer
    /// slots do (0x4CA548 and its twin in 0x4CA590) once they have turned the pointer's x into an
    /// index. `bar_at` answers the fraction; the count is the accessor's own `max - min + 1`,
    /// which is the list 0x4BA329 builds, and the index is the value less the minimum (0x4BA34A).
    /// `RE/notes/controls.md` §4.81.
    fn set_accessor_fraction(&mut self, name: &str, f: f32) {
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        let Some((_, lo, hi)) = self.range_value(base) else { return };
        let n = (hi - lo + 1).max(1);
        // 0x5A0F98 truncates toward zero and 0x4CA535 pins the top index, so the bar can never
        // answer `n` itself
        let v = ((n as f32 * f) as i32).min(n - 1).max(0) + lo;
        if base.starts_with("Audio.") {
            let a = &mut self.snd.audio_settings;
            let v = v.clamp(0, 127) as u8;
            match base {
                "Audio.MusicLevel" => a.music_level = v,
                "Audio.SfxLevel" => a.sfx_level = v,
                "Audio.EngineLevel" => a.engine_level = v,
                "Audio.SpeechLevel" => a.speech_level = v,
                "Audio.AmbientLevel" => a.ambient_level = v,
                _ => {}
            }
            return;
        }
        if let Some(key) = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == base).map(|(_, k, ..)| *k) {
            self.choices.insert(key.to_string(), v.clamp(lo, hi) as usize);
        }
    }

    fn set_accessor(&mut self, name: &str, delta: i32) {
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        if let Some((v, lo, hi)) = self.range_value(base).filter(|_| !base.starts_with("Audio.")) {
            let key = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == base).map(|(_, k, ..)| *k).unwrap_or_default();
            // **one index per press.** An int range is a list of `max − min + 1` entries
            // (0x4BA329 builds exactly that count) whose index *is* the value less the minimum
            // (0x4BA34A / 0x4BA362), and the combo slider adds its own `[widget+0x1A8]` to the
            // accessor's current index (0x4C6CFF…0x4C6D10) — which the `.lay` sets from a
            // `NFS5.CARSETTINGBUTTON`'s `step`, and every one of the 34 shipped buttons carries
            // ±1. So the volume sliders move by one of 128 steps, not by a twentieth of the range
            // as this port used to assume.
            self.choices.insert(key.to_string(), (v + delta).clamp(lo, hi) as usize);
            return;
        }
        if let Some((cur, options)) = self.choice_index(base) {
            let key = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == base).map(|(_, k, _)| *k).unwrap_or_default();
            let next = (cur as i32 + delta).rem_euclid(options.len() as i32) as usize;
            self.choices.insert(Self::choice_slot(base, key).to_string(), next);
            return;
        }
        // the network screens' own sliders and lists
        if base == "Comms.Positional"
            && let Some(n) = self.net.session.as_mut()
        {
            n.positional = !n.positional;
            return;
        }
        if let Some(l) = self.net.lobby.as_mut() {
            match base {
                "Comms.Positional" => {
                    l.positional = !l.positional;
                    return;
                }
                "Comms.ServerList"
                | "Comms.GamesList"
                | "Comms.IPList"
                | "Comms.LobbyPlayerList"
                | "Comms.PlayerList" => {
                    let n = l.found.len().max(1);
                    l.sel = ((l.sel as i32 + delta).rem_euclid(n as i32)) as usize;
                    return;
                }
                _ => {}
            }
        }
        let Some(m) = self.fe.menu.as_mut() else { return };
        let step = |i: usize, n: usize| ((i as i32 + delta).rem_euclid(n.max(1) as i32)) as usize;
        match base {
            // the knockout screen's four combo boxes edit the selected row of its table
            "Knockout.Track" | "Knockout.Laps" | "Knockout.Traffic" | "Knockout.Direction" => {
                let n = m.tracks.len();
                let sel = m.ko_sel;
                m.fit_ko();
                let Some(k) = m.ko.get_mut(sel) else { return };
                match base {
                    "Knockout.Track" => k.track_idx = ((k.track_idx as i32 + delta).rem_euclid(n.max(1) as i32)) as usize,
                    "Knockout.Laps" => k.laps = (k.laps as i32 + delta).clamp(1, 10) as u32,
                    "Knockout.Traffic" => k.traffic = ((k.traffic as i32 + delta).rem_euclid(4)) as u32,
                    _ => k.backwards = !k.backwards,
                }
            }
            "Track1.Track" | "LoopedTrack.Track" | "Record.Track" => {
                m.track_idx = step(m.track_idx, m.tracks.len())
            }
            // the opponent combo steps the five choices, not a car list
            "OpponentCar.Car" => m.opp_car = ((m.opp_car as i32 + delta).rem_euclid(5)) as u8,
            // the used-car lot steps its own selection, not the catalogue's
            "UsedCar.Car" => {
                let n = self.used_cars().len();
                self.fe.usedcar_sel = step(self.fe.usedcar_sel, n);
                self.bind_indicator_bars();
            }
            "zMergedList.Car" | "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car"
            | "CARSETTINGS.CAR" => {
                m.car_idx = step(m.car_idx, m.cars.len());
                m.paint_idx = 0;
            }
            // the compare screen's own combo box moves only the right-hand car
            "GenericCar2.Car" => m.car2_idx = step(m.car2_idx, m.cars.len()),
            // two accessors, two ranges (0x4E5539 and 0x4E556C): 0…7 for the race, **1**…7 for
            // the knockout table, both starting at 4
            // 0x4BC240 clamps the new index to `count - 1` and then to 0, so the two rows wrap
            // into nothing — the stepper simply runs off each end
            "Game.Damage" => m.damage = (usize::from(m.damage) as i32 + delta).clamp(0, 1) != 0,
            "Game.NumOpponents" => m.opponents = (m.opponents as i32 + delta).clamp(0, 7) as u32,
            "Game.NumKOOpponents" => {
                m.ko_opponents = (m.ko_opponents as i32 + delta).clamp(1, 7) as u32;
                m.fit_ko();
            }
            "Track1.Laps" => m.laps = (m.laps as i32 + delta).clamp(1, 10) as u32,
            "Track1.Traffic" => {
                m.traffic = (m.traffic as i32 + delta).clamp(0, 3) as u32
            }
            "Track1.Direction" => m.backwards = !m.backwards,
            "OwnedCar.ExtColour" | "GenericCar.Colour" | "DealerCar.Colour" => {
                let Some(c) = m.car() else { return };
                let n = c.paint_sets.len().max(1);
                m.paint_idx = step(m.paint_idx, n);
            }
            _ => {
                // the audio levels are 0..127 and the mixers read them every tick, so a slider
                // changes the mix straight away
                let a = &mut self.snd.audio_settings;
                let lvl = |v: u8| (v as i32 + delta * 8).clamp(0, 127) as u8;
                match base {
                    "Audio.MusicLevel" => a.music_level = lvl(a.music_level),
                    "Audio.SfxLevel" => a.sfx_level = lvl(a.sfx_level),
                    "Audio.EngineLevel" => a.engine_level = lvl(a.engine_level),
                    "Audio.SpeechLevel" => a.speech_level = lvl(a.speech_level),
                    "Audio.AmbientLevel" => a.ambient_level = lvl(a.ambient_level),
                    _ => {}
                }
            }
        }
    }

    fn open_layout(&mut self, name: &str) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        match nfs_game::layout::LayoutScreen::load(r, &self.paths, name) {
            Ok(mut l) => {
                l.cheat_bits = self.cheat_bits;
                if self.fe.layout_bottom.is_none()
                    && let Some(r) = self.renderer.as_mut()
                {
                    self.fe.layout_bottom =
                        nfs_game::layout::LayoutScreen::load(r, &self.paths, "bottombar")
                            .map_err(|e| tracing::debug!(error = %e, "no bottom bar"))
                            .ok();
                    if let Some(b) = self.fe.layout_bottom.as_mut() {
                        b.set_overlay(true);
                    }
                }
                self.fe.layout_preview = Some(l);
                self.fe.layout_name = Some(name.to_string());
                self.fe.layout_sel = 0;
                self.fe.layout_open_combo = None;
                // a freshly opened screen needs the device list before its first draw, not after
                let (pads, dz) = (self.fe_pads(), Self::fe_deadzone());
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_pads(pads, 0);
                    if let Some(dz) = dz {
                        l.set_deadzone_values(0, dz);
                    }
                }
                // the four screens whose `callbacklist=` is a comms one need a browsing socket
                // a browsing socket is only wanted while looking for a game; once a session is
                // open the lobby screens read from it instead
                self.net.lobby = match is_network_layout(name) && self.net.session.is_none() {
                    true => netplay::Lobby::open(&self.player_name)
                        .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                        .ok(),
                    false => None,
                };
                self.bind_car_logos();
                self.bind_lists();
            self.bind_checks();
                self.bind_indicator_bars();
                self.bind_chronicle_icons();
            self.bind_chronicle_group();
            self.bind_repair_bars();
            self.bind_usedcars_panels();
            self.bind_garage();
            self.bind_player_buttons();
            self.bind_parts_buttons();
            self.bind_mergedlist_panel();
            self.bind_showcase_pictures();
            self.bind_track_maps();
            self.bind_player_pictures();
            self.bind_end_of_era();
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.transition_in();
                }
            }
            Err(e) => tracing::warn!(error = %e, name, "layout unavailable"),
        }
    }

    /// Silence the race audio (engine layers, squeal / horn loops) when leaving to the menu.
    fn stop_race_audio(&mut self) {
        if let Some(a) = &self.snd.audio {
            if let Some(mut e) = self.snd.engine_sound.take() {
                e.stop(a);
            }
            if let Some(s) = self.snd.sfx.as_mut() {
                s.stop_loops(a);
            }
            if let Some(ai) = self.snd.ai_engines.as_mut() {
                ai.stop_all(a);
            }
            if let Some(amb) = self.snd.ambience.as_mut() {
                amb.stop(a);
            }
        }
    }

    /// Mission over: evaluate the skill test (or the race position) and store the result.
    fn settle_mission(&mut self) {
        let Some(ms) = self.mission.clone() else {
            return;
        };
        if !self
            .career
            .as_ref()
            .is_some_and(|c| c.pending.as_ref().is_some_and(|p| p.mission.is_some()))
        {
            return;
        }
        let (order, times) = self.finishing_order();
        let position = order
            .iter()
            .position(|d| *d == 0)
            .map(|p| p + 1)
            .unwrap_or(8);
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
                if let Some(r) = ms.mission.min_rank {
                    pass &= position <= r as usize;
                }
                if let Some(limit) = ms.mission.time_limit_s {
                    pass &= t < limit as u64 * 64;
                }
                if let Some(d) = ms.mission.damage_limit_pct {
                    pass &= damage <= d as f32;
                }
                (pass, t, 0)
            }
        };
        if let Some(c) = self.career.as_mut() {
            c.finish_mission(ms.index, pass, t, cones);
            tracing::info!(
                mission = ms.mission.code,
                pass,
                time_s = t as f32 / 64.0,
                cones,
                message = c.message,
                "mission recorded"
            );
        }
    }

    /// Finishing order as driver indices (0 = player, 1.. = opponents in grid order): finished cars
    /// by total time, then unfinished cars by progress.
    fn finishing_order(&self) -> (Vec<usize>, [u64; 8]) {
        let key = |p: &RaceProgress, last_tick: u64| -> (u8, u64, i64) {
            if p.finished {
                (0, p.lap_times.iter().sum::<u64>(), 0)
            } else {
                (
                    1,
                    0,
                    -(p.progress * 1e6) as i64 + last_tick as i64 / 1_000_000,
                )
            }
        };
        let mut all: Vec<(usize, (u8, u64, i64))> = Vec::new();
        let mut times = [0u64; 8];
        if let Some(r) = &self.race {
            all.push((0, key(r, self.player_progress_tick)));
            if r.finished {
                times[0] = r.lap_times.iter().sum();
            }
        }
        for (i, o) in self.road.opponents.iter().enumerate() {
            all.push((i + 1, key(&o.progress, o.last_progress_tick)));
            if o.progress.finished && i + 1 < 8 {
                times[i + 1] = o.progress.lap_times.iter().sum();
            }
        }
        all.sort_by_key(|(_, k)| *k);
        (all.into_iter().map(|(i, _)| i).collect(), times)
    }

    fn resume_inner(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("Porsche Unleashed (native)")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut renderer = Renderer::new(window.clone()).expect("renderer");
        if let Some(m) = self.fe.menu.as_mut() {
            let dir = self.paths.fe_data().join("Art");
            m.font = GamePaths::find_file(&dir, "Univers14b.ffn")
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
                .map(|f| BitmapFont::upload(&mut renderer, f));
        }
        if self.fe.menu.is_some() {
            // `--layout <name>`: draw the original front-end screen instead of the text menu
            if let Some(name) = self.fe.layout_name.clone() {
                match nfs_game::layout::LayoutScreen::load(&mut renderer, &self.paths, &name) {
                    Ok(mut l) => {
                        l.cheat_bits = self.cheat_bits;
                        self.fe.layout_bottom = nfs_game::layout::LayoutScreen::load(
                            &mut renderer,
                            &self.paths,
                            "bottombar",
                        )
                        .ok();
                        if let Some(b) = self.fe.layout_bottom.as_mut() {
                            b.set_overlay(true);
                        }
                        self.fe.layout_preview = Some(l);
                        // the same as `open_layout`: the device list before the first draw
                        let (pads, dz) = (self.fe_pads(), Self::fe_deadzone());
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.set_pads(pads, 0);
                            if let Some(dz) = dz {
                                l.set_deadzone_values(0, dz);
                            }
                        }
                    }
                    Err(e) => tracing::error!(error = %e, name, "layout unavailable"),
                }
            }
            self.renderer = Some(renderer);
            self.window = Some(window);
            self.last_frame = Instant::now();
            self.fps_timer = Instant::now();
            // `--layout serverlist` and friends start straight on a network screen, which needs a
            // browsing socket just as `open_layout` gives one
            if let Some(name) = self.fe.layout_name.as_deref()
                && is_network_layout(name)
            {
                self.net.lobby = netplay::Lobby::open(&self.player_name)
                    .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                    .ok();
            }
            self.bind_car_logos();
            self.bind_lists();
            self.bind_checks();
            self.bind_indicator_bars();
            self.bind_chronicle_icons();
            self.bind_chronicle_group();
            self.bind_repair_bars();
            self.bind_usedcars_panels();
            self.bind_garage();
            self.bind_player_buttons();
            self.bind_parts_buttons();
            self.bind_mergedlist_panel();
            self.bind_showcase_pictures();
            self.bind_track_maps();
            self.bind_player_pictures();
            self.bind_end_of_era();
            if let Some(l) = self.fe.layout_preview.as_ref() {
                for (c, n) in l.unrendered() {
                    tracing::info!(
                        class = c,
                        count = n,
                        "layout widget placed by code, not drawn"
                    );
                }
                for (c, src) in l.unrendered_sources() {
                    tracing::info!(class = c, source = src, "unbound widget source");
                }
            }
            // `--sprite-selftest`: draw three quads through the real sprite path — a grey ground,
            // a black square and a white square — and print what came back. The smallest case that
            // can show the fault in `RE/AUDIT.md` F2, using the pipeline the front end uses.
            if std::env::var("NFS_SPRITE_SELFTEST").is_ok() {
                let white = self.fe.layout_preview.as_ref().map(|l| l.white_texture());
                let atlas = self.fe.layout_preview.as_ref().and_then(|l| l.a_font_texture());
                if let (Some(tex), Some(r)) = (white, self.renderer.as_mut()) {
                    let q = |x: f32, c: [f32; 4]| nfs_renderer::Sprite {
                        texture: tex,
                        pos: [x, 0.25],
                        size: [0.2, 0.2],
                        uv_min: [0.0, 0.0],
                        uv_max: [1.0, 1.0],
                        color: c,
                        rotation: 0.0,
                        pivot: [0.0, 0.0],
                    };
                    let mut scene = nfs_renderer::Scene::default();
                    let _ = &atlas;
                    scene.sprites = vec![
                        nfs_renderer::Sprite { pos: [0.0, 0.0], size: [1.0, 1.0], ..q(0.0, [0.5, 0.5, 0.5, 1.0]) },
                        q(0.1, [0.0, 0.0, 0.0, 1.0]),
                        q(0.4, [1.0, 1.0, 1.0, 1.0]),
                        q(0.7, [1.0, 0.0, 0.0, 1.0]),
                        // the same three at the size a glyph is actually drawn
                        nfs_renderer::Sprite { pos: [0.15, 0.6], size: [0.01875, 0.01875], ..q(0.0, [0.0, 0.0, 0.0, 1.0]) },
                        nfs_renderer::Sprite { pos: [0.45, 0.6], size: [0.01875, 0.01875], ..q(0.0, [1.0, 1.0, 1.0, 1.0]) },
                        nfs_renderer::Sprite { pos: [0.75, 0.6], size: [0.01875, 0.01875], ..q(0.0, [1.0, 0.0, 0.0, 1.0]) },
                    ];
                    // and the same three with the font atlas, sampling one texel that the ASCII
                    // dump of `Univers8b` shows solid inside 'A' (atlas 240×61, glyph at 26,32)
                    if let Some(atlas) = atlas {
                        let uv = [28.5 / 240.0, 37.5 / 61.0];
                        let a = |x: f32, c: [f32; 4]| nfs_renderer::Sprite {
                            texture: atlas,
                            pos: [x, 0.8],
                            size: [0.01875, 0.01875],
                            uv_min: [uv[0] - 0.0005, uv[1] - 0.0005],
                            uv_max: [uv[0] + 0.0005, uv[1] + 0.0005],
                            color: c,
                            rotation: 0.0,
                            pivot: [0.0, 0.0],
                        };
                        scene.sprites.push(a(0.15, [0.0, 0.0, 0.0, 1.0]));
                        scene.sprites.push(a(0.45, [1.0, 1.0, 1.0, 1.0]));
                        scene.sprites.push(a(0.75, [1.0, 0.0, 0.0, 1.0]));
                    }
                    let rgba = r.render_to_image(&scene, &self.camera, 200, 100).expect("render");
                    let at = |x: usize, y: usize| {
                        let i = (y * 200 + x) * 4;
                        (rgba[i], rgba[i + 1], rgba[i + 2])
                    };
                    println!("big:   ground {:?}  black {:?}  white {:?}  red {:?}", at(5, 50), at(30, 30), at(90, 30), at(150, 30));
                    println!("glyph: black {:?}  white {:?}  red {:?}", at(31, 61), at(91, 61), at(151, 61));
                    println!("atlas: black {:?}  white {:?}  red {:?}", at(31, 81), at(91, 81), at(151, 81));
                }
                event_loop.exit();
                return;
            }
            if self.layout_sweep {
                self.layout_sweep = false;
                self.run_layout_sweep();
                event_loop.exit();
                return;
            }
            if let Some(path) = self.screenshot.take() {
                // a screen fades in over half a second (`RE/notes/controls.md` §6), so a shot taken
                // on the first frame catches every faded widget at alpha 0 and looks empty. Let the
                // transition finish first.
                if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                    // a movie starts on a black frame and runs at fifteen a second, so a screen
                    // with one gets the same half second the fade does, whether or not the fade
                    // has already settled. A screenshot of a playing movie catches whatever frame
                    // its clock is on; this picks a consistent one.
                    let least = if l.has_movies() { 32 } else { 0 };
                    for i in 0..120 {
                        l.advance_movies(r, 16);
                        if l.advance_transition(16) && i >= least {
                            break;
                        }
                    }
                }
                // a network screen is taken on its first frame, before any `'d'` has had time to
                // come back, so give the probe half a second — otherwise the shot shows an empty
                // list that says nothing about the screen
                if self.net.lobby.is_some() {
                    let until = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < until {
                        if let Some(l) = self.net.lobby.as_mut() {
                            l.poll();
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    self.bind_lists();
                    self.bind_checks();
                }
                // the same scene the front end draws every frame, so a screenshot shows the 3D room too
                self.rebuild_scene();
                let mut scene = self.scene.clone();
                // `NFS_FE_KEEP=<n>` renders only the first n sprites of the front end's own list —
                // the bisect for `RE/AUDIT.md` F2
                if let Ok(n) = std::env::var("NFS_FE_KEEP").map(|v| v.parse::<usize>().unwrap_or(usize::MAX)) {
                    println!("fe sprites: {}, keeping {}", scene.sprites.len(), n.min(scene.sprites.len()));
                    scene.sprites.truncate(n);
                }
                let scene = scene;
                let rgba = self
                    .renderer
                    .as_mut()
                    .unwrap()
                    .render_to_image(&scene, &self.camera, 1280, 800)
                    .expect("render");
                image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8).expect("png");
                event_loop.exit();
            }
            return;
        }
        self.start_race(renderer, window, event_loop);
    }

    fn start_race(
        &mut self,
        mut renderer: Renderer,
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
    ) {
        // the player's typed name is the car's `carinfo+0x17C`, which the specs' tail compares
        // with the two cheat names (0x49D6E7…0x49D875), in the gear loop beside the manual ×1.05
        if let Some(name) = self.career.as_ref().map(|c| c.profile.name.clone()) {
            let automatic = self.player_automatic();
            self.specs.apply_driver_name(&name, automatic);
        }
        // `Replay_startRace` 0x413EA0 + `0x414EB0`: rewind the stream and seed the shared RNG from
        // the race's seed, which is what makes a replay re-simulate the same race (replay.md §11).
        let playing = self
            .replay.stream
            .as_ref()
            .is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing);
        if let Some(r) = self.replay.stream.as_mut() {
            r.start_race();
        }
        if playing {
            // `Replay_startRace` 0x413EA0 opens playback on the **first** entry of the cycle
            // table, `DirectorsCam` (0x14) — "whichever track-side camera the viewed car is
            // currently passing", the classic NFS director's cut. Playback does not cycle the
            // car's own camera list at all: it has its own nine ids (`replay.md` §8).
            self.replay.camera = 0;
        }
        if !playing {
            // A fresh race gets a fresh seed unless the run pinned one.
            self.traffic_seed = std::env::var("NFS_TRAFFIC_SEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u32)
                        .unwrap_or(1)
                });
            self.replay.setup = nfs_game::replay::ReplaySetup {
                track: self.track.name.clone(),
                car: self.car.model.clone(),
                sim: self.sim_name.clone(),
                car_id: self.car_id,
                paint: self.paint,
                laps: self.num_laps,
                opponents: self.num_opponents,
                traffic: TRAFFIC_COUNT
                    .iter()
                    .position(|c| *c == self.road.traffic_count)
                    .unwrap_or(0) as u32,
                cops: self.road.cops_enabled,
                backwards: self.backwards,
                mirror: self.mirror,
                seed: self.traffic_seed,
                skill: self.race_skill,
            };
        }
        self.replay.setup.seed = self.traffic_seed;
        self.replay.saved = false;
        self.replay.loop_count = 0;
        nfs_game::rng::seed(self.traffic_seed);
        let mut world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
        // the car shadow projectors' texture, `[0x6115CC]+0x18C` (`crp-geometry.md` §8)
        renderer.set_projector_texture(world.projector_texture);
        self.car.tiny = self.cheat_bits & (1 << 1) != 0;
        let car = CarModel::upload_with_env(
            &mut renderer,
            &self.car,
            &self.paths,
            1,
            self.paint,
            world.env_map,
            self.track.is_night(),
            // `.CABRIODOWN` 0x24B: the player's car (kind 1) keeps `Car.Roof`, which 0x4F1E40
            // forces to 0 on a car without the convertible flag; every other car is sent 0
            // (0x4EEF0C…0x4EEF1A)
            Some(if self.car.cabrio { self.choice_index("Car.Roof").map_or(0, |(i, _)| i as u32) } else { 0 }),
        )
        .expect("car");
        self.sky = Sky::upload(&mut renderer, &self.track);
        // a night track switches the dials to their `*N` faces and needles (`hud.md` §3.2)
        let night = self.track.is_night();
        if night {
            tracing::info!(track = self.track.name, "night track");
        }
        self.hud = Hud::load(&mut renderer, &self.paths, self.dials, night);
        if let (Some(h), Some(t)) = (
            self.hud.as_mut(),
            SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok(),
        ) {
            h.set_map(
                &mut renderer,
                &if self.backwards { t.reversed() } else { t },
            );
        }
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new()
                .map_err(|e| tracing::warn!(error = %e, "audio disabled"))
                .ok();
        }
        if let Some(a) = &self.snd.audio {
            self.snd.engine_sound = nfs_audio::engine::EngineSound::load(
                a,
                &self.paths.game_data().join("Sounds"),
                &self.snd.sound_set,
                &self.sim_name,
            )
            .map_err(|e| tracing::warn!(error = %e, "engine sound unavailable"))
            .ok();
            // track ambience (`engine-audio.md` §7): the `<track>_audio.scn` emitters over `track.viv`
            self.snd.ambience = match self.track.load_scene("_audio.scn") {
                Ok(Some(scene)) => nfs_audio::ambience::Ambience::load(
                    &self.paths.game_data().join("Sounds"),
                    &[&scene],
                    self.snd.audio_settings.ambient_level,
                )
                .map_err(|e| tracing::warn!(error = %e, "ambience unavailable"))
                .ok(),
                other => {
                    if let Err(e) = other {
                        tracing::warn!(error = %e, "audio scene unreadable");
                    }
                    None
                }
            };
            if self.snd.ai_engines.is_none() {
                self.snd.ai_engines =
                    nfs_audio::ai_engine::AiEngines::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "AI engine banks unavailable"))
                        .ok();
            }
            if self.snd.sfx.is_none() {
                self.snd.sfx =
                    nfs_audio::sfx::CollisionSfx::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "collision sfx unavailable"))
                        .ok();
            }
            if self.snd.speech.is_none() {
                self.snd.speech =
                    nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech"))
                        .map_err(|e| tracing::warn!(error = %e, "speech unavailable"))
                        .ok();
            }
            self.snd.countdown_said = 0;
            if self.snd.music.is_none() && self.autotest.is_none() && self.screenshot.is_none() {
                // `MusicPlay(NULL, 0)` at race start: any entry of the saved playlist but the current one
                self.snd.music = nfs_audio::music::MusicPlayer::new(
                    &self.paths.game_data().join("Music"),
                    &self.paths.game_data().join("Sounds"),
                    &self.snd.audio_settings,
                )
                .map_err(|e| tracing::warn!(error = %e, "music unavailable"))
                .ok();
                if let Some(m) = self.snd.music.as_mut() {
                    m.start_race(a);
                }
            }
        }
        // start pose from the slice-based grid rule (pole position, single racer)
        // `<track>.jnc` carries the branch weights the AI's junction choice reads (`race-logic.md` §10)
        let table = SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok().map(|mut t| {
            if let Some(p) = GamePaths::find_file(&self.track.dir, &format!("{}.jnc", self.track.name))
                && let Ok(d) = std::fs::read(&p)
                && !t.apply_jnc(&d)
            {
                tracing::warn!(path = %p.display(), "junction weights rejected: the file does not match the node count");
            }
            if self.backwards { t.reversed() } else { t }
        });
        let start_scene = if self.backwards {
            self.track
                .start_backward
                .as_ref()
                .or(self.track.start_forward.as_ref())
        } else {
            self.track.start_forward.as_ref()
        };
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
                tracing::info!(
                    slice,
                    lap_slices = t.lap_len,
                    grid = self.player_grid,
                    cars = n_cars,
                    "grid pose"
                );
                // opponents: distinct catalogue models, parked on their grid slots
                self.road.opponents.clear();
                self.road.opponent_models.clear();
                self.road.opponent_sets.clear();
                self.road.opponent_paints.clear();
                let catalog = nfs_formats::fedata::parse_car_catalog(
                    &std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car"))
                        .unwrap_or_default(),
                )
                .unwrap_or_default();
                // (catalogue entry, paint, name, bend-speed factor) per grid slot
                let plan: Vec<(
                    nfs_formats::fedata::CarCatalogEntry,
                    [u8; 3],
                    String,
                    Option<(f32, [[f32; 2]; 4])>,
                    Option<u32>,
                )> = match &self.career_opponents {
                    Some(list) => list
                        .iter()
                        .map(|o| {
                            (
                                o.car.clone(),
                                o.paint,
                                o.name.clone(),
                                Some((o.difficulty, o.skill)),
                                o.grid,
                            )
                        })
                        .collect(),
                    // quick race: `FEOpponentCarAccessor` 0x4F4FA0 (`modes.md` §1.1a). The candidate
                    // list is every racing-car record with a model on disk (0x4F3E40), the choice is
                    // the menu's `OpponentCar.Car`, and each opponent draws its own paint set.
                    None => {
                        use nfs_game::opponents::{CRand, FeRng, OpponentChoice, fill};
                        let dir = self.paths.car_model_dir();
                        let candidates: Vec<usize> = catalog
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| {
                                c.kind == 0
                                    && !c.sim.is_empty()
                                    && GamePaths::find_file(&dir, &format!("{}.crp", c.model))
                                        .is_some()
                            })
                            .map(|(i, _)| i)
                            .collect();
                        let selected = self
                            .player_car_id
                            .and_then(|id| catalog.iter().position(|c| c.id == id))
                            .or_else(|| {
                                catalog.iter().position(|c| {
                                    c.model.eq_ignore_ascii_case(&self.car.model)
                                        && !c.sim.is_empty()
                                })
                            })
                            .unwrap_or(0);
                        // the paint packages of `GameData/CarModel/packages.ini` (`[header] count`)
                        let package_count = std::fs::read_to_string(dir.join("packages.ini"))
                            .ok()
                            .and_then(|t| {
                                t.lines().find_map(|l| {
                                    l.trim()
                                        .strip_prefix("count=")
                                        .and_then(|v| v.trim().parse::<u32>().ok())
                                })
                            })
                            .unwrap_or(0);
                        let choice = OpponentChoice::from_index(self.opp_choice);
                        let picks = fill(
                            &catalog,
                            &candidates,
                            selected,
                            choice,
                            &mut FeRng::from_clock(),
                            &mut CRand::default(),
                            package_count,
                        );
                        tracing::info!(
                            choice = choice.label(),
                            candidates = candidates.len(),
                            player = catalog[selected].display_name,
                            cars = ?picks.iter().take(self.num_opponents as usize).map(|p| (catalog[p.car].display_name.as_str(), p.paint_set, p.package, p.decal)).collect::<Vec<_>>(),
                            "opponent cars"
                        );
                        picks
                            .iter()
                            .take(self.num_opponents as usize)
                            .map(|p| {
                                let entry = catalog[p.car].clone();
                                let paint = entry
                                    .paint_sets
                                    .get(p.paint_set)
                                    .map(|s| s.colours[0])
                                    .unwrap_or([0x20, 0x40, 0xC0]);
                                let name = entry.display_name.clone();
                                (entry, paint, name, None, None)
                            })
                            .collect()
                    }
                };
                for (g, (entry, paint, name, skill, grid)) in plan.iter().enumerate() {
                    // the opponent record's parts (0x4ED880), for a career opponent
                    let parts: Vec<nfs_formats::career::Part> = self.career_opponents.as_ref().and_then(|l| l.get(g)).map(|o| o.parts.clone()).unwrap_or_default();
                    let swaps: Vec<[i16; 3]> = parts.iter().flat_map(|p| p.geometry_swaps.iter().copied()).collect();
                    let g = grid.unwrap_or(g as u32);
                    let entry_993 = self.cheat_993(entry.clone());
                    let entry = &entry_993;
                    let (Ok(assets), Ok(sim)) = (
                        CarAssets::load(&self.paths, &entry.model).map(|c| {
                            let mut c = c.with_style(entry.variant).with_cabrio(entry.cabrio).with_style_overlay(swaps);
                            c.tiny = self.cheat_bits & (1 << 1) != 0;
                            c
                        }),
                        CarAssets::load_sim(&self.paths, &entry.sim),
                    ) else {
                        continue;
                    };
                    let paint = *paint;
                    let Ok(model) = CarModel::upload_with_env(
                        &mut renderer,
                        &assets,
                        &self.paths,
                        1,
                        paint,
                        world.env_map,
                        self.track.is_night(),
                        Some(0),
                    ) else {
                        continue;
                    };
                    let mut specs = CarSpecs::from_sim(
                        &sim,
                        entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935"),
                    );
                    if self.cheat_bits & (1 << 1) != 0 {
                        specs.apply_tinycar();
                    }
                    // `CarSim_applyParts` on the opponent's parts, whose grip ratio the `.ais`
                    // loader then reads (0x49CAD7, `ai.md` §2.1)
                    let grip_ratio = specs.apply_parts(&parts.iter().collect::<Vec<_>>(), &entry.sim_family);
                    let (p, r, sl) = grid_pose(t, start_scene, g, n_cars);
                    let mut y = p.y;
                    if let Some(h) = world.collision.ground(p.x, p.y + 5.0, p.z, 50.0) {
                        y = h.point.y + model.wheel_radius;
                    }
                    let mut vehicle = Vehicle::new(
                        Vec3::new(p.x, y, p.z),
                        r,
                        &specs,
                        model.half_track,
                        model.wheel_radius,
                    );
                    vehicle.race_started = false;
                    vehicle.traction_control = true; // AI cars never spin their wheels (Tyre 0x4A20D0)
                    vehicle.ai_driven = true;
                    vehicle.body_points = nfs_game::world::car_body_points(&assets);
                    vehicle.fit_half_extents();
                    // `.TINYCAR` takes 0.4 of the body box too (0x445136…0x44517C)
                    if self.cheat_bits & (1 << 1) != 0 {
                        vehicle.half_extents *= 0.4;
                    }
                    let mut model = model;
                    if self.cheat_bits & (1 << 1) != 0 {
                        model.scale = 0.4;
                    }
                    // its dash needles' calibration (0x4EF331…0x4EF37B, per car)
                    model.set_dash(entry.dash[0], entry.dash[1], entry.dash[2], entry.dash[3]);
                    self.road.opponent_models.push(model);
                    let controller = match CarAssets::load_ais(&self.paths, &entry.model) {
                        Ok(ais) => {
                            let mut d = nfs_game::ai::AiDriver::new(ais, &specs, &sim.unknown_tail);
                            d.grip_ratio = grip_ratio;
                            // the family key `nfs5.car` +0x115 the steering table is keyed on
                            d.set_track(&self.track.name, self.backwards, &entry.sim_family);
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
                    self.road.opponent_sets.push(entry.sound_set.clone());
                    self.road.opponent_paints.push(paint);
                    self.road.opponents.push(RaceCar {
                        name: name.clone(),
                        vehicle,
                        specs,
                        progress: RaceProgress::new(sl, t),
                        controller,
                        grid_pos: g,
                        model: self.road.opponent_models.len() - 1,
                        last_progress_tick: 0,
                        wall_hits: 0,
                        resets: 0,
                        eliminated: false,
                    });
                }
                // network play: every opponent seat belongs to a remote player. Their cars sit on
                // the grid until the first 0xBE arrives and are driven by it afterwards, so their
                // controllers never run.
                if let Some(n) = self.net.session.as_mut() {
                    n.reserve((0..self.road.opponents.len()).collect());
                    for o in self.road.opponents.iter_mut() {
                        o.controller = Controller::Parked;
                    }
                    tracing::info!(
                        seats = self.road.opponents.len(),
                        host = n.is_host(),
                        "network seats reserved"
                    );
                }
                // traffic (`traffic.md` §1, §8, §9): the track's seven traffic car ids are
                // `nfs5.trk +0x7C`; their `nfs5.car` records carry only a display name, a model and a
                // sound set (`Sbus`/`Tbus` mark the buses, which respawn five times less often).
                // Which of the race's car slots take which id is not in the notes (§8, UNKNOWN), so
                // the ids are handed out in table order and cycled when more cars than ids are asked
                // for (the heavy career setting wants eight).
                self.road.traffic = None;
                self.road.traffic_models.clear();
                self.road.traffic_sets.clear();
                self.road.traffic_paints.clear();
                self.road.traffic_prev.clear();
                if self.road.traffic_count > 0 {
                    let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
                        .ok()
                        .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
                        .unwrap_or_default();
                    let ids: Vec<u16> = trk
                        .iter()
                        .find(|r| r.file.eq_ignore_ascii_case(&self.track.name))
                        .map(|r| r.traffic_cars.iter().copied().filter(|i| *i != 0).collect())
                        .unwrap_or_default();
                    let mut plan: Vec<TrafficSpec> = Vec::new();
                    let mut by_id: Vec<u16> = Vec::new();
                    for k in 0..self.road.traffic_count as usize {
                        let Some(id) = ids.get(k % ids.len().max(1)).copied() else {
                            break;
                        };
                        let Some(entry) = catalog.iter().find(|c| c.id == id) else {
                            continue;
                        };
                        let bus = entry.sound_set.eq_ignore_ascii_case("sbus")
                            || entry.sound_set.eq_ignore_ascii_case("tbus");
                        let slot: usize = match by_id.iter().position(|x| *x == id) {
                            Some(i) => i,
                            None => {
                                let assets = match CarAssets::load(&self.paths, &entry.model) {
                                    Ok(a) => a,
                                    Err(e) => {
                                        tracing::warn!(id, model = entry.model, error = %e, "traffic car assets missing");
                                        continue;
                                    }
                                };
                                let paint = entry
                                    .paint_sets
                                    .first()
                                    .map(|p| p.colours[0])
                                    .unwrap_or([0x8C, 0x8C, 0x90]);
                                let model = match CarModel::upload_with_env(
                                    &mut renderer,
                                    &assets,
                                    &self.paths,
                                    1,
                                    paint,
                                    world.env_map,
                                    self.track.is_night(),
                                    Some(0),
                                ) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        tracing::warn!(id, model = entry.model, error = %e, "traffic car upload failed");
                                        continue;
                                    }
                                };
                                self.road.traffic_models.push(model);
                                by_id.push(id);
                                self.road.traffic_models.len() - 1
                            }
                        };
                        // the collision box is half the model's bounding box (`traffic.md` §21.4)
                        let m = &self.road.traffic_models[slot];
                        self.road.traffic_sets.push(entry.sound_set.clone());
                        self.road.traffic_paints.push(
                            entry
                                .paint_sets
                                .first()
                                .map(|p| p.colours[0])
                                .unwrap_or([0x8C, 0x8C, 0x90]),
                        );
                        plan.push(TrafficSpec {
                            model: slot,
                            bus,
                            half_extents: (m.bbox_max - m.bbox_min) * 0.5,
                        });
                    }
                    if self.road.traffic_specs.is_none() {
                        self.road.traffic_specs = CarAssets::load_sim(&self.paths, "default")
                            .ok()
                            .map(|sim| CarSpecs::from_sim(&sim, false));
                    }
                    self.road.traffic_bodies = plan
                        .iter()
                        .map(|sp| {
                            let specs = self
                                .road.traffic_specs
                                .clone()
                                .unwrap_or_else(|| self.specs.clone());
                            let mut v = Vehicle::new(
                                Vec3::ZERO,
                                glam::Mat3::IDENTITY,
                                &specs,
                                sp.half_extents.x,
                                0.35,
                            );
                            v.half_extents = sp.half_extents;
                            // `car+0x52C` bit 7 — what `Damage_byNormal` checks before the roof zone
                            v.is_traffic = true;
                            v
                        })
                        .collect();
                    if !plan.is_empty() {
                        // the table is already reversed for a backwards race, so the route direction
                        // is always +1 here; `linear` = a point-to-point track (`LINEAR_TRACK`)
                        let linear = !self
                            .records
                            .track(&self.track.name)
                            .map(|r| r.circuit)
                            .unwrap_or(false);
                        // `NFS_TRAFFIC_SEED` makes a run repeatable (the original seeds from the clock)
                        let seed = self.traffic_seed;
                        let mut set = TrafficSet::new(&plan, t, 1.0, linear, seed);
                        set.cops_race = self.road.cops_race;
                        // `[0x5E8BF8]` for the §5.2 rule 3 early-race boost
                        set.opponents = self.num_opponents;
                        // The ×0.9 is gated on `[0x657A24]`, which the option registry names
                        // **`WEATHER`** (id 0x1E at 0x5D1F90) — not the track's night flag. Both
                        // the shipped `fe.txt` and `fe_out.txt` say `weather = 0`, so it is off in
                        // every normal race; the port used to pass the night flag here and slowed
                        // traffic on every night track. Corrected 2026-09-21.
                        set.weather = false;
                        // `scripts.bin` drives the reactions of `traffic.md` §12.3 / §12.9 / §30
                        if let Some(p) = GamePaths::find_file(
                            &self.paths.game_data().join("Simulation").join("AI"),
                            "scripts.bin",
                        ) && let Ok(d) = std::fs::read(&p)
                        {
                            match nfs_formats::scripts::Scripts::parse(&d) {
                                Ok(sc) => set.scripts = sc,
                                Err(e) => tracing::warn!(error = %e, "scripts.bin unreadable"),
                            }
                        }
                        self.road.traffic = Some(set);
                        for sp in &plan {
                            tracing::debug!(model = sp.model, bus = sp.bus, half = ?sp.half_extents, mass = sp.mass(), "traffic body");
                        }
                        tracing::info!(
                            cars = plan.len(),
                            models = self.road.traffic_models.len(),
                            bodies = self.road.traffic_bodies.len(),
                            linear,
                            "traffic placed"
                        );
                    }
                }
                // police (`traffic.md` §17.1, §25): the track's five cop car ids are `nfs5.trk +0x8A`
                // and the spawn points `GameData/Track/Cops/<track>.cop`; a single race gets four cars.
                self.road.police = None;
                self.road.police_models.clear();
                self.road.police_prev.clear();
                self.road.police_bodies.clear();
                if self.road.cops_enabled {
                    let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
                        .ok()
                        .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
                        .unwrap_or_default();
                    let ids: Vec<u16> = trk
                        .iter()
                        .find(|r| r.file.eq_ignore_ascii_case(&self.track.name))
                        .map(|r| r.police_cars.iter().copied().filter(|i| *i != 0).collect())
                        .unwrap_or_default();
                    let cop_file = GamePaths::find_file(
                        &self.paths.game_data().join("Track").join("Cops"),
                        &format!("{}.cop", self.track.name),
                    )
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|d| {
                        nfs_formats::cop::CopFile::parse(&d)
                            .map_err(|e| tracing::warn!(error = %e, "cop file unreadable"))
                            .ok()
                    });
                    if let Some(file) = cop_file
                        && !ids.is_empty()
                    {
                        let mut cars = Vec::new();
                        let mut by_id: Vec<u16> = Vec::new();
                        for k in 0..4usize {
                            let id = ids[k % ids.len()];
                            let Some(entry) = catalog.iter().find(|c| c.id == id) else {
                                continue;
                            };
                            let entry_993 = self.cheat_993(entry.clone());
                            let entry = &entry_993;
                            let slot = match by_id.iter().position(|x| *x == id) {
                                Some(i) => i,
                                None => {
                                    let Ok(assets) = CarAssets::load(&self.paths, &entry.model)
                                    else {
                                        continue;
                                    };
                                    let paint = entry
                                        .paint_sets
                                        .first()
                                        .map(|p| p.colours[0])
                                        .unwrap_or([0xE0, 0xE0, 0xE0]);
                                    let Ok(model) = CarModel::upload_with_env(
                                        &mut renderer,
                                        &assets,
                                        &self.paths,
                                        1,
                                        paint,
                                        world.env_map,
                                        self.track.is_night(),
                                        Some(0),
                                    ) else {
                                        continue;
                                    };
                                    let mut model = model;
                                    if self.cheat_bits & (1 << 1) != 0 {
                                        model.scale = 0.4;
                                    }
                                    self.road.police_models.push(model);
                                    by_id.push(id);
                                    self.road.police_models.len() - 1
                                }
                            };
                            let m = &self.road.police_models[slot];
                            let half = (m.bbox_max - m.bbox_min) * 0.5;
                            // a cop car has a real `.sim` (§17.1), so its mass is the sim's
                            let mass = CarAssets::load_sim(&self.paths, &entry.sim)
                                .ok()
                                .map(|s| CarSpecs::from_sim(&s, false).mass)
                                .unwrap_or(1400.0);
                            // §17.2: the cop's chase level is the heat record's `level[role]`, not
                            // the race skill — role 0 is always level 2
                            cars.push(nfs_game::police::CopCar::new(
                                slot,
                                half,
                                mass,
                                nfs_game::police::COP_LEVEL[0],
                            ));
                        }
                        if !cars.is_empty() {
                            let one_way = t.slices.get(1).map(|s| s.lanes_a == 0).unwrap_or(false);
                            self.road.police_bodies = cars
                                .iter()
                                .map(|c| {
                                    let specs = self
                                        .road.traffic_specs
                                        .clone()
                                        .unwrap_or_else(|| self.specs.clone());
                                    let mut v = Vehicle::new(
                                        Vec3::ZERO,
                                        glam::Mat3::IDENTITY,
                                        &specs,
                                        c.half_extents.x,
                                        0.35,
                                    );
                                    v.half_extents = c.half_extents;
                                    v
                                })
                                .collect();
                            tracing::info!(
                                cars = cars.len(),
                                models = self.road.police_models.len(),
                                points = file.records.len(),
                                "police on duty"
                            );
                            let mut pol =
                                nfs_game::police::Police::new(file, cars, self.race_skill, one_way);
                            // the same `WEATHER` the traffic set gets — `[0x657A24]`, off in a
                            // normal race, and 0x4081B0 scales every AI car's rate by it
                            pol.weather = false;
                            pol.budget = nfs_game::police::duty_budget(
                                1 + self.num_opponents as usize,
                                false,
                            );
                            self.road.police = Some(pol);
                        }
                    }
                }
                self.racing_line = self.track.racing_line(&self.paths, self.backwards);
                tracing::info!(
                    opponents = self.road.opponents.len(),
                    racing_line = self.racing_line.as_ref().map(|l| l.points.len()),
                    "opponents placed"
                );
                rot
            }
            None => {
                self.car_pos = world.start;
                glam::Mat3::IDENTITY
            }
        };
        if let Some(h) =
            world
                .collision
                .ground(self.car_pos.x, self.car_pos.y + 5.0, self.car_pos.z, 50.0)
        {
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
                if let Some(r) = self.race.as_mut()
                    && let Some(t) = &table
                {
                    *r = RaceProgress::new(t.find_slice(pos, 0), t);
                }
            }
            let shown: Vec<usize> = sk.initial_arrow().into_iter().collect();
            world.set_shown(&sk.arrows, &shown);
            // laps never end a skill test: the finish trigger does
            self.num_laps = 99;
            tracing::info!(
                checkpoints = sk.checkpoints.len(),
                cones = sk.cones.len(),
                arrows = sk.arrows.len(),
                "skill test loaded"
            );
        }
        self.prev_car_pos = self.car_pos;
        self.slices = table;
        self.car_half_width = (car.bbox_max.x - car.bbox_min.x).abs() * 0.5;
        let mut vehicle = Vehicle::new(
            self.car_pos,
            rot,
            &self.specs,
            car.half_track,
            car.wheel_radius,
        );
        // `.TRANSMISSION` 0x221 → `carinfo+0xC` (0x4EEF9F): `Car.Transmission`'s index, 1 being
        // 561 "automatic" — the value 0x49DDD5 and 0x49DEDC compare with 1
        vehicle.automatic = self.player_automatic();
        // `.TRACTION` for the player's car: set only in Factory Driver mission 0xE
        // (0x4EB9D2…0x4EB9D7 → 0x4EEFDB…0x4EEFF1); `Car.Traction` itself is overwritten by the
        // per-car routine's later entry (`physics.md` §5.4)
        vehicle.tcs = self.mission.as_ref().is_some_and(|m| m.index == 0xE);
        // **The damage the car brings with it.** `Car_raceInit` 0x41083A copies ten dwords from
        // `carInfo+0xC8 … +0xEC` into `car+0x4E4`, the per-zone body damage, and 0x41085F copies
        // twenty more from `carInfo+0x78 … +0xC4` into each installed part's `+0x164`. The port had
        // the second half — `part_wear_for` sums those states into `CarSpecs::wear` — and started
        // every race with an undented body whatever the garage said.
        if let Some(owned) = self
            .career
            .as_ref()
            .and_then(|cu| cu.pending.as_ref().and_then(|p| p.garage_idx).and_then(|i| cu.career.garage.get(i)))
            .or(self.fe.test_drive.as_ref().and_then(|(_, o)| o.as_ref()))
        {
            vehicle.damage = owned.damage;
            tracing::info!(damage = ?owned.damage, "body damage carried into the race");
        }
        // the body the car actually collides with (`BndP`, `RE/notes/car-model.md` §12)
        vehicle.body_points = nfs_game::world::car_body_points(&self.car);
        vehicle.fit_half_extents();
                    // `.TINYCAR` takes 0.4 of the body box too (0x445136…0x44517C)
                    if self.cheat_bits & (1 << 1) != 0 {
                        vehicle.half_extents *= 0.4;
                    }
        self.vehicle = Some(vehicle);
        tracing::info!(pos = ?self.car_pos, "car placed");
        // `GameData/Render/common.fsh`: `SKDP` is the tyre dust (the one texture the original's
        // per-surface table points at throughout, 0x5D00E8) and `SHAD` the shadow every car casts
        // (0x43B960 looks it up by that FourCC)
        let sprite = |renderer: &mut Renderer,
                      quad: &nfs_renderer::MeshData,
                      name: &str|
         -> Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)> {
            let path = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")?;
            let data = std::fs::read(&path).ok()?;
            let images = nfs_assets::texture::decode_pack(&path, &data).ok()?;
            let img = images
                .iter()
                .find(|t| t.name.trim().eq_ignore_ascii_case(name))?;
            let tex = renderer.upload_texture(&nfs_renderer::TextureData {
                width: img.width,
                height: img.height,
                mips: img.mips.clone(),
            });
            let mesh = renderer.upload_mesh(quad);
            let material = renderer.create_material(nfs_renderer::MaterialDesc {
                alpha_blend: true,
                cull_back: false,
                depth_test: true,
                ..nfs_renderer::MaterialDesc::textured(tex)
            });
            Some((mesh, material))
        };
        let mut quad = nfs_renderer::MeshData::default();
        for (x, y, u, v) in [
            (-0.5f32, -0.5f32, 0.0f32, 1.0f32),
            (0.5, -0.5, 1.0, 1.0),
            (0.5, 0.5, 1.0, 0.0),
            (-0.5, 0.5, 0.0, 0.0),
        ] {
            quad.vertices.push(nfs_renderer::Vertex {
                position: [x, y, 0.0],
                normal: [0.0, 1.0, 0.0],
                uv: [u, v],
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        quad.indices = vec![0, 1, 2, 0, 2, 3];
        self.particle_draw = sprite(&mut renderer, &quad, "SKDP");
        self.shadow_draw = sprite(&mut renderer, &quad, "SHAD");
        self.glare_draw = sprite(&mut renderer, &quad, "CARG");
        // the track flares' definitions and their textures, out of `common.fsh` by the exact
        // name the loader swapped into place (the pack has both `spik` and `SPIK`)
        self.flare_defs = std::fs::read(self.paths.render_dir().join("flare.dat")).map(|d| nfs_formats::flare::parse(&d)).unwrap_or_default();
        self.flare_sprites.clear();
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
        {
            let mut names: Vec<String> = self.flare_defs.iter().flat_map(|d| d.textures.iter().flatten().cloned()).collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(img) = images.iter().find(|t| t.name.trim() == name) else {
                    tracing::debug!(name, "flare texture not in common.fsh");
                    continue;
                };
                let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
                let mesh = renderer.upload_mesh(&quad);
                let material = renderer.create_material(nfs_renderer::MaterialDesc {
                    alpha_blend: true,
                    cull_back: false,
                    depth_test: true,
                    ..nfs_renderer::MaterialDesc::textured(tex)
                });
                self.flare_sprites.insert(name, (mesh, material));
            }
        }
        // `HEAD` — what `Headlights::LayerUVLight` projects (0x452645 fetches it by that
        // four-character code into `[0x61E1D4]`, whatever the night flag says)
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
            && let Some(img) = images.iter().find(|t| t.name.trim() == "HEAD")
        {
            let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
            renderer.set_headlight_texture(Some(tex));
        }
        // the track's particle systems, started by its kind-3 `ef` parts (`particles.md` §6).
        // APPROXIMATION: the budget `[0x6197E0]` is the detail level's (0, 100, 200, 256 at
        // 0x44ED10) and this port has none, so it takes the highest
        let defs = std::fs::read(self.paths.render_dir().join("particle.dat")).map(|d| nfs_formats::particle_dat::parse(&d)).unwrap_or_default();
        self.track_particle_sprites.clear();
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "particle.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
        {
            let mut names: Vec<String> = defs.iter().map(|d| d.material.clone()).collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(img) = images.iter().find(|t| t.name.trim() == name) else {
                    tracing::debug!(name, "particle texture not in particle.fsh");
                    continue;
                };
                let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
                let mesh = renderer.upload_mesh(&quad);
                let material = renderer.create_material(nfs_renderer::MaterialDesc {
                    alpha_blend: true,
                    cull_back: false,
                    depth_test: true,
                    ..nfs_renderer::MaterialDesc::textured(tex)
                });
                self.track_particle_sprites.insert(name, (mesh, material));
            }
        }
        // made on the first update, once the track is up
        self.track_particle_defs = defs;
        self.track_particles = None;
        self.particles.clear();
        // the trackside cameras and their triggers travel with the track (`race-logic.md` §3.3)
        self.track_cameras = self
            .track
            .cameras
            .as_ref()
            .map(nfs_game::cameras::TrackCameras::from_scene);
        if let Some(t) = &self.track_cameras {
            tracing::info!(
                cameras = t.cameras.len(),
                triggers = t.triggers.len(),
                "trackside cameras"
            );
        }
        // the headlight projector's bitmap, loaded on a night track as `Headlights_init` 0x452590
        // does (`RE/notes/lighting.md` §5)
        if self.track.is_night()
            && let Some(m) = nfs_assets::track::night_map(&self.paths)
        {
            renderer.set_headlight_map(&m);
        }
        self.world = Some(world);
        // diagnostic: `NFS_PROJECTOR_VIEW=k` sets the car down on the track's k-th shadow projector
        if let Some(k) = std::env::var("NFS_PROJECTOR_VIEW").ok().and_then(|v| v.parse::<usize>().ok())
            && let Some(q) = self.world.as_ref().and_then(|w| w.projectors.get(k)).cloned()
        {
            let ground = self.world.as_ref().and_then(|w| w.collision.ground(q.point.x, q.point.y + 50.0, q.point.z, 200.0)).map(|h| h.point.y).unwrap_or(q.point.y);
            if let Some(v) = self.vehicle.as_mut() {
                v.position = Vec3::new(q.point.x, ground + 0.5, q.point.z);
                self.car_pos = v.position;
            }
        }
        let mut car = car;
        // `.TINYCAR`: the model at 0.4 (0x4433D2)
        if self.cheat_bits & (1 << 1) != 0 {
            car.scale = 0.4;
        }
        // the dash needles' calibration, off the player's `nfs5.car` record (the same pick as the
        // grid's: the chosen car id, else the model's first record with a sim)
        let catalog = nfs_formats::fedata::parse_car_catalog(
            &std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default(),
        )
        .unwrap_or_default();
        if let Some(e) = self
            .player_car_id
            .and_then(|id| catalog.iter().find(|c| c.id == id))
            .or_else(|| catalog.iter().find(|c| c.model.eq_ignore_ascii_case(&self.car.model) && !c.sim.is_empty()))
        {
            car.set_dash(e.dash[0], e.dash[1], e.dash[2], e.dash[3]);
        }
        self.car_model = Some(car);
        // the cockpit view's eye point, for the car's own body style
        // `drivercamposstyle<N>` is keyed on the **raw 0-based** style number — `550.tpg` has a
        // `drivercamposstyle0` — where `[styleN]` is 1-based. This passed a flat 1.
        self.driver_cam = nfs_assets::car::DriverCam::from_tpg(&self.car.tpg, self.car.style_index)
            .map(|dc| if self.mirror { dc.mirrored() } else { dc });
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
                    if !self.follow_road && phase > 0.66 {
                        self.input.press(Action::SteerRight);
                    }
                }
                self.simulate();
                // NFS_AI_TRACE=<opponent index>: per-quarter-second trace of one AI car (target vs speed, pedals, slip)
                if t % 16 == 0
                    && let Some(idx) = std::env::var("NFS_AI_TRACE")
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                    && let Some(o) = self.road.opponents.get(idx)
                {
                    let target = match &o.controller {
                        Controller::Ai(a) => a.target_speed * 3.6,
                        _ => -1.0,
                    };
                    let (n_ahead, t_off, t_dot) = match &o.controller {
                        Controller::Ai(a) => (a.dbg_n, a.dbg_offset, a.dbg_dot),
                        _ => (0, 0.0, 0.0),
                    };
                    let car_off = self
                        .slices
                        .as_ref()
                        .map(|t| {
                            let sl = &t.slices[o.progress.slice];
                            (o.vehicle.position - sl.centre).dot(sl.lateral)
                        })
                        .unwrap_or(0.0);
                    let lat = o.vehicle.v_local.x;
                    tracing::info!(t = t as f32 / 64.0, slice = o.progress.slice, speed = (o.vehicle.speed() * 3.6) as i32, target = target as i32, thr = (o.vehicle.throttle * 100.0) as i32, brk = (o.vehicle.brake * 100.0) as i32, steer = o.vehicle.steer, n_ahead, t_off = (t_off * 10.0).round() / 10.0, t_dot = (t_dot * 100.0).round() / 100.0, car_off = (car_off * 10.0).round() / 10.0, v_lat = lat, slip = ?o.vehicle.wheels.iter().map(|w| (w.slip * 10.0) as i32).collect::<Vec<_>>(), locked = ?o.vehicle.wheels.iter().map(|w| w.locked as u8).collect::<Vec<_>>(), walls = o.wall_hits, "ai trace");
                }
                if t % 64 == 0
                    && let Some(v) = &self.vehicle
                {
                    tracing::info!(t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, opp = ?self.road.opponents.iter().map(|o| (o.progress.slice, (o.vehicle.speed() * 3.6) as i32, match &o.controller { Controller::Ai(a) => (a.target_speed * 3.6) as i32, _ => -1 }, o.vehicle.gear, o.wall_hits, o.resets, o.vehicle.wheels[0].steer as i32, (o.vehicle.throttle * 100.0) as i32, (o.vehicle.brake * 100.0) as i32, o.vehicle.position.x as i32, o.vehicle.position.z as i32)).collect::<Vec<_>>(), wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), police = ?self.road.police.as_ref().map(|p| p.cars.iter().filter(|c| c.active).map(|c| (c.slice, (c.speed * 3.6) as i32, (c.position - v.position).length() as i32, c.mode)).collect::<Vec<_>>()), violation = ?self.road.police.as_ref().map(|p| p.violation), traffic = ?self.road.traffic.as_ref().map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| (c.slice, c.dir as i32, (c.speed * 3.6) as i32, (c.lateral * 10.0) as i32, (c.position - v.position).length() as i32, (c.position.y - v.position.y) as i32)).collect::<Vec<_>>()), pos = ?v.position, "autotest");
                }
            }
            self.rebuild_scene();
            self.update_damage_decals();
            self.settle_mission();
            if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
                let (order, times) = self.finishing_order();
                let zones = self.vehicle.as_ref().map(|v| v.damage);
                if let Some(c) = self.career.as_mut() {
                    if let Some(z) = zones {
                        c.park(z);
                    }
                    c.finish(&order, &times);
                    tracing::info!(order = ?order, message = c.message, money = c.career.money, "career event recorded");
                }
            }
            if self.screenshot.is_none() {
                // The original saves the replay from the race de-init chain after *every* race,
                // finished or not (replay.md §6.2) — so a scripted run saves on its way out too.
                self.save_replay();
                // scripted runs never stay open
                event_loop.exit();
            }
        }
        if let Some(path) = self.screenshot.take() {
            let rgba = self
                .renderer
                .as_mut()
                .unwrap()
                .render_to_image(&self.scene, &self.camera, 1280, 800)
                .expect("offscreen render");
            image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8)
                .expect("write png");
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
                // **A binding row waiting for a control takes every key** — its slot 4, 0x51C610,
                // answers true whatever it does, so nothing else on the screen sees the key (not
                // even Esc). It takes one only when exactly **one** key is down (the walk of the
                // key-down table 0x5DE028 at 0x51C623), and 0x51D0A0 then drops any key 0x51C050
                // cannot name — the row keeps waiting. A named key becomes `{3, 0, scancode, 0, 0,
                // code}`, is committed as `Bindings::assign` describes, and focus goes (0x51C65C).
                // `CameraView` and `LookBehind` are `I_*` bindings this port does not read, so
                // their rows only let focus go.
                if event.state == winit::event::ElementState::Pressed
                    && let Some((i, acc)) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_binding_row())
                {
                    if let winit::keyboard::PhysicalKey::Code(k) = event.physical_key
                        && let Some(sc) = nfs_input::scancode(k)
                        && self.input.keys_down_count() == 1
                    {
                        let code = nfs_input::bindings::display_code(sc);
                        let (name, arc) = nfs_game::keyname::key_name(code, &self.fe.strings);
                        if !name.is_empty() || arc.is_some() {
                            if let Some(id) = accessors::controller_action(&acc).and_then(nfs_input::binding_id) {
                                let b = nfs_input::bindings::Binding { kind: 3, device: 0, index: sc as u32, dir: 0, pov: 0, key: code };
                                self.input.bindings.assign(id, b);
                                tracing::info!(row = %acc, scancode = sc, "control rebound");
                            }
                            if let Some(l) = self.fe.layout_preview.as_mut() {
                                l.release_focus(i);
                            }
                        }
                    }
                    return;
                }
                // **A focused input box swallows the keyboard.** 0x4CCC20 is slot 4 of the three
                // input classes, and it returns *true* for every key it takes, which is what stops
                // the screen's own arrows and Enter from seeing it. Return commits, backspace
                // erases, and everything printable but `'%'` goes in — the filter itself lives in
                // `LayoutScreen::type_char`, beside the addresses it came from.
                if event.state == winit::event::ElementState::Pressed
                    && let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_input())
                {
                    use winit::keyboard::{Key, NamedKey};
                    let mut took = false;
                    match event.logical_key.as_ref() {
                        Key::Named(NamedKey::Enter) => {
                            if let Some(l) = self.fe.layout_preview.as_mut()
                                && let Some((name, text)) = l.commit_input(i)
                            {
                                tracing::info!(accessor = %name, value = %text, "input box committed");
                                took = true;
                            }
                        }
                        Key::Named(NamedKey::Backspace) => {
                            took = self.fe.layout_preview.as_mut().is_some_and(|l| l.backspace(i));
                        }
                        Key::Character(t) => {
                            if let Some(c) = t.chars().next()
                                && let Some(l) = self.fe.layout_preview.as_mut()
                            {
                                took = l.type_char(i, c);
                            }
                        }
                        Key::Named(NamedKey::Space) => {
                            took = self.fe.layout_preview.as_mut().is_some_and(|l| l.type_char(i, ' '));
                        }
                        _ => {}
                    }
                    // Every accepted path in 0x4CCC20 ends at `0x4AADB0(9)`.
                    if took {
                        self.fe_sound(9);
                        return;
                    }
                }
                if self.fe.menu.is_some() && event.state == winit::event::ElementState::Pressed {
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
                        if self.leave_race() {
                            self.input.end_tick();
                            return;
                        }
                        event_loop.exit();
                    } else if self.vehicle.is_some() {
                        self.paused = true;
                        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                            s.stop_loops(a);
                        }
                    } else {
                        event_loop.exit();
                    }
                }
                // Playback speed (replay.md §7 commands 4..7). Which key carries which command is
                // UNKNOWN in the original (§15), so the digits are this port's choice —
                // APPROXIMATION.
                if event.state == winit::event::ElementState::Pressed
                    && self
                        .replay.stream
                        .as_ref()
                        .is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing)
                {
                    use nfs_game::replay::Speed;
                    use winit::keyboard::{KeyCode, PhysicalKey};
                    let speed = match event.physical_key {
                        PhysicalKey::Code(KeyCode::Digit1) => Some(Speed::Quarter),
                        PhysicalKey::Code(KeyCode::Digit2) => Some(Speed::Half),
                        PhysicalKey::Code(KeyCode::Digit3) => Some(Speed::Normal),
                        PhysicalKey::Code(KeyCode::Digit4) => Some(Speed::Double),
                        _ => None,
                    };
                    if let Some(sp) = speed {
                        self.replay.speed = sp;
                        tracing::info!(speed = ?sp, "replay speed");
                    }
                }
                if self.paused
                    && event.state == winit::event::ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter)
                {
                    self.paused = false;
                    self.input.end_tick();
                    return;
                }
                // after the finish, Enter returns to the menu (window and renderer are kept)
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter)
                    && self.race.as_ref().map(|r| r.finished).unwrap_or(false)
                    && let Some(menu) = self.fe.menu_template.take()
                {
                    self.settle_mission();
                    if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
                        let (order, times) = self.finishing_order();
                        let zones = self.vehicle.as_ref().map(|v| v.damage);
                        if let Some(c) = self.career.as_mut() {
                            // the car is parked with the damage it took, which sticks to its parts
                            // until the shop repairs them (`career.md` §11)
                            if let Some(z) = zones {
                                c.park(z);
                            }
                            c.finish(&order, &times);
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
                            let pos_of = |d: usize| {
                                order
                                    .iter()
                                    .position(|x| *x == d)
                                    .map(|p| p as u32 + 1)
                                    .unwrap_or(99)
                            };
                            let alive_idx: Vec<usize> =
                                (0..k.n as usize).filter(|i| k.alive[*i]).collect();
                            if pos_of(0) > remaining {
                                k.active = false;
                                k.message =
                                    format!("KNOCKED OUT IN RACE {} OF {}", k.race_idx + 1, k.n);
                            } else {
                                k.start_pos[0] = pos_of(0);
                                for (slot, i) in alive_idx.iter().enumerate() {
                                    let p = pos_of(slot + 1);
                                    if p <= remaining {
                                        k.start_pos[i + 1] = p
                                    } else {
                                        k.alive[*i] = false
                                    }
                                }
                                k.race_idx += 1;
                                if k.race_idx >= k.n {
                                    k.active = false;
                                    k.message = format!("KNOCKOUT WON  ({} RACES)", k.n);
                                } else {
                                    k.message = format!(
                                        "RACE {} OF {} NEXT: {}   (P{} LAST RACE)   ENTER",
                                        k.race_idx + 1,
                                        k.n,
                                        k.tracks[k.race_idx as usize].to_uppercase(),
                                        pos_of(0)
                                    );
                                }
                            }
                            menu.knockout_line = k.message.clone();
                        }
                    }
                    self.player_grid_override = None;
                    self.fe.menu = Some(menu);
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
            // The pointer, mapped back through `place`: the design area keeps 4:3 and is centred
            // in a wider window, so the inverse is `(u - x0) / fw * 640` across and `v * 480` down.
            WindowEvent::CursorMoved { position, .. } => {
                self.fe.cursor = self.window.as_ref().map(|w| {
                    let size = w.inner_size();
                    let (sw, sh) = (size.width.max(1) as f32, size.height.max(1) as f32);
                    let (u, v) = (position.x as f32 / sw, position.y as f32 / sh);
                    let design_aspect = 640.0 / 480.0;
                    let aspect = sw / sh;
                    let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
                    ((u - x0) / fw * 640.0, v * 480.0)
                });
                // **`FEControl::onMouseEnter` 0x4C7B90 plays sound 6** through slot 23 (0x4C7A30):
                // on entering a widget whose state is now `"hover"` (the string compare at
                // 0x4C7BD9), that was not hovered already (0x4C7BA5) and is not disabled (bit 0x10,
                // 0x4C7C18). Not carried: 0x4C7C1C's accessor-driven disable (0x4E08E0), which
                // this port does not model (§4.110).
                if let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_mut(), self.fe.cursor) {
                    l.set_cursor(Some((cx, cy)));
                }
                // a dialog's buttons take the pointer too (slot 8, 0x515060)
                if let (Some((d, _)), Some((cx, cy)), Some(l)) = (self.fe.dialog.as_mut(), self.fe.cursor, self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref())) {
                    let th = l.dialog_text_height(&d.text);
                    let sounds = d.pointer(cx, cy, th);
                    for s in sounds {
                        self.fe_sound(s as usize);
                    }
                }
                let now = self.fe.layout_preview.as_ref().and_then(|l| l.hovered());
                if now != self.fe.last_hover {
                    self.fe.last_hover = now;
                    if let (Some(i), Some(l)) = (now, self.fe.layout_preview.as_ref()) {
                        let controls = l.controls();
                        let sel = controls.get(self.fe.layout_sel.min(controls.len().saturating_sub(1))).map(|c| c.index);
                        if l.state_name(i, sel).as_deref() == Some("hover") && !l.widget_disabled(i) {
                            self.fe_sound(6);
                        }
                    }
                }
                // Dragging the scrollbar's knob. `FeScrollBar`'s slot 8, 0x4D0F20: with the knob
                // grabbed (`[this+0x10C] == 3`) and the pointer still on the bar, it sets the
                // model's position to
                // `grabPosition + (y - grabY) · (positions - 1) / (trackHeight - knobHeight)`
                // — the exact inverse of the placement at 0x4D0BBE — and re-places the knob.
                if let (Some((index, gy, at)), Some((_, cy))) = (self.fe.list_drag, self.fe.cursor) {
                    let span = self.fe.layout_preview.as_ref().and_then(|l| l.list_scrollbar(index)).map(|b| (b.span, b.positions));
                    if let Some((span, positions)) = span
                        && span > 0.0
                        && positions > 1
                        && let Some(l) = self.fe.layout_preview.as_mut()
                    {
                        let moved = ((cy - gy) * (positions - 1) as f32 / span).round() as i32;
                        l.set_list_scroll(index, at as i32 + moved);
                    }
                }
                // `NFS5.ColorBox`'s slot 8, 0x4C4830: while the editor is up the widget holds the
                // capture, so every move reaches it, and it drags the hue, the saturation or the
                // luminance whenever a button is held (§4.66)
                if let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_mut(), self.fe.cursor) {
                    l.picker_move(cx, cy);
                    l.deadzone_drag(cx, cy);
                }
                // the open drop-down's scrollbar tracks the pointer so its arrows can show their
                // hovered frame — `[this+0xEC]`, which 0x4DB2F0 writes on every move (§4.101)
                if let Some((cx, cy)) = self.fe.cursor {
                    let at = self.fe.layout_preview.as_ref().and_then(|l| l.combo_scroll_at(cx, cy));
                    let held = self.fe.mouse_down;
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.set_combo_scroll(at.map(|(i, r)| (i, r, held)));
                    }
                }
                if let Some((cx, cy)) = self.fe.cursor
                    && let Some(l) = self.fe.layout_preview.as_mut()
                {
                    l.picture_drag_to(cx, cy);
                }
                if self.fe.bar_grab.is_some()
                    && let Some((cx, cy)) = self.fe.cursor
                    && let Some((_, name, f)) = self.fe.layout_preview.as_ref().and_then(|l| l.bar_at(cx, cy))
                {
                    self.set_accessor_fraction(&name, f);
                }
            }
            WindowEvent::CursorLeft { .. } => self.fe.cursor = None,
            // `RE/notes/controls.md` §4.40: the pump stamps a `MouseButtonEvent` type **4** on the
            // press and **8** on the release (0x529806). The base's slot 5, 0x5258B0, fires the
            // widget's `"CLICKED"` message — the one reference to that string in the whole image —
            // and `NFS5.GOTOBUTTON` overrides only slot 9 (0x4C9470), where it plays sound 5 and
            // runs the screen transition: **a button navigates on the release, not the press**.
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let down = state == ElementState::Pressed;
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_pressed(down);
                }
                self.fe.mouse_down = down;
                // a dialog's input slot 3 (0x512F10) hands the event to its buttons first, and
                // stops at the one that takes it (slot 9, 0x515130)
                if !down && let Some((d, _)) = self.fe.dialog.as_mut() && let Some(s) = d.release() {
                    self.fe_sound(s as usize);
                    return;
                }
                // the bottom bar is a screen of its own, and its buttons navigate on the release
                if !down && self.fe_bottom_click(event_loop) {
                    return;
                }
                // `NFS5.ChroniclePicture` drags its picture about: slot 5 takes hold, slot 8
                // pans, slot 9 lets go (§4.119)
                if let Some((cx, cy)) = self.fe.cursor {
                    let hit = self
                        .fe.layout_preview
                        .as_ref()
                        .and_then(|l| l.hovered())
                        .filter(|i| self.fe.layout_preview.as_ref().and_then(|l| l.class_of(*i)).as_deref() == Some("NFS5.ChroniclePicture"));
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        match (down, hit) {
                            (true, Some(i)) => l.picture_press(i, cx, cy),
                            (false, _) => l.picture_release(),
                            _ => {}
                        }
                    }
                }
                if !down {
                    // 0x4C1FB0: the release hands `[this+0x14C]` to 0x5292D0 and zeroes it
                    self.fe.setting_repeat = None;
                }
                // `FeListScrollBar::onPress` 0x4DB040: the hit test first, then the jump table at
                // 0x4DB164. Arms 1 and 2 step the list and swallow the press — the drop-down must
                // not close under the pointer (§4.101).
                if let Some((cx, cy)) = self.fe.cursor
                    && let Some((_, region)) = self.fe.layout_preview.as_ref().and_then(|l| l.combo_scroll_at(cx, cy))
                {
                    // arms 1 and 2 play click 8 through 0x4AADB0 whether or not the list moves
                    // (0x4DB08B, 0x4DB0A1)
                    if down && matches!(region, 1 | 2) {
                        self.fe_sound(8);
                    }
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        let i = l.open_combo();
                        l.set_combo_scroll(i.map(|i| (i, region, down)));
                        if down {
                            l.combo_scroll_press(region);
                        }
                    }
                    return;
                }
                // `NFS5.DEADZONE` takes the button before anything else on its screen: its slot 5
                // (0x51BB60) hit-tests its four handles, takes focus and sets the value, its slot
                // 8 (0x51BCD0) drags while one is held and its slot 9 (0x51BCB0) lets go.
                if let Some((cx, _)) = self.fe.cursor
                    && !down
                    && let Some(l) = self.fe.layout_preview.as_mut()
                {
                    let _ = cx;
                    l.deadzone_release();
                }
                if let (Some((cx, cy)), true) = (self.fe.cursor, down)
                    && let Some(l) = self.fe.layout_preview.as_mut()
                    && l.deadzone_press(cx, cy)
                {
                    return;
                }
                // `NFS5.GouraudIndicatorBar`'s slot 5 (0x4CA470) takes the button when the bar is
                // `data.changeable` and the pointer is on it, and sets the accessor straight from
                // where the pointer landed. Its slot 8 (0x4CA590) then drags, gated on the focus
                // the press granted (0x4C7DD0) and on the pointer still being over the part.
                // §4.81.
                if !down {
                    self.fe.bar_grab = None;
                }
                if let (Some((cx, cy)), true) = (self.fe.cursor, down)
                    && let Some((_, name, f)) = self.fe.layout_preview.as_ref().and_then(|l| l.bar_at(cx, cy))
                {
                    self.fe.bar_grab = Some(name.clone());
                    self.set_accessor_fraction(&name, f);
                    return;
                }
                // The colour editor eats the button while it is up: its slots 5 and 9 return
                // without ever reaching the combo's own (0x4C4730, 0x4C4980 fall through to the
                // base only when the widget has **lost** focus or the latch is down) — §4.66.
                if let (Some(open), Some((cx, cy))) = (self.fe.layout_preview.as_ref().and_then(|l| l.open_picker()), self.fe.cursor) {
                    let closed = if let Some(l) = self.fe.layout_preview.as_mut() {
                        if down {
                            l.picker_press(cx, cy);
                            false
                        } else {
                            l.picker_release(cx, cy)
                        }
                    } else {
                        false
                    };
                    if closed {
                        // "ok" and "cancel" both play sound 5 (0x4C49C4, 0x4C4A16)
                        self.fe_sound(5);
                        self.fe.layout_open_combo = None;
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.set_open_combo(None);
                        }
                    }
                    let _ = open;
                    return;
                }
                // the press moves the screen's current widget onto whatever is under the pointer,
                // and the release runs it
                // a list's scrollbar arrows, hit-tested in design coordinates: the original moves
                // the position in the scrollbar object the row loop reads (§4.45)
                if !down {
                    if let (Some((i, _, _)), Some(l)) = (self.fe.list_drag, self.fe.layout_preview.as_mut()) {
                        l.park_knob(i, None);
                    }
                    self.fe.list_drag = None;
                    self.fe.list_repeat = None;
                }
                if down && let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_ref(), self.fe.cursor) {
                    let mut moved = None;
                    let mut grabbed = None;
                    let mut parked = None;
                    // A click on one of `Parts.AllParts`'s two section headers opens or closes it.
                    // `FePartsAllPartsAccessor`'s slot 11, 0x515440, decodes the row (0x5152E0) and
                    // for a **header** flips `[this+0x2C]` or `[this+0x2D]` with a `sete`
                    // (0x51548B, 0x5154B2) before refreshing. The second header's row is 1 while
                    // the first section is closed and `1 + <its rows>` while it is open.
                    let mut picked_part = None;
                    let section = l
                        .list_boxes()
                        .into_iter()
                        .find(|w| w.accessor.as_deref() == Some("Parts.AllParts"))
                        .and_then(|w| l.list_row_at(w.index, cx, cy))
                        .and_then(|row| match row {
                            0 => Some(0usize),
                            r if r == self.parts_header_b() => Some(1),
                            // any other row is a part, and clicking it moves the accessor's own
                            // index — which is what 0x4F1ADE reads back (§4.114)
                            r => {
                                picked_part = Some(r);
                                None
                            }
                        });
                    for w in l.list_boxes() {
                        let Some(b) = l.list_scrollbar(w.index) else { continue };
                        let inside = |r: (f32, f32, f32, f32)| cx >= r.0 && cx < r.0 + r.2 && cy >= r.1 && cy < r.1 + r.3;
                        if inside(b.up) {
                            moved = Some((w.index, -1));
                        } else if inside(b.down) {
                            moved = Some((w.index, 1));
                        } else if inside(b.knob) {
                            // 0x4D0973: the press remembers where it grabbed and the position it
                            // grabbed at, and the move (0x4D0F20) works from those two
                            grabbed = Some((w.index, cy, b.at));
                        } else if cx >= b.up.0 && cx < b.up.0 + b.up.2 && cy >= b.track_top && cy < b.down.1 {
                            // regions 4 and 5, the track either side of the knob (0x4D0995): the
                            // press does **not** move the model — it remembers the knob's own
                            // top-left as the grab (0x4D09D6, 0x4D09E3) and the position it is on
                            // (0x4D09F6), so the bar only moves once the pointer does. It does
                            // move the knob's **drawn** rect to the pointer, though (0x4D0A0A /
                            // 0x4D0A18 → 0x5276E0), and re-runs the hit test so the region becomes
                            // the knob — a look that lasts until the first move.
                            grabbed = Some((w.index, b.knob.1, b.at));
                            parked = Some((w.index, cy));
                        }
                    }
                    if let Some(g) = grabbed {
                        self.fe.list_drag = Some(g);
                        if let (Some((i, ky)), Some(l)) = (parked, self.fe.layout_preview.as_mut()) {
                            l.park_knob(i, Some(ky));
                        }
                        return;
                    }
                    if let Some((index, delta)) = moved
                        && let Some(l) = self.fe.layout_preview.as_mut()
                    {
                        l.scroll_list(index, delta);
                        self.fe.list_repeat = Some((index, delta, 0));
                        return;
                    }
                    if let Some(sec) = section {
                        self.fe.parts_open[sec] = !self.fe.parts_open[sec];
                        // the two headers move the second one's row, so the picked part goes with
                        // the list rather than staying on a row that is no longer a part
                        self.fe.parts_sel = None;
                        self.bind_lists();
                        self.bind_checks();
                        self.bind_parts_buttons();
                        return;
                    }
                    if let Some(r) = picked_part {
                        self.fe.parts_sel = Some(r);
                        self.bind_parts_buttons();
                        return;
                    }
                    // a click on `Replay.List` puts its accessor on that row (FeListBox slot 9 →
                    // the accessor's slot 11)
                    if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                        l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some("Replay.List")).and_then(|w| l.list_row_at(w.index, cx, cy))
                    }) {
                        self.fe.replay_sel = row;
                        self.bind_lists();
                        return;
                    }
                    // the CD player's two lists keep their own selected rows. Both carry
                    // `data.selectmode = 1`, so a click on the row already selected clears the
                    // selection (0x4C6D12…0x4C6D2E: accessor slot 16, 0x4BC610) instead of setting
                    // it again
                    for (acc, avail) in [("CDPlayer.AvailableList", true), ("CDPlayer.SelectedList", false)] {
                        if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                            l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some(acc)).and_then(|w| l.list_row_at(w.index, cx, cy))
                        }) {
                            let sel = if avail { &mut self.fe.cd_available } else { &mut self.fe.cd_selected };
                            *sel = if *sel == Some(row) { None } else { Some(row) };
                            self.bind_lists();
                            return;
                        }
                    }
                    // and one on `UserList.UserList` selects that profile's row
                    if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                        l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some("UserList.UserList")).and_then(|w| l.list_row_at(w.index, cx, cy))
                    }) {
                        self.fe.user_sel = Some(row);
                        self.bind_lists();
                        return;
                    }

                }
                if let Some(hit) = self.fe.layout_preview.as_ref().and_then(|l| l.hovered()) {
                    // An input box takes focus when it is activated — `FeInputBox`'s slot 9,
                    // 0x4CCB70, which the base's release path reaches through the `"CLICKED"`
                    // message (§4.40). Three classes share that slot. §4.109.
                    if !down
                        && let Some(l) = self.fe.layout_preview.as_mut()
                    {
                        let class = l.class_of(hit).unwrap_or_default();
                        // `NFS5.CONTROLLERSETUP`'s slot 9 is **0x51C5F0**, three calls: take
                        // focus (0x4C7D70), then 0x51F000 and 0x51F310 — the input layer's
                        // "start listening for the next control". §4.117.
                        if matches!(class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP") {
                            l.focus_input(hit);
                        }
                        if class == "NFS5.CONTROLLERSETUP" {
                            self.fe.binding_base = Some(self.input.pads.clone());
                        }
                        // `NFS5.GOTOBUTTON`'s slot 9, 0x4C9470: unless disabled (0x4C7E40), sound 5
                        // before the transition
                        if class == "NFS5.GOTOBUTTON" && !l.widget_disabled(hit) {
                            self.fe_sound(5);
                        }
                        // `NFS5.3DCAR`'s slot 9, 0x4C0EB0: a click opens or closes the part under
                        // the pointer (§4.153)
                        let l = match self.fe.layout_preview.as_mut() {
                            Some(l) => l,
                            None => return,
                        };
                        if class == "NFS5.3DCAR" {
                            let events = l.car_click(hit);
                            self.fe_car_part_events(hit, &events);
                        }
                    }
                    // `usedcars.lay`'s two panel buttons flip `[FEUsedCarsScreen + 0x290]`
                    // through the `SHOWPARTS` and `SHOWDAMAGE` callbacks the constructor
                    // registers at 0x4F71C0 and 0x4F71D1 (§4.113)
                    if !down
                        && let Some(which) = self
                            .fe.layout_preview
                            .as_ref()
                            .and_then(|l| l.widgets_named(&["ID_SHOWPARTSBUTTON", "ID_SHOWDAMAGEBUTTON"]).into_iter().find(|(i, _)| *i == hit).map(|(_, k)| k))
                    {
                        self.fe.usedcars_showparts = which == 0;
                        self.bind_usedcars_panels();
                        return;
                    }
                    // a click on a timeline cell — `FEChronicleMainBoxControl`'s slot 9,
                    // 0x5080C0: row 0 opens `chroniclepicture` and row 1 `chroniclemedia`, each
                    // for the column's own year and variant, and only when 0x5074A0 finds files
                    // there (§4.120)
                    if !down
                        && let Some((cx, cy)) = self.fe.cursor
                        && let Some((col, row)) = self.fe.layout_preview.as_ref().and_then(|l| l.chronicle_cell_at(cx, cy))
                    {
                        self.fe.chronicle_col = Some(col);
                        self.bind_chronicle_icons();
                        self.bind_lists();
                        if row < 2 && !self.chronicle_media_for(row).is_empty() {
                            self.open_layout(if row == 0 { "chroniclepicture" } else { "chroniclemedia" });
                        }
                        return;
                    }
                    // and a click on one of `chronicle.lay`'s five model-group buttons moves
                    // `[0x5DBA44]` — 0x506E68, 0x506EA6 and the three arms after them each write
                    // the group their button stands for and then reload the screen (§4.111)
                    if !down
                        && let Some(which) = self
                            .fe.layout_preview
                            .as_ref()
                            .and_then(|l| l.widgets_named(&CHRONICLE_GROUP_BUTTONS).into_iter().find(|(i, _)| *i == hit).map(|(_, k)| k))
                    {
                        self.fe.chronicle_group = which;
                        self.bind_chronicle_group();
                        self.bind_chronicle_icons();
                        self.bind_lists();
                        return;
                    }
                    let controls = self.fe.layout_preview.as_ref().map(|l| l.controls()).unwrap_or_default();
                    // a combo takes focus on the **release** (0x4C6CC1, after sound 2 at
                    // 0x4C6C97), and focus is what opens its list (0x4C53A0) — clicking the open
                    // one again gives the focus back
                    let combos = self.fe.layout_preview.as_ref().map(|l| l.combo_sliders()).unwrap_or_default();
                    if !down && combos.iter().any(|c| c.index == hit) {
                        let open = self.fe.layout_open_combo == Some(hit);
                        self.fe.layout_open_combo = (!open).then_some(hit);
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.set_open_combo(self.fe.layout_open_combo);
                            // the editor's draw copies all eight wells the first frame it is up,
                            // and cancel puts that copy back (§4.66)
                            if l.open_picker().is_some() {
                                l.picker_opened();
                            }
                        }
                    } else if !down {
                        self.fe.layout_open_combo = None;
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.set_open_combo(None);
                        }
                    }
                    // `NFS5.CheckBox` is no button and carries no `accessor`, so it is not in
                    // `controls()` at all — its tick comes off the hit itself. `repair.lay`'s ten
                    // are the only ones in the front end, each naming a `QUICKFIX.*` bool (§4.95).
                    if down
                        && let Some(name) = self
                            .fe.layout_preview
                            .as_ref()
                            .and_then(|l| l.check_boxes().into_iter().find(|(i, _)| *i == hit).map(|(_, a)| a))
                        && let Some(n) = name.strip_prefix("QUICKFIX.")
                        && let Some(k) = QUICKFIX_NAMES.iter().find(|k| **k == n)
                    {
                        if !self.fe.repair_ticks.remove(*k) {
                            self.fe.repair_ticks.insert(k);
                        }
                        self.bind_checks();
                        return;
                    }
                    if let Some(n) = controls.iter().position(|c| c.index == hit) {
                        self.fe.layout_sel = n;
                        // `NFS5.CARSETTINGBUTTON` steps its accessor on the **press**, not the
                        // release: slot 5 (0x4C1F6C) takes focus and slot 27 (0x4C1F20) hands
                        // `data.step` — `[this+0x144]`, bound at 0x4C1ED7 — to the accessor.
                        // Seventeen of the 34 carry 1 and seventeen −1, so the arrow that points
                        // left really does subtract. §4.84.
                        if down
                            && controls[n].step != 0
                            && let Some(a) = controls[n].accessor.clone()
                        {
                            let step = controls[n].step;
                            self.fe.setting_repeat = Some((a.clone(), step, 0));
                            self.set_accessor(&a, step);
                            self.bind_car_logos();
                            self.bind_indicator_bars();
                            self.bind_chronicle_icons();
                            self.bind_showcase_pictures();
                            self.bind_lists();
                            self.bind_checks();
                            self.bind_mergedlist_panel();
                            return;
                        }
                        if !down && !combos.iter().any(|c| c.index == hit) {
                            self.layout_key(MenuKey::Enter, event_loop);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested if self.fe.menu.is_some() => {
                // gamepad: d-pad / left stick moves, South (A / cross) selects
                self.gamepads.poll(&mut self.input);
                let stick = self.input.pad.map(|p| p.steer).unwrap_or(0.0);
                let mut keys = Vec::new();
                if self.input.take_pressed(Action::SteerLeft) {
                    keys.push(MenuKey::Left);
                }
                if self.input.take_pressed(Action::SteerRight) {
                    keys.push(MenuKey::Right);
                }
                if self.input.take_pressed(Action::Throttle) {
                    keys.push(MenuKey::Up);
                }
                if self.input.take_pressed(Action::Brake) {
                    keys.push(MenuKey::Down);
                }
                if self.input.take_pressed(Action::ShiftUp) {
                    keys.push(MenuKey::Enter);
                }
                if self.input.take_pressed(Action::Handbrake) {
                    keys.push(MenuKey::Back);
                }
                if stick.abs() > 0.6 && !self.pad_stick_latched {
                    keys.push(if stick < 0.0 {
                        MenuKey::Left
                    } else {
                        MenuKey::Right
                    });
                }
                self.pad_stick_latched = stick.abs() > 0.6;
                self.input.end_tick();
                for k in keys {
                    self.menu_key(k, event_loop);
                    if self.fe.menu.is_none() {
                        return;
                    }
                }
                // the dialog's tick (0x512980 and the kind's own), its answer, and the covering
                // the manager keeps while it is up (0x4D2000 on the raise, 0x4D1E70 on the dismiss)
                if self.fe_dialog_answer(event_loop) {
                    return;
                }
                // the end-of-era show's tick (0x4FF180): back one screen when it is over
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.end_of_era_leave()) {
                    self.menu_key(MenuKey::Back, event_loop);
                    return;
                }
                // the front end draws from `rebuild_scene` like everything else, so it keeps
                // animating between key presses
                let now = Instant::now();
                self.frame_dt = (now - self.last_frame).as_secs_f32();
                self.last_frame = now;
                self.draw_frame();
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
                    self.accumulator -= GAME_TICK;
                    // replay.md §5.1: playback speed is tick *decimation* inside the catch-up
                    // loop — never a `dt` scale. Every iteration still renders.
                    let n = match &self.replay.stream {
                        Some(r) if r.mode == nfs_game::replay::Mode::Playing => {
                            self.replay.speed.ticks_at(self.replay.loop_count)
                        }
                        _ => 1,
                    };
                    self.replay.loop_count += 1;
                    for _ in 0..n {
                        self.simulate();
                    }
                }
                self.draw_frame();
            }
            _ => {}
        }
    }
}

/// A replay file in either format: the port's own, or an original `.rpl` (§6.1 + §16) — the setup
/// snapshot, the final tick and the same stream, played back as far as the port's simulation
/// matches.
fn read_replay_file(d: &[u8]) -> Option<nfs_game::replay::ReplayFile> {
    nfs_game::replay::ReplayFile::parse(d).or_else(|| {
        let (setup, final_tick, stream) = nfs_game::replay::parse_original_rpl(d)?;
        tracing::info!(track = setup.track, "original-format .rpl");
        Some(nfs_game::replay::ReplayFile { setup, final_tick, stream })
    })
}

/// An original `.rpl` names the car by its `nfs5.car` id only (§16.2): the model, the simulation
/// and the first paint from the catalogue.
fn resolve_replay_car(paths: &GamePaths, rs: &mut nfs_game::replay::ReplaySetup) {
    if rs.car.is_empty()
        && rs.car_id != 0
        && let Ok(cat) = std::fs::read(paths.fe_data().join("Data").join("nfs5.car")).map_err(|e| tracing::warn!(error = %e, "nfs5.car"))
        && let Ok(entries) = nfs_formats::fedata::parse_car_catalog(&cat)
        && let Some(e) = entries.iter().find(|c| c.id == rs.car_id)
    {
        rs.car = e.model.clone();
        rs.sim = e.sim.clone();
        rs.paint = e.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let paths = GamePaths::locate()?;
    tracing::info!(root = %paths.root.display(), "located original game data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    // positional arguments = tokens that are neither flags nor flag values
    let flags_with_value = [
        "--screenshot",
        "--autotest",
        "--opponents",
        "--record",
        "--laps",
        "--career",
        "--layout",
        "--replay",
        "--net",
        "--net-name",
        "--net-browse",
    ];
    // flags without a value: --follow, --mission
    let mut positional = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = flags_with_value.contains(&a.as_str());
            continue;
        }
        positional.push(a.clone());
    }
    // `--replay <file>`: play back a recording the port saved (`RE/notes/replay.md` §14.5 — the
    // original's 118 312-byte .rpl is the registry dump of §16, which `--rpl` writes)
    let replay_file = args
        .iter()
        .position(|a| a == "--replay")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| {
            let d = std::fs::read(p)
                .map_err(|e| tracing::error!(error = %e, path = p, "replay unreadable"))
                .ok()?;
            read_replay_file(&d)
                .or_else(|| {
                    tracing::error!(path = p, "not a replay file");
                    None
                })
        });
    let career_id: Option<i16> = args
        .iter()
        .position(|a| a == "--career")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let mission_flag = args.iter().any(|a| a == "--mission");
    let use_menu = positional.is_empty()
        && !args.iter().any(|a| a == "--autotest")
        && career_id.is_none()
        && !mission_flag
        && replay_file.is_none();
    // `--career <id>`: start the next event of tournament <id> from the saved career (buying the
    // cheapest eligible car when the garage has none); used for headless smoke runs
    let mut career = if use_menu || career_id.is_some() || mission_flag {
        CareerUi::load(&paths)
            .map_err(|e| tracing::warn!(error = %e, "career unavailable"))
            .ok()
    } else {
        None
    };
    let mut career_request = None;
    // `--mission`: start the current Factory Driver mission headlessly (with --autotest)
    if mission_flag && let Some(ui) = career.as_mut() {
        ui.set_mode(3);
        career_request = Some(
            ui.activate_mission()
                .ok_or_else(|| anyhow::anyhow!("cannot start a mission: {}", ui.message))?,
        );
    }
    if let (Some(id), Some(ui)) = (career_id, career.as_mut()) {
        let idx = ui
            .tournaments()
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow::anyhow!("tournament {id} not in the current era"))?;
        ui.t_idx = idx;
        ui.ensure_car();
        ui.car_sel = 0;
        career_request = Some(
            ui.activate()
                .ok_or_else(|| anyhow::anyhow!("cannot start tournament {id}: {}", ui.message))?,
        );
    }
    let mut replay_setup = replay_file.as_ref().map(|r| r.setup.clone());
    if let Some(rs) = replay_setup.as_mut() {
        resolve_replay_car(&paths, rs);
    }
    let track_name = replay_setup
        .as_ref()
        .map(|r| r.track.clone())
        .or(career_request.as_ref().map(|r| r.track_file.clone()))
        .or(positional.first().cloned())
        .unwrap_or_else(|| "alps".into());
    let car_name = replay_setup
        .as_ref()
        .map(|r| r.car.clone())
        .or(career_request.as_ref().map(|r| r.car.model.clone()))
        .or(positional.get(1).cloned())
        .unwrap_or_else(|| "993".into());
    let (track_name, car_name) = (track_name.as_str(), car_name.as_str());
    let t0 = Instant::now();
    let mut track = TrackAssets::load(&paths, track_name)?;
    let mut skill = None;
    if let Some(ms) = career_request.as_ref().and_then(|r| r.mission.as_ref())
        && let Some(suffix) = &ms.scene_suffix
    {
        if let Some(scene) = track.load_scene(suffix)? {
            skill = Some(SkillTest::from_scene(&scene, &track.name));
            track.extra_scenes.push(scene);
        }
    }
    let car = CarAssets::load(&paths, car_name)?;
    // simulation variant: catalogue entry whose model matches, else "<car>coupe36"-style guess
    let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(
        paths.fe_data().join("Data").join("nfs5.car"),
    )?)?;
    let sim_name = replay_setup
        .as_ref()
        .map(|r| r.sim.clone())
        .or_else(|| career_request.as_ref().map(|r| r.car.sim.clone()))
        .or_else(|| positional.get(2).cloned())
        .or_else(|| {
            catalog
                .iter()
                .find(|c| c.model.eq_ignore_ascii_case(car_name) && !c.sim.is_empty())
                .map(|c| c.sim.clone())
        })
        .unwrap_or_else(|| "default".into());
    let sim = CarAssets::load_sim(&paths, &sim_name)?;
    let sound_set = catalog
        .iter()
        .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
        .map(|c| c.sound_set.clone())
        .unwrap_or_else(|| "sd7a".into());
    let paint = replay_setup
        .as_ref()
        .map(|r| r.paint)
        .or_else(|| career_request.as_ref().map(|r| r.paint))
        .or_else(|| {
            catalog
                .iter()
                .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
                .and_then(|c| c.paint_sets.first())
                .map(|p| p.colours[0])
        })
        .unwrap_or([0xD4, 0x10, 0x10]);
    let race_class = sim_name.to_lowercase().starts_with("gt") || sim_name.starts_with("935");
    let mut specs = CarSpecs::from_sim(&sim, race_class);
    if let Some(r) = &career_request {
        let family = catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| c.sim_family.clone())
            .unwrap_or_default();
        specs.apply_parts(&r.parts.iter().collect::<Vec<_>>(), &family);
    }
    let specs = specs;
    tracing::info!(
        sim = sim_name,
        name = specs.name,
        top_speed_mps = specs.top_speed,
        "car specs"
    );
    tracing::info!(
        track = track_name,
        car = car_name,
        secs = t0.elapsed().as_secs_f32(),
        "assets loaded"
    );
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    // `--fe` boots into the original front end instead of the text menu, on the quick-race screen
    // or on the layout named after it (`--fe carcompare`)
    let fe_start = args.iter().find(|a| a.starts_with("--fe")).map(|a| match a.split_once('=') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => "QuickRace".to_string(),
    });
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1).cloned());
    let layout_sweep = args.iter().any(|a| a == "--layout-sweep");
    // The sweep measures **gaps**, and a widget that is correctly blank because an animation has
    // not started yet is not one. `data.animating` keeps a wordwrap widget empty for its first
    // 0x97 = 151 ticks (§4.87), so a sweep taken at clock 0 counts five of them; preloading the
    // screen clock past that wait keeps the measurement comparable with every earlier round.
    // An explicit `NFS_FE_CLOCK` still wins, so a sweep can be taken at any moment on purpose.
    if layout_sweep && std::env::var("NFS_FE_CLOCK").is_err() {
        // SAFETY: single-threaded start-up, before any screen is loaded
        unsafe { std::env::set_var("NFS_FE_CLOCK", "5000") };
    }
    let fe_start = if layout_sweep { fe_start.or_else(|| Some("QuickRace".to_string())) } else { fe_start };
    // the views the player cycles through come from the registry, like everything else in `fe.txt`
    let camera_cycle =
        nfs_game::camera::cycle_from_registry(&nfs_formats::fetxt::Registry::load(&paths.root), 0);
    tracing::info!(cycle = ?camera_cycle, "camera cycle");
    // diagnostic: `NFS_CAMERA=<index into the cycle>` starts on that view instead of the first
    let camera_mode = std::env::var("NFS_CAMERA")
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(0);
    let autotest = args
        .iter()
        .position(|a| a == "--autotest")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let follow_road = args.iter().any(|a| a == "--follow");
    // `--net` without `--opponents` still needs one seat for the other player
    let num_opponents_default = u32::from(args.iter().any(|a| a == "--net"));
    let num_opponents: u32 = replay_setup
        .as_ref()
        .map(|r| r.opponents)
        .or_else(|| career_request.as_ref().map(|r| r.opponents.len() as u32))
        .or_else(|| {
            args.iter()
                .position(|a| a == "--opponents")
                .and_then(|i| args.get(i + 1)?.parse().ok())
        })
        .unwrap_or(num_opponents_default);
    let num_laps: u32 = replay_setup
        .as_ref()
        .map(|r| r.laps)
        .or_else(|| career_request.as_ref().map(|r| r.laps))
        .or_else(|| {
            args.iter()
                .position(|a| a == "--laps")
                .and_then(|i| args.get(i + 1)?.parse().ok())
        })
        .unwrap_or(2)
        .max(1);
    // Quick Knockout: laps = opponents (modes.md §4)
    let num_laps = if args.iter().any(|a| a == "--quick-knockout") {
        num_opponents.max(1)
    } else {
        num_laps
    };
    let record = args
        .iter()
        .position(|a| a == "--record")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| {
            let mut w = std::io::BufWriter::new(std::fs::File::create(p).ok()?);
            use std::io::Write;
            writeln!(
                w,
                "tick,throttle,brake,steer,handbrake,x,y,z,vx,vy,vz,rpm,gear,slice"
            )
            .ok()?;
            Some(w)
        });
    let backwards = match &replay_setup {
        Some(r) => r.backwards,
        None => {
            args.iter().any(|a| a == "--backwards")
                || career_request
                    .as_ref()
                    .map(|r| r.backwards)
                    .unwrap_or(false)
        }
    };
    let records_path = std::env::var_os("NFS5_RECORDS_SAVE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Records::default_save_path);
    let records = Records::load(&paths, &records_path);
    // race options from the front-end registry (shipped: skill 0, one lap, no traffic, no cops)
    let fe = nfs_formats::fetxt::RaceSettings::from_registry(&nfs_formats::fetxt::Registry::load(
        &paths.root,
    ));
    let menu = if use_menu {
        let mut cars = catalog
            .iter()
            .filter(|c| !c.sim.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        cars.sort_by_key(|c| c.year);
        // `NFS_FE_CAR=<display name substring>` preselects a car, for stills of the showroom — the
        // same kind of diagnostic as `NFS_MENU_MODE`
        let car_idx = std::env::var("NFS_FE_CAR")
            .ok()
            .and_then(|want| cars.iter().position(|c| c.display_name.contains(want.as_str())))
            .or_else(|| cars.iter().position(|c| c.model.eq_ignore_ascii_case("993") && c.sim.contains("coupe")))
            .unwrap_or(0);
        let mut menu0 = Menu {
            mode: std::env::var("NFS_MENU_MODE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            tracks: TrackAssets::list(&paths),
            cars,
            track_idx: 0,
            car_idx,
            car2_idx: 0,
            ko: Vec::new(),
            ko_sel: 0,
            opponents: 4,
            ko_opponents: 4,
            damage: true,
            opp_car: std::env::var("NFS_OPPCAR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            paint_idx: 0,
            laps: num_laps,
            traffic: std::env::var("NFS_TRAFFIC")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(fe.traffic_density as u32),
            cops: std::env::var("NFS_COPS")
                .map(|v| v != "0")
                .unwrap_or(fe.cops),
            backwards,
            mirror: false,
            circuits: records.circuits(),
            knockout_line: "KNOCKOUT   ENTER STARTS A SERIES".into(),
            row: 0,
            font: None,
        };
        menu0.fit_ko();
        Some(menu0)
    } else {
        None
    };
    if let (Some(m), Some(c)) = (&menu, career.as_mut()) {
        c.set_mode(m.mode);
    }
    let mission_start = career_request.as_ref().and_then(|r| r.mission.clone());

    // `--net-browse N`: probe for N seconds and print the sessions that answer, then exit. The
    // same `'w'` / `'d'` exchange the network screens run, without a window.
    if let Some(secs) = args
        .iter()
        .position(|a| a == "--net-browse")
        .and_then(|i| args.get(i + 1)?.parse::<f32>().ok())
    {
        let mut lobby = netplay::Lobby::open("browser")?;
        let until = std::time::Instant::now() + Duration::from_secs_f32(secs);
        while std::time::Instant::now() < until {
            lobby.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        let games = lobby.games();
        println!("{} session(s):", games.len());
        for g in &games {
            println!("  {g}");
        }
        return Ok(());
    }

    // `--net host[:PORT]` / `--net join:HOST[:PORT]`: two copies of the port in one race.
    let net = match args
        .iter()
        .position(|a| a == "--net")
        .and_then(|i| args.get(i + 1))
    {
        Some(spec) => {
            let who = args
                .iter()
                .position(|a| a == "--net-name")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| {
                    if spec.starts_with("host") {
                        "host".into()
                    } else {
                        "racer".into()
                    }
                });
            Some(netplay::NetPlay::start(spec, &who)?)
        }
        None => None,
    };

    let audio_settings = nfs_audio::music::Settings::load(&paths.root);
    let fe_strings = std::fs::read(paths.fe_data().join("Locale").join("FEStrings.csv"))
        .map(|d| nfs_formats::locale::Strings::parse(&d, 0))
        .unwrap_or_default();
    let race_skill = fe.skill;
    let music_names = nfs_audio::music::song_titles(&paths.game_data().join("Music")).unwrap_or_default();
    let music_lengths = nfs_audio::music::song_lengths_ms(&paths.game_data().join("Music")).unwrap_or_default();
    let mut app = App {
        fe: FrontEnd {
            era_music: false,
            bar_grab: None,
            chronicle_group: CHRONICLE_GROUP,
            chronicle_col: None,
            usedcars_showparts: false,
            usedcar_sel: 0,
            parts_sel: None,
            setting_repeat: None,
            mouse_down: false,
            repair_ticks: Default::default(),
            binding_base: None,
            parts_forced: false,
            last_hover: None,
            dialog: None,
            dialog_forced: false,
            dialog_answer: None,
            replay_sel: 0,
            user_sel: None,
            cd_available: None,
            cd_selected: None,
            cd_title: 0,
            cd_current: -1,
            after_replay: false,
            test_drive: None,
            cursor: None,
            // both of `Parts.AllParts`'s sections start closed (0x515190, 0x515193)
            parts_open: [false, false],
            factory_profile: 0,
            menu,
            menu_template: None,
            room: Vec::new(),
            cars: Vec::new(),
            cars_key: Vec::new(),
            ms: 0,
            layout_preview: None,
            layout_bottom: None,
            strings: fe_strings,
            layout_name: args
                .iter()
                .position(|a| a == "--layout")
                .and_then(|i| args.get(i + 1).cloned())
                .or(fe_start),
            layout_sel: 0,
            layout_open_combo: None,
            list_drag: None,
            list_repeat: None,
            layout_stack: Vec::new(),
        },
        layout_sweep,
        registry: nfs_formats::fetxt::Registry::load(&paths.root),
        choices: std::collections::HashMap::new(),
        paths,
        net: Network { session: net, lobby: None },
        career,
        career_opponents: career_request.map(|r| r.opponents),
        // diagnostic: `NFS_OPPCAR=0..4` picks `OpponentCar.Car` without going through the menu
        opp_choice: std::env::var("NFS_OPPCAR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        player_car_id: None,
        pad_stick_latched: false,
        skill,
        mission: mission_start,
        prev_car_pos: Vec3::ZERO,
        world_actions: Vec::new(),
        render_prev: None,
        frame_dt: 1.0 / 60.0,
        paused: false,
        knockout: None,
        player_grid_override: None,
        backwards,
        mirror: replay_setup
            .as_ref()
            .map(|r| r.mirror)
            .unwrap_or_else(|| args.iter().any(|a| a == "--mirror")),
        quick_knockout: args.iter().any(|a| a == "--quick-knockout"),
        player_eliminated: false,
        records,
        replay: Playback {
            stream: Some(match &replay_file {
                Some(f) => nfs_game::replay::Replay::playing(f.stream.clone(), f.final_tick),
                None => nfs_game::replay::Replay::new(),
            }),
            speed: nfs_game::replay::Speed::Normal,
            setup: replay_setup
                .clone()
                .unwrap_or(nfs_game::replay::ReplaySetup {
                    track: track_name.to_string(),
                    car: car_name.to_string(),
                    sim: sim_name.clone(),
                    car_id: catalog
                        .iter()
                        .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
                        .map(|c| c.id)
                        .unwrap_or(0),
                    paint,
                    laps: num_laps,
                    opponents: num_opponents,
                    traffic: std::env::var("NFS_TRAFFIC")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(fe.traffic_density as u32),
                    cops: std::env::var("NFS_COPS")
                        .map(|v| v != "0")
                        .unwrap_or(fe.cops),
                    backwards,
                    mirror: args.iter().any(|a| a == "--mirror"),
                    seed: 0,
                    skill: fe.skill,
                }),
            path: nfs_game::save::save_dir().join("replay.prpl"),
            saved: false,
            loop_count: 0,
            end_ticks: 0,
            camera: 0,
        },
        // the original seeds the race RNG from the registry's RAND_SEED (replay.md §11); a replay
        // carries the seed so playback re-simulates the same traffic, police and AI
        cheat_bits: 0,
        traffic_seed: match (
            &replay_setup,
            std::env::var("NFS_TRAFFIC_SEED")
                .ok()
                .and_then(|v| v.parse().ok()),
        ) {
            (Some(r), _) => r.seed,
            (None, Some(s)) => s,
            (None, None) => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u32)
                .unwrap_or(1),
        },
        scripted: autotest.is_some(),
        stuck_ticks: 0,
        detail: std::env::var("NFS_DETAIL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        split_times: 1,
        records_path,
        car_id: catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| c.id)
            .unwrap_or(0),
        player_name: nfs_game::save::read_player_lst(&nfs_game::save::save_dir())
            .and_then(|s| nfs_game::save::Profile::from_stem(&s))
            .map(|p| p.name)
            .unwrap_or_else(|| "DRIVER".to_string()),
        record_done: false,
        track,
        car,
        screenshot,
        autotest,
        window: None,
        renderer: None,
        world: None,
        car_model: None,
        driver_cam: None,
        // `NFS_CAMERA=0..3` picks the starting view (chase / bonnet / trackside / cockpit) —
        // the port's own switch, for screenshots and tests
        sky: None,
        scene: Scene::default(),
        camera: Camera::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        car_pos: Vec3::ZERO,
        car_rot: Quat::IDENTITY,
        specs,
        vehicle: None,
        camera_mode,
        camera_cycle,
        particles: nfs_game::particles::Particles::default(),
        particle_draw: None,
        shadow_draw: None,
        shadow_meshes: Vec::new(),
        glare_draw: None,
        flare_defs: Vec::new(),
        flare_sprites: std::collections::HashMap::new(),
        track_particles: None,
        track_particle_defs: Vec::new(),
        track_particle_sprites: std::collections::HashMap::new(),
        track_cameras: None,
        env_bound: [None; 3],
        slices: None,
        race: None,
        road: RoadCars {
            opponents: Vec::new(),
            opponent_models: Vec::new(),
            opp_prev: Vec::new(),
            traffic: None,
            traffic_models: Vec::new(),
            opponent_sets: Vec::new(),
            traffic_sets: Vec::new(),
            opponent_paints: Vec::new(),
            traffic_paints: Vec::new(),
            police: None,
            police_models: Vec::new(),
            police_prev: Vec::new(),
            police_bodies: Vec::new(),
            cops_race: false,
            cops_enabled: std::env::var("NFS_COPS")
                .map(|v| v != "0")
                .unwrap_or(fe.cops),
            traffic_prev: Vec::new(),
            traffic_bodies: Vec::new(),
            traffic_specs: None,
            traffic_spin: Vec::new(),
            // the registry's `traffic_density`, or `NFS_TRAFFIC=0..3` for the menu-less --autotest start
            traffic_count: TRAFFIC_COUNT[replay_setup
                .as_ref()
                .map(|r| r.traffic as usize)
                .unwrap_or_else(|| {
                    std::env::var("NFS_TRAFFIC")
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(fe.traffic_density as usize)
                })
                .min(3)],
        },
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
        resets: 0,
        hud: None,
        dials: catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| (c.dial_kmh, c.dial_tach)),
        paint,
        snd: AudioState {
            audio: None,
            engine_sound: None,
            sfx: None,
            ai_engines: None,
            ambience: None,
            reverb: nfs_audio::reverb::Reverb::default(),
            music: None,
            music_names,
            music_lengths,
            audio_settings,
            speech: None,
            countdown_said: 0,
            sound_set,
            last_gear: 1,
            crash_played: false,
            fe_sfx: None,
        },
        sim_name: sim_name.clone(),
        race_skill,
        last_frame: Instant::now(),
        accumulator: Duration::ZERO,
        tick: 0,
        frames: 0,
        fps_timer: Instant::now(),
    };
    // the player's key bindings are `fe.txt`'s `C1_*` lines (`nfs_input::bindings`)
    app.input.bindings = nfs_input::bindings::Bindings::from_registry(|k| app.registry.get(k));
    // …and the profile's own, when it has saved any
    app.apply_saved_controls();
    // every `<Axis>.Min` / `.Max` starts at 0 / 0x80 (0x51E546, 0x51E550)
    app.input.axis_bands = [[0, 0x80]; 8];
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// The `nfs5.prt` categories one `QUICKFIX.<name>` box covers — the same mapping the per-category
/// cost getters use (0x4EFBC0 and its nine neighbours), and what `repair.lay`'s ten boxes tick.
/// `RE/notes/controls.md` §4.96.
fn quickfix_kinds(name: &str) -> Option<&'static [u16]> {
    Some(match name {
        "ENGINE" => &[1],
        "BRAKES" => &[2],
        "TRANSMISSION" => &[3],
        "SUSPENSION" => &[4],
        "SHOCKS" => &[5],
        "BODYWORK" => &[10],
        "SWAYBARS" => &[8],
        "TIRES" => &[9],
        "COSMETIC" => &[6, 7],
        "OVERALL" => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        _ => return None,
    })
}

/// The ten `QUICKFIX.*` names `repair.lay`'s check boxes carry, in the order the accessors are
/// registered at 0x4F252A…0x4F258B. `RE/notes/controls.md` §4.96.
const QUICKFIX_NAMES: [&str; 10] = [
    "ENGINE", "BRAKES", "TRANSMISSION", "SUSPENSION", "SHOCKS", "BODYWORK", "SWAYBARS", "TIRES", "COSMETIC", "OVERALL",
];
