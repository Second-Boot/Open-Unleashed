//! In-race HUD and the front end's overlay sprites: the `Hud` state, its layout constants and the helper types (`RE/notes/hud.md`).
//! The drawing code is split by what it draws: `geometry` (loading, anchors, text), `screens` (results, dialogs, buttons), `race` (the in-race trays, dials, map).

use crate::*;
mod geometry;
mod screens;
mod race;

/// In-race HUD (`RE/notes/hud.md`, CONFIRMED): dial faces and needles from `nfs5.car` dial
/// records (`speedK/tach.fsh` entry = style, `needle2/needle.fsh` entry = needle style), 128 px
/// dials bottom-right, needle sweep `φ = start + v·deg` calibrated clockwise-from-12 as `φ − 90°`
/// (Boxster/996 records `−φ − 90°`, HYPOTHESIS on the projection sign), texts in `UniversC12b`.
/// Not reproduced: translucent trays, slide-in, track map, mirror.
/// The in-race trays' strings, each fetched from `FEStrings` by the element that draws it.
pub(crate) struct HudText {
    /// `HSpeed` 0x42E570: the value and FE 11 "km/h" / FE 20 "mph"
    pub(crate) speed: String,
    pub(crate) speed_label: String,
    /// `HGear` 0x421CB0: FE 18145 "R" / 18146 "N" / "1"…"7", under FE 10 "gear"
    pub(crate) gear: String,
    pub(crate) gear_label: String,
    /// `HTime` 0x4317B0: FE 18 "time"
    pub(crate) time_label: String,
    /// `HWrongWay` 0x436FD7: FE 18159 "WRONG WAY"
    pub(crate) wrong_way: String,
    /// `SPEEDOMETER` / `HUDTACHOMETER` (0 off, 1 digital, 2 analog) and `GEAR` (0 off, 1 on): the
    /// analog dials draw only on 2 (0x41E940 / 0x41E960), `HSpeed` only on 1 (0x41E920), `HGear`
    /// only on 1 (0x41E900); `fe.txt` ships 2, 2, 1. The digital tachometer (`HTachDigital`, 1 —
    /// 0x41E980) is not ported.
    pub(crate) speedometer: usize,
    pub(crate) tachometer: usize,
    pub(crate) gear_on: bool,
    /// `HTachDigital` 0x42F740: FE 18144 "rpm"
    pub(crate) rpm_label: String,
    /// `HTach`'s needle scale `[+0xA9D0] = (R − 750) / R`, `R = car+0xEE0 = ftol(carSpecs+0x94)` the
    /// redline (0x406EF0 / 0x42F50F…0x42F52C)
    pub(crate) tach_scale: f32,
}

/// One string's layout in its font ([`Hud::text_run`]): the glyph quads in font texels
/// (`BitmapFont::place`) and the measured width (`BitmapFont::measure`'s `.0`).
pub(crate) struct TextRun {
    /// 0 `UniversC12b`, 1 `unviers9b`, 2 `unviers9bi`
    pub(crate) font: u8,
    /// the height the run is drawn at when the size class fixes it (the small and italic fonts'
    /// own line heights, the italic's fallback `FONT_SMALL`); `None`: the caller's `h`
    pub(crate) fixed_h: Option<f32>,
    pub(crate) glyphs: Vec<nfs_game::text::PlacedGlyph>,
    pub(crate) width: f32,
}

pub(crate) struct Hud {
    /// `HTray`'s fill: a 1 × 2 texture, black at alpha 63 over black at 255, sampled between the
    /// texel centres so the quad runs top to bottom from one to the other (see [`Hud::tray`])
    pub(crate) tray_fill: TextureId,
    /// `HGraphicDamage`: `car.fsh` entries 0…6 (six body parts and the backdrop) and the two
    /// gauge gradients (0xB2058228 → 0xB2FFFF00, 0xB2FFFF00 → 0xB2DC1905)
    pub(crate) damage_parts: [Option<TextureId>; 7],
    pub(crate) damage_gauge: (TextureId, TextureId),
    /// `HBillBoard`'s fill (1 × 2: 0xB2FEDD81 over 0x8082A8B0) and its strip shade (2 × 1: black at
    /// alpha 0x80 beside black at 0xFF), used the same way
    pub(crate) billboard_fill: TextureId,
    pub(crate) strip_shade: TextureId,
    /// `HButtonVisual`'s `+0x103` dressing (0x41B4A4…0x41B50C), as two-texel gradients sampled
    /// texel centre to texel centre: `[gloss top, gloss bottom, dark ↓, dark →, light ↓, light →]` —
    /// the gloss of 0x435DA0 (white a → 0 over the top half, black 0 → a over the bottom, `a = 127`
    /// for its 100 %), and the 0xCC / 0x19 ramps of the bevel 0x436130 and the shadow 0x435F80.
    pub(crate) button_ramps: [TextureId; 6],
    /// The height the billboard currently has while it opens (`HBillBoard +0xE8`, see
    /// [`Hud::billboard`]); set by the caller before a post-race page is drawn
    pub(crate) billboard_h: std::cell::Cell<f32>,
    /// `[0x606AC8]` now and at the billboard's creation, for its strips' moving split
    pub(crate) billboard_clock: std::cell::Cell<(u64, u64)>,
    /// `DISPLAYCOLORS` (0x4204F0, `Display.DispCols`): 0xCCFAB400 orange, 0xCCFF0606 red, 0xCC1EE605
    /// green — every value text's colour (the 32 callers pass it to `HTextVisual`'s constructor or
    /// its `+0x24`) and, as RGB at alpha 0x3F, the trays' outline and rule (0x4345F0). Set each frame.
    pub(crate) display: std::cell::Cell<[f32; 4]>,
    pub(crate) dial: TextureId,
    /// speedometer needle: texture, (width, height) px, pivot px above the bottom
    pub(crate) needle: (TextureId, f32, f32, f32),
    pub(crate) hub: Option<TextureId>,
    pub(crate) kmh: nfs_formats::fedata::DialSpec,
    pub(crate) tach_spec: nfs_formats::fedata::DialSpec,
    /// countdown digits 3, 2, 1 (solid) from `321go.fsh` / `321go2.fsh`
    pub(crate) digits: [Option<TextureId>; 3],
    pub(crate) rings: Option<(TextureId, TextureId)>,
    /// `HDial`'s ring `+0xC8` (0x42131C…0x42135D): `circletach.fsh` entry 0 over the dial's 128 × 128,
    /// all four corners 0x80000000 — a half-black disc under the face, drawn while 0x41E5C0 is
    /// false (0x42166A), i.e. while `TRANSLUCENCY` is 0 (see [`Hud::boxes`]). Both dials pass it.
    pub(crate) dial_ring: Option<TextureId>,
    /// `HTrackMap`'s blip `+0xE4` (0x432122…0x432143): `radar.fsh` entry 0, "Car" (26 × 63), a white
    /// top-down car the blip loop tints with each car's paint.
    pub(crate) radar_car: Option<TextureId>,
    /// `HPosition`'s link badge `+0x250` (ctor 0x42AC8B…0x42ACCC): `DashHud/pingmeter.fsh` entry 0 `dot`,
    /// 5 × 5 (`[0x5B2D88]`, `[0x5B2D8C]`), tinted by 0x490140's answer (0x5B2D90) in a network race.
    pub(crate) ping_dot: Option<TextureId>,
    /// `TRANSLUCENCY == 0` — the shipped default, which the options screen labels "on" (the accessor
    /// lists 1501, 1500). 0x425298 / 0x4267A0 call 0x41E540 for 0, which puts `[0x5CCB34]` = 0x80 into
    /// the top byte of the layout record's flags `cfg1+0x14`, and 0x41E590 otherwise, which clears it;
    /// 0x41E5C0 tests that byte. With it clear no tray draws its fill, rule or frame (0x4341B6 →
    /// 0x4342CF) and the dials lose their ring. Set each frame.
    pub(crate) boxes: std::cell::Cell<bool>,
    /// The racing trays are in place (state 3, `TraySlide`), set per frame: `HTray`'s rule is drawn
    /// only then (0x43428D…0x434297).
    pub(crate) trays_in: std::cell::Cell<bool>,
    pub(crate) font: Option<BitmapFont>,
    /// `unviers9b.ffn` (0x420600) and `unviers9bi.ffn` (0x420620), drawn at their own size when a
    /// text asks for `FONT_SMALL` / `FONT_ITALIC` (see [`Hud::font_for`])
    pub(crate) font_small: Option<BitmapFont>,
    pub(crate) font_italic: Option<BitmapFont>,
    pub(crate) tach: Option<(TextureId, (TextureId, f32, f32, f32))>,
    /// 1×1 white texture for the translucent trays (HTray: dark box + orange outline)
    pub(crate) white: TextureId,
    /// track map (120×120 @ (0,358) in the original, an HRenderVisual top-down render; here the
    /// lap route rasterised once — MEDIUM) and its world→map projection (x0, z0, scale)
    /// the map texture and `[centre x, centre z, half width]` of `HTrackMap`'s whole-track view
    pub(crate) map: Option<(TextureId, [f32; 3])>,
    /// the route the map draws, and the transform of the last frame (`[cx, cz, scale, fx, fz]`)
    pub(crate) map_route: Vec<(f32, f32)>,
    pub(crate) map_frame: std::cell::Cell<[f32; 5]>,
    /// The route's pixel endpoints the map texture was last drawn from, so [`Hud::map_update`]
    /// redraws and re-uploads only when one of them moved (performance work A9, 2026-10-01). The
    /// picture is a pure function of them (and the constant grid), so equal endpoints are the same
    /// pixels already in the texture. Reset by `set_map`.
    pub(crate) map_drawn: std::cell::RefCell<Option<Vec<(i32, i32)>>>,
    /// the buffer the next frame's endpoints are computed into
    pub(crate) map_scratch: std::cell::RefCell<Vec<(i32, i32)>>,
    /// The laid-out glyph runs of the strings the HUD draws, per size class (`FONT_SMALL`,
    /// `FONT_ITALIC`, any other height), so a string drawn again is not laid out or measured again
    /// (performance work PG, 2026-10-01; see [`Hud::text_run`]).
    pub(crate) text_runs: std::cell::RefCell<[std::collections::HashMap<String, std::rc::Rc<TextRun>>; 3]>,
    /// The grid layer of the map picture (it never changes): built once, copied into each redraw.
    pub(crate) map_grid: std::cell::OnceCell<Vec<u8>>,
    /// the picture last uploaded, kept only under `NFS_MAP_VERIFY` (see `map_update`)
    pub(crate) map_check: std::cell::RefCell<Option<Vec<u8>>>,
    /// the picture's buffer, kept between redraws (`map_update` draws into it in place; the upload
    /// copies it into the renderer's staging bytes)
    pub(crate) map_tex: std::cell::RefCell<Option<TextureData>>,
    /// `HReplay`'s twelve glyphs from `DashHud/replay.fsh`, by the file's own index
    /// (`RE/notes/hud.md` §5.2): 0 cam, 1 paus, 2 quit, 3 rest, 4 Irac, 5…8 the speed steps,
    /// 9 save, 10 next, 11 play.
    pub(crate) replay_glyphs: [Option<(TextureId, f32, f32)>; 12],
    /// `HCongrats` 0x41CAF0's pictures: the solid place digits `1.tg`/`2.tg`/`3.tg`, the outline
    /// digits `1OL`…`8OL`, and `circlefull` / `congrats1` / `circleoutline`
    pub(crate) congrats: CongratsTex,
    /// `HMissionSummary`'s portraits: `factorypeople.fsh`, 54×64, one per narrator (0x428A19)
    pub(crate) portraits: [Option<TextureId>; 6],
}

/// What `HPosition` adds to a row in a network race (FEGAME_TYPE 3): the badge tint and, for a
/// racer in state 3, the "DNF" text (`network.md` §11.13).
#[derive(Debug, Clone)]
pub(crate) struct LinkBadge {
    /// 0x5B2D90[0x490140(race index)] as RGBA
    pub(crate) colour: [f32; 4],
    /// FE 18191 when the racer's state is 3 (0x42B279)
    pub(crate) dnf: Option<String>,
}

/// The textures `HCongrats`' constructor loads (0x41CB84…0x41CED8), in its own field order.
pub(crate) struct CongratsTex {
    /// `+0x1C…+0x24`: `congrats3`/`1.tg`, `congrats4`/`2.tg`, `congrats4`/`3.tg`
    pub(crate) solid: [Option<TextureId>; 3],
    /// `+0x28…+0x44`: `321go2`/`1OL`, `321go`/`2OL`, `321go`/`3OL`, `congrats2`/`4OL`…`7OL`,
    /// `congrats3`/`8OL`
    pub(crate) outline: [Option<TextureId>; 8],
    /// `+0x10` `circlefull`, `+0x14` `congrats1` (the broken ring), `+0x18` `circleoutline`
    pub(crate) full: Option<TextureId>,
    pub(crate) cut: Option<TextureId>,
    pub(crate) ring: Option<TextureId>,
}

/// `[0x5B29B8…]`: the solid digits' width × height, and `[0x5B29D0…]` the outline digits'.
pub(crate) const CONGRATS_SOLID: [(f32, f32); 3] = [(42.0, 55.0), (44.0, 57.0), (45.0, 59.0)];

pub(crate) const CONGRATS_OUTLINE: [(f32, f32); 8] =
    [(44.0, 57.0), (46.0, 59.0), (47.0, 60.0), (48.0, 57.0), (43.0, 58.0), (44.0, 59.0), (41.0, 57.0), (45.0, 59.0)];

/// `[0x5B2A10…]` (read as `[0x5B2A0C + place·4]`): gold, silver, bronze for places 1–3.
pub(crate) const CONGRATS_RGB: [u32; 3] = [0xFFBE02, 0xB4B4B4, 0xC36405];

/// Where the `HCongrats` picture sits and how big it is, `t` ticks after it opened — in the HUD's
/// 640×480 space. It starts at the screen centre (0x41DE29, `+0x54/+0x58`). The **tournament**
/// instance, past 0xC0 ticks, slides for 32 ticks (`_ftol(0.5 · 64)`, 0x41DE71) towards
/// `(_ftol(W·0.2 − 128·0.5·0.5), _ftol(H·0.2))` (0x41DE2F…0x41DE6B), each axis clamped at its target,
/// and shrinks as it goes: `scale = 1 − 0.5 · moved_x / total_x` (0x41D2CD…0x41D357). CONFIRMED.
/// `sw` × `sh` is the render target in pixels (the original's `W` / `H`, 0x60C900 / 0x60C81C) and the
/// result is in its pixels.
pub(crate) fn congrats_layout(t: u64, tournament: bool, sw: f32, sh: f32) -> (f32, f32, f32) {
    let (cx, cy) = ((sw / 2.0).trunc(), (sh / 2.0).trunc());
    if !tournament || t < 0xC0 {
        return (cx, cy, 1.0);
    }
    let (tx, ty) = ((sw as f64 * 0.2f32 as f64 - 128.0 * 0.5 * 0.5) as i64, (sh as f64 * 0.2f32 as f64) as i64);
    let steps = (0.5f64 * 64.0) as f64;
    let (rx, ry) = ((cx as i64 - tx) as f64 / steps, (cy as i64 - ty) as f64 / steps);
    let d = (t - 0xC0) as f64;
    let x = ((cx as f64 - d * rx) as i64).max(tx);
    let y = ((cy as f64 - d * ry) as i64).max(ty);
    let scale = 1.0 - (cx as i64 - x) as f64 * 0.5 / (cx as i64 - tx) as f64;
    (x as f32, y as f32, scale as f32)
}

pub(crate) const MAP_PX: u32 = 120;

/// One `nfs5-1.hud` / `nfs5-2.hud` **"Config N"** preset's element positions, in the HUD's
/// 640×480 design space — the pixel values **0x41FB40** computes (`int(cfg.x × screenW)`,
/// `int(cfg.y × screenH)`, `hud.md` §2/§2.1). `Display.ScreenLayout` (`SCREENLAYOUT`, cfg1's own
/// selection field, `0x657DB8`) picks which of these is live: `0x41F7A0`'s selection needs
/// **both** `cfg2[i].w/h == screen W/H` **and** `cfg1[i].name == "Config <SCREENLAYOUT+1>"`
/// (table `0x5CCB38`), and at this port's design resolution (always 640×480 before
/// [`Hud::anchor`]/[`Hud::place`] letterbox it into a wider window) only the shipped file's first
/// four records ever match — the fifth ("Config 2" @ 1600×1200, the record `0x41EBF0` generates
/// the first time the original runs at that size, `hud.md` §2.2) never does. Sizes are constant
/// across every config (`hud.md` §2: "sizes never change, only positions").
///
/// The values below are read **directly off the shipped
/// `FEData/Data/nfs5-1.hud`/`nfs5-2.hud`** (records 1..4, offsets `+0x188…+0x1D8` for the cfg1
/// fields), then run through the same `int(x × 640)`, `int(y × 480)` 0x41FB40 does — not
/// hand-typed off `hud.md`'s own summary table, whose "Config 4" column turns out to actually be
/// the generated 1600×1200 record's values for `HTrackMap` (a transcription slip in that note,
/// left as read here rather than silently "corrected" without re-deriving the rest of that
/// table).
///
/// **Confirmed dead FE control until this port** (COVERAGE §14.6.3):
/// `Display.ScreenLayout` was registered and persisted (`main.rs`'s `CHOICE_ACCESSORS`,
/// `nfs-formats/src/sav.rs`) but nothing read the selected value — picking Config 2/3/4 in the
/// options menu moved nothing on the race HUD.
///
/// **`HMultiTime`'s own position** (cfg2 `+0xC`/`+0x10`, `hud.md` §2.1, tray 73×34): read directly
/// off the shipped `nfs5-2.hud`'s 104-byte records the same way as every other field here —
/// `(427,0)` for Config 1/3/4, `(427,36)` for Config 2 — and wired into [`App::hud_sprites`]'s
/// `multi` tray draw (was hardcoded to Config 1's own `(427.0, 0.0)`, `hud.md`'s own audit before
/// this fix: "five sites read the front end's table and drew nothing: HMultiTime's 18172/21…24").
pub(crate) struct HudLayout {
    pub(crate) gear: (f32, f32),
    pub(crate) speed: (f32, f32),
    pub(crate) time: (f32, f32),
    pub(crate) tach_digital: (f32, f32),
    pub(crate) tach: (f32, f32),
    pub(crate) speedometer: (f32, f32),
    /// `HPosition`'s tray (the opponent list / network leaderboard)
    pub(crate) position: (f32, f32),
    pub(crate) track_map: (f32, f32),
    pub(crate) leader: (f32, f32),
    pub(crate) rearview: (f32, f32),
    /// `HMultiTime`'s own tray (mission goal time / record / record lap / best lap / last lap)
    pub(crate) multi_time: (f32, f32),
}

/// `HGraphicDamage` and `HTextDamage` are not in this table: their cfg1 `+0x1A0`/`+0x1B0` fields
/// read `(0, 0)` in every one of the four records (verified the same way), so they never move.
pub(crate) const SCREEN_LAYOUTS: [HudLayout; 4] = [
    // Config 1 — the shipped default (`fe.txt` has no `screenlayout` key, so `SCREENLAYOUT` opens
    // on cfg1's own default index, 0 — `hud.md` §2).
    HudLayout {
        gear: (555.0, 444.0),
        speed: (460.0, 444.0),
        time: (427.0, 33.0),
        tach_digital: (380.0, 444.0),
        tach: (512.0, 307.0),
        speedometer: (392.0, 352.0),
        position: (502.0, 1.0),
        track_map: (0.0, 358.0),
        leader: (125.0, 0.0),
        rearview: (185.0, 0.0),
        multi_time: (427.0, 0.0),
    },
    // Config 2
    HudLayout {
        gear: (554.0, 444.0),
        speed: (460.0, 444.0),
        time: (427.0, 0.0),
        tach_digital: (380.0, 444.0),
        tach: (512.0, 303.0),
        speedometer: (394.0, 352.0),
        position: (502.0, 0.0),
        track_map: (0.0, 358.0),
        leader: (125.0, 0.0),
        rearview: (186.0, 0.0),
        multi_time: (427.0, 36.0),
    },
    // Config 3
    HudLayout {
        gear: (555.0, 444.0),
        speed: (437.0, 408.0),
        time: (427.0, 33.0),
        tach_digital: (437.0, 444.0),
        tach: (512.0, 307.0),
        speedometer: (392.0, 352.0),
        position: (502.0, 1.0),
        track_map: (0.0, 358.0),
        leader: (120.0, 0.0),
        rearview: (177.0, 0.0),
        multi_time: (427.0, 0.0),
    },
    // Config 4
    HudLayout {
        gear: (427.0, 343.0),
        speed: (567.0, 444.0),
        time: (525.0, 0.0),
        tach_digital: (490.0, 444.0),
        tach: (116.0, 352.0),
        speedometer: (0.0, 309.0),
        position: (425.0, 0.0),
        track_map: (0.0, 358.0),
        leader: (125.0, 0.0),
        rearview: (186.0, 0.0),
        multi_time: (427.0, 0.0),
    },
];

/// Race-start countdown (CONFIRMED values, `RE/notes/timing.md`): game ticks 256/320/384/448 =
/// 4/5/6/7 s; the race starts at tick 448 and brakes are forced until then.
pub(crate) const RACE_START_TICK: u64 = 448;

pub(crate) const COUNTDOWN_TICKS: [u64; 3] = [256, 320, 384];

/// HUD reference height (px) — sizes are fixed pixels in the original (hud.md §2).
pub(crate) const HUD_REF_H: f32 = 480.0;

/// [`App::hud_cfg`]: the profile the tables were read for, and the tables.
#[derive(Default)]
pub(crate) struct HudCfgCache {
    pub(crate) loaded: bool,
    pub(crate) who: Option<(std::path::PathBuf, String)>,
    pub(crate) cfg: Option<hud_cfg::HudCfg>,
    /// the profile's own `<Name>2.hud` existed (or was written): the master of the Display options
    pub(crate) from_profile: bool,
}

/// A string [`Hud::text`] has pushed: its font texture, the first glyph quad's position as pushed, the glyph
/// count and the design-space pen, kept until [`Hud::resolve_text`] knows the element's final shift.
pub(crate) struct PendingText {
    pub(crate) texture: TextureId,
    pub(crate) pos: [f32; 2],
    pub(crate) count: usize,
    pub(crate) pen: [f32; 2],
}

thread_local! {
    pub(crate) static PENDING_TEXT: std::cell::RefCell<Vec<PendingText>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// The render target's size in pixels, for [`Hud::place`] / [`Hud::anchor`] (see [`Hud::set_screen`]).
    pub(crate) static HUD_PX: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((640, 480)) };
}

thread_local! {
    /// The stored positions of the record in use (N01a), see [`Hud::set_stored`]: `(design x, design y,
    /// width, stored (fx, fy))` per element; empty when the size has no record (the derivation runs).
    pub(crate) static HUD_STORED: std::cell::RefCell<Vec<(f32, f32, f32, (f32, f32))>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) const HUD_REF_W: f32 = 640.0;

pub(crate) const HUD_ORANGE: [f32; 4] = [0.98, 0.706, 0.0, 1.0];

/// **`HButtonVisual`'s two colour pairs** — 0x41B340, its vtable slot 6 (`RE/notes/hud.md` §5.3).
/// The slot sets two colours per button: the plate through `0x419F50(&this+0x14, …)` and the glyph
/// through `[this+0xF8]+0x24`. Idle (`this+0xFC == 0` and the pointer outside the box) is
/// **0xFFBC0000** on the plate — the same dark red the `.lay` files give a button's `maincolor` —
/// with a **white** glyph (0x41B4E0); hovered is **0xFFFAB400**, the HUD orange, with a **black**
/// glyph (0x41B3F8), and crossing into the box once plays sound **9** (`0x4AADB0`, 0x41B3C4).
/// The in-race dialogs take the pointer as the original does ([`Hud::over_button`],
/// `App::hud_click`). *(The replay bar's cells were drawn in the idle pair until 2026-09-26; they
/// are not `HButtonVisual`s — see [`Hud::replay_bar`].)*
/// Every `HBillBoard` dialog — race summary, standings, records, `HInfo`, mission summary, pause
/// menu, yes/no — is constructed **500 × 325** (`[0x5B2C70]`/`[0x5B2C74]` and the nine other
/// pairs passed to 0x41A510, all equal) and centred by 0x41B100: `((W − w)/2, (H − h)/2)`, i.e.
/// (70, 77) on the 640 × 480 layout. This port's pages each had a box of their own size.
pub(crate) const BILLBOARD: (f32, f32, f32, f32) = (70.0, 77.0, 500.0, 325.0);

/// One row of `HRaceSummary`'s table.
pub struct SummaryRow {
    pub name: String,
    pub car: String,
    pub colour: [f32; 4],
    /// `+0xCFC`, a signed dword
    pub best: Option<i32>,
    /// `+0xCD8`, a signed dword (0 = dashes)
    pub total: Option<i32>,
}

/// What an `HBillBoard` frame carries (constructor 0x41A510's third to sixth arguments): the
/// text drawn left in the top strip, the one drawn right, and whether the bottom strip shows FE
/// 18100 "Esc to exit" (`+0x108`) and, with it, 18101 "Spacebar to proceed" (the enable byte
/// `+8`, 0x41AC56…0x41ACBC). Most dialogs pass the empty string `0x5E8E50` on the left.
pub struct Billboard<'a> {
    pub left: &'a str,
    pub right: &'a str,
    /// `(esc text, Some(space text))` when the bottom strip is shown
    pub hints: Option<(&'a str, Option<&'a str>)>,
}

/// `HButtonVisual` (0x41B140) size on the dialogs: 142 × 19 (`[0x5B2C78]`/`[0x5B2D48]`).
pub(crate) const HUD_BUTTON: (f32, f32) = (142.0, 19.0);

pub(crate) const HUD_GREY: [f32; 4] = [0.706, 0.706, 0.706, 1.0];

/// The HUD's fonts at their own size — the original draws them in native pixels on the 640 × 480 layout.
/// A size here is a **line height** and picks the font (`Hud::font_for`): `UniversC12b.ffn` has line 18
/// (`FONT_MAIN`), `unviers9b` 14 (`FONT_SMALL`) and the digits-only `unviers9bi` 11 (`FONT_ITALIC`), each
/// drawn from its own atlas; a string a font has no glyphs for falls back to C12b. The text measure 0x431400 reports as a text's height the extent of
/// "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789" (0x5CD0D4, the whole string: lower case,
/// upper case and digits): 16 in C12b (`EXT_MAIN`), 14 in 9b (`EXT_SMALL`), 10 in 9bi (`EXT_ITALIC`) —
/// `fntextent` example with that text. *(Corrected 2026-10-01 (P1): 9b was 13, the extent of the lower case
/// alone; the 9bi value 10 is confirmed on the original's frames — the damage gauge's top at y 74.)*
/// This port had drawn the texts at 10–12 (and 16 for a few values), about two thirds of their size.
pub(crate) const FONT_MAIN: f32 = 18.0;

pub(crate) const FONT_SMALL: f32 = 14.0;

pub(crate) const FONT_ITALIC: f32 = 11.0;

pub(crate) const EXT_MAIN: f32 = 16.0;

pub(crate) const EXT_SMALL: f32 = 14.0;

pub(crate) const EXT_ITALIC: f32 = 10.0;

/// The shared state of the race HUD's `+4` trays (the factory 0x424460 gives every tray the byte
/// `+4` = 1; the map and the mirror `+5` too): `state` 0 hidden, 1 sliding in, 2 sliding out, 3 in
/// place, `t` the `[0x606AC8]` ticks since the transition began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TraySlide {
    pub(crate) state: u8,
    pub(crate) t: i32,
}

/// The race HUD's state the in-race toggle keys change (`controls.md` §4.170, read 2026-09-27):
/// the "HUD is live" flag and the runtime copies of three `nfs5-2.hud` options.
///
/// `HUD_init` 0x41E170 loads the `nfs5-1.hud` / `nfs5-2.hud` records (the profile copies the
/// Display screen writes through 0x4CB860 → 0x41EA50) and sets `[0x60C970] = 1` (0x41E1CB) only
/// on its first reference (`[0x60C978]` 0 → 1); the one caller in a race is the HUD factory
/// 0x424460, called once per **session** (0x4B6878) — the in-place restart (0x415721 → 0x412090)
/// and the automatic replay run on the same HUD. So the toggles last for the session and a new
/// session starts from the options again; nothing writes them back (0x41EA50's in-race caller is
/// the layout drag 0x434565 only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HudKeys {
    /// `[0x60C970]`: F5 (0x4273B0) clears it (0x41E720) and sets it back (0x41E710) — its only
    /// writers. Read by the `+4` draw / slide gates (0x425A35, 0x425BB3, 0x425E55), `HTime`'s draw
    /// (0x4316F4), DAMAGEDISPLAY 3 (0x41E8D0), OPPONENTLIST (0x41E880), SPLITTIMES (0x41E850) and
    /// the map / mirror slot 10 (0x433400 / 0x42C800).
    pub(crate) live: bool,
    /// cfg2`+0x40` MAP: 0 off, 1 rotating, 2 static, 3 radar.
    pub(crate) map: usize,
    /// `[0x60C821]` / `[0x60C824]`: F7's (or F5's) suspend 0x41E6E0 keeps the MAP it zeroed; the
    /// restore 0x41E690 puts it back, or 1 when nothing was kept (0x41E6CB). `HUD_init` clears
    /// the flag (0x41E29A).
    map_kept: Option<usize>,
    /// cfg2`+0x5C` REARVIEWMIRRROR (`!= 0`, 0x41E9A0; F6 sets 1 / 0 through 0x41E730 / 0x41E750).
    pub(crate) rearview: bool,
    /// `[0x60C9A8]` / `[0x60C9F8]`: whether F5's hide found the map / the mirror on, which its show
    /// brings back (0x427420 / 0x42743A, 0x42744A / 0x427464). (The first press seeds both from the
    /// options through `[0x60CA01]`, 0x4273B0…0x4273E7, which a hide then overwrites — the show
    /// never reads the seed, since the HUD starts live.)
    pub(crate) map_at_hide: bool,
    pub(crate) mirror_at_hide: bool,
    /// cfg1`+0x14`'s top byte non-zero (0x41E5C0 is its `== 0`): the trays' box, rule and gloss
    /// ([`Hud::boxes`]). The HUD's setup (0x425298) sets it from `TRANSLUCENCY` (0x80 for 0,
    /// 0x41E540; cleared otherwise, 0x41E590); F8 (0x4275D0) flips it the same way.
    pub(crate) boxes: bool,
}

impl HudKeys {
    pub(crate) fn new(map: usize, rearview: bool, boxes: bool) -> Self {
        Self { live: true, map, map_kept: None, rearview, map_at_hide: false, mirror_at_hide: false, boxes }
    }

    /// 0x41E6E0: MAP kept and zeroed.
    pub(crate) fn suspend_map(&mut self) {
        self.map_kept = Some(self.map);
        self.map = 0;
    }

    /// 0x41E690: the kept MAP back (the flag cleared), or 1.
    pub(crate) fn restore_map(&mut self) {
        self.map = self.map_kept.take().unwrap_or(1);
    }
}

/// One step of a `+4` HUD element's slide as 0x425950 drives it, for the state `(state, stamp)`
/// at the clock `clock`: none before the countdown ends (`before_start`; the port's stand-in for
/// the clock-448 gates 0x425A29 / 0x425BAB / 0x425CCA); with a billboard queued (`[0x60C9C4]`
/// non-empty) state 3 → 2 (0x425C86…0x425CC0); otherwise 0x425E20(0) puts state 0 → 1 when
/// `may_open` — `[0x60C970]` and the element's slot 10 clear (0x425E55…0x425EDA); a transition
/// runs 11 ticks (slot 14, 0x4349F0), 1 ending in 3 and 2 in 0.
pub(crate) fn step_slide((mut st, mut stamp): (u8, u64), clock: u64, before_start: bool, billboard: bool, may_open: bool) -> (u8, u64) {
    if before_start {
        st = 0;
    } else if billboard {
        if st == 3 {
            (st, stamp) = (2, clock);
        }
    } else if st == 0 && may_open {
        (st, stamp) = (1, clock);
    }
    // diagnostic: `NFS_TRAYS_IN=1` puts the trays in place at once — `--screenshot` renders too
    // few frames to watch a slide finish
    if st == 1 && nfs_game::config::cfg().nfs_trays_in.var().is_ok() {
        stamp = clock.saturating_sub(11);
    }
    if clock.saturating_sub(stamp) >= 11 {
        st = match st {
            1 => 3,
            2 => 0,
            x => x,
        };
    }
    (st, stamp)
}

#[cfg(test)]
mod congrats_tests {
    use super::congrats_layout;

    /// 0x41DDD0 / 0x41D2CD: the race instance stays centred; the tournament one leaves the centre at
    /// 0xC0 ticks and reaches (96, 96) at half size 32 ticks later, and stays there.
    #[test]
    fn the_tournament_place_slides_to_the_corner_and_halves() {
        assert_eq!(congrats_layout(500, false, 640.0, 480.0), (320.0, 240.0, 1.0));
        assert_eq!(congrats_layout(0xBF, true, 640.0, 480.0), (320.0, 240.0, 1.0));
        assert_eq!(congrats_layout(0xC0, true, 640.0, 480.0), (320.0, 240.0, 1.0));
        let (x, y, s) = congrats_layout(0xC0 + 16, true, 640.0, 480.0);
        assert_eq!((x, y), (208.0, 168.0));
        assert!((s - 0.75).abs() < 1e-6);
        assert_eq!(congrats_layout(0xC0 + 32, true, 640.0, 480.0), (96.0, 96.0, 0.5));
        assert_eq!(congrats_layout(0xC0 + 400, true, 640.0, 480.0), (96.0, 96.0, 0.5));
    }
}

#[cfg(test)]
mod cop_blip_tests {
    use super::Hud;

    /// 0x43333E…0x433377: a flip only when the tick is more than 0x20 past the stamp (signed), the
    /// stamp re-taken then; a new race's tick behind an old stamp waits it out.
    #[test]
    fn a_cop_blip_flips_on_the_draw_after_its_stamp_plus_0x20() {
        let mut st = (0xC000_00FFu32, 0i32);
        assert_eq!(Hud::cop_blip_step(&mut st, 0x20), 0xC000_00FF);
        assert_eq!(Hud::cop_blip_step(&mut st, 0x21), 0xC0FF_0000);
        assert_eq!(st.1, 0x21);
        assert_eq!(Hud::cop_blip_step(&mut st, 0x41), 0xC0FF_0000);
        assert_eq!(Hud::cop_blip_step(&mut st, 0x50), 0xC000_00FF);
        // the next race starts at tick 0 with the stamp at 0x50
        assert_eq!(Hud::cop_blip_step(&mut st, 5), 0xC000_00FF);
        assert_eq!(Hud::cop_blip_step(&mut st, 0x71), 0xC0FF_0000);
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::Hud;

    /// 0x41E480 / 0x41EFF0 / 0x41FB40 in constant pixels, checked against the original's frames
    /// (`RE/notes/hud.md` §2.2, 2026-10-01): HGear's double pass puts the gear tray at y 564 / 732 /
    /// 685 at 800×600 / 1024×768 / 1280×720, and nothing moves at 640×480.
    #[test]
    fn anchoring_in_pixels() {
        let at = |w: u32, h: u32, r: (f32, f32, f32, f32), passes: u8| {
            Hud::set_screen(w, h);
            let (ox, oy) = Hud::origin();
            let (x, y) = Hud::anchor_passes(r.0, r.1, r.2, r.3, passes);
            (x + ox, y + oy)
        };
        let gear = (555.0, 444.0, 85.0, 35.0);
        assert_eq!(at(640, 480, gear, 2), (555.0, 444.0));
        assert_eq!(at(800, 600, gear, 2), (715.0, 564.0));
        assert_eq!(at(1024, 768, gear, 2), (939.0, 732.0));
        assert_eq!(at(1280, 720, gear, 2), (1195.0, 685.0));
        // a box that straddles the centre line is re-centred (HRearview): W/2 - w/2
        assert_eq!(at(1024, 768, (185.0, 0.0, 240.0, 84.0), 1), (392.0, 0.0));
        // left / top elements stay
        assert_eq!(at(1280, 720, (0.0, 0.0, 127.0, 105.0), 1), (0.0, 0.0));
        Hud::set_screen(640, 480);
    }

    /// N01a: with a stored record the positions are `ftol(frac · W)` of its fields, not the derivation —
    /// the shipped "Config 2" @ 1600×1200 (`nfs5-2.hud` record 5): HTach y 1033 (the derivation 1023),
    /// HLeader x 129 (125), the others equal; no record: the derivation.
    #[test]
    fn a_stored_record_is_used_as_stored() {
        let l = &crate::hud::SCREEN_LAYOUTS[1];
        let mut s = [(0.0f32, 0.0f32); 13];
        s[4] = (0.92, 0.860_833_35);
        s[8] = (0.081_249_997, 0.0);
        s[9] = (0.425_000_01, 0.0);
        Hud::set_screen(1600, 1200);
        let (ox, oy) = Hud::origin();
        let at = |r: (f32, f32, f32, f32)| {
            let (x, y) = Hud::anchor_passes(r.0, r.1, r.2, r.3, 1);
            (x + ox, y + oy)
        };
        let (tach, leader, rear) = ((l.tach.0, l.tach.1, 128.0, 128.0), (l.leader.0, l.leader.1, 59.0, 85.0), (l.rearview.0, l.rearview.1, 240.0, 84.0));
        assert_eq!(at(tach), (1472.0, 1023.0), "derived");
        assert_eq!(at(leader), (125.0, 0.0));
        Hud::set_stored(1, Some(s));
        assert_eq!(at(tach), (1472.0, 1033.0), "stored");
        assert_eq!(at(leader), (129.0, 0.0));
        assert_eq!(at(rear), (680.0, 0.0));
        Hud::set_stored(1, None);
        assert_eq!(at(tach), (1472.0, 1023.0));
        Hud::set_screen(640, 480);
    }

    /// The pen of every HUD text after Thrash's float round trip (0x536A60, `Hud::snap_pen`), against the
    /// original's frames at four resolutions (P1): every static label's pen x and y (6 labels x 4 resolutions).
    #[test]
    fn text_pen_snap_matches_the_frames() {
        let snap = |x: f32, y: f32, w: f32, h: f32| Hud::snap_pen(x, y, w, h);
        // x: the time label (tray 427 + 5), position label, gear label, damage "0%"
        assert_eq!(snap(432.0, 16.0, 640.0, 480.0), (432.0, 15.0));
        assert_eq!(snap(591.0, 16.0, 800.0, 600.0), (590.0, 16.0));
        assert_eq!(snap(666.0, 62.0, 800.0, 600.0), (665.0, 61.0));
        assert_eq!(snap(720.0, 581.0, 800.0, 600.0), (719.0, 581.0));
        assert_eq!(snap(5.0, 60.0, 800.0, 600.0), (4.0, 59.0));
        assert_eq!(snap(816.0, 16.0, 1024.0, 768.0), (816.0, 16.0));
        assert_eq!(snap(1071.0, 16.0, 1280.0, 720.0), (1071.0, 15.0));
        assert_eq!(snap(1146.0, 62.0, 1280.0, 720.0), (1146.0, 61.0));
        assert_eq!(snap(5.0, 60.0, 1280.0, 720.0), (5.0, 60.0));
    }
}

impl App {
    /// N01a: tell the HUD which record's stored positions to use for the logical size `(w, h)` —
    /// 0x41F7A0 finds the record of `Display.ScreenLayout` at the screen's size and the HUD uses its
    /// fields as stored; none (0x41EBF0 would make it, 0x41EFF0 anchoring its copy): cleared, and
    /// `Hud::anchor_passes` derives the same thing at draw time.
    pub(crate) fn hud_stored_sync(&self, w: u32, h: u32) {
        self.hud_cfg_ensure();
        // the layout the draw picks (`SCREEN_LAYOUTS`, `Display.ScreenLayout`)
        let layout = self.choice_index("Display.ScreenLayout").map_or(0, |(i, _)| i.min(3));
        let cache = self.hud_cfg.borrow();
        let stored = cache.cfg.as_ref().and_then(|c| c.find(layout, w, h).map(|i| c.stored_positions(i)));
        Hud::set_stored(layout, stored);
    }
}
