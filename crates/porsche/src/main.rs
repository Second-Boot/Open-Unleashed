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

mod career_ui;
mod netplay;

use anyhow::Result;
use career_ui::{CareerOpponent, CareerUi};
use glam::{Mat4, Quat, Vec3};
use nfs_assets::{GamePaths, car::CarAssets, track::TrackAssets};
use nfs_game::layout::{ListCell, ListRow};
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
const HUD_REF_W: f32 = 640.0;
const HUD_ORANGE: [f32; 4] = [0.98, 0.706, 0.0, 1.0];
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
    pub fn results_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        rows: &[(String, String, Option<u64>, Option<u64>)],
        player: usize,
    ) {
        if self.font.is_none() {
            return;
        }
        let n = rows.len().max(1) as f32;
        let (x, y, w, h) = (60.0, 120.0, 520.0, 26.0 + 20.0 * n + 24.0);
        self.tray(out, aspect, x, y, w, h);
        let cols = [x + 8.0, x + 46.0, x + 190.0, x + 380.0, x + 450.0];
        let head = ["", "NAME", "CAR", "BEST LAP", "TIME"];
        for (i, t) in head.iter().enumerate() {
            self.text(out, aspect, t, cols[i], y + 6.0, 11.0, HUD_GREY);
        }
        let ord = ["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th"];
        for (k, (name, car, best, total)) in rows.iter().enumerate() {
            let yy = y + 26.0 + 20.0 * k as f32;
            let color = if k == player {
                HUD_ORANGE
            } else {
                [1.0, 1.0, 1.0, 0.95]
            };
            let cells = [
                ord[k.min(7)].to_string(),
                name.chars().take(18).collect::<String>(),
                car.chars().take(24).collect::<String>(),
                best.map(format_ticks).unwrap_or_else(|| "-".into()),
                total.map(format_ticks).unwrap_or_else(|| "-".into()),
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
/// The rows whose slot is not a registry slot are not here: `Track<n>.Laps`, `Track<n>.Traffic`,
/// `Track<n>.Direction`, `OwnedCar.PackageNum`, `Car.Transmission`, `Game.Difficutly` (the
/// original's own spelling), `CDPlayer.View`, `Comms.Positional` and `Comms.Collision` keep their
/// choice in screen state, and the port already answers the ones it has state for.
const CHOICE_ACCESSORS: &[(&str, &str, &[u32])] = &[
    ("Audio.Mode", "audio3d", &[1500, 1501]),
    ("Audio.Quality", "audioquality", &[1504, 1503, 1502, 1500]),
    ("CDPlayer.Options", "randomplaylist", &[1856, 1855]),
    ("Display.Damage", "damagedisplay", &[1500, 1805, 1806, 1807]),
    ("Display.DispCols", "displaycolors", &[1821, 1822, 1823]),
    ("Display.Gear", "gear", &[1500, 1501]),
    ("Display.Map", "map", &[1500, 1808, 1809, 1830]),
    ("Display.MapScale", "mapscale", &[1810, 1811, 1812, 1813, 1814]),
    ("Display.OppList", "opponentlist", &[1500, 1832, 1833]),
    ("Display.RearView", "rearviewmirrror", &[1500, 1501]),
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
        let a = &self.audio_settings;
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
    fn choice_index(&self, name: &str) -> Option<(usize, &'static [u32])> {
        let (_, key, options) = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == name)?;
        let v = match self.choices.get(*key) {
            Some(v) => *v,
            None => self.registry.u32(key, 0) as usize,
        };
        Some((v.min(options.len() - 1), options))
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
    opponents: u32,
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
    /// Keep the knockout location table as long as the opponent count, which is what
    /// `Knockout.TrackNumber`'s accessor measures it by (0x4BA786 reads `Game.NumKOOpponents`, and
    /// 0x4BA7CF answers the empty string for any row at or past it). A row that appears takes the
    /// screen's single-race settings, which is what it shows before the player edits it.
    fn fit_ko(&mut self) {
        let want = self.opponents as usize;
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
                &format!("TRACK   < {} >", self.tracks[self.track_idx].to_uppercase()),
                [0.08, 0.30],
                0.045,
                sel(1),
            ));
        }
        let c = &self.cars[self.car_idx];
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
            &format!("OPPONENTS  < {} >", self.opponents),
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
                &format!("LAPS    {}  (ONE CAR OUT PER LAP)", self.opponents.max(1)),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        } else if self.mode == 5 {
            out.extend(f.sprites(
                &format!(
                    "RACES   {}   LAPS < {} >   (LAST CAR OUT EACH RACE)",
                    self.opponents.max(1),
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
    menu: Option<Menu>,
    /// menu kept for returning after a race
    menu_template: Option<Menu>,
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
    /// Input recording / playback (`RE/notes/replay.md`): the original records the four-byte
    /// controller packet per 32 Hz frame and re-simulates everything else.
    replay: Option<nfs_game::replay::Replay>,
    replay_speed: nfs_game::replay::Speed,
    replay_setup: nfs_game::replay::ReplaySetup,
    replay_path: std::path::PathBuf,
    replay_saved: bool,
    /// Loop-iteration counter the playback-speed decimation of `replay.md` §5.1 indexes.
    replay_loop: u64,
    /// Ticks since a played-back stream ran out.
    replay_end_ticks: u64,
    /// Seed of the shared RNG for this race; travels with a replay so playback re-simulates the
    /// same traffic, police and AI.
    traffic_seed: u32,
    /// The profile name the original stores in a track record (`player.lst` → `LoadInfo`).
    player_name: String,
    /// `nfs5.car` id of the car being driven, for the replay snapshot's car slot.
    car_id: u16,
    /// the roll-over crash sound fires once per roll-over
    crash_played: bool,
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
    /// `Factory.ProfileNum` — which of the seven factory-team profiles `teamprofile.lay` shows.
    /// The original registers the accessor with a default of 6 (0x4BA310); the port starts at 0 and
    /// the screen has no control of its own to move it, so this only moves if something else sets
    /// it. MEDIUM.
    factory_profile: usize,
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
    /// the front end's 3D room and the car standing in it (`RE/notes/frontend-3d.md`), each keyed
    /// by the model name so a screen change only reloads what actually changed
    fe_room: Vec<(String, CarModel)>,
    /// the cars standing in the front end: the room's own (`usize::MAX`) and one per
    /// `NFS5.3DCAR` widget with a `positiontype` of its own
    fe_cars: Vec<(usize, String, CarModel)>,
    /// milliseconds the front end has been up, for the turntable
    fe_ms: u64,
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
    /// `CARG`, the glare slot 0 of every light kind in the table at 0x5CD73C
    glare_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// trackside cameras and the triggers that switch them (`RE/notes/race-logic.md` §3.3)
    track_cameras: Option<nfs_game::cameras::TrackCameras>,
    /// tunnel reverb level (`engine-audio.md` §4.7)
    reverb: nfs_audio::reverb::Reverb,
    /// environment map currently bound on the car materials
    env_bound: Option<nfs_renderer::TextureId>,
    slices: Option<SliceTable>,
    race: Option<RaceProgress>,
    /// opponents (parked until the AI rules are implemented) with their own models
    opponents: Vec<RaceCar>,
    opponent_models: Vec<CarModel>,
    /// `--layout <name>`: draw a `FEData/Layouts/<name>.lay` screen instead of the menu
    layout_preview: Option<nfs_game::layout::LayoutScreen>,
    /// `bottombar.lay`, which the front end loads once and draws over every screen (0x4D39D3).
    layout_bottom: Option<nfs_game::layout::LayoutScreen>,
    /// `FEData/Locale/FEStrings.csv`, for the layout string ids
    fe_strings: nfs_formats::locale::Strings,
    layout_name: Option<String>,
    /// selected button on the previewed screen, and the screens we came from
    layout_sel: usize,
    layout_stack: Vec<String>,
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
    ai_engines: Option<nfs_audio::ai_engine::AiEngines>,
    ambience: Option<nfs_audio::ambience::Ambience>,
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
    racing_line: Option<nfs_formats::ais::RacingLine>,
    /// `--net host[:PORT]` / `--net join:HOST[:PORT]` — network play (`netplay`, `network.md` §7).
    net: Option<netplay::NetPlay>,
    /// The front end's side of it, alive while one of the four network layouts is open.
    lobby: Option<netplay::Lobby>,
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
    music: Option<nfs_audio::music::MusicPlayer>,
    /// The song names in `GameData/Music`, read once at start-up. The CD-player screen needs them
    /// in the **front end**, where there is no `MusicPlayer` yet — the original plays menu music
    /// there (every screen block carries `musicfile=menu*`) and that screen edits the playlist, so
    /// the lists must fill without a race having been started.
    music_names: Vec<String>,
    /// `fe.txt` audio settings (playlist, shuffle, the five level sliders)
    audio_settings: nfs_audio::music::Settings,
    speech: Option<nfs_audio::speech::Speech>,
    countdown_said: u8,
    sound_set: String,
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
            .audio_settings
            .playlist
            .iter()
            .map(|i| *i as u32)
            .collect();
        // what the settings screens hold, for the save's accessor block
        let a = &self.audio_settings;
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
            laps: self.menu.as_ref().map(|m| m.laps).unwrap_or(self.num_laps),
            opponents: self
                .menu
                .as_ref()
                .map(|m| m.opponents)
                .unwrap_or(self.num_opponents),
            traffic: self.menu.as_ref().map(|m| m.traffic).unwrap_or(0),
        };
        if let Some(c) = self.career.as_mut() {
            c.knockout = ko;
            c.playlist = playlist;
            c.settings = settings;
        }
    }

    /// Abandon the running race and go back to the menu; `false` when there is no menu to go back
    /// to (the app was started straight into a race).
    /// Values for the front-end accessors an `NFS5.AccessorWatch` can name. Only the ones the port
    /// actually has state for are answered; anything else returns `None` and the watch is left
    /// blank rather than filled with something invented. The `(MODIFIER)` suffix the layouts use
    /// (`OwnedCars.Car(SHORT)`) selects a shorter form of the same value.
    fn accessor_value(&self, name: &str) -> Option<String> {
        let (base, modifier) = match name.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')')),
            None => (name, ""),
        };
        let menu = self.menu.as_ref();
        let career = self.career.as_ref();
        Some(match base {
            "Showcase.Text" => self.showcase_text(modifier.parse::<usize>().ok()?)?,
            // the tournament screen. Each field is one the `nfs5.trn` record already carries, and
            // `(LOCATION)` is what 0x506992 makes it: the accessor looks up
            // `Tournament.RemainingTracks`, calls its vtable +0x70 for an id and hands that to the
            // `FEStrings` lookup 0x4E06D0 — the name of the event the tournament is on.
            //
            // `(DIFFICULTY)` is `FEStrings[450 + n]` (0x506823 adds 0x1C2 to a two-byte field and
            // looks it up), and 450…452 are the three names `Game.Difficutly` uses. The port takes
            // `n` from the record's `+0xE6`, which is the difficulty it already parsed; the getter
            // reads `+0x78` of a run-time object instead, so the two are **MEDIUM**, not confirmed
            // to be the same field.
            "Tournament.Tournament" => {
                let c = self.career.as_ref()?;
                let t = c.current()?;
                match modifier {
                    "SHORT" | "" => self.fe_strings.get(t.name_string_id)?.to_uppercase(),
                    "DESCRIPTION" => self.fe_strings.get(t.description_string_id)?.to_string(),
                    "CARRESTRICTION" => self.fe_strings.get(t.restriction_string_id)?.to_string(),
                    "ENTRYFEE" => format!("${}", t.entry_fee),
                    "PRIZEMONEY" => format!("${}", t.prize.first()?),
                    "DIFFICULTY" => self.fe_strings.get(450 + t.difficulty_level.min(2) as u32)?.to_string(),
                    "LOCATION" => {
                        // the event the tournament is on: the first one the player has not finished
                        let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                        let ev = t.events.get(done.min(t.events.len().saturating_sub(1)))?;
                        c.tables.track_name(ev.track_id).to_uppercase()
                    }
                    _ => return None,
                }
            }
            // the end-of-era lines on `VictoryMessage.lay`. Both read the era's finale tournament
            // record: `ENDERAHEADER` is `FEStrings[[rec+0x24]]` (0x506389) and `ENDERADESCRIP`
            // `FEStrings[[rec+0x28]]` (0x5063DE), and +0x24 and +0x28 are the port's
            // `title_string_id` and `description_string_id`. The record is what
            // `0x504990(0x504800())` returns; the port takes the era's hidden finale, which its
            // own `visibility == 2` marks.
            "Tournament.Data" if matches!(modifier, "ENDERAHEADER" | "ENDERADESCRIP") => {
                let c = self.career.as_ref()?;
                let list = c.tables.era_tournaments(c.career.era);
                let t = list.iter().find(|t| t.visibility == 2).or_else(|| list.last())?;
                let id = if modifier == "ENDERAHEADER" { t.title_string_id } else { t.description_string_id };
                self.fe_strings.get(id)?.to_string()
            }
            // the factory-drive mission the career is on (`missionbrief.lay`, `missionmap.lay`).
            // `NARRNAME` is `FEStrings[(i16)[mission+0x1E] + 0x9C4]` — 0x519DCA reads the field and
            // 0x519DCE adds 2500 — `MISSONDESC` is `FEStrings[[mission+0]]`, the id 0x519D67 loads
            // straight out of the record, and `ENDOFLEVEL` is `FEStrings[[record+0xC]]` (0x519E31).
            // All three offsets are ones the port already parses: `narrator` at +0x1E,
            // `intro_string_id` at +0 and `promotion_string_id` at +0x0C.
            "Factory.Data" if matches!(modifier, "NARRNAME" | "MISSONDESC" | "TRACKNAME" | "ENDOFLEVEL") => {
                let c = self.career.as_ref()?;
                let m = &c.tables.missions[c.career.current_mission(&c.tables)?];
                match modifier {
                    "NARRNAME" => self.fe_strings.get(2500 + m.narrator.max(0) as u32)?.to_string(),
                    "MISSONDESC" => self.fe_strings.get(m.intro_string_id)?.to_string(),
                    // the promotion line at the end of a stage: 0x519E31 reads `[record+0xC]` and
                    // looks it up, and +0x0C is the port's `promotion_string_id`
                    "ENDOFLEVEL" => self.fe_strings.get(m.promotion_string_id)?.to_string(),
                    _ => c.tables.track_name(m.track_id).to_uppercase(),
                }
            }
            // one of the factory team's profiles (`teamprofile.lay`). Each getter ends in the same
            // `FEStrings` lookup with the profile index scaled by four: `profilename` is
            // `0xDAD + 4n` (0x519F1F), `profiletitle` `0xDAE + 4n` (0x519FBD), `profileage`
            // `0xDAF + 4n` (0x519F6E) and `profiledesc` the next — 3501 "Dieter", 3502 "Porsche
            // Executive", 3503 "45", 3504 his description, then the same four for Billy.
            // CONFIRMED, and `FEStrings.csv` carries exactly 28 of them: seven profiles.
            "Factory.Data" => {
                let base = match modifier {
                    "profilename" => 3501,
                    "profiletitle" => 3502,
                    "profileage" => 3503,
                    "profiledesc" => 3504,
                    _ => return None,
                };
                self.fe_strings.get(base + 4 * self.factory_profile as u32)?.to_string()
            }
            // a range setting the port has no other state for — the force-feedback volumes and
            // the gamma — shows its number
            "FF.TotalVolume" | "FF.RoadVolume" | "FF.EngineVolume" | "FF.CollisionVolume"
            | "FF.RoadGripVolume" | "Graphics.GammaCorrection" => {
                let (v, _, hi) = self.range_value(base)?;
                if modifier == "PERCENT" { format!("{}%", v * 100 / hi.max(1)) } else { v.to_string() }
            }
            // a multiple-choice setting: the text is the `FEStrings` entry its chosen index names
            _ if CHOICE_ACCESSORS.iter().any(|(n, _, _)| *n == base) => {
                let (i, options) = self.choice_index(base)?;
                self.fe_strings.get(options[i])?.to_string()
            }
            // the car-compare screen's five numbers — `CarCompare.Data(<n>CarCompareCar<m>)`, the
            // same source name its bars read (`fn compare_rating`)
            "CarCompare.Data" => self.compare_rating(modifier)?.to_string(),
            // the network screens (`netplay::Lobby`): the names come out of the layouts themselves
            "Comms.IP" => self.lobby.as_ref()?.ip.clone(),
            "Comms.Data" => {
                // once a session is open the lobby screens read from it, not from the browser
                if let Some(n) = self.net.as_ref() {
                    return Some(match modifier {
                        "GAMENAME" => n.game_name(),
                        "MYIPADDRESS" => n.my_address(),
                        "POSITIONALUPDATES" => if n.positional { "ON" } else { "OFF" }.to_string(),
                        "TRACKNAME" => menu.map(|m| m.tracks[m.track_idx].to_uppercase())?,
                        "TRACKLAPS" => menu?.laps.to_string(),
                        "TRACKDIRECTION" => if menu?.backwards {
                            "REVERSE"
                        } else {
                            "FORWARD"
                        }
                        .to_string(),
                        _ => return None,
                    });
                }
                let l = self.lobby.as_ref()?;
                match modifier {
                    "GAMENAME" => l.game_name(),
                    // `[desc+0x44]` copies the socket's own record out of `sock+0x08` (0x5878F0)
                    "MYIPADDRESS" => l.my_address(),
                    // the `0xBE` broadcast of §5.2, which `Comms.Positional` switches
                    "POSITIONALUPDATES" => if l.positional { "ON" } else { "OFF" }.to_string(),
                    // the race the host chose. Which message carries it to a joiner has not been
                    // traced, so these show this machine's own settings and no more.
                    "TRACKNAME" => menu.map(|m| m.tracks[m.track_idx].to_uppercase())?,
                    "TRACKLAPS" => menu?.laps.to_string(),
                    "TRACKDIRECTION" => if menu?.backwards {
                        "REVERSE"
                    } else {
                        "FORWARD"
                    }
                    .to_string(),
                    _ => return None,
                }
            }
            "Comms.Positional" => match self.net.as_ref() {
                Some(n) => if n.positional { "ON" } else { "OFF" }.to_string(),
                None => {
                    let l = self.lobby.as_ref()?;
                    if l.positional { "ON" } else { "OFF" }.to_string()
                }
            },
            // this port has no modem, so the modem screen's fields are blank, which is what a
            // machine without one shows
            "Comms.PhoneNumber" => String::new(),
            // chat is carried by a message the port has not traced, so the box stays empty
            "Comms.ChatInput" => String::new(),
            "Chronicle.Year" => career?.career.year.to_string(),
            "Game.NumOpponents" | "Game.NumKOOpponents" => menu?.opponents.to_string(),
            "Game.Difficutly" => {
                ["EASY", "MEDIUM", "HARD"][(self.race_skill as usize).min(2)].to_string()
            }
            "Track1.Track" | "LoopedTrack.Track" | "Knockout.Track" | "Record.Track"
            | "Track.Track" => {
                let t = menu
                    .map(|m| m.tracks[m.track_idx].clone())
                    .unwrap_or_else(|| self.track.name.clone());
                match modifier {
                    // the track's own best time, which is what the port's records table holds
                    "RECORDTIME" => {
                        format_ticks(self.records.track(&t)?.entries.first()?.time as u64)
                    }
                    _ => t.to_uppercase(),
                }
            }
            // the car-setup screen: the 17 floats the original keeps on the car and reads back
            // nowhere (`RE/notes/layouts.md`), so the port shows them and changes nothing
            b if nfs_game::career::CAR_SETUP_NAMES.contains(&b) => {
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let k = nfs_game::career::CAR_SETUP_NAMES
                    .iter()
                    .position(|n| *n == b)?;
                let v = c.career.garage.get(idx)?.setup[k];
                if v.fract() == 0.0 {
                    format!("{}", v as i64)
                } else {
                    format!("{v:.2}")
                }
            }
            // the repair screen: what each category costs on the selected car (`RepairCost`
            // 0x518880 per installed part, summed by the QUICKFIX screen 0x50F110)
            b if b.starts_with("QUICKFIX.") => {
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let kinds: &[u16] = match &b["QUICKFIX.".len()..] {
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
                };
                format!("${}", c.career.repair_cost(&c.tables, idx, kinds))
            }
            // the shop's basket total (`parts.lay` watches `Parts.ShoppingList(TOTAL)`)
            "Parts.ShoppingList" => {
                let c = career?;
                match modifier {
                    "TOTAL" | "" => format!("${}", c.basket_total()),
                    _ => return None,
                }
            }
            // one cell of `locationsetup.lay`'s table. The modifier is `<row>TRACK<FIELD>`:
            // 0x4BA838 takes the leading digit as the row (the same shape the car-compare bars
            // use), 0x4BA786 reads `Game.NumKOOpponents` for how many rows there are, and
            // 0x4BA7CF answers the empty string for a row at or past that count. `TRACKNUMBER` is
            // the row plus one (0x4BA800 formats `ebp + 1` with `"%d"`).
            "Knockout.TrackNumber" => {
                let m = menu?;
                let (row, field) = match modifier.as_bytes().first() {
                    Some(c) if c.is_ascii_digit() => (usize::from(c - b'0'), &modifier[1..]),
                    _ => (m.ko_sel, modifier),
                };
                if row >= m.opponents as usize {
                    return None;
                }
                let slot = m.ko.get(row)?;
                match field {
                    "TRACKNUMBER" => (row + 1).to_string(),
                    "TRACKNAME" => m.tracks.get(slot.track_idx)?.to_uppercase(),
                    "TRACKLAPS" => slot.laps.to_string(),
                    "TRACKTRAFFIC" => self.fe_strings.get(TRAFFIC_OPTIONS[(slot.traffic as usize).min(3)])?.to_string(),
                    "TRACKDIRECTION" => self.fe_strings.get(DIRECTION_OPTIONS[usize::from(slot.backwards)])?.to_string(),
                    _ => return None,
                }
            }
            "Track1.Laps" | "Knockout.Laps" => match menu {
                Some(m) if base == "Knockout.Laps" => m.ko.get(m.ko_sel).map_or(m.laps, |k| k.laps).to_string(),
                Some(m) => m.laps.to_string(),
                None => self.num_laps.to_string(),
            },
            // the screen's own option list, not a bare number: `FEStrings` 403, 402, 401, 400
            "Track1.Traffic" => {
                let t = match menu {
                    Some(m) if base == "Knockout.Traffic" => m.ko.get(m.ko_sel).map_or(m.traffic, |k| k.traffic),
                    Some(m) => m.traffic,
                    None => 0,
                };
                self.fe_strings.get(TRAFFIC_OPTIONS[(t as usize).min(3)])?.to_string()
            }
            "Track1.Direction" | "Knockout.Direction" => {
                let b = match menu {
                    Some(m) if base == "Knockout.Direction" => m.ko.get(m.ko_sel).map_or(m.backwards, |k| k.backwards),
                    Some(m) => m.backwards,
                    None => self.backwards,
                };
                self.fe_strings.get(DIRECTION_OPTIONS[usize::from(b)])?.to_string()
            }
            "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                // the garage screens ask the same accessor for the car's money and damage figures
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let owned = c.career.garage.get(idx);
                match modifier {
                    "CURRENTVALUE" => format!("${}", c.career.car_value(&c.tables, idx)?),
                    "PURCHASEPRICE" => format!("${}", owned?.purchase_price),
                    "OVERALLDAMAGE" => format!("{:.0}%", owned?.damage_pct),
                    // one part category's condition: `100 - mean(part damage)` over the installed
                    // parts of that category (`PART_DAMAGE_CATEGORY`)
                    m if PART_DAMAGE_CATEGORY.iter().any(|(n, _)| *n == m) => {
                        let cat = PART_DAMAGE_CATEGORY.iter().find(|(n, _)| *n == m).map(|(_, c)| *c)?;
                        let o = owned?;
                        let mut sum = 0.0f32;
                        let mut n = 0u32;
                        for (id, state) in &o.part_state {
                            if c.tables.parts.iter().any(|p| p.id == *id && p.kind == cat) {
                                sum += state;
                                n += 1;
                            }
                        }
                        let mean = if n > 1 { sum / n as f32 } else { sum };
                        format!("{:.0}", 100.0 - mean)
                    }
                    _ => owned
                        .and_then(|o| c.tables.car(o.car_id))
                        .map(|e| e.display_name.to_uppercase())
                        .unwrap_or_else(|| self.specs.name.to_uppercase()),
                }
            }
            // the car the front end has selected, and (quick race puts the field in every car) the
            // opponents' car, which this port takes from the same list
            "zMergedList.Car" | "OpponentCar.Car" | "UsedCar.Car" | "PrizeCars.Car" => match menu {
                // `(SHORT)` asks for the short form, which for a car is its family name
                Some(m) if modifier == "SHORT" => m.cars[m.car_idx].family.to_uppercase(),
                Some(m) => m.cars[m.car_idx].display_name.to_uppercase(),
                None => self.specs.name.to_uppercase(),
            },
            "OwnedCar.ExtColour" | "GenericCar.Colour" | "DealerCar.Colour" => {
                format!("{}", menu.map(|m| m.paint_idx + 1).unwrap_or(1))
            }
            "Game.GameType" => "QUICK RACE".to_string(),
            // the key-binding rows of `GameCtrl.lay` / `fluffctrl.lay`. The names are this port's
            // own keyboard map (`nfs-input`), not the original's — the original keeps its bindings
            // in its own configuration, which the port does not read.
            b if b.starts_with("Controller.") => {
                use nfs_input::Action;
                let action = match &b["Controller.".len()..] {
                    "Accel" => Action::Throttle,
                    "Brake" => Action::Brake,
                    "SteerLeft" => Action::SteerLeft,
                    "SteerRight" => Action::SteerRight,
                    "HandBrake" => Action::Handbrake,
                    "ShiftUp" => Action::ShiftUp,
                    "ShiftDown" => Action::ShiftDown,
                    "ResetCar" => Action::Reset,
                    "CameraView" => Action::CameraNext,
                    "LookBehind" => Action::RearView,
                    "Horn" => Action::Horn,
                    _ => return None,
                };
                nfs_input::key_for(action)?.to_string()
            }
            // the options screens: the levels the port actually mixes with (`fe.txt`, 0..127)
            // the CD player's now-playing line, and the sound-quality setting (`AUDIOQUALITY`)
            "CDPlayer.Title" => self
                .music
                .as_ref()
                .and_then(|m| m.now_playing())
                .map(|t| nfs_audio::music::split_title(t).0.to_uppercase())?,
            "Audio.Quality" => self.audio_settings.audio_quality.to_string(),
            // the draw-distance setting, as the original's index into `0x5CE90C` = {0.5, .67, .83, 1}
            "Graphics.ViewDistance" => {
                let idx = [0.5f32, 0.67, 0.83, 1.0]
                    .iter()
                    .position(|v| (v - self.detail).abs() < 0.01)
                    .unwrap_or(3);
                idx.to_string()
            }
            "Audio.MusicLevel" => self.audio_settings.music_level.to_string(),
            "Audio.SfxLevel" => self.audio_settings.sfx_level.to_string(),
            "Audio.EngineLevel" => self.audio_settings.engine_level.to_string(),
            "Audio.SpeechLevel" => self.audio_settings.speech_level.to_string(),
            "Audio.AmbientLevel" => self.audio_settings.ambient_level.to_string(),
            _ => return None,
        })
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
        let m = self.menu.as_ref()?;
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
        let car = match rest {
            "CarCompareCar1" => car1,
            "CarCompareCar2" => car2,
            _ => return None,
        };
        Some(car.compare[index] as i32)
    }

    /// The chronicle's picture files for the current model group, sorted, and where the career's
    /// year sits in that list. The name is `M<letter><yy><index><variant>.fsh` — see
    /// [`App::chronicle_picture`] — so sorting by name sorts by year.
    fn chronicle_list(&self) -> (Vec<String>, usize) {
        let letter = CHRONICLE_GROUP_LETTERS.as_bytes()[CHRONICLE_GROUP] as char;
        let dir = self.paths.fe_data().join("Chron");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                let Some(stem) = n.strip_suffix(".fsh") else { return false };
                let b = stem.as_bytes();
                b.len() == 6 && b[0] == b'M' && b[1] as char == letter && stem[2..4].parse::<u32>().is_ok()
            })
            .collect();
        files.sort();
        let cur = self.chronicle_picture().and_then(|f| files.iter().position(|n| *n == f)).unwrap_or(0);
        (files, cur)
    }

    fn chronicle_picture(&self) -> Option<String> {
        let year = self.career.as_ref()?.career.year;
        let letter = CHRONICLE_GROUP_LETTERS.as_bytes()[CHRONICLE_GROUP] as char;
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

    /// The catalogue record an `NFS5.CarLogo`'s source names. The original resolves the source in
    /// the order at 0x4C14E0: `othersource` `FactoryDriver` → the `FactoryDriver.Car` accessor,
    /// `EndOfEra` → `PrizeCars.Car`, `FactoryDriverPrize` → the factory-drive prize car, then the
    /// widget's own `accessor`, then `CarCompare1` → the compare screen's left car. `FactoryDriverPrize` and
    /// `CarCompare1` are code-side objects this port does not keep, so those widgets stay
    /// unrendered; `Chron` is not a car at all and goes to [`App::chronicle_picture`].
    fn logo_car(
        &self,
        accessor: Option<&str>,
        othersource: Option<&str>,
    ) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let name = match othersource {
            Some("FactoryDriver") => "FactoryDriver.Car",
            Some("EndOfEra") => "PrizeCars.Car",
            // `0x51B050` reads the current factory record's `+0x10`, the prize car of the stage the
            // player has just finished, and 0x4F4360 looks it up in the master list
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
            _ => accessor?,
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
            // the same list the port's other car accessors answer from
            "zMergedList.Car" | "OpponentCar.Car" | "UsedCar.Car" | "PrizeCars.Car"
            | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                let m = self.menu.as_ref()?;
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
                let l = self.lobby.as_ref()?;
                Some((
                    l.games().into_iter().map(ListRow::Text).collect(),
                    Some(l.sel),
                ))
            }
            "Comms.IPList" => {
                let l = self.lobby.as_ref()?;
                Some((
                    l.addresses().into_iter().map(ListRow::Text).collect(),
                    Some(l.sel),
                ))
            }
            "Comms.LobbyPlayerList" | "Comms.PlayerList" => {
                let rows = match self.net.as_ref() {
                    Some(n) => n.players(),
                    None => self.lobby.as_ref()?.players(),
                };
                Some((rows.into_iter().map(ListRow::Text).collect(), None))
            }
            // no modem in this port
            "Comms.ModemList" | "Comms.PhoneList" => Some((Vec::new(), None)),
            "Track1.Track" | "LoopedTrack.Track" | "Knockout.Track" | "Track.Track" => {
                let m = self.menu.as_ref()?;
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
                    .menu
                    .as_ref()
                    .map(|m| m.tracks[m.track_idx].clone())
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
            // the CD player (`cdplayer.lay`): the songs in `zzzymus.viv` that are not in the
            // playlist, and the playlist itself — the two lists the original moves entries between
            "CDPlayer.AvailableList" | "CDPlayer.SelectedList" => {
                // in a race the live player owns both lists; in the front end there is no player
                // yet, so they come from the files and the saved playlist
                let names: &[String] = match self.music.as_ref() {
                    Some(m) => &m.names,
                    None => &self.music_names,
                };
                let list: Vec<usize> = match self.music.as_ref() {
                    Some(m) => m.playlist().to_vec(),
                    None => self.audio_settings.playlist.iter().map(|i| *i as usize).collect(),
                };
                let rows: Vec<ListRow> = match base {
                    "CDPlayer.SelectedList" => list
                        .iter()
                        .filter_map(|i| names.get(*i))
                        .map(|n| ListRow::Text(nfs_audio::music::split_title(n).0.to_uppercase()))
                        .collect(),
                    _ => names
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !list.contains(i))
                        .map(|(_, n)| {
                            ListRow::Text(nfs_audio::music::split_title(n).0.to_uppercase())
                        })
                        .collect(),
                };
                Some((rows, None))
            }
            "UserList.UserList" => {
                let dir = nfs_game::save::save_dir();
                let profiles = nfs_game::save::list_profiles(&dir);
                let current = self
                    .career
                    .as_ref()
                    .and_then(|c| profiles.iter().position(|p| *p == c.profile));
                Some((
                    profiles
                        .into_iter()
                        .map(|p| ListRow::Text(p.name))
                        .collect(),
                    current,
                ))
            }
            "Replay.List" => {
                // `RebuildReplayList` reads `%s*.rpl` from the replay directory (0x4E5E1B)
                let dir = self.replay_path.parent()?.to_path_buf();
                let mut names: Vec<String> = std::fs::read_dir(&dir)
                    .ok()?
                    .flatten()
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|x| x.eq_ignore_ascii_case("rpl"))
                    })
                    .filter_map(|e| {
                        e.path()
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                    })
                    .collect();
                names.sort();
                Some((names.into_iter().map(ListRow::Text).collect(), None))
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
                let mut rows = Vec::new();
                let mut group = None;
                for p in parts {
                    if group != Some(p.kind) {
                        group = Some(p.kind);
                        let heading = nfs_formats::career::part_category_string(p.kind)
                            .and_then(|id| self.fe_strings.get(id))
                            .unwrap_or("")
                            .to_string();
                        rows.push(ListRow::Header(heading));
                    }
                    let name = self
                        .fe_strings
                        .get(p.name_string_id)
                        .unwrap_or("")
                        .to_string();
                    let right = if damage_list {
                        ListCell {
                            anchor: "ID_DAMAGEPOS".into(),
                            dx: 3.0,
                            text: format!("{} %", c.career.part_damage(idx, p) as i64),
                        }
                    } else {
                        ListCell {
                            anchor: "ID_PRICEPOS".into(),
                            dx: -4.0,
                            text: format!("{}", p.price),
                        }
                    };
                    rows.push(ListRow::Columns(vec![
                        ListCell {
                            anchor: String::new(),
                            dx: 3.0,
                            text: format!("{name} {}", p.kind_name),
                        },
                        right,
                    ]));
                }
                Some((rows, None))
            }
            _ => None,
        }
    }

    /// Fill every `NFS5.ListBox` on the previewed screen from its accessor.
    fn bind_lists(&mut self) {
        let Some(l) = self.layout_preview.as_ref() else {
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
            .collect();
        let Some(l) = self.layout_preview.as_mut() else {
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
        let Some(l) = self.layout_preview.as_ref() else {
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
        let chron: Vec<usize> = l
            .car_logos()
            .into_iter()
            .filter(|w| w.othersource.as_deref() == Some("Chron"))
            .map(|w| w.index)
            .collect();
        let picture = (!chron.is_empty())
            .then(|| self.chronicle_picture())
            .flatten();
        let (Some(l), Some(r)) = (self.layout_preview.as_mut(), self.renderer.as_mut()) else {
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
    }

    /// `--layout-sweep`: load every shipped `.lay`, draw it once with the port's real accessors and
    /// print every widget class that produced no sprite at all, with the source it names. A widget
    /// lands here when the original places its content from code and the file carries neither a
    /// shape nor a string for it — which is exactly the list of front-end things still to port.
    fn run_layout_sweep(&mut self) {
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
        let mut screens = 0usize;
        for name in &names {
            self.open_layout(name);
            if self.layout_preview.is_none() {
                continue;
            }
            screens += 1;
            self.bind_car_logos();
            self.bind_lists();
            self.bind_indicator_bars();
            self.bind_chronicle_icons();
            self.bind_showcase_pictures();
            // `NFS_SWEEP_DUMP=<layout>` prints what every accessor of that one screen resolves to
            if std::env::var("NFS_SWEEP_DUMP").as_deref() == Ok(name.as_str())
                && let Some(l) = self.layout_preview.as_ref()
            {
                for e in &l.layout.elements {
                    if let Some(a) = e.get("accessor").filter(|a| !a.is_empty()) {
                        println!("  {:<26} {a:<28} -> {:?}", e.class, self.accessor_value(a));
                    }
                }
            }
            let Some(l) = self.layout_preview.as_ref() else { continue };
            for (class, src) in l.blank_widgets(nfs_game::layout::DESIGN.0 / nfs_game::layout::DESIGN.1, &self.fe_strings, &|a| self.accessor_value(a)) {
                *total.entry((class, src)).or_default() += 1;
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
        println!("\nby source:");
        let mut srcs: Vec<(usize, String, String)> = total.iter().map(|((c, s), n)| (*n, c.clone(), s.clone())).collect();
        srcs.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        for (n, c, s) in &srcs {
            println!("  {n:4}  {c:<30} {s}");
        }
    }

    /// Fill the chronicle's thumbnail column. The four `NFS5.ChroniclePictureIcon`s carry
    /// `number` 0…3 and sit bottom to top (`number=0` at y 363, `number=3` at y 177), so the
    /// column runs upwards from the entry the screen is on. Each pack's entry 0 is its `bi` icon
    /// and entry 1 its `ci`; the port draws `ci` for the entry the screen is showing and `bi` for
    /// the rest — **APPROXIMATION**, the two names' meaning was not traced, only their size, which
    /// is the widget's own `size=41,25`.
    fn bind_chronicle_icons(&mut self) {
        let Some(l) = self.layout_preview.as_ref() else { return };
        let icons = l.numbered_widgets("NFS5.ChroniclePictureIcon");
        if icons.is_empty() {
            return;
        }
        let (files, cur) = self.chronicle_list();
        let wanted: Vec<(usize, String, usize)> = icons
            .into_iter()
            .map(|(i, number)| {
                let n = cur + number.max(0) as usize;
                let entry = usize::from(n != cur);
                (i, files.get(n).cloned().unwrap_or_default(), entry)
            })
            .collect();
        let (Some(l), Some(r)) = (self.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, file, entry) in wanted {
            l.set_pack_picture(r, &self.paths, "Chron", index, &file, entry);
        }
    }

    /// Fill the showcase screens' picture mosaic. `NFS5.ShowcasePicture` is only a place — its
    /// template has no drawable part — and the `number` its `.lay` block carries is the entry of
    /// the selected car's own pack, `FEData/Showcase/<key>.fsh`, where `key` is the catalogue
    /// record's +0xE3 string (86 of the 87 racing records name a pack that is on disk). The match
    /// is exact: `general.lay` and `glancestats.lay` give `number` 0…7, and each widget's `size`
    /// is entry `number`'s own width and height. CONFIRMED.
    fn bind_showcase_pictures(&mut self) {
        let Some(l) = self.layout_preview.as_ref() else { return };
        let want = l.numbered_widgets("NFS5.ShowcasePicture");
        if want.is_empty() {
            return;
        }
        let pack = self
            .logo_car(Some("zMergedList.Car"), None)
            .map(|c| format!("{}.fsh", c.showcase))
            .unwrap_or_default();
        let (Some(l), Some(r)) = (self.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, number) in want {
            l.set_pack_picture(r, &self.paths, "Showcase", index, &pack, number.max(0) as usize);
        }
    }

    /// Fill the `NFS5.GouraudIndicatorBar`s of whatever screen is up. Only the car-compare screen
    /// has any, and its bars read the same `<n>CarCompareCar<m>` sources as its numbers.
    fn bind_indicator_bars(&mut self) {
        let Some(l) = self.layout_preview.as_ref() else { return };
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
                    match self.range_value(name) {
                        Some((v, lo, hi)) if hi > lo => Some((v - lo) * 100 / (hi - lo)),
                        _ => self
                            .accessor_value(&src)
                            .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                            .map(|v| v.round() as i32),
                    }
                };
                (i, v)
            })
            .collect();
        let Some(l) = self.layout_preview.as_mut() else { return };
        for (index, value) in wanted {
            l.set_bar(index, value);
        }
    }

    fn leave_race(&mut self) -> bool {
        let Some(menu) = self.menu_template.take() else {
            return false;
        };
        self.save_replay();
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
        self.menu = Some(menu);
        self.vehicle = None;
        self.race = None;
        self.world = None;
        self.car_model = None;
        self.sky = None;
        self.scene = Scene::default();
        true
    }
    /// Drain the network and put every remote player on its seat. Returns the seats the network
    /// owns this tick, so the AI/physics below leaves them alone — a remote car is driven by what
    /// arrived, exactly as the original's receiver drives it (`network.md` §5.2).
    fn net_step(&mut self) -> Vec<usize> {
        if self.net.is_none() {
            return Vec::new();
        }
        let poses: Vec<(Vec3, Quat)> = self
            .opponents
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
        let mut net = self.net.take();
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
                if let Some(o) = self.opponents.get_mut(s) {
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
        self.net = net;
        seats
    }

    fn simulate(&mut self) {
        self.tick += 1;
        self.render_prev = Some((self.car_pos, self.car_rot));
        self.opp_prev = self
            .opponents
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
        let Some(v) = self.vehicle.as_mut() else {
            return;
        };
        let Some(w) = self.world.as_ref() else { return };
        if self.tick % 2 == 0 {
            // 32 Hz: latch controller, run the force step (Sc32)
            let mut steer = (self.input.steer() * 126.0) as i32; // right = +x in the body frame
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
            // countdown speech (Phrase_kCountdown: SOR-a-3/2/1/Go at ticks 256/320/384/448)
            let phase = COUNTDOWN_TICKS
                .iter()
                .chain(std::iter::once(&RACE_START_TICK))
                .filter(|&&t| self.tick >= t)
                .count() as u8;
            if phase > self.countdown_said
                && let (Some(a), Some(sp)) = (&self.audio, &self.speech)
            {
                self.countdown_said = phase;
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
                gear_request: None,
            };
            // replay.md §4: the hook sits inside the 32 Hz input step, before the car reads the
            // packet, so recording and playback are in phase with the simulation by construction
            let controls = match self.replay.as_mut() {
                // `.RAMPS` is set because this port always ramps the steering (0x49DFAD's digital
                // path); the gas and brake axes are not ramped.
                Some(r) => r.step(
                    controls,
                    nfs_game::replay::Ramp {
                        steer: true,
                        gas: false,
                        brake: false,
                    },
                    self.tick,
                ),
                None => controls,
            };
            // `DraftFactor` 0x49D9B0: the racer array only — traffic and police do not draft
            let racers: Vec<Vec3> = self.opponents.iter().map(|o| o.vehicle.position).collect();
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
            let drivable = |p: Vec3| {
                w.collision
                    .ground(p.x, p.y + 2.0, p.z, 6.0)
                    .map(|h| h.surface & 0xF != 0)
                    .unwrap_or(false)
            };
            let landing = v.impact; // set by a hard landing during step32 (closing speed)
            v.impact = 0.0;
            if v.collide_walls(&w.walls, &drivable, 0.0, 0.0, 0.0) {
                self.wall_hits += 1;
                if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                    let j = v.impact;
                    if j >= v.speed() / 8.0 {
                        s.wall_hit(a, j, v.last_wall_class, 0.8)
                    } else {
                        s.wall_scrape(a, v.speed(), v.last_wall_class, 0.6)
                    }
                }
                v.impact = 0.0;
            } else if landing > 6.0
                && let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut())
            {
                s.landing(a, landing, 0.8);
            }
            // reset to the current slice when the car has left the world or rolled over for > 2 s
            // (original: reset mode 3, 0x40F540, also bound to a key) — APPROXIMATION of the placement rule
            let fallen = self
                .slices
                .as_ref()
                .zip(self.race.as_ref())
                .map(|(t, r)| v.position.y < t.slices[r.slice].centre.y - 15.0)
                .unwrap_or(false);
            let rolled = v.cos_tilt < 0.3 || v.orientation.row(1).y < 0.3;
            self.rolled_ticks = if rolled { self.rolled_ticks + 1 } else { 0 };
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
            // 0x40F7EB: a reset request (the AI's `car+0xD8B = 5`, or the player's key) only goes
            // through to `0x40F540(car, 2, 0)` when the race has run more than 0x300 ticks and the
            // car is doing **27.6 m/s or less**; the skidpad is exempt from resets altogether.
            let asked = self.input.take_pressed(Action::Reset);
            let allowed = self.tick > 0x300
                && v.speed() <= 27.6
                && !self.track.name.eq_ignore_ascii_case("skidpad");
            if (fallen || bot_stuck || self.rolled_ticks > 64 || (asked && allowed))
                && let (Some(t), Some(r)) = (&self.slices, &self.race)
            {
                self.rolled_ticks = 0;
                self.resets += 1;
                let sl = &t.slices[r.slice];
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
                tracing::info!(slice = r.slice, "car reset to track");
            }
            // crash / roll-over one-shot (Damage 0x494500 zone rule; roll-over 0x49BC80 at level 127)
            let rolled = v.orientation.row(1).y < -0.86;
            if (v.crash_sound || rolled)
                && let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut())
            {
                if !self.crash_played {
                    s.crash(a, 0.9);
                }
                self.crash_played = rolled;
            } else {
                self.crash_played = false;
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
                let about_to_reset = v.orientation.row(1).y < 0.0;
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
            }
            // both trigger sweeps above compare the position with the previous tick's
            self.prev_car_pos = v.position;
            // traffic AI (priority 6 on the 32 Hz list, before ground contact and the collision pass)
            if let (Some(tr), Some(t)) = (self.traffic.as_mut(), self.slices.as_ref()) {
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
                others.extend(self.opponents.iter().map(|o| {
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
                tr.update(t, self.tick, reference, &others);
                tr.update(t, self.tick, reference, &others);
            }
            // police (`traffic.md` §17.3 violation, §25 spawn, §27 chase)
            if let (Some(pol), Some(t)) = (self.police.as_mut(), self.slices.as_ref()) {
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
                    }];
                    pol.update(t, self.tick, &target, &others);
                }
            }
            // car–car contact (approximation, see nfs_physics::vehicle::collide_cars)
            let no_carcol = std::env::var_os("NFS_NO_CARCOL").is_some();
            for o in self.opponents.iter_mut() {
                if !no_carcol
                    && let Some(effect) = nfs_physics::vehicle::collide_cars(
                        v,
                        self.specs.mass,
                        &mut o.vehicle,
                        o.specs.mass,
                    )
                    && let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut())
                {
                    s.car_car(a, effect, 0.8);
                }
            }
            // traffic vs everyone (`traffic.md` §21.5): the broad phase walks every car pair, so a
            // traffic car is a full rigid body for the solver. The impulse it takes lives until the
            // next AI tick overwrites its velocity, exactly as in the original.
            if !no_carcol && let Some(tr) = self.traffic.as_mut() {
                for (i, c) in tr.cars.iter_mut().enumerate().filter(|(_, c)| c.active) {
                    let Some(b) = self.traffic_bodies.get_mut(i) else {
                        continue;
                    };
                    b.position = c.position;
                    b.orientation = c.orientation;
                    b.velocity = c.velocity;
                    b.angular_velocity = Vec3::ZERO;
                    b.half_extents = c.half_extents;
                    if let Some(effect) =
                        nfs_physics::vehicle::collide_cars(v, self.specs.mass, b, c.mass)
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
                        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                            s.car_car(a, effect, 0.8);
                        }
                    }
                    for o in self.opponents.iter_mut() {
                        if let Some(effect) = nfs_physics::vehicle::collide_cars(
                            &mut o.vehicle,
                            o.specs.mass,
                            b,
                            c.mass,
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
            if !no_carcol && let Some(pol) = self.police.as_mut() {
                for (i, c) in pol.cars.iter_mut().enumerate().filter(|(_, c)| c.active) {
                    let Some(b) = self.police_bodies.get_mut(i) else {
                        continue;
                    };
                    b.position = c.position;
                    b.orientation = c.orientation;
                    b.velocity = c.velocity;
                    b.angular_velocity = c.angular_velocity;
                    b.half_extents = c.half_extents;
                    if let Some(effect) =
                        nfs_physics::vehicle::collide_cars(v, self.specs.mass, b, c.mass)
                    {
                        tracing::debug!(cop = i, effect, "player hit a cop");
                        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
                            s.car_car(a, effect, 0.8);
                        }
                    }
                    for o in self.opponents.iter_mut() {
                        nfs_physics::vehicle::collide_cars(&mut o.vehicle, o.specs.mass, b, c.mass);
                    }
                    c.position = b.position;
                    c.velocity = b.velocity;
                    c.angular_velocity = b.angular_velocity;
                }
            }
            for i in 0..self.opponents.len() {
                if no_carcol {
                    break;
                }
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
                let mut order: Vec<(f32, Vec3, usize, f32, f32, bool)> = self
                    .opponents
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
                if let Some(tr) = &self.traffic {
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
                    .chain(self.opponents.iter().map(|o| o.vehicle.position))
                    .collect();
                for (oi, o) in self.opponents.iter_mut().enumerate() {
                    o.vehicle.race_started = self.tick >= RACE_START_TICK;
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
                    if o.vehicle.collide_walls(&w.walls, &drivable, 0.0, 0.0, 0.0) {
                        o.wall_hits += 1;
                    }
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
                    // stuck / fallen: reset onto the slice (AI reset request car+0xD8B = 5)
                    let fallen = o.vehicle.position.y < t.slices[o.progress.slice].centre.y - 15.0;
                    let stuck = match &mut o.controller {
                        Controller::Ai(ai) => ai.wants_reset(),
                        _ => false,
                    };
                    if fallen || stuck || o.vehicle.cos_tilt < 0.3 {
                        let sl = &t.slices[o.progress.slice];
                        let fwd = sl.forward.normalize();
                        let up = sl.up.normalize();
                        let right = up.cross(fwd).normalize();
                        o.vehicle.reset_to(
                            sl.centre + up * 0.8,
                            glam::Mat3::from_cols(right, fwd.cross(right).normalize(), fwd)
                                .transpose(),
                        );
                        o.resets += 1;
                    }
                }
            }
            // Quick Knockout cut for opponents that completed a lap this tick (modes.md §4)
            if self.quick_knockout && !ko_laps.is_empty() {
                let mut all: Vec<(usize, f32)> = self
                    .opponents
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
                                    if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
                                        sp.say(a, "KO-eliminated.bnk", 0.9);
                                    }
                                }
                            } else if let Some(o) = self.opponents.get_mut(d - 1)
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
                let n_cars = self.opponents.len() + 1;
                let place = 1 + self
                    .opponents
                    .iter()
                    .filter(|o| o.progress.progress > r.progress)
                    .count();
                if lap_done {
                    tracing::info!(
                        lap = r.laps,
                        time_s = r.lap_times.last().map(|t| *t as f32 / 64.0),
                        "lap completed"
                    );
                    if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
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
                                .opponents
                                .iter()
                                .map(|o| o.progress.progress)
                                .fold(r.progress, f32::max);
                            let speed = v.speed();
                            if n_cars == 2 {
                                // kPositionRel: seconds ahead (+) / behind (−) of the rival
                                if let Some(o) = self.opponents.first()
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
                        && let (Some(a), Some(m)) = (&self.audio, self.music.as_mut())
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
                            && let (Some(a), Some(sp)) = (&self.audio, &self.speech)
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
                            .opponents
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
                        if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
                            sp.say(a, "KO-eliminated.bnk", 0.9);
                        }
                        tracing::info!(lap, place, "player knocked out");
                    }
                }
            }
            if self.input.take_pressed(Action::CameraNext) {
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
        if let (Some(pol), Some(t)) = (self.police.as_mut(), self.slices.as_ref()) {
            self.police_prev = pol
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
        if let (Some(tr), Some(t)) = (self.traffic.as_mut(), self.slices.as_ref()) {
            self.traffic_prev = tr
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
            self.traffic_spin.resize(tr.cars.len(), 0.0);
            for (sp, c) in self.traffic_spin.iter_mut().zip(tr.cars.iter()) {
                *sp = if c.active {
                    *sp + c.speed / 64.0 / 0.32
                } else {
                    0.0
                };
            }
        }
        v.step64(&self.specs);
        for o in self.opponents.iter_mut() {
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
        if let (Some(a), Some(sp)) = (&self.audio, &self.speech) {
            sp.update(a);
        }
        let mirror = self.mirror;
        // AI engine loops (`engine-audio.md` §5): two crossfaded samples plus a road loop per opponent.
        // Traffic cars are left silent: the original drives their loops from `car+0xDB4`, and the port's
        // kinematic traffic has no engine speed to drive them with.
        if let (Some(a), Some(ai)) = (&self.audio, self.ai_engines.as_mut()) {
            let cam = self.camera.position;
            // `MIRROR` negates the pan angle before the atan2 that produces it (0x4A704D, 0x4A78C1
            // and the other four sites of `modes.md` §2.1), so flip the right vector with the world
            let cam_right = (self.camera.target - cam)
                .normalize_or_zero()
                .cross(Vec3::Y)
                .normalize_or_zero()
                * if mirror { -1.0 } else { 1.0 };
            let level = self.audio_settings.engine_level as f32 / 255.0;
            for (i, o) in self.opponents.iter().enumerate() {
                let set = self.opponent_sets.get(i).cloned().unwrap_or_default();
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
                    self.audio_settings.audio_quality,
                    64,
                    level,
                );
            }
            // traffic uses the same cheap path. Its engine speed is real: `0x408360` computes
            // `car+0xDB4 = carSpecs[0x60 + 4·gear] · speed` at 0x4084AD with the gear from
            // `Traffic_pickGear` 0x408130 — 1 below 2 m/s, otherwise 0 while the car has not been hit
            // (`car+0x104C == −1`), and the highest gear whose top speed is under the current speed
            // while it is recovering from one.
            if let (Some(tr), Some(specs)) = (&self.traffic, &self.traffic_specs) {
                let base = self.opponents.len();
                for (i, c) in tr.cars.iter().enumerate() {
                    let set = self.traffic_sets.get(i).cloned().unwrap_or_default();
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
                        self.audio_settings.audio_quality,
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
        if let (Some(a), Some(amb)) = (&self.audio, self.ambience.as_mut()) {
            let cam = self.camera.position;
            let fwd = (self.camera.target - cam).normalize_or_zero();
            let right = fwd.cross(Vec3::Y).normalize_or_zero();
            amb.update(a, self.tick, cam.into(), right.into(), fwd.into());
        }
        // end-of-stream advance through the playlist (0x4ADD6D)
        if let (Some(a), Some(m)) = (&self.audio, self.music.as_mut()) {
            m.update(a);
        }
        // tunnel reverb (`engine-audio.md` §4.7): the enclosure test every audio update, then the
        // level that snaps to 100 under a road deck and decays 100 → 68 → 36 → 8 on the way out
        if let Some(a) = &self.audio {
            let in_tunnel = w.in_tunnel(v.position);
            if self.reverb.update(in_tunnel, false, false).is_some() {
                a.set_reverb(self.reverb.wet());
            }
        }
        if let (Some(a), Some(e)) = (&self.audio, self.engine_sound.as_mut()) {
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
                    running: true, // the port's engine never stops, so the stop sample never plays
                    // `car+0x548`, the rev limiter's ignition-cut counter (`physics.md` §4.1) — the
                    // engine carries it, so the loud-load branch of `engine-audio.md` §3 gets the real
                    // thing rather than "at or past the redline"
                    rev_cut: v.ignition_cut > 0,
                    mode,
                    cos,
                    pan,
                    distance: dist,
                    doppler: 1.0,
                    detail: self.audio_settings.audio_quality,
                    crowding: 0,
                    engine_level: self.audio_settings.engine_level,
                    shift: (v.shift_timer > 0 && v.gear > self.last_gear)
                        .then_some((v.shift_timer, self.specs.shift_ticks)),
                },
            );
            // the click fires on the last tick of the shift, not at the gear change (§4.3)
            if v.shift_timer == 1 && v.gear > self.last_gear {
                e.play_shift(a, 0.6);
            }
        }
        self.last_gear = v.gear;
        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
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
            let sfx_level = self.audio_settings.sfx_level as f32 / 255.0;
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
            if let Some(e) = &self.engine_sound {
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
        let Some(r) = self.replay.as_ref() else {
            return;
        };
        match r.mode {
            Mode::Recording | Mode::Full => {
                let done = self.race.as_ref().map(|r| r.finished).unwrap_or(false)
                    || self.player_eliminated;
                if done && !self.replay_saved {
                    self.save_replay();
                }
            }
            Mode::Exhausted => {
                self.replay_end_ticks += 1;
                if self.replay_end_ticks >= nfs_game::replay::END_GRACE_TICKS {
                    self.replay_end_ticks = 0;
                    tracing::info!("replay finished");
                    if !self.leave_race() {
                        self.replay = None;
                    }
                }
            }
            Mode::Playing => {}
        }
    }

    /// Write the port's replay file (`replay.md` §6.2 saves `replay.rpl` after every race; §14.5
    /// explains why this is the port's own format and not the original's 118 312-byte dump).
    fn save_replay(&mut self) {
        self.replay_saved = true;
        let Some(r) = self.replay.as_mut() else {
            return;
        };
        if r.written() == 0 {
            return;
        }
        if r.mode == nfs_game::replay::Mode::Recording {
            r.final_tick = self.tick;
        }
        let file = nfs_game::replay::ReplayFile {
            setup: self.replay_setup.clone(),
            final_tick: r.final_tick,
            stream: r.stream().to_vec(),
        };
        let path = self.replay_path.clone();
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
        let l = self.layout_preview.as_ref()?;
        nfs_game::room::room(l.layout.screen().and_then(|s| s.get("room")).unwrap_or(""))
    }

    /// Load the room's models and the car that stands in it, keeping whatever is already loaded.
    fn load_fe_room(&mut self) {
        let Some(room) = self.fe_room_of() else {
            self.fe_room.clear();
            return;
        };
        // the car the screen's own `NFS5.3DCAR` names — with `positiontype` 0 the room owns it
        // the cars the screen's `NFS5.3DCAR` widgets name. `positiontype` 0 hands the car to the
        // room; 1, 2 and 3 put one of their own at a fixed spot (`RE/notes/frontend-3d.md` §4). A
        // source the port cannot resolve falls back to the car the front end has selected, as the
        // accessor answers elsewhere.
        let fallback = self.menu.as_ref().map(|m| m.cars[m.car_idx].model.clone());
        let widgets = self
            .layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        let mut wanted: Vec<(usize, String)> = Vec::new();
        for w in &widgets {
            let model = self
                .logo_car(w.accessor.as_deref(), w.othersource.as_deref())
                .map(|c| c.model.clone())
                .or_else(|| fallback.clone());
            if let Some(model) = model {
                wanted.push((
                    if w.position_type == 0 {
                        usize::MAX
                    } else {
                        w.index
                    },
                    model,
                ));
            }
        }
        // a real room with no `NFS5.3DCAR` on the screen still stands the selected car in it; an
        // empty room (`ID_NONE`) without one shows nothing
        if wanted.is_empty()
            && !room.models.is_empty()
            && let Some(model) = fallback
        {
            wanted.push((usize::MAX, model));
        }
        if self
            .fe_room
            .iter()
            .map(|(n, _)| n.as_str())
            .ne(room.models.iter().copied())
        {
            self.fe_room.clear();
            let dir = self.paths.fe_data().join("Models");
            for name in room.models {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load_in(&dir, name)
                    .map_err(anyhow::Error::from)
                    .and_then(|a| CarModel::upload(r, &a, &self.paths, 1, [0xC0, 0xC0, 0xC0]));
                match loaded {
                    Ok(m) => self.fe_room.push((name.to_string(), m)),
                    Err(e) => {
                        tracing::warn!(room = room.id, model = name, error = %e, "front-end room model unavailable")
                    }
                }
            }
            tracing::info!(
                room = room.id,
                models = self.fe_room.len(),
                "front-end room loaded"
            );
        }
        if self
            .fe_cars
            .iter()
            .map(|(i, n, _)| (*i, n.as_str()))
            .ne(wanted.iter().map(|(i, n)| (*i, n.as_str())))
        {
            self.fe_cars.clear();
            let paint = self.paint;
            for (slot, model) in wanted {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load(&self.paths, &model)
                    .map_err(anyhow::Error::from)
                    .and_then(|a| CarModel::upload(r, &a, &self.paths, 1, paint));
                match loaded {
                    Ok(m) => self.fe_cars.push((slot, model, m)),
                    Err(e) => tracing::warn!(model, error = %e, "front-end car model unavailable"),
                }
            }
        }
    }

    /// The room's meshes and the car turning in it, plus the camera and lighting the room's own rig
    /// gives them (`RE/notes/frontend-3d.md` §2, §3).
    fn fe_room_items(&mut self) {
        let Some(room) = self.fe_room_of() else {
            return;
        };
        // the room slides toward the camera as the screen fades out
        let fade = self
            .layout_preview
            .as_ref()
            .map(|l| l.transition().progress() as f32 / nfs_game::layout::TRANS_FULL as f32)
            .unwrap_or(0.0);
        let p = room.faded_pos(fade);
        let pos = Vec3::new(p[0], p[1], p[2]);
        let turn = |t: f32| {
            let rot = Quat::from_rotation_y((t + room.rot[1]) * std::f32::consts::TAU)
                * Quat::from_rotation_x(room.rot[0] * std::f32::consts::TAU)
                * Quat::from_rotation_z(room.rot[2] * std::f32::consts::TAU);
            Mat4::from_rotation_translation(rot, pos)
        };
        let spin = room.spin_turns(self.fe_ms);
        let mut items = Vec::new();
        let room_m = turn(room.models_turn.unwrap_or(spin));
        for (_, model) in &self.fe_room {
            items.extend(model.draw_items(room_m, [0.0; 4], 0.0));
        }
        let widgets = self
            .layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        // the room's own car goes in with the room; a widget with a `positiontype` of its own is
        // drawn into an offscreen view instead, so it lands in that widget's place in the screen's
        // drawing order rather than behind all of it
        let mut views: Vec<(usize, Vec<nfs_renderer::DrawItem>)> = Vec::new();
        for (slot, _, car) in &self.fe_cars {
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
                    views.push((*slot, car.draw_items(m, [0.0; 4], 0.0)));
                }
                None => items.extend(car.draw_items(turn(spin), [0.0; 4], 0.0)),
            }
        }
        self.scene.items = items;
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
            let reuse = self.layout_preview.as_ref().and_then(|l| l.car_view(slot));
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
        if let Some(l) = self.layout_preview.as_mut() {
            for (slot, tex) in rendered {
                l.set_car_view(slot, Some(tex));
            }
        }
    }

    /// The front end's own sprites: the previewed `.lay` screen over its background, the bottom bar
    /// and the port's key hint, or the text menu when no layout is loaded.
    fn fe_sprites(&self) -> Vec<Sprite> {
        match &self.layout_preview {
            Some(l) => {
                let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                let controls = l.controls();
                let sel = controls
                    .get(self.layout_sel.min(controls.len().saturating_sub(1)))
                    .map(|c| c.index);
                let mut v = l.letterbox_bars(aspect);
                v.extend(l.sprites(aspect, &self.fe_strings, sel, &|a| self.accessor_value(a)));
                // the bottom bar the front end draws on every screen (0x4D39D3)
                if let Some(b) = &self.layout_bottom {
                    v.extend(
                        b.sprites(aspect, &self.fe_strings, None, &|a| self.accessor_value(a)),
                    );
                }
                if let Some(f) = self.menu.as_ref().and_then(|m| m.font.as_ref()) {
                    let name = self.layout_name.clone().unwrap_or_default();
                    let hint = if l.can_play() {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER RACE   ESC BACK"
                    } else {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER FOLLOW   ESC BACK"
                    };
                    v.extend(f.sprites(
                        &format!("{}   {hint}", name.to_uppercase()),
                        [0.02, 0.96],
                        0.025,
                        [0.6, 0.6, 0.6, 0.9],
                    ));
                }
                v
            }
            None => self
                .menu
                .as_ref()
                .map(|m| m.sprites(self.career.as_ref()))
                .unwrap_or_default(),
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
    }

    fn update_env_map(&mut self) {
        let Some(w) = self.world.as_ref() else { return };
        let slice = self.race.as_ref().map(|r| r.slice as u32).unwrap_or(0);
        let covered = w.in_tunnel(self.car_pos);
        let want = w.env_for(slice, covered).or(w.env_map);
        if want == self.env_bound {
            return;
        }
        self.env_bound = want;
        if let (Some(r), Some(t)) = (self.renderer.as_mut(), want) {
            r.set_env_texture(t);
        }
    }

    /// The blob a car drops on the ground (`SHAD`, 0x43B820): a quad the size of the car's
    /// footprint, laid on the surface under it and turned with it. APPROXIMATION of the original's
    /// distance-stepped 1…6 patch grid — the port always draws the single far-away quad.
    fn shadow_item(&self, pos: Vec3, rot: Quat, half: Vec3) -> Option<nfs_renderer::DrawItem> {
        let (mesh, material) = self.shadow_draw?;
        let w = self.world.as_ref()?;
        let h = w.collision.ground(pos.x, pos.y + 2.0, pos.z, 12.0)?;
        let yaw = {
            let f = rot * Vec3::Z;
            f.x.atan2(f.z)
        };
        let m = Mat4::from_translation(Vec3::new(pos.x, h.point.y + 0.04, pos.z))
            * Mat4::from_rotation_y(yaw)
            * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::new(half.x * 2.6, half.z * 2.3, 1.0));
        Some(nfs_renderer::DrawItem {
            light: [0.0; 3],
            alpha: 0.9,
            ..nfs_renderer::DrawItem::new(mesh, material, m)
        })
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

    /// The view the camera is on, by the original's ids (`nfs_game::camera`).
    fn camera_view(&self) -> u16 {
        self.camera_cycle
            .get(self.camera_mode as usize)
            .copied()
            .unwrap_or(0x02)
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
        if let Some(f) = self.menu.as_mut().and_then(|m| m.font.as_mut()) {
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
        if self.vehicle.is_none() && self.menu.is_some() {
            self.scene.clear_color = [0.0, 0.0, 0.0, 1.0];
            self.load_fe_room();
            self.fe_room_items();
            // a screen with a room behind it must not paint its own black fill over it
            let room = !self.fe_room.is_empty() || !self.fe_cars.is_empty();
            if let Some(l) = self.layout_preview.as_mut() {
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
            let position = if self.opponents.is_empty() {
                None
            } else {
                // rank the player among the opponents by progress (ties: earlier progress tick)
                let mut better = 0;
                if let Some(r) = &self.race {
                    for o in &self.opponents {
                        if o.progress.progress > r.progress
                            || (o.progress.progress == r.progress
                                && o.last_progress_tick < self.player_progress_tick)
                        {
                            better += 1;
                        }
                    }
                }
                Some((better, self.opponents.len() + 1))
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
            // the original's blip loop walks the whole racer list (0x433328), so traffic is on the map too
            let tick = self.tick;
            let mut others: Vec<(Vec3, [f32; 4])> = self
                .opponents
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    (
                        o.vehicle.position,
                        Hud::blip_color(
                            self.opponent_paints
                                .get(i)
                                .copied()
                                .unwrap_or([0x20, 0x40, 0xC0]),
                            false,
                            tick,
                        ),
                    )
                })
                .collect();
            others.extend(self.police.iter().flat_map(|p| {
                p.cars.iter().filter(|c| c.active).map(|c| {
                    (
                        c.position,
                        Hud::blip_color([0xE0, 0xE0, 0xE0], c.lights, tick),
                    )
                })
            }));
            others.extend(self.traffic.iter().flat_map(|tr| {
                tr.cars
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.active)
                    .map(|(i, c)| {
                        (
                            c.position,
                            Hud::blip_color(
                                self.traffic_paints
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
                        let o = &self.opponents[d - 1];
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
                h.results_sprites(&mut table, hud_aspect, &rows, player);
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
        let dt = self.frame_dt.clamp(0.001, 0.1);
        let fwd = car_rot * Vec3::Z;
        let rear_view = self.input.held(Action::RearView) && self.vehicle.is_some();
        // the horn is a loop on the car's own engine bank (`engine-audio.md` §6)
        if let (Some(a), Some(e)) = (&self.audio, self.engine_sound.as_mut()) {
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
        }
        if let Some(w) = &self.world {
            self.scene.items.extend(w.visible_items());
            self.scene.ambient = w.ambient;
        }
        if let Some(pol) = &self.police {
            for (i, c) in pol.cars.iter().enumerate().filter(|(_, c)| c.active) {
                let (p1, r1) = (
                    c.position,
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.police_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                let m = Mat4::from_rotation_translation(r, p);
                let spin = c.speed.abs() * self.tick as f32 / 64.0 / 0.32;
                let light = self
                    .world
                    .as_ref()
                    .map(|w| w.light_at(p))
                    .unwrap_or([1.0; 3]);
                self.scene
                    .items
                    .extend(self.police_models[c.model].draw_items_lit(m, [spin; 4], 0.0, light));
            }
        }
        if let Some(tr) = &self.traffic {
            for (i, c) in tr.cars.iter().enumerate().filter(|(_, c)| c.active) {
                let (p1, r1) = (
                    c.position,
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.traffic_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                let m = Mat4::from_rotation_translation(r, p);
                // a kinematic car has no wheel state: the wheels are rolled from the distance driven
                let spin = self.traffic_spin.get(i).copied().unwrap_or(0.0);
                let light = self
                    .world
                    .as_ref()
                    .map(|w| w.light_at(p))
                    .unwrap_or([1.0; 3]);
                self.scene
                    .items
                    .extend(self.traffic_models[c.model].draw_items_lit(m, [spin; 4], 0.0, light));
            }
        }
        for (i, o) in self.opponents.iter().enumerate() {
            let (p1, r1) = (
                o.vehicle.position,
                Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
            );
            let (p, r) = match self.opp_prev.get(i) {
                Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                    (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                }
                _ => (p1, r1),
            };
            let m = Mat4::from_rotation_translation(r, p);
            let w = &o.vehicle.wheels;
            let light = self
                .world
                .as_ref()
                .map(|wl| wl.light_at(p))
                .unwrap_or([1.0; 3]);
            self.scene
                .items
                .extend(self.opponent_models[o.model].draw_items_lit(
                    m,
                    [w[0].rotation, w[1].rotation, w[2].rotation, w[3].rotation],
                    w[0].steer / 256.0,
                    light,
                ));
        }
        // every car drops a blob shadow on the ground under it (0x43B820)
        if self.shadow_draw.is_some() {
            let half = self
                .vehicle
                .as_ref()
                .map(|v| v.half_extents)
                .unwrap_or(Vec3::new(0.9, 0.6, 2.1));
            let mut shadows: Vec<nfs_renderer::DrawItem> = Vec::new();
            if self.vehicle.is_some() {
                shadows.extend(self.shadow_item(car_pos, car_rot, half));
            }
            for o in &self.opponents {
                let r = Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize();
                shadows.extend(self.shadow_item(o.vehicle.position, r, o.vehicle.half_extents));
            }
            for c in self
                .traffic
                .iter()
                .flat_map(|t| t.cars.iter())
                .filter(|c| c.active)
            {
                let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                shadows.extend(self.shadow_item(c.position, r, Vec3::new(0.9, 0.6, 2.1)));
            }
            for c in self
                .police
                .iter()
                .flat_map(|p| p.cars.iter())
                .filter(|c| c.active)
            {
                let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                shadows.extend(self.shadow_item(c.position, r, Vec3::new(0.9, 0.6, 2.1)));
            }
            self.scene.items.extend(shadows);
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
                    v.wheels[0].steer / 256.0,
                ),
                None => ([0.0; 4], 0.0),
            };
            // the car picks up the light of the ground it stands on (`RE/notes/lighting.md`)
            let light = self
                .world
                .as_ref()
                .map(|w| w.light_at(car_pos))
                .unwrap_or([1.0; 3]);
            self.scene
                .items
                .extend(c.draw_items_lit(m, spin, steer, light));
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
                for o in &self.opponents {
                    let r = Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.opponent_models[o.model],
                        o.vehicle.position,
                        r,
                        right,
                        up,
                    );
                }
                for c in self
                    .traffic
                    .iter()
                    .flat_map(|t| t.cars.iter())
                    .filter(|c| c.active)
                {
                    let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.traffic_models[c.model],
                        c.position,
                        r,
                        right,
                        up,
                    );
                }
                for c in self
                    .police
                    .iter()
                    .flat_map(|p| p.cars.iter())
                    .filter(|c| c.active)
                {
                    let r = Quat::from_mat3(&c.orientation.transpose()).normalize();
                    self.glare_items(
                        &mut glares,
                        &self.police_models[c.model],
                        c.position,
                        r,
                        right,
                        up,
                    );
                }
                self.scene.items.extend(glares);
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
        if self.layout_preview.is_some() {
            self.layout_key(key, event_loop);
            return;
        }
        let Some(m) = self.menu.as_mut() else { return };
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
                            .any(|c| c.eq_ignore_ascii_case(&m.tracks[m.track_idx]))
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
                                        .any(|c| c.eq_ignore_ascii_case(&m.tracks[m.track_idx]))
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
                        4 => {
                            m.opponents = (m.opponents as i32 + delta)
                                .clamp(if m.mode >= 4 { 1 } else { 0 }, 7)
                                as u32
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
                self.menu_template = Some(template);
                self.num_opponents = req.opponents.len() as u32;
                self.num_laps = req.laps;
                self.paint = req.paint;
                self.career_opponents = Some(req.opponents.clone());
                self.traffic_count = req.traffic;
                self.race_skill = req.skill;
                self.backwards = req.backwards;
                self.mirror = false;
                self.quick_knockout = false;
                let car = req.car;
                self.mission = req.mission.clone();
                self.skill = None;
                match (
                    TrackAssets::load(&self.paths, &req.track_file),
                    CarAssets::load(&self.paths, &car.model),
                    CarAssets::load_sim(&self.paths, &car.sim),
                ) {
                    (Ok(mut t), Ok(c), Ok(mut sim)) => {
                        nfs_game::career::apply_parts(
                            &mut sim,
                            &req.parts.iter().collect::<Vec<_>>(),
                        );
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
                            }
                        }
                        self.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
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
                        if let Some(mut mt) = self.menu_template.take() {
                            mt.font = mt.font.take();
                            self.menu = Some(mt);
                        }
                    }
                }
            }
            MenuKey::Enter if m.mode == 5 => {
                let car = m.cars[m.car_idx].clone();
                // new series unless one is active
                if !self.knockout.as_ref().map(|k| k.active).unwrap_or(false) {
                    let n = m.opponents.clamp(1, 7);
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
                            m.tracks.get(idx).cloned().unwrap_or_else(|| m.tracks[m.track_idx].clone())
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
                self.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                self.cops_enabled = m.cops;
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
                self.menu_template = Some(template);
                tracing::info!(
                    track = track_name,
                    race = k.race_idx + 1,
                    of = k.n,
                    "knockout race"
                );
                match (
                    TrackAssets::load(&self.paths, &track_name),
                    CarAssets::load(&self.paths, &car.model),
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
                        self.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
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
                let (track_name, car) = (m.tracks[m.track_idx].clone(), m.cars[m.car_idx].clone());
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
                self.num_opponents = m.opponents;
                self.opp_choice = m.opp_car as u32;
                self.player_car_id = Some(car.id);
                self.quick_knockout = m.mode == 4;
                self.num_laps = if m.mode == 4 {
                    m.opponents.max(1)
                } else {
                    m.laps
                };
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                self.career_opponents = None;
                self.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                self.cops_enabled = m.cops;
                self.paint = car
                    .paint_sets
                    .get(m.paint_idx % car.paint_sets.len().max(1))
                    .map(|p| p.colours[0])
                    .unwrap_or([0xD4, 0x10, 0x10]);
                self.menu_template = Some(template);
                tracing::info!(
                    track = track_name,
                    car = car.model,
                    sim = car.sim,
                    "menu: start race"
                );
                match (
                    TrackAssets::load(&self.paths, &track_name),
                    CarAssets::load(&self.paths, &car.model),
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
                        self.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_tach));
                        self.menu = None;
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
        let Some(l) = self.layout_preview.as_ref() else {
            return;
        };
        let controls = l.controls();
        if controls.is_empty() {
            if key == MenuKey::Back {
                match self.layout_stack.pop() {
                    Some(prev) => self.open_layout(&prev),
                    None => event_loop.exit(),
                }
            }
            return;
        }
        let sel = self.layout_sel.min(controls.len() - 1);
        match key {
            MenuKey::Up => self.layout_sel = (sel + controls.len() - 1) % controls.len(),
            MenuKey::Down => self.layout_sel = (sel + 1) % controls.len(),
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
            self.bind_showcase_pictures();
                    self.bind_lists();
                }
            }
            MenuKey::Enter => {
                let c = controls[sel].clone();
                // the network screens' own callbacks. What the original's handlers behind
                // `ID_COMMSSERVERSCREEN` and friends do has not been read; these do what the
                // protocol of `network.md` §7 allows and nothing more.
                if self.lobby.is_some()
                    && let Some(cb) = c.callback.as_deref()
                    && matches!(
                        cb,
                        "CREATE" | "JOIN" | "CONNECT" | "DIAL" | "ENTER" | "EJECTPLAYER" | "READY"
                    )
                {
                    self.network_callback(cb, event_loop);
                    return;
                }
                // the bottom bar's PLAYGAME, or any screen that says it can start a game
                if c.callback.as_deref() == Some("PLAYGAME")
                    || (c.destination.is_none() && l.can_play())
                {
                    self.start_from_layout(event_loop);
                    return;
                }
                let Some(dest) = c.destination else { return };
                if let Some(cur) = self.layout_name.clone() {
                    self.layout_stack.push(cur);
                }
                self.open_layout(&dest);
            }
            MenuKey::Back => match self.layout_stack.pop() {
                Some(prev) => self.open_layout(&prev),
                None => event_loop.exit(),
            },
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
            "CREATE" => self.lobby.as_ref().map(|l| l.create()),
            "JOIN" | "CONNECT" => self.lobby.as_ref().map(|l| l.connect()),
            "DIAL" => {
                tracing::info!("the modem screen has nothing behind it in this port");
                return;
            }
            // `EJECTPLAYER` is message 0xC6, which only a host can mean — and only once a session
            // is actually running, since the lobby's socket has no peers of its own
            "EJECTPLAYER" => {
                let slot = self.lobby.as_ref().map(|l| l.sel).unwrap_or(0) as u8;
                match self.net.as_mut() {
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
                if let Some(l) = self.lobby.as_mut() {
                    l.ip = l.target().map(|a| a.to_string()).unwrap_or_default();
                }
                self.bind_lists();
                return;
            }
        };
        match opened {
            Some(Ok(mut n)) => {
                let host = n.is_host();
                tracing::info!(cb, host, "network session open");
                n.positional = self.lobby.as_ref().is_none_or(|l| l.positional);
                self.net = Some(n);
                // one seat per player the session can hold, minus ourselves
                if self.num_opponents == 0 {
                    self.num_opponents = 1;
                }
                // 0x50A0DB / 0x50AB31: the lobby screen, not the race
                if let Some(cur) = self.layout_name.clone() {
                    self.layout_stack.push(cur);
                }
                self.open_layout(if host { "creategame" } else { "joingame" });
            }
            Some(Err(e)) => tracing::warn!(cb, error = %e, "cannot open the network session"),
            None => {}
        }
    }

    /// Advance the screens, rebuild the scene and present it. **Both** redraw arms end here: the
    /// menu arm used to handle its keys and then ask for another redraw without ever drawing, which
    /// left the window blank for the whole front end.
    fn draw_frame(&mut self) {
        // the front end's screens slide and fade in over half a second (`RE/notes/controls.md` §6)
        let dt_ms = (self.frame_dt * 1000.0).round().clamp(0.0, 250.0) as u32;
        self.fe_ms += dt_ms as u64;
        if let Some(l) = self.layout_preview.as_mut() {
            l.advance_transition(dt_ms);
        }
        // a network screen probes once a second and refills its lists from the answers
        if self.lobby.is_some() {
            let before = self.lobby.as_ref().map(|l| l.found.len());
            if let Some(l) = self.lobby.as_mut() {
                l.poll();
            }
            if before != self.lobby.as_ref().map(|l| l.found.len()) {
                self.bind_lists();
            }
        }
        if let Some(b) = self.layout_bottom.as_mut() {
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
    fn start_from_layout(&mut self, event_loop: &ActiveEventLoop) {
        let Some(m) = self.menu.as_ref() else { return };
        let (track_name, car) = (m.tracks[m.track_idx].clone(), m.cars[m.car_idx].clone());
        self.num_opponents = m.opponents;
        self.num_laps = m.laps;
        self.backwards = m.backwards;
        self.mirror = m.mirror;
        self.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
        self.cops_enabled = m.cops;
        self.paint = car
            .paint_sets
            .get(m.paint_idx % car.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        tracing::info!(track = track_name, car = car.model, "front end: start race");
        match (
            TrackAssets::load(&self.paths, &track_name),
            CarAssets::load(&self.paths, &car.model),
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
                self.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_tach));
                self.layout_preview = None;
                self.layout_name = None;
                self.menu = None;
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
    fn set_accessor(&mut self, name: &str, delta: i32) {
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        if let Some((v, lo, hi)) = self.range_value(base).filter(|_| !base.starts_with("Audio.")) {
            let key = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == base).map(|(_, k, ..)| *k).unwrap_or_default();
            // the step the sliders move by is the port's own; the registration's own step field was
            // not traced. APPROXIMATION.
            let step = ((hi - lo) / 20).max(1);
            self.choices.insert(key.to_string(), (v + delta * step).clamp(lo, hi) as usize);
            return;
        }
        if let Some((cur, options)) = self.choice_index(base) {
            let key = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == base).map(|(_, k, _)| *k).unwrap_or_default();
            let next = (cur as i32 + delta).rem_euclid(options.len() as i32) as usize;
            self.choices.insert(key.to_string(), next);
            return;
        }
        // the network screens' own sliders and lists
        if base == "Comms.Positional"
            && let Some(n) = self.net.as_mut()
        {
            n.positional = !n.positional;
            return;
        }
        if let Some(l) = self.lobby.as_mut() {
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
        let Some(m) = self.menu.as_mut() else { return };
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
            "zMergedList.Car" | "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car"
            | "CARSETTINGS.CAR" => {
                m.car_idx = step(m.car_idx, m.cars.len());
                m.paint_idx = 0;
            }
            // the compare screen's own combo box moves only the right-hand car
            "GenericCar2.Car" => m.car2_idx = step(m.car2_idx, m.cars.len()),
            "Game.NumOpponents" | "Game.NumKOOpponents" => {
                m.opponents = (m.opponents as i32 + delta).clamp(0, 7) as u32;
                m.fit_ko();
            }
            "Track1.Laps" => m.laps = (m.laps as i32 + delta).clamp(1, 10) as u32,
            "Track1.Traffic" => {
                m.traffic = (m.traffic as i32 + delta).clamp(0, 3) as u32
            }
            "Track1.Direction" => m.backwards = !m.backwards,
            "OwnedCar.ExtColour" | "GenericCar.Colour" | "DealerCar.Colour" => {
                let n = m.cars[m.car_idx].paint_sets.len().max(1);
                m.paint_idx = step(m.paint_idx, n);
            }
            _ => {
                // the audio levels are 0..127 and the mixers read them every tick, so a slider
                // changes the mix straight away
                let a = &mut self.audio_settings;
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
            Ok(l) => {
                if self.layout_bottom.is_none()
                    && let Some(r) = self.renderer.as_mut()
                {
                    self.layout_bottom =
                        nfs_game::layout::LayoutScreen::load(r, &self.paths, "bottombar")
                            .map_err(|e| tracing::debug!(error = %e, "no bottom bar"))
                            .ok();
                    if let Some(b) = self.layout_bottom.as_mut() {
                        b.set_overlay(true);
                    }
                }
                self.layout_preview = Some(l);
                self.layout_name = Some(name.to_string());
                self.layout_sel = 0;
                // the four screens whose `callbacklist=` is a comms one need a browsing socket
                // a browsing socket is only wanted while looking for a game; once a session is
                // open the lobby screens read from it instead
                self.lobby = match is_network_layout(name) && self.net.is_none() {
                    true => netplay::Lobby::open(&self.player_name)
                        .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                        .ok(),
                    false => None,
                };
                self.bind_car_logos();
                self.bind_lists();
                self.bind_indicator_bars();
                self.bind_chronicle_icons();
            self.bind_showcase_pictures();
                if let Some(l) = self.layout_preview.as_mut() {
                    l.transition_in();
                }
            }
            Err(e) => tracing::warn!(error = %e, name, "layout unavailable"),
        }
    }

    /// Silence the race audio (engine layers, squeal / horn loops) when leaving to the menu.
    fn stop_race_audio(&mut self) {
        if let Some(a) = &self.audio {
            if let Some(mut e) = self.engine_sound.take() {
                e.stop(a);
            }
            if let Some(s) = self.sfx.as_mut() {
                s.stop_loops(a);
            }
            if let Some(ai) = self.ai_engines.as_mut() {
                ai.stop_all(a);
            }
            if let Some(amb) = self.ambience.as_mut() {
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
        for (i, o) in self.opponents.iter().enumerate() {
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
        if let Some(m) = self.menu.as_mut() {
            let dir = self.paths.fe_data().join("Art");
            m.font = GamePaths::find_file(&dir, "Univers14b.ffn")
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
                .map(|f| BitmapFont::upload(&mut renderer, f));
        }
        if self.menu.is_some() {
            // `--layout <name>`: draw the original front-end screen instead of the text menu
            if let Some(name) = self.layout_name.clone() {
                match nfs_game::layout::LayoutScreen::load(&mut renderer, &self.paths, &name) {
                    Ok(l) => {
                        self.layout_bottom = nfs_game::layout::LayoutScreen::load(
                            &mut renderer,
                            &self.paths,
                            "bottombar",
                        )
                        .ok();
                        if let Some(b) = self.layout_bottom.as_mut() {
                            b.set_overlay(true);
                        }
                        self.layout_preview = Some(l);
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
            if let Some(name) = self.layout_name.as_deref()
                && is_network_layout(name)
            {
                self.lobby = netplay::Lobby::open(&self.player_name)
                    .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                    .ok();
            }
            self.bind_car_logos();
            self.bind_lists();
            self.bind_indicator_bars();
            self.bind_chronicle_icons();
            self.bind_showcase_pictures();
            if let Some(l) = self.layout_preview.as_ref() {
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
                let white = self.layout_preview.as_ref().map(|l| l.white_texture());
                let atlas = self.layout_preview.as_ref().and_then(|l| l.a_font_texture());
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
                if let Some(l) = self.layout_preview.as_mut() {
                    for _ in 0..120 {
                        if l.advance_transition(16) {
                            break;
                        }
                    }
                }
                // a network screen is taken on its first frame, before any `'d'` has had time to
                // come back, so give the probe half a second — otherwise the shot shows an empty
                // list that says nothing about the screen
                if self.lobby.is_some() {
                    let until = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < until {
                        if let Some(l) = self.lobby.as_mut() {
                            l.poll();
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    self.bind_lists();
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
        // `Replay_startRace` 0x413EA0 + `0x414EB0`: rewind the stream and seed the shared RNG from
        // the race's seed, which is what makes a replay re-simulate the same race (replay.md §11).
        let playing = self
            .replay
            .as_ref()
            .is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing);
        if let Some(r) = self.replay.as_mut() {
            r.start_race();
        }
        if playing {
            // playback opens on `DirectorsCam` (0x14) — "whichever track-side camera the viewed car
            // is currently passing", the classic NFS director's cut (`replay.md` §8)
            if !self.camera_cycle.contains(&0x14) {
                self.camera_cycle.push(0x14);
            }
            self.camera_mode = self
                .camera_cycle
                .iter()
                .position(|v| *v == 0x14)
                .unwrap_or(0) as u8;
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
            self.replay_setup = nfs_game::replay::ReplaySetup {
                track: self.track.name.clone(),
                car: self.car.model.clone(),
                sim: self.sim_name.clone(),
                car_id: self.car_id,
                paint: self.paint,
                laps: self.num_laps,
                opponents: self.num_opponents,
                traffic: TRAFFIC_COUNT
                    .iter()
                    .position(|c| *c == self.traffic_count)
                    .unwrap_or(0) as u32,
                cops: self.cops_enabled,
                backwards: self.backwards,
                mirror: self.mirror,
                seed: self.traffic_seed,
                skill: self.race_skill,
            };
        }
        self.replay_setup.seed = self.traffic_seed;
        self.replay_saved = false;
        self.replay_loop = 0;
        nfs_game::rng::seed(self.traffic_seed);
        let mut world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
        let car = CarModel::upload_with_env(
            &mut renderer,
            &self.car,
            &self.paths,
            1,
            self.paint,
            world.env_map,
            self.track.is_night(),
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
        if self.audio.is_none() {
            self.audio = nfs_audio::AudioOutput::new()
                .map_err(|e| tracing::warn!(error = %e, "audio disabled"))
                .ok();
        }
        if let Some(a) = &self.audio {
            self.engine_sound = nfs_audio::engine::EngineSound::load(
                a,
                &self.paths.game_data().join("Sounds"),
                &self.sound_set,
                &self.sim_name,
            )
            .map_err(|e| tracing::warn!(error = %e, "engine sound unavailable"))
            .ok();
            // track ambience (`engine-audio.md` §7): the `<track>_audio.scn` emitters over `track.viv`
            self.ambience = match self.track.load_scene("_audio.scn") {
                Ok(Some(scene)) => nfs_audio::ambience::Ambience::load(
                    &self.paths.game_data().join("Sounds"),
                    &[&scene],
                    self.audio_settings.ambient_level,
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
            if self.ai_engines.is_none() {
                self.ai_engines =
                    nfs_audio::ai_engine::AiEngines::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "AI engine banks unavailable"))
                        .ok();
            }
            if self.sfx.is_none() {
                self.sfx =
                    nfs_audio::sfx::CollisionSfx::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "collision sfx unavailable"))
                        .ok();
            }
            if self.speech.is_none() {
                self.speech =
                    nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech"))
                        .map_err(|e| tracing::warn!(error = %e, "speech unavailable"))
                        .ok();
            }
            self.countdown_said = 0;
            if self.music.is_none() && self.autotest.is_none() && self.screenshot.is_none() {
                // `MusicPlay(NULL, 0)` at race start: any entry of the saved playlist but the current one
                self.music = nfs_audio::music::MusicPlayer::new(
                    &self.paths.game_data().join("Music"),
                    &self.paths.game_data().join("Sounds"),
                    &self.audio_settings,
                )
                .map_err(|e| tracing::warn!(error = %e, "music unavailable"))
                .ok();
                if let Some(m) = self.music.as_mut() {
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
                self.opponents.clear();
                self.opponent_models.clear();
                self.opponent_sets.clear();
                self.opponent_paints.clear();
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
                    let g = grid.unwrap_or(g as u32);
                    let (Ok(assets), Ok(sim)) = (
                        CarAssets::load(&self.paths, &entry.model),
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
                    ) else {
                        continue;
                    };
                    let specs = CarSpecs::from_sim(
                        &sim,
                        entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935"),
                    );
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
                    self.opponent_models.push(model);
                    let controller = match CarAssets::load_ais(&self.paths, &entry.model) {
                        Ok(ais) => {
                            let mut d = nfs_game::ai::AiDriver::new(ais, &specs, &sim.unknown_tail);
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
                    self.opponent_sets.push(entry.sound_set.clone());
                    self.opponent_paints.push(paint);
                    self.opponents.push(RaceCar {
                        name: name.clone(),
                        vehicle,
                        specs,
                        progress: RaceProgress::new(sl, t),
                        controller,
                        grid_pos: g,
                        model: self.opponent_models.len() - 1,
                        last_progress_tick: 0,
                        wall_hits: 0,
                        resets: 0,
                        eliminated: false,
                    });
                }
                // network play: every opponent seat belongs to a remote player. Their cars sit on
                // the grid until the first 0xBE arrives and are driven by it afterwards, so their
                // controllers never run.
                if let Some(n) = self.net.as_mut() {
                    n.reserve((0..self.opponents.len()).collect());
                    for o in self.opponents.iter_mut() {
                        o.controller = Controller::Parked;
                    }
                    tracing::info!(
                        seats = self.opponents.len(),
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
                self.traffic = None;
                self.traffic_models.clear();
                self.traffic_sets.clear();
                self.traffic_paints.clear();
                self.traffic_prev.clear();
                if self.traffic_count > 0 {
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
                    for k in 0..self.traffic_count as usize {
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
                                ) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        tracing::warn!(id, model = entry.model, error = %e, "traffic car upload failed");
                                        continue;
                                    }
                                };
                                self.traffic_models.push(model);
                                by_id.push(id);
                                self.traffic_models.len() - 1
                            }
                        };
                        // the collision box is half the model's bounding box (`traffic.md` §21.4)
                        let m = &self.traffic_models[slot];
                        self.traffic_sets.push(entry.sound_set.clone());
                        self.traffic_paints.push(
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
                    if self.traffic_specs.is_none() {
                        self.traffic_specs = CarAssets::load_sim(&self.paths, "default")
                            .ok()
                            .map(|sim| CarSpecs::from_sim(&sim, false));
                    }
                    self.traffic_bodies = plan
                        .iter()
                        .map(|sp| {
                            let specs = self
                                .traffic_specs
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
                        // `[0x5E8BF8]` for the §5.2 rule 3 early-race boost
                        set.opponents = self.num_opponents;
                        // traffic drives a tenth slower at night (`ai.md` §5.2.3)
                        set.night = night;
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
                        self.traffic = Some(set);
                        for sp in &plan {
                            tracing::debug!(model = sp.model, bus = sp.bus, half = ?sp.half_extents, mass = sp.mass(), "traffic body");
                        }
                        tracing::info!(
                            cars = plan.len(),
                            models = self.traffic_models.len(),
                            bodies = self.traffic_bodies.len(),
                            linear,
                            "traffic placed"
                        );
                    }
                }
                // police (`traffic.md` §17.1, §25): the track's five cop car ids are `nfs5.trk +0x8A`
                // and the spawn points `GameData/Track/Cops/<track>.cop`; a single race gets four cars.
                self.police = None;
                self.police_models.clear();
                self.police_prev.clear();
                self.police_bodies.clear();
                if self.cops_enabled {
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
                                    ) else {
                                        continue;
                                    };
                                    self.police_models.push(model);
                                    by_id.push(id);
                                    self.police_models.len() - 1
                                }
                            };
                            let m = &self.police_models[slot];
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
                            self.police_bodies = cars
                                .iter()
                                .map(|c| {
                                    let specs = self
                                        .traffic_specs
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
                                models = self.police_models.len(),
                                points = file.records.len(),
                                "police on duty"
                            );
                            let mut pol =
                                nfs_game::police::Police::new(file, cars, self.race_skill, one_way);
                            pol.budget = nfs_game::police::duty_budget(
                                1 + self.num_opponents as usize,
                                false,
                            );
                            self.police = Some(pol);
                        }
                    }
                }
                self.racing_line = self.track.racing_line(&self.paths, self.backwards);
                tracing::info!(
                    opponents = self.opponents.len(),
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
        vehicle.automatic = true;
        // the body the car actually collides with (`BndP`, `RE/notes/car-model.md` §12)
        vehicle.body_points = nfs_game::world::car_body_points(&self.car);
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
        self.world = Some(world);
        self.car_model = Some(car);
        // the cockpit view's eye point, for the car's own body style
        self.driver_cam = nfs_assets::car::DriverCam::from_tpg(&self.car.tpg, 1);
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
                    && let Some(o) = self.opponents.get(idx)
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
                    tracing::info!(t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, opp = ?self.opponents.iter().map(|o| (o.progress.slice, (o.vehicle.speed() * 3.6) as i32, match &o.controller { Controller::Ai(a) => (a.target_speed * 3.6) as i32, _ => -1 }, o.vehicle.gear, o.wall_hits, o.resets, o.vehicle.wheels[0].steer as i32, (o.vehicle.throttle * 100.0) as i32, (o.vehicle.brake * 100.0) as i32, o.vehicle.position.x as i32, o.vehicle.position.z as i32)).collect::<Vec<_>>(), wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), police = ?self.police.as_ref().map(|p| p.cars.iter().filter(|c| c.active).map(|c| (c.slice, (c.speed * 3.6) as i32, (c.position - v.position).length() as i32, c.mode)).collect::<Vec<_>>()), violation = ?self.police.as_ref().map(|p| p.violation), traffic = ?self.traffic.as_ref().map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| (c.slice, c.dir as i32, (c.speed * 3.6) as i32, (c.lateral * 10.0) as i32, (c.position - v.position).length() as i32, (c.position.y - v.position.y) as i32)).collect::<Vec<_>>()), pos = ?v.position, "autotest");
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
                        if self.leave_race() {
                            self.input.end_tick();
                            return;
                        }
                        event_loop.exit();
                    } else if self.vehicle.is_some() {
                        self.paused = true;
                        if let (Some(a), Some(s)) = (&self.audio, self.sfx.as_mut()) {
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
                        .replay
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
                        self.replay_speed = sp;
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
                    && let Some(menu) = self.menu_template.take()
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
                    if self.menu.is_none() {
                        return;
                    }
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
                    let n = match &self.replay {
                        Some(r) if r.mode == nfs_game::replay::Mode::Playing => {
                            self.replay_speed.ticks_at(self.replay_loop)
                        }
                        _ => 1,
                    };
                    self.replay_loop += 1;
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
            nfs_game::replay::ReplayFile::parse(&d)
                .or_else(|| {
                    // an original-format `.rpl` (§6.1 + §16): the setup snapshot, the final tick and the
                    // same stream, so the port can play one back as far as its own simulation matches
                    let (setup, final_tick, stream) = nfs_game::replay::parse_original_rpl(&d)?;
                    tracing::info!(path = p, track = setup.track, "original-format .rpl");
                    Some(nfs_game::replay::ReplayFile {
                        setup,
                        final_tick,
                        stream,
                    })
                })
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
    // an original `.rpl` names the car by its `nfs5.car` id only (§16.2), so resolve the model and
    // the simulation from the catalogue
    if let Some(rs) = replay_setup.as_mut()
        && rs.car.is_empty()
        && rs.car_id != 0
        && let Ok(cat) = std::fs::read(paths.fe_data().join("Data").join("nfs5.car"))
            .map_err(|e| tracing::warn!(error = %e, "nfs5.car"))
        && let Ok(entries) = nfs_formats::fedata::parse_car_catalog(&cat)
        && let Some(e) = entries.iter().find(|c| c.id == rs.car_id)
    {
        rs.car = e.model.clone();
        rs.sim = e.sim.clone();
        rs.paint = e
            .paint_sets
            .first()
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
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
    let mut sim = CarAssets::load_sim(&paths, &sim_name)?;
    if let Some(r) = &career_request {
        nfs_game::career::apply_parts(&mut sim, &r.parts.iter().collect::<Vec<_>>());
    }
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
    let specs = CarSpecs::from_sim(&sim, race_class);
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
        let car_idx = cars
            .iter()
            .position(|c| c.model.eq_ignore_ascii_case("993") && c.sim.contains("coupe"))
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
            opponents: 3,
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
    let mut app = App {
        layout_sweep,
        factory_profile: 0,
        registry: nfs_formats::fetxt::Registry::load(&paths.root),
        choices: std::collections::HashMap::new(),
        paths,
        net,
        lobby: None,
        menu,
        menu_template: None,
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
        opp_prev: Vec::new(),
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
        replay: Some(match &replay_file {
            Some(f) => nfs_game::replay::Replay::playing(f.stream.clone(), f.final_tick),
            None => nfs_game::replay::Replay::new(),
        }),
        replay_speed: nfs_game::replay::Speed::Normal,
        replay_setup: replay_setup
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
        replay_path: nfs_game::save::save_dir().join("replay.prpl"),
        replay_saved: false,
        replay_loop: 0,
        replay_end_ticks: 0,
        // the original seeds the race RNG from the registry's RAND_SEED (replay.md §11); a replay
        // carries the seed so playback re-simulates the same traffic, police and AI
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
        crash_played: false,
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
        fe_room: Vec::new(),
        fe_cars: Vec::new(),
        fe_ms: 0,
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
        glare_draw: None,
        track_cameras: None,
        reverb: nfs_audio::reverb::Reverb::default(),
        env_bound: None,
        slices: None,
        race: None,
        opponents: Vec::new(),
        opponent_models: Vec::new(),
        layout_preview: None,
        layout_bottom: None,
        fe_strings,
        layout_name: args
            .iter()
            .position(|a| a == "--layout")
            .and_then(|i| args.get(i + 1).cloned())
            .or(fe_start),
        layout_sel: 0,
        layout_stack: Vec::new(),
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
        cops_enabled: std::env::var("NFS_COPS")
            .map(|v| v != "0")
            .unwrap_or(fe.cops),
        ai_engines: None,
        ambience: None,
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
        dials: catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| (c.dial_kmh, c.dial_tach)),
        paint,
        audio: None,
        engine_sound: None,
        sfx: None,
        last_gear: 1,
        music: None,
        music_names,
        audio_settings,
        speech: None,
        countdown_said: 0,
        sound_set,
        sim_name: sim_name.clone(),
        race_skill,
        last_frame: Instant::now(),
        accumulator: Duration::ZERO,
        tick: 0,
        frames: 0,
        fps_timer: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
