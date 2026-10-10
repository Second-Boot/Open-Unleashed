//! Driving / collision sound effects from `Sounds/SND_coll.bnk` (`RE/notes/sfx.md`).
//!
//! * One-shots (§3–4): slot groups of three with a no-repeat random pick; level
//!   `64 + clamp(6.3·vol, 2, 63)` (scrapes `clamp(76·vol, 2, 127)`), gated per frame by
//!   `level >> 4 > gate` with the gate decaying by one per update. Wall hits: `vol = 3·j/8`
//!   (j = normal closing speed), scrapes `vol = speed/32`, car–car `vol = effect`, landings
//!   `vol = closing`. The wall material comes from the `.edg` word through the 0x49442C /
//!   0x4943BC tables (metal / wood / snow / ice / earth / rock; HIGH on the class order).
//! * Tyre squeal loops (§5): front / rear voices, slot by (tyre class, surface material), level
//!   `min(127·slipSum, 127)` (rear × 0.5), pitch 1.0. CONFIRMED.
//! * Gear-shift click: engine bank slot 2 (`engine::EngineSound::play_shift`).

use crate::mixer::{byte_gain, AudioOutput, Clip, ClipCache, VoiceId, BEND_ROOT, LEVEL_MAX};
use crate::voice_loop::{LoopCall, LoopVoice};
use nfs_formats::bnk::Bnk;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Material ids (`0x5D1610` table order) used by the slot rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Asphalt = 5,
    WetAsphalt = 6,
    Gravel = 7,
    Metal = 8,
    Wood = 9,
    Snow = 10,
    Ice = 11,
    Earth = 12,
    Rock = 13,
}

/// Surface nibble (`mt.flags & 0xF`) → material (`0x5CC4D8`, CONFIRMED).
pub fn surface_material(nibble: usize) -> Material {
    use Material::*;
    [Snow, Asphalt, Gravel, Earth, Rock, Earth, WetAsphalt, Wood, Ice, Snow, Asphalt, Metal, Gravel, Gravel, Gravel, Snow][nibble & 0xF]
}

/// `carInfo+0x1C0` → the slot table's column (`idA`), which is **not** the identity:
/// 0x40FD0C…0x40FD27 maps 0 → 0, **1 → 2** and **2 → 1**, so a race car (class 2) takes the
/// `RaceTires` column and class 1 the `RainTires` one. This port indexed the table with the raw
/// class until 2026-09-23, giving every race car the rain-tyre sample.
pub fn tyre_column(class: usize) -> usize {
    match class {
        0 => 0,
        1 => 2,
        _ => 1,
    }
}

/// Tyre squeal loop slot (§5.2) for a **column** (0 Tires, 1 RaceTires, 2 RainTires — see
/// [`tyre_column`]) on a surface.
pub fn squeal_slot(tyre_class: usize, m: Material) -> usize {
    use Material::*;
    match m {
        Asphalt | Metal | Wood | Rock => [0x2A, 0x2E, 0x2F][tyre_class.min(2)],
        WetAsphalt => 0x2D,
        Gravel => 0x29,
        Snow | Ice => 0x2B,
        Earth => 0x2C,
    }
}

/// `SND_coll` slots of the loops: the road roll's base (`0x11 + nibble`, §4.5) and the wind.
const SLOT_ROAD_BASE: usize = 0x11;
const SLOT_WIND: usize = 0x1F;
/// The turbo / blower (voice 0, §4.6).
const SLOT_TURBO: usize = 0x0F;
/// The first collision-group slot: from here 0x4A9A56 scales a one-shot by the tunnel and takes
/// the aux byte.
const FIRST_COLLISION_SLOT: usize = 0x30;
/// 0x4A9A56…0x4A9A90's `0x96 − reverb` and its divisor `150`.
const TUNNEL_REVERB_SPAN: i32 = 0x96;
const _: () = assert!(SLOT_ROAD_BASE == 0x11 && SLOT_WIND == 0x1F && FIRST_COLLISION_SLOT == GROUP_CAR_CAR && TUNNEL_REVERB_SPAN == 150);

/// One-shot slot group bases (§3).
pub const GROUP_CAR_CAR: usize = 0x30;
pub const GROUP_WALL_WOOD: usize = 0x33;
pub const GROUP_WALL_SNOW: usize = 0x36;
pub const GROUP_WALL_EARTH: usize = 0x39;
pub const GROUP_WALL_ROCK: usize = 0x3C;
pub const GROUP_LANDING: usize = 0x40;
pub const GROUP_LANDING_GRAVEL: usize = 0x46;
pub const GROUP_SCRAPE_METAL: usize = 0x60;
pub const GROUP_SCRAPE_ROCK: usize = 0x6C;
pub const SLOT_CRASH: usize = 0x20;
pub const GROUP_PYLON: usize = 0x56;
/// CarBody × Bush (`sfx.md` §3): three one-shots from 0x7C.
pub const GROUP_BUSH: usize = 0x7C;

/// One entry of a car's sound queue `car+0x1110` (0x40F020: `{type, chan, idA, idB, magnitude,
/// 0}`, at most 7, count `car+0x110C`) as this port keeps it — the one-shots by the event that
/// produced them (their pair and level are worked out as the player's are), the loops by their
/// sample. See [`SfxQueue`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueuedSfx {
    /// 0x40F740, a wall hit (`0x40008 + class`): `j` the closing speed, `car+0x430 = 3·j`
    WallHit { j: f32, class: u8 },
    /// 0x40F740, a wall scrape (`0x40015 + class`): `car+0x430 = speedEst/4`
    WallScrape { speed: f32, class: u8 },
    /// 0x40F740, CarBody × CarBody (`0x50003` with `car+0x434` a car): `car+0x430` = the effect
    CarCar { effect: f32 },
    /// 0x40F740, a car–car contact whose other body is a smackable: `car+0x438 = 0x50000 |
    /// smackable+0x44C` with `car+0x434` set (0x496CA0…0x496CBD), so `idB` = the smackable's
    /// material id (0x40FA38) and `vol = 0x430` (0x40FA2E) — a cone is CarBody × Pylon
    Smackable { effect: f32, material: u32 },
    /// 0x40F740, CarBody × a track prop (`0x50000 | class`, no other car)
    Prop { impact: f32, class: u32 },
    /// 0x40F740, CarBody × Bush (`0x6000F` at 15)
    Bush,
    /// 0x40F740, the body–ground contact (`0x30000 | car+0x464`, `GroundImpulse` 0x496104):
    /// `effect` = `car+0x430 = 4·|vel·n|` (0x496844), `nibble` = `car+0x464` → idB `0x5CC4D8[·]`
    Ground { effect: f32, nibble: usize },
    /// `Car_ground`'s hard landing, `0x40F020(car, 1, −1, 0 Tires, 5 Asphalt, closing, 0)`
    /// (0x49C1D6…0x49C1E6) — Asphalt whatever the surface
    Landing { closing: f32 },
    /// `Damage`'s crash (0x494500) or `Car_ground`'s roll-over (0x49C0CE…0x49C0DE, HeadLight ×
    /// CarBody at 40): slot 0x20 at full level
    Crash,
    /// A loop entry (`chan ≥ 0`) from 0x40FBC0 / 0x40FFF0: `chan` 0x12 front / 0x14 rear, the
    /// `SND_coll` loop slot the pair maps to (0x4A93C0's table, §5.2) and the magnitude — 0 for
    /// the stop entries (types 3 and 5), which 0x4A93C0 turns into a release.
    Loop { chan: u8, sample: usize, v: f32 },
}

/// The per-car loop producers' inputs (0x40FFF0 → 0x40FBC0), in the car's fields.
#[derive(Debug, Clone, Copy)]
pub struct LoopInput {
    /// `car+0x3A0`, the cosine of the tilt
    pub cos_tilt: f32,
    /// `car+0x424`, the body's height above the contact plane
    pub body_height: f32,
    /// `car+0x35C`, the speed estimate
    pub speed_est: f32,
    /// `wheel+0xB8` of the front pair summed (`car+0x870 + car+0x934`) and of the rear
    /// (`car+0x9F8 + car+0xABC`) — only `Car_simulate` 0x49FD30 writes them, so a car driven by
    /// 0x408360 (traffic, cops) has zero
    pub front_slip: f32,
    pub rear_slip: f32,
    /// `car+0x78`: 0 for a racer, else 0x417190's distance to the nearest racer
    pub racer_distance: f32,
    /// `carInfo(car+0x760)+0x1C0`, the tyre class (→ [`tyre_column`])
    pub tyre_class: usize,
    /// `car+0x464`, the surface nibble (`nfs_physics::vehicle::Vehicle::surface_nibble`)
    pub surface_nibble: usize,
    /// 0x40FBC8…0x40FC0D: `[0x606AE0]` — set by the race load 0x414DB0 only when the track is
    /// `autobahn` (0x414DFF…0x414E11, `_stricmp` 0x5AE3C0 with 0x5CC7F8) — and **nothing overhead**:
    /// the enclosure test 0x473330(car+8, car+0x330, 0), the flags word of the lowest patch more
    /// than 3.5 m above the car, answers 0. Asphalt then squeals as WetAsphalt
    pub wet: bool,
    /// `car+0x52C & 4`, the human
    pub human: bool,
}

/// A car's sound queue `car+0x1110` / `car+0x110C` (0x40F020) and the loop-active bits
/// `car+0xE00` the producers keep in it.
///
/// The producers push; the audio pass drains the queue of every car that holds an audio slot
/// (0x4A73DE inside 0x4A7080, [`CollisionSfx::drain_ai`]) and `0x498220`, at the head of the
/// next even tick, zeroes every live car's count (0x498240) — so an entry is heard once, or never
/// when its car holds no slot. The cadence matches: the original pushes on the even (physics)
/// tick and drains on the odd one that follows (the audio update 0x4A8120, `timing.md` §7), and
/// so does the port since its audio block runs on the odd tick only (2026-09-26). *(Until then
/// it drained in the 64 Hz block of the same even tick, and this said so; earlier still it said
/// "approximation: the port drains once per frame" — wrong, the drain sits inside the tick loop.)*
#[derive(Debug, Clone, Default)]
pub struct SfxQueue {
    entries: Vec<QueuedSfx>,
    /// `car+0xE00`: bit 0 front squeal, bit 1 rear squeal, bit 2 roof scrape — kept with the
    /// original's arithmetic (`dec`, `add −2`, `add −4`), which the roof-scrape stop arm
    /// (0x410269) applies without testing the bit
    loop_bits: u32,
}

impl SfxQueue {
    /// 0x40F020's capacity: an entry is dropped when the count is already 7 (`cmp 6; jg`, 0x40F02A).
    pub const CAP: usize = 7;

    pub fn push(&mut self, e: QueuedSfx) {
        if self.entries.len() < Self::CAP {
            self.entries.push(e);
        }
    }

    /// 0x40F740's gate for the collision one-shot: it is pushed only when `car+0x430 > 1.5`
    /// (0x40F952). The landing and the crash are pushed straight (0x49C1E6, 0x494500).
    pub fn push_impact(&mut self, e: QueuedSfx) {
        let mag = match e {
            QueuedSfx::WallHit { j, .. } => 3.0 * j,
            QueuedSfx::WallScrape { speed, .. } => speed * 0.25,
            QueuedSfx::CarCar { effect } => effect,
            QueuedSfx::Smackable { effect, .. } => effect,
            QueuedSfx::Prop { impact, .. } => impact,
            QueuedSfx::Bush => 15.0,
            QueuedSfx::Ground { effect, .. } => effect,
            _ => f32::INFINITY,
        };
        if mag > 1.5 {
            self.push(e);
        }
    }

    pub fn entries(&self) -> &[QueuedSfx] {
        &self.entries
    }

    /// `0x498220`: the count to zero (0x498240).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// `Car_bodyScrapeSfx` **0x40FFF0**, which runs the squeal producer **0x40FBC0** when the car
    /// is upright — read to the instruction 2026-09-26, CONFIRMED. `rand16` draws the LCG value the
    /// roof scrape makes (`[0x5D1028] · [0x5D102C]`, `>> 8 & 0xFFFF`, 0x4100F8…0x41011E).
    ///
    /// On its side or roof (`!(car+0x3A0 > 0.2)`, 0x40FFFB): with `car+0x424 < 0.2` and
    /// `v = car+0x35C / 4 > 0.1`, push `{4, 0x14, CarBody, surf, v − rand16·v/3/65536}` and set
    /// bit 2 (0x410142…0x4101CB); otherwise push the stop `{5, 0x14, CarBody, Asphalt, 0}` and
    /// take 4 off the bits (0x4101E5…0x410269) — whether bit 2 was set or not. Upright: the stop
    /// and `−4` when bit 2 is set (0x41000C…0x41009D), then 0x40FBC0:
    ///
    /// * enabled when `car+0x78 < 60`, `car+0x424 < 0.1` and `car+0x3A0 > 0.5` (0x40FC12…0x40FC54);
    ///   then `f = F > 0 ? min(F, 10) : 0` and `r = R > 0 ? min(R, 10) : 0` — **independently**:
    ///   `f` stays on the FPU stack (0x40FC80 / 0x40FC84) while `r` is written over both stack
    ///   slots `[esp+0x18]` / `[esp+0x10]` (0x40FCC0…0x40FCCA), and the front push takes the FPU
    ///   copy (0x40FD29…0x40FDAC). Disabled, both 0.
    /// * `surf = 0x5CC4D8[car+0x464]`, Asphalt → WetAsphalt on the autobahn out in the open
    ///   ([`LoopInput::wet`], 0x40FCEC…0x40FCFC); `idA` = [`tyre_column`] (0x40FD0C…0x40FD27).
    /// * front: `f == 0` ⇒ with bit 0 set, the stop `{3, 0x12, idA, surf, 0}` and `bits −= 1`
    ///   (0x40FDF8…0x40FE84); else when `f > 0` or the human, `{2, 0x12, idA, surf, f}` and bit 0.
    /// * rear: `r == 0` ⇒ with bit 1 set, `{5, 0x14, idA, surf, 0}` and `bits −= 2`
    ///   (0x40FF54…0x40FFDA); else when `r > 0` or the human, `{4, 0x14, idA, surf, r/2}` and
    ///   bit 1 (0x40FEC0…0x40FF49).
    ///
    /// The bits are set even when the queue is full (0x40FDE3, 0x40FF40, 0x4101D3).
    pub fn produce_loops(&mut self, c: &LoopInput, rand16: impl FnOnce() -> u32) {
        let surf = surface_material(c.surface_nibble);
        if !(c.cos_tilt > 0.2) {
            let v = c.speed_est * 0.25;
            if c.body_height < 0.2 && v > 0.1 {
                let mag = v - rand16() as f32 * v * (1.0 / 3.0) / 65536.0;
                self.push(QueuedSfx::Loop { chan: 0x14, sample: body_scrape_slot(surf), v: mag });
                self.loop_bits |= 4;
            } else {
                self.push(QueuedSfx::Loop { chan: 0x14, sample: body_scrape_slot(Material::Asphalt), v: 0.0 });
                self.loop_bits = self.loop_bits.wrapping_sub(4);
            }
            return;
        }
        if self.loop_bits & 4 != 0 {
            self.push(QueuedSfx::Loop { chan: 0x14, sample: body_scrape_slot(Material::Asphalt), v: 0.0 });
            self.loop_bits = self.loop_bits.wrapping_sub(4);
        }
        let enabled = c.racer_distance < 60.0 && c.body_height < 0.1 && c.cos_tilt > 0.5;
        let clamp = |x: f32| if x > 0.0 { x.min(10.0) } else { 0.0 };
        let (f, r) = if enabled { (clamp(c.front_slip), clamp(c.rear_slip)) } else { (0.0, 0.0) };
        let m = match surf {
            Material::Asphalt if c.wet => Material::WetAsphalt,
            other => other,
        };
        let sample = squeal_slot(tyre_column(c.tyre_class), m);
        if f == 0.0 {
            if self.loop_bits & 1 != 0 {
                self.push(QueuedSfx::Loop { chan: 0x12, sample, v: 0.0 });
                self.loop_bits = self.loop_bits.wrapping_sub(1);
            }
        } else if f > 0.0 || c.human {
            self.push(QueuedSfx::Loop { chan: 0x12, sample, v: f });
            self.loop_bits |= 1;
        }
        if r == 0.0 {
            if self.loop_bits & 2 != 0 {
                self.push(QueuedSfx::Loop { chan: 0x14, sample, v: 0.0 });
                self.loop_bits = self.loop_bits.wrapping_sub(2);
            }
        } else if r > 0.0 || c.human {
            self.push(QueuedSfx::Loop { chan: 0x14, sample, v: r * 0.5 });
            self.loop_bits |= 2;
        }
    }
}

/// The wind's two random walks, `[0x5D1838]` / `[0x5D1840]` (the level and its step) and
/// `[0x5D183C]` / `[0x5D1844]` (the 16.16 pitch and its step), stepped once per wind update by
/// 0x4AA4EB…0x4AA5A5. The image's values start them — 0x28 / −1 and 0xFA24 / −1 — and only
/// 0x4AA050 writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindWalk {
    pub level: i32,
    pub level_step: i32,
    pub pitch: i32,
    pub pitch_step: i32,
}

impl Default for WindWalk {
    fn default() -> Self {
        Self { level: 0x28, level_step: -1, pitch: 0xFA24, pitch_step: -1 }
    }
}

impl WindWalk {
    /// One step: both walks move by their steps (0x4AA4FC…0x4AA50E); then the level is clamped
    /// into 0x28…0x7F and the pitch into 0xFA24…0x105DC (signed `jle` / `jge`), each clamp drawing
    /// its new step from `draw` — 0x538040 — the level's first: `−1 − (r & 7)` (0x4AA520…0x4AA52A),
    /// `(r & 7) + 1` (0x4AA544…0x4AA54D), `−20 − r % 80` (0x4AA566…0x4AA57B), `r % 50 + 10`
    /// (0x4AA594…0x4AA5A5; `div`, unsigned).
    pub fn step(&mut self, draw: &mut impl FnMut() -> u32) {
        self.level = self.level.wrapping_add(self.level_step);
        self.pitch = self.pitch.wrapping_add(self.pitch_step);
        if self.level > LEVEL_MAX {
            self.level = LEVEL_MAX;
            self.level_step = -1 - (draw() & 7) as i32;
        }
        if self.level < 0x28 {
            self.level = 0x28;
            self.level_step = (draw() & 7) as i32 + 1;
        }
        if self.pitch > 0x1_05DC {
            self.pitch = 0x1_05DC;
            self.pitch_step = -20 - (draw() % 80) as i32;
        }
        if self.pitch < 0xFA24 {
            self.pitch = 0xFA24;
            self.pitch_step = (draw() % 50) as i32 + 10;
        }
    }
}

pub struct CollisionSfx {
    /// What `Voice_play`'s update of a playing voice hands the strip hook this pass
    /// ([`crate::mixer::voice_play_filter`], 0x4A9FA8…0x4AA03B): set by the caller every audio update.
    pub filter: Option<i32>,
    bank: Bnk,
    data: Vec<u8>,
    clips: ClipCache<usize>,
    /// last random pick per group base (no-repeat rule 0x4A9380)
    last_pick: HashMap<usize, u8>,
    rng: u32,
    /// per-frame level gate `0x656860` (level >> 4)
    gate: i32,
    front: Option<LoopVoice>,
    rear: Option<LoopVoice>,
    /// road / tyre roll loop (voice 0x18, §4.4) and the slot it is playing
    road: Option<LoopVoice>,
    /// wind loop (voice 0x1A, slot 0x1F, §4.5)
    wind: Option<LoopVoice>,
    /// the wind's two random walks ([`WindWalk`])
    wind_walk: WindWalk,
    /// turbo / blower loop (voice 0, slot 0x0F, §4.6)
    turbo: Option<LoopVoice>,
    /// the police sirens by audio slot (`0x6562E8[slot]`, started by 0x4AAE90)
    sirens: HashMap<usize, VoiceId>,
    /// `game.spl[5]` `[0x5D14C4]` — the siren's mix level
    siren_trim: u32,
    /// `game.spl` — the category trims of 0x4A98E0 / 0x4A9C10
    spl: nfs_formats::spl::GameSpl,
    /// `SfxLevel [0x5D1744]`, `EngineLevel [0x5D1748]` (0..127) and the reverb level `[0x656869]`
    sfx_level: u8,
    engine_level: u8,
    reverb: i32,
    /// the queue's distance attenuation for the player car (0x4A93D0…0x4A9420): `512/(d+2)`, or
    /// `4 · 2048/(d+1)` on view 0x0C — the volume of the one-shots and the base of the squeal's
    atten: i32,
    /// the pan the queue's loop voices take (0x4A93C0's sixth argument, the 16-bit angle of
    /// `mixer::pan_angle`): the car's own pan angle (see [`CollisionSfx::set_levels`])
    pan: u16,
    /// the pan the queue's one-shots take: for the player the angle to the car's last contact
    /// point `car+0x440` (0x4A7BB1…0x4A7C3B, [`CollisionSfx::set_one_shot_pan`])
    one_shot_pan: u16,
    /// the slot cars' loop voices `slot + 0x22` by audio slot: the voice, the `SND_coll` slot it
    /// plays (`[0x656184 + 8·voice]`) and the bend byte latched when it started (`0x5D17E8[voice]`)
    ai_loops: HashMap<usize, LoopVoice>,
}

/// 0x4A98E0 / 0x4A9C10's mixer volume `(categoryLevel · game.spl[k] · vol · 85) >> 21` — `imul`s,
/// `shl 4 ; add ; lea ·5` for the 85, `sar 0x15` (0x4A9E96…0x4A9EB0) — the int `setVolume`
/// 0x566340 stores `<< 16` in `voice+0x2C` (0x5662E3, no clamp). This is where the Audio options'
/// levels reach a voice; the library's master `[0x6B499D]` stays 0x7F (`mixer::level_byte`). The
/// port hands the mixer `byte / 127`, which its `round(v · 127)` gives back exactly. *(Until
/// 2026-09-27 the port multiplied `vol / 127` by the float `level · trim · 85 / 2²¹`, rounding
/// what the original truncates — a byte high about half the time.)*
fn category(level: u8, trim: u32, vol: i32) -> i32 {
    (level as i32).wrapping_mul(trim as i32).wrapping_mul(vol).wrapping_mul(85) >> 21
}

/// The siren's distance term (0x4A72F3…0x4A7309 then 0x4A7607…0x4A7626): the car's distance to the
/// listener in whole metres, squared and capped at 0x1324 (70²), turned into
/// `(0x1324 − d) · 127 / 0x1324` — full level on top of the listener, silent from 70 m. The AI
/// horn uses the same term (0x4A7554…0x4A7592).
pub fn siren_volume(distance: f32) -> i32 {
    let m = (distance as i32).clamp(0, 71);
    let d = (m * m).min(0x1324);
    (0x1324 - d) * 127 / 0x1324
}

/// `Voice_play`'s pitch 0x10000 (the sample at its root) as the word `>> 4` — what 0x4A93C0's loops
/// (0x4A984C) and every `pitch 1.0` call hand.
const WORD_UNIT: u16 = 0x1000;

/// **The turbo's volume and 16.16 pitch** (0x4AA74A…0x4AA831), on the x87 stack:
/// * `f0 = (int)throttle · boost · f32(1/127.5)` (`fild`, `fmul`, the f32 0.0078431377 at 0x5B4B50);
/// * `g`: when `0.5 · redline < rpm` (`fcomp`, `test ah, 1`) `x = 0.9 · redline − rpm` over
///   `0.9 · redline` (the f32 0.9 at 0x5B25E8), else `x = 0.5 · rpm − 0.25 · redline` over
///   `0.25 · redline`; `x` below 0 is 0 (`test ah, 0x41`); the product is `f0 · (x / den)`;
/// * the pitch `(rpm · 2.5 / redline + 0.5) · (int)pitchBase` (`fimul`), stored as an f32
///   (`fstp dword`), then `fistp` — rounded to nearest (even), the default control word;
/// * the volume `(int)lvl · f0 · g` stored as an f32, `fistp`, then `min(·, 0x7F)` and `≤ 0 → 0`
///   (0x4AA807…0x4AA82B).
///
/// APPROXIMATION (narrowed 2026-10-02, WP-AN): this code runs on the game's **main thread**, the one
/// that creates the DirectX 7 device. `dx7z.dll` passes `SetCooperativeLevel` flags 0x408 / 0x411
/// only (0x600030CE…0x600030EA, 0x600034F0…0x60003523 → the calls 0x6000226F / 0x60003165), never
/// `DDSCL_FPUPRESERVE` 0x1000 / `FPUSETUP` 0x800, so on Microsoft's DirectX 7 the runtime's default
/// applies — single precision for that thread (ddraw.dll's documented behaviour; it is not in the
/// shipped files, so not verifiable here) — while Wine keeps 0x027F (measured below). The mixer
/// side (`resample.rs`) is on its own worker thread and is settled at 53-bit. Here the x87's
/// intermediate precision is taken as f64 (the f32 stores, which round far
/// coarser, are kept). Narrowed 2026-09-28 (`career.md` §11.4a, `RE/tools/fpucw.py`): every real
/// `fldcw`/`fnstcw` in `.text` was surveyed (59 sites; ~680 more are IAT/global address bytes
/// mis-decoded, not instructions) and none is in this file's own code, `sfx.rs`'s callers, or the
/// audio library 0x596B60…0x59D350 at all — nothing here ever sets or restores precision itself.
/// What remains open is only the ambient word the process starts under and what DirectX 7's own
/// runtime (not in the shipped files) leaves it at, unchanged since before this survey.
///
/// **Measured 2026-09-28** (`career.md` §11.4c): nine `gdb`-attach samples of the live process
/// on a Debian test machine, under both `dx7z.dll` and the repack's default `opengl3` driver,
/// all read control word `0x027F` — PC = 53-bit (double), matching the `f64` reading above. Not
/// promoted to matched: wine's own driver stack, not Microsoft's DX7 on Windows, and no sample
/// was inside a live tick (the front end could not be driven to a race in the time available).
///
/// **In-tick samples, 2026-10-01 (SV, `fe = 0` direct launches, gdb breakpoints inside the live race
/// loop — the tyre code 0x4A1F25 in the physics tick, once per wheel per tick, ~400 000 hits; the
/// engine-audio block allocation 0x4ACC6E):** every sample reads `0x027F` (PC = 53-bit double,
/// rounding nearest), six of them inside live race ticks on coastal, alps and canyon — the `f64`
/// reading above is what the running original uses under Wine. Still Wine's stack, not Microsoft's
/// DX7; the row stays on that one reservation.
pub fn turbo_terms(boost: f32, throttle: u8, rpm: f32, redline: f32, lvl: i32, pitch_base: i32) -> (i32, i32) {
    let (b, r, dbs) = (boost as f64, redline as f64, rpm as f64);
    let f0 = throttle as f64 * b * (1.0f32 / 127.5) as f64;
    let (x, den) = if 0.5 * r < dbs {
        let den = r * 0.9f32 as f64;
        (den - dbs, den)
    } else {
        let den = r * 0.25;
        (dbs * 0.5 - den, den)
    };
    let x = if x >= 0.0 { x } else { 0.0 };
    let fg = f0 * (x / den);
    let pitch = ((dbs * 2.5 / r + 0.5) * pitch_base as f64) as f32;
    let vol = ((lvl as f64 * fg) as f32).round_ties_even() as i32;
    let vol = vol.min(LEVEL_MAX).max(0);
    (vol, pitch.round_ties_even() as i32)
}

/// The siren's start: the word of 0x4AAE90's pitch 0x10000 (`>> 4`, 0x4A9AC5), which no later
/// call changes.
const SIREN_WORD: u16 = 0x1000;

/// The siren's bend byte at the start, `0x5D1878[slot]` — 0x20 for every slot in the image, and
/// 0x4AAF50 writes 0x20 again at every release; no other writer (byte scan, `sfx.md` §6).
const SIREN_START_BEND: u8 = 0x20;

/// 0x4AAFB0's bend byte (0x4AB033…0x4AB056): `pitchBase << 6`, `>> 16` toward zero (`cdq ; and
/// edx, 0xFFFF ; add ; sar 16`), negative → 0 (`sets ; dec ; and`), capped at 0x7F — 0x40, no
/// bend, at the still listener's `pitchBase` 0x10000.
pub fn siren_bend(pitch_base: i32) -> u8 {
    let x = pitch_base.wrapping_shl(6);
    let q = (x + ((x >> 31) & 0xFFFF)) >> 16;
    q.clamp(0, LEVEL_MAX) as u8
}

/// The siren sample 0x4AAE90 picks: slot 9 for a car with `0x52C & 0x40` (no cop has it in this
/// build, `traffic.md` §17), otherwise 6 for an odd audio slot and 8 for an even one.
pub fn siren_slot(audio_slot: usize, alt: bool) -> usize {
    if alt {
        9
    } else if audio_slot & 1 == 1 {
        6
    } else {
        8
    }
}

/// The **CarBody** column of §5.2's slot table — what the body sounds like dragged over a surface:
/// Gravel 0x29, Snow and Ice 0x2B, Earth 0x2C, everything else (Asphalt, WetAsphalt, Metal, Wood,
/// Rock) 0x28.
pub fn body_scrape_slot(m: Material) -> usize {
    use Material::*;
    match m {
        Gravel => 0x29,
        Snow | Ice => 0x2B,
        Earth => 0x2C,
        _ => 0x28,
    }
}

/// The **CarBody** row of 0x4A8E60 (0x4A8FF8…0x4A9302, CONFIRMED 2026-09-26): the group base of the
/// pair CarBody × `idb`. Rock / Asphalt / Gravel 0x3C, Snow / Ice 0x36, Earth 0x39, Wood 0x33,
/// Metal and CarBody 0x30, WoodSign 0x70, MetalSign 0x73, Glass 0x76, WoodBox 0x79, Pylon 0x56,
/// Tree 0x53, Bush 0x7C, the scrapes 0x60 / 0x63 / 0x66 (Snow and Ice) / 0x69 / 0x6C, Tires 0x40.
/// Anything else — WetAsphalt among them — has no case: the slot stays 0 (`xor eax, eax` at
/// 0x4A8E6A, 0x4A92F2 `jne 0x4A9345`). Whether 0x4A9C10 then sounds slot 0 is not traced; the port
/// plays nothing, as for the Tires row's gaps ([`landing_group`]).
pub fn carbody_group(idb: u32) -> Option<usize> {
    Some(match idb {
        13 | 5 | 7 => 0x3C,
        10 | 11 => 0x36,
        12 => GROUP_WALL_EARTH,
        9 => 0x33,
        8 | 3 => 0x30,
        19 => 0x70,
        18 => 0x73,
        17 => 0x76,
        20 => 0x79,
        16 => 0x56,
        14 => 0x53,
        15 => 0x7C,
        21 => 0x60,
        22 => 0x63,
        23 | 24 => 0x66,
        25 => 0x69,
        26 => 0x6C,
        0 => 0x40,
        _ => return None,
    })
}

/// The landing group for a surface (`0x4A8E8A…0x4A8FF8`, the `idA == 0 Tires` arm of 0x4A8E60):
/// Asphalt, Metal, Wood and Tires share 0x40, Snow takes 0x43, Gravel 0x46, Earth 0x4C and Ice
/// 0x50. **Rock and WetAsphalt are not in the arm at all** — it falls through with the slot still
/// 0, so a landing there is silent. This port played the hard-ground group on every surface until
/// 2026-09-23.
pub fn landing_group(m: Material) -> Option<usize> {
    use Material::*;
    match m {
        Asphalt | Metal | Wood => Some(GROUP_LANDING),
        Snow => Some(0x43),
        Gravel => Some(GROUP_LANDING_GRAVEL),
        Earth => Some(0x4C),
        Ice => Some(0x50),
        Rock | WetAsphalt => None,
    }
}

/// `V` of §4.4 (0x4AA0BA…0x4AA106): `(int)(|car+0xD60| · k)`, `k` 1 on asphalt (`car+0x464 == 1`)
/// and 3 elsewhere — the factor inside the truncation (the port multiplied the whole metres).
pub fn road_v(speed: f32, nibble: usize) -> i32 {
    (speed.abs() * if nibble == 1 { 1.0 } else { 3.0 }) as i32
}

/// §4.4's road-layer volume (0x4AA393…0x4AA3F1): `edi = half ? V >> 1 : V`, then
/// `min((A · max(edi − 15, 0)) >> 7, 122)`.
pub fn road_volume(a: i32, v: i32, half: bool) -> i32 {
    let edi = if half { v >> 1 } else { v };
    ((a * (edi - 15).max(0)) >> 7).min(122)
}

/// 0x4A93C0's distance attenuation (0x4A93D0…0x4A9420): `0x2000000 / (d + 0x20000)` in 16.16 —
/// `512/(d+2)` — or on view 0x0C `4 · 0x8000000 / (d + 0x10000)` = `4 · 2048/(d+1)`.
/// The `· 4` is applied before the floor (0x4A93F1 `shl eax, 2` ahead of the `>> 16`,
/// `crate::fix_atten`); the port took `4 · floor(2048/(d+1))` until 2026-09-26.
fn atten_for(d: f32, view_0c: bool) -> i32 {
    if view_0c { crate::ATTEN_VIEW_0C.at(d) } else { crate::ATTEN_ONE_SHOT.at(d) }
}

impl CollisionSfx {
    pub fn load(sounds_dir: &Path) -> anyhow::Result<Self> {
        let path = sounds_dir.join("SND_coll.bnk");
        let data = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let bank = Bnk::parse(&data)?;
        tracing::info!(slots = bank.slot_count(), "collision bank loaded");
        // `game.spl` read once for both
        let spl = nfs_formats::spl::GameSpl::load(sounds_dir);
        Ok(Self {
            filter: None,
            bank,
            data,
            clips: ClipCache::new(false),
            last_pick: HashMap::new(),
            rng: 0x2545_F491,
            gate: 0,
            front: None,
            rear: None,
            road: None,
            wind: None,
            // the image's `[0x5D1838]` / `[0x5D1840]` / `[0x5D183C]` / `[0x5D1844]`
            wind_walk: WindWalk::default(),
            turbo: None,
            sirens: HashMap::new(),
            siren_trim: spl.loops_6564d0(),
            spl,
            sfx_level: 127,
            engine_level: 127,
            reverb: 8,
            atten: 256,
            pan: 0,
            one_shot_pan: 0,
            ai_loops: HashMap::new(),
        })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        let (bank, data) = (&self.bank, &self.data);
        self.clips.get_or_load(slot, || Some(Arc::new(Clip::from_pcm(bank.decode_slot(data, slot).ok()?))))
    }

    fn rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(0x0001_9660).wrapping_add(0x3C6E_F35F);
        self.rng >> 8
    }

    /// A 16-bit draw (`>> 8 & 0xFFFF`, the shape of 0x41010C…0x41010F's) for the roof-scrape
    /// producer, from this bank's own generator (see [`SfxQueue::produce_loops`]).
    pub fn rand16(&mut self) -> u32 {
        self.rand() & 0xFFFF
    }

    /// `base + rand % 3`, never the previous pick of that group — except CarBody × Earth (0x39),
    /// whose arm 0x4A9048…0x4A9058 takes the bare `rand() % 3` with no remembered pick.
    fn pick(&mut self, base: usize) -> usize {
        if base == GROUP_WALL_EARTH {
            return base + (self.rand() % 3) as usize;
        }
        let prev = self.last_pick.get(&base).copied();
        let mut r = (self.rand() % 3) as u8;
        if prev == Some(r) {
            r = (r + 1 + (self.rand() % 2) as u8) % 3;
        }
        self.last_pick.insert(base, r);
        base + r as usize
    }

    /// The sliders, the tunnel reverb level (`reverb.rs`) the categories read, and the listener's
    /// distance to the player car with whether the view is 0x0C, for the queue's attenuation, and
    /// the car's pan.
    ///
    /// *Read 2026-09-26 (0x4A7660, 0x4A7080, 0x4A93C0):* a queue entry is `{type, chan, idA, idB,
    /// magnitude, 0}` (0x40F020) and carries **no point** — every event the drain 0x4A7AF8 hands to
    /// 0x4A93C0 is attenuated by the distance of the **car that owns the queue**, `|car+0x330 −
    /// listener 0x655E50|` in 16.16 (0x4A78E8…0x4A7909 → `[esp+0x18]` → 0x4A93C0's fifth argument
    /// at 0x4A7C4B / 0x4A7C68), through `512/(d+2)` (view 0x0C: `4 · 2048/(d+1)`, 0x4A93D0…0x4A9420).
    /// So the earlier "approximation: the queue measures to each event's own point" was wrong and
    /// is withdrawn: `distance` here is exactly the original's. What differs per event is the
    /// **pan** (the sixth argument): a loop (`chan ≥ 0`) takes the car's own pan angle (`ebp`,
    /// from `car+0x330`, 0x4A7862…0x4A78E5; 0x4A7C61), a one-shot (`chan < 0`) the angle to the
    /// car's last contact point `car+0x440` (0x4A7BB1…0x4A7C3B) — unless `[0x657404] == 1` (split
    /// screen) or its `idA` is GearShift 0x1C / BackFire 0x1E, which take the car's pan too.
    /// Both reach the voice (0x4A94D5…0x4A94EA, 0x4A983A…0x4A9854). This port had neither pan.
    /// `pan` here is the loops'; the one-shots' comes from [`Self::set_one_shot_pan`], called once
    /// by [`Self::drain_player`].
    /// *(2026-09-26: the "approximation: every one-shot takes the car's pan" that stood here is
    /// withdrawn — `nfs_physics::vehicle::Vehicle::last_contact` now carries `car+0x440`.)*
    pub fn set_levels(&mut self, sfx_level: u8, engine_level: u8, reverb: i32, distance: f32, view_0c: bool, pan: u16) {
        self.sfx_level = sfx_level;
        self.engine_level = engine_level;
        self.reverb = reverb;
        let d = distance.max(0.0);
        self.atten = atten_for(d, view_0c);
        self.pan = pan;
    }

    /// The one-shots' pan (0x4A93C0's sixth argument for `chan < 0`): the player's drain measures
    /// it to the car's last contact point `car+0x440` (0x4A7BB1…0x4A7C3B: `atan2` of the offset
    /// from the listener 0x655E50 dotted with its axes 0x655E5C / 0x655E74, negated under `MIRROR`
    /// 0x4A7C1A) — the same angle as the car's own (0x4A7862…0x4A78E5) with the point swapped.
    /// Called once by [`Self::drain_player`], with `car+0x440` as it stands at the drain (after
    /// the whole tick's physics) — not per event: see that method. *(Until 2026-09-27 this was
    /// called once per one-shot, as its event happened, each against `car+0x440`'s value at that
    /// moment — an approximation now withdrawn: `main.rs`'s collision block queues the one-shots
    /// instead of playing them, so they all reach this the same way the loops do.)*
    pub fn set_one_shot_pan(&mut self, pan: u16) {
        self.one_shot_pan = pan;
    }

    /// The default category, `SfxLevel · game.spl[9]` — everything in this bank but the slots below.
    fn sfx_volume(&self, vol: i32) -> f32 {
        byte_gain(category(self.sfx_level, self.spl.sfx(), vol))
    }

    /// A one-shot's volume byte (0x4A98E0): the default category (0x4A9A49…0x4A9A59, stored as
    /// the request's byte), and for slots from 0x30 (the collision groups) that byte further
    /// scaled by `(0x96 − reverb) · (i8)byte / 150` (0x4A9A56…0x4A9A90: `imul`, the magic
    /// 0x1B4E81B5 `sar 4`, the sign bit added back — toward zero), so a hit in a tunnel (reverb
    /// 100) is a third as loud as outside (8).
    fn one_shot_volume(&self, slot: usize, vol: i32) -> f32 {
        let b = category(self.sfx_level, self.spl.sfx(), vol) as i8 as i32;
        let b = if slot >= FIRST_COLLISION_SLOT { ((TUNNEL_REVERB_SPAN - self.reverb) * b / TUNNEL_REVERB_SPAN) as i8 as i32 } else { b };
        byte_gain(b)
    }

    /// Call once per audio update: the one-shot gate decays by one.
    pub fn update(&mut self) {
        self.gate = (self.gate - 1).max(0);
    }

    /// The collision one-shot of 0x4A93C0 (chan −1): `level` decides the gate; the call is
    /// `Voice_play(0x33, slot, bend 0x40, 0x10000, vol = atten, pan, aux = level)` (0x4A94D5…0x4A94EA).
    /// The aux byte goes two ways in 0x4A98E0: `0x7F − level` into the request's `+0xA` →
    /// `voice+0x45` (0x4A9A5F, slots from 0x30, → 0x5906D7), whose consumer is 0x589D70, the
    /// voice's **effect send** — `+0x45 · +0x44` (the patch's code 0x13) times the environment bus
    /// over 127·127 — so a hard hit is heard nearly dry in a tunnel (`AudioOutput::set_fx_aux`);
    /// and, for every slot, the request's `+6` (0x4A9B89…0x4A9B90), which the patch start tests
    /// against each record's codes 0x01 / 0x02 and multiplies the patch volume by, over 127
    /// (0x5905D0 → `voice+0x3C`, a factor of the voice level at 0x589E1C) — the port passes it as
    /// `sel` (`AudioOutput::play_panned_sel`). *(Until 2026-09-27 this said the level was "not a
    /// volume"; before 2026-09-26 the port had it as the gain.)* Voice 0x33 (0x4A94D5): `Voice_play`
    /// 0x4A9C10 takes 0x33 to `Bank_playOneShot` but never writes the pause's table for it
    /// (`AudioOutput::silence_voices`, `RE/notes/sfx.md` "Audit 2026-09-27, audio port 11") — one
    /// voice number shared by every concurrent hit.
    fn play_one_shot(&mut self, out: &AudioOutput, slot: usize, level: f32) {
        let level = level.clamp(0.0, 127.0);
        if (level as i32) >> 4 <= self.gate {
            return;
        }
        self.gate = (level as i32) >> 4;
        let vol = self.atten.clamp(0, 127);
        if let Some(clip) = self.clip(slot) {
            let id = out.play_word_sel_untabled(clip, self.one_shot_volume(slot, vol), crate::resample::PITCH_UNIT, BEND_ROOT, self.one_shot_pan, false, level as i32 as u8);
            // the aux byte is taken only for slots from 0x30 (0x4A9A56 `jl`), which every
            // collision group is
            if slot >= FIRST_COLLISION_SLOT {
                out.set_fx_aux(id, level as i32 as u8);
            }
        }
    }

    /// Wall hit with normal closing speed `j` (m/s) on a wall of material class `class`
    /// (0 metal, 1 wood, 2 snow, 3 ice, 4 earth, 5 rock): `vol = 3j/8`, level `64 + clamp(6.3·vol, 2, 63)`.
    pub fn wall_hit(&mut self, out: &AudioOutput, j: f32, class: u8) {
        let mag = 3.0 * j;
        if mag <= 1.5 {
            return;
        }
        let base = [GROUP_CAR_CAR, GROUP_WALL_WOOD, GROUP_WALL_SNOW, GROUP_WALL_SNOW, GROUP_WALL_EARTH, GROUP_WALL_ROCK][(class as usize).min(5)];
        let slot = self.pick(base);
        let level = 64.0 + (mag * 0.125 * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level);
    }

    /// Wall scrape at speed `speed` (m/s) along a wall of material class `class`: `vol = speed/32`,
    /// level `clamp(76·vol, 2, 127)`.
    pub fn wall_scrape(&mut self, out: &AudioOutput, speed: f32, class: u8) {
        let mag = speed * 0.25;
        if mag <= 1.5 {
            return;
        }
        let base = [GROUP_SCRAPE_METAL, 0x63, 0x66, 0x66, 0x69, GROUP_SCRAPE_ROCK][(class as usize).min(5)];
        let slot = self.pick(base);
        let level = (mag * 0.125 * 127.0 * 0.6).clamp(2.0, 127.0);
        self.play_one_shot(out, slot, level);
    }

    /// Car–car contact with effect magnitude `effect` (`car+0x430`): `vol = effect` (×8 of the ⅛ rule).
    pub fn car_car(&mut self, out: &AudioOutput, effect: f32) {
        if effect <= 1.5 {
            return;
        }
        let slot = self.pick(GROUP_CAR_CAR);
        let level = 64.0 + (effect * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level);
    }

    /// 0x4A93C0's one-shot level for the pair CarBody × `idb` at `vol`: the scrapes (idB 21…26)
    /// `clamp(127·0.6·vol, 2, 127)`, everything else `64 + clamp(6.3·vol, 2, 63)`; the slot from
    /// [`carbody_group`] (nothing when it has no case).
    fn carbody_one_shot(&mut self, out: &AudioOutput, idb: u32, vol: f32) {
        let Some(base) = carbody_group(idb) else { return };
        let slot = self.pick(base);
        let level = if (21..=26).contains(&idb) { (vol * 127.0 * 0.6).clamp(2.0, 127.0) } else { 64.0 + (vol * 6.3).clamp(2.0, 63.0) };
        self.play_one_shot(out, slot, level);
    }

    /// The body–ground one-shot — event kind 3, read 2026-09-26 (0x40F740, CONFIRMED): `idA =
    /// 0x5CC518[3]` = **CarBody** (the table is `{3, 0, 0, 3, 3, 3, 3, …}`: only kinds 1–2 are
    /// Tires), `idB = 0x5CC4D8[car+0x438 & 0xFF]` (0x40F9CB…0x40F9DD) — the surface under the body
    /// — at `vol = car+0x430 · 0.125` (0x40F958, no ×8: that is kind 5's) over the `car+0x430 >
    /// 1.5` gate (0x40F944). The sparks 0x43F4C0 the same arm spawns (0x40F9EA…0x40FA17) are not
    /// sound. So a tilted car's body meeting Asphalt / Gravel / Rock plays 0x3C…, Snow / Ice
    /// 0x36…, Earth 0x39…; WetAsphalt has no CarBody case ([`carbody_group`]).
    pub fn ground_hit(&mut self, out: &AudioOutput, effect: f32, nibble: usize) {
        if effect <= 1.5 {
            return;
        }
        self.carbody_one_shot(out, surface_material(nibble) as u32, effect * 0.125);
    }

    /// A car–car contact with a smackable (a skill-test cone): the racer's own `car+0x430` at
    /// `vol = effect` over the 1.5 gate, CarBody × the smackable's material `smackable+0x44C`
    /// (0x496CA0…0x496CBD → 0x40FA2E…0x40FA48) — a cone (0x10) plays the Pylon group 0x56.
    pub fn car_smackable(&mut self, out: &AudioOutput, effect: f32, material: u32) {
        if effect <= 1.5 {
            return;
        }
        self.carbody_one_shot(out, material, effect);
    }

    /// A track prop's hit (event kind 5 with no other car, `sfx.md` §2/§4): CarBody against
    /// `0x5CC534[class]` = WoodSign, MetalSign, Glass, WoodBox, Pylon, Tree for classes 0…5 —
    /// groups 0x70, 0x73, 0x76, 0x79, 0x56, 0x53 — at `vol = 0x430 · 0.125 · 8` over the 1.5 gate
    /// (0x40F740), the level as `car_car`'s.
    pub fn prop_hit(&mut self, out: &AudioOutput, impact: f32, class: u32) {
        if impact <= 1.5 {
            return;
        }
        let Some(base) = [0x70usize, 0x73, 0x76, 0x79, 0x56, 0x53].get(class as usize).copied() else { return };
        let slot = self.pick(base);
        let level = 64.0 + (impact * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level);
    }

    /// Hard landing with closing speed `closing` (> 6 m/s), level `64 + clamp(6.3·closing, 2, 63)`,
    /// on the group [`landing_group`] gives `surface_nibble`. `Car_ground` pushes the pair
    /// **Tires × Asphalt** whatever the ground (`0x40F020(car, 1, −1, 0, 5, closing, 0)`,
    /// 0x49C1D6…0x49C1E6: `push 5` is a constant), so the callers pass nibble 1 (Asphalt).
    pub fn landing(&mut self, out: &AudioOutput, closing: f32, surface_nibble: usize) {
        let Some(base) = landing_group(surface_material(surface_nibble)) else { return };
        let slot = self.pick(base);
        let level = 64.0 + (closing * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level);
    }

    /// Driving through a bush: the event `0x6000F` at 15 (0x498EF6), CarBody × Bush group 0x7C, the
    /// fixed-15 level 75 of §4.3, as the pylon's.
    pub fn bush_hit(&mut self, out: &AudioOutput) {
        let slot = self.pick(GROUP_BUSH);
        self.play_one_shot(out, slot, 64.0 + (15.0 * 0.125 * 6.3f32).clamp(2.0, 63.0));
    }

    /// The trigger chime 0x4AAE30 (a checkpoint whose field 15 is 1, 0x470460 / 0x470D58): slot 2
    /// of `SND_coll` straight through 0x4A98E0 at level 0x7F, pan 0x40, pitch 0x10000 — no
    /// `Voice_play` call, so not in the pause's table (`AudioOutput::silence_voices`).
    pub fn trigger_chime(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(2) {
            out.play_word_untabled(clip, self.sfx_volume(LEVEL_MAX), crate::resample::PITCH_UNIT, BEND_ROOT, 0, false);
        }
    }

    /// The "wrong way" cue 0x4AAE70 (0x4A7744): slot 1 of `SND_coll` straight at level 0x28 — same
    /// direct `Bank_playOneShot` path as [`trigger_chime`](Self::trigger_chime), not in the table.
    pub fn wrong_way_cue(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(1) {
            out.play_word_untabled(clip, self.sfx_volume(0x28), crate::resample::PITCH_UNIT, BEND_ROOT, 0, false);
        }
    }

    /// The police siren for one audio slot (0x4A75C2…0x4A765C): a cop (`0x52C & 0x20`) with its
    /// lights on (`0xECC & 2`) starts [`siren_slot`] once — 0x4AAE90: 0x4A98E0(`[0x6564D0]`,
    /// sample, pan angle 0x40, volume 0, bend byte `0x5D1878[slot]` (0x20 in the image and after
    /// every release), pitch 0x10000 → the word 0x1000, 0x7F), nothing else that pass — and every
    /// later update 0x4AAFB0 restarts it if it has finished (the three samples loop, so it never
    /// does), sets the bend byte [`siren_bend`] of `pitchBase` (0x4AB033…0x4AB05F), the volume
    /// `(AmbientLevel · game.spl[5] · v · 85) >> 21` with `v` = [`siren_volume`] (0x4AB0A0…
    /// 0x4AB0C7) and the pan byte (0x4AB0F1…0x4AB13E). No `setPitch`: the word stays the start's
    /// 0x1000. Lights off releases the voice (0x4AAF50, 0x4A9890). *(Until 2026-09-27 the start
    /// took the car's pan and no bend; both silent at volume 0, but the bend 0x20 holds for that
    /// first pass.)* The `0x52C & 0x40` kind's `((AmbientLevel · spl[5] · 85) >> 7) · v >> 14`
    /// (0x4AB073…0x4AB099) has no cop in this build.
    #[allow(clippy::too_many_arguments)]
    pub fn siren(&mut self, out: &AudioOutput, audio_slot: usize, on: bool, distance: f32, pan: u16, pitch_base: i32, ambient_level: u8) {
        if !on {
            if let Some(id) = self.sirens.remove(&audio_slot) {
                tracing::debug!(audio_slot, "siren release");
                // 0x4AAF50: 0x565DD0(h, 5, −1) (0x4AAF9F), the ramp of `mixer::Ramp`
                out.release(id);
            }
            return;
        }
        match self.sirens.get(&audio_slot) {
            Some(&id) => {
                let v = siren_volume(distance);
                let vol = category(ambient_level, self.siren_trim, v);
                out.set_word(id, byte_gain(vol), SIREN_WORD, siren_bend(pitch_base));
                out.set_pan(id, pan);
            }
            None => {
                if let Some(c) = self.clip(siren_slot(audio_slot, false)) {
                    tracing::debug!(audio_slot, sample = siren_slot(audio_slot, false), distance, "siren start");
                    // 0x4AAE90 is a direct Bank_playOneShot 0x4A98E0, no Voice_play voice number:
                    // not in the pause's table (AudioOutput::silence_voices)
                    self.sirens.insert(audio_slot, out.play_word_untabled(c, 0.0, SIREN_WORD, SIREN_START_BEND, 0x40, true));
                }
            }
        }
    }

    /// Cone hit, from the HUD's cone count (`skill-tests.md` §8): **0x4AAE50** plays slot 0 of the
    /// kind-3 bank `[0x6564D0]` (`SND_coll.bnk`) straight through 0x4A98E0 at level 0x7F, pan 0x40,
    /// pitch 0x10000 — not through the collision queue, so no pair lookup and no one-shot gate.
    /// (The port had CarBody × Pylon, group 0x56, at the gated fixed-15 level.) Same direct
    /// `Bank_playOneShot` path as [`trigger_chime`](Self::trigger_chime): not in the pause's table.
    pub fn pylon_hit(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(0) {
            out.play_word_untabled(clip, self.sfx_volume(LEVEL_MAX), crate::resample::PITCH_UNIT, BEND_ROOT, 0, false);
        }
    }

    /// Crash / glass one-shot (damage ≥ 30 on a corner, roll-over): slot 0x20 at full level.
    pub fn crash(&mut self, out: &AudioOutput) {
        self.play_one_shot(out, SLOT_CRASH, 127.0);
    }

    /// **The player's queue drain, 0x4A7AF8…0x4A7C97 — one last-first walk of every entry**: the
    /// tick's collision one-shots (`Car_postImpact` 0x410280 / `Car_ground` 0x49C1D6 / 0x49C095,
    /// `main.rs`'s collision pass, queued there instead of played straight since 2026-09-27) and,
    /// pushed after them by this same audio update, the loop entries of 0x40FFF0 / 0x40FBC0
    /// ([`SfxQueue::produce_loops`]) — each into 0x4A93C0, a loop on voice 0x12 (front) or 0x14
    /// (rear) ([`Self::loop_voice`]), a one-shot dispatched by kind ([`Self::wall_hit`] etc.).
    /// `one_shot_pan` is the angle to the car's last contact point `car+0x440` (0x4A7BB1…0x4A7C3B)
    /// as the caller measures it **once**, at this call — the value the original's drain reads,
    /// after the whole tick's physics, so every one-shot of the tick takes the same, final contact
    /// ([`Self::set_one_shot_pan`]); the loops' own pan is the car's ([`Self::set_levels`]).
    ///
    /// Because the walk is last-first and [`Self::play_one_shot`]'s gate carries across the whole
    /// call (raised by the first one-shot processed, tested by every one after), the LAST entry
    /// *pushed* this tick is the FIRST one *played*, and can silence a quieter one-shot pushed
    /// earlier in the tick, exactly as the original's does. A wall hit and a body–ground scrape
    /// cannot both queue (a wall contact clears `ground_event`, `main.rs`), but a wall hit and a
    /// car–car hit can — pushed in that order (walls run before the car-pair rounds) — and a wall
    /// scrape queued first, silenced by a louder car hit pushed after it, is now inaudible where
    /// the port's old immediate-play design (each against its own contact, no shared order) played
    /// both. So this closes a real, if narrow, audible gap, not only the pan's timing.
    ///
    /// So the roof scrape (a rear loop entry) never touches the front voice, and when the car comes
    /// upright after a scrape the scrape's stop, pushed first, is taken last: a rear squeal the
    /// same tick starts (the scrape's 25-tick replace release) and is released by it (five ticks),
    /// to start again next update. *(Until 2026-09-27 this was `drain_player_loops`, walking only
    /// the `Loop` entries; the one-shots were played straight from the collision pass, each against
    /// `car+0x440` as it stood at its own call — the port had no player one-shot queue at all. See
    /// `sfx.md`'s "The drain's distance and pan".)*
    pub fn drain_player(&mut self, out: &AudioOutput, queue: &SfxQueue, one_shot_pan: u16) {
        self.set_one_shot_pan(one_shot_pan);
        for e in queue.entries.iter().rev() {
            match *e {
                QueuedSfx::WallHit { j, class } => self.wall_hit(out, j, class),
                QueuedSfx::WallScrape { speed, class } => self.wall_scrape(out, speed, class),
                QueuedSfx::CarCar { effect } => self.car_car(out, effect),
                QueuedSfx::Smackable { effect, material } => self.car_smackable(out, effect, material),
                QueuedSfx::Prop { impact, class } => self.prop_hit(out, impact, class),
                QueuedSfx::Bush => self.bush_hit(out),
                QueuedSfx::Ground { effect, nibble } => self.ground_hit(out, effect, nibble),
                QueuedSfx::Landing { closing } => self.landing(out, closing, 1),
                QueuedSfx::Crash => self.crash(out),
                QueuedSfx::Loop { chan, sample, v } => self.loop_voice(out, if chan == 0x12 { 0 } else { 1 }, sample, v),
            }
        }
    }

    /// One squeal-channel loop (0x4A93C0, chan ≥ 0, voices 0x12…0x15): for a strength `v` (the
    /// clamped slip sum, or the scrape's `speedEst/4`) the bend byte is `min(12.7·v, 127)`
    /// (0x4A96BC…0x4A96E0) — the squeal rises in pitch as it gets harder — and the volume
    /// `atten · min(127·v, 127) / 128` (0x4A96FA…0x4A9724); `v ≤ 0` releases it. The port had
    /// `min(127·v, 127)` as a gain with no distance until 2026-09-24.
    fn loop_voice(&mut self, out: &AudioOutput, which: usize, slot: usize, v: f32) {
        let master = (self.sfx_level, self.spl.sfx());
        let v16 = (v.max(0.0) * 65536.0) as i64;
        let bend = ((v16 * 127 / 10) >> 16).min(127) as i32;
        let level = ((v16 * 127) >> 16).min(127) as i32;
        let vol = ((self.atten * level) >> 7).clamp(0, 127);
        let mut voice = if which == 0 { self.front } else { self.rear };
        if v <= 0.0 {
            // 0x4A956F / 0x4A97EF: 0x565DD0(h, 5, −1), the five-tick release (`mixer::Ramp`)
            LoopVoice::release(&mut voice, out);
        } else {
            let amp = byte_gain(category(master.0, master.1, vol));
            // the loops take the car's own pan (0x4A7C61 → 0x4A983A)
            // 0x4A9840…0x4A9854: `Voice_play(voice, slot, bend, 0x10000, vol, pan, 0x7F)` — the word
            // 0x1000, the bend byte on the voice
            let call = LoopCall { slot, amp, word: WORD_UNIT, bend: bend as u8, pan: self.pan, filter: self.filter };
            LoopVoice::drive(&mut voice, out, call, false, || self.clip(slot));
        }
        if which == 0 { self.front = voice } else { self.rear = voice }
    }


    /// §4.4 road / tyre roll — voice 0x18, `SND_coll` slots 0x11..0x1C picked by the surface nibble.
    ///
    /// The original's slot is `0x11 + (car+0x540 != 0 ? 4 : nibble)`, and `car+0x540` is the
    /// **damage counter** 0x40F740 raises only while it is already non-zero — whose one non-zero
    /// writer is the front-end path 0x439206, so in a race it stays 0 and the `+4` arm never fires
    /// (`traffic.md` §21.7, `physics.md` §4.1). This port read that arm as the **handbrake** and so
    /// swapped the surface loop for slot 0x15 whenever the handbrake was down; that was an
    /// invention and is gone (2026-09-23). `gain` is the engine distance gain `A` of §4.1 and
    /// `doppler` the same factor the engine layers use.
    ///
    /// *(Corrected 2026-09-26, 0x4AA0CE…0x4AA42E:)* the volume's speed is `edi` — `V` in modes 0
    /// and 1 but **`V >> 1` in modes 2 and 3** (the chase and detached views, 0x4AA2C8) — and
    /// `vol = (A · max(edi − 15, 0)) >> 7`, capped at 122, with `A` the rpm-boosted distance gain
    /// (not `engineGain`'s `· 127/128`), or `12800/(d+1)` on view 0x0C (0x4AA3B2…0x4AA3E1); `V` is
    /// [`road_v`]. The port had `V` in every view and fed `2048/(d+2) · 127/128` with no rpm boost.
    ///
    /// The call (0x4AA419…0x4AA42E): `Voice_play(0x18, 0x11 + idx, level, pitchBase, vol, pan,
    /// 0x7F)` — `pitch_base` is 0x4AA050's own argument `[ebp+0x14]` (0x4A781D's doppler word; the
    /// tiny car's doubling at 0x4AA906 is the engine record's only), the word `pitchBase >> 4`, and
    /// `level` the bend byte on the voice. *(Until 2026-09-27 the float `Clip::bend(level) ·
    /// doppler`.)*
    #[allow(clippy::too_many_arguments)]
    pub fn road_roll(&mut self, out: &AudioOutput, speed: f32, nibble: usize, a: i32, half: bool, pitch_base: i32, pan: u16) {
        let master = (self.sfx_level, self.spl.road_roll());
        let slot = SLOT_ROAD_BASE + nibble.min(11);
        let v = road_v(speed, nibble);
        let level = (v / 2 + 64).clamp(0, 127);
        let vol = road_volume(a, v, half);
        let mut voice = self.road;
        if vol == 0 {
            // 0x4AA454: 0x565DD0(h, 5, −1)
            LoopVoice::release(&mut voice, out);
        } else {
            // `level` is the bend byte: the roll rises in pitch with speed
            let amp = byte_gain(category(master.0, master.1, vol));
            let word = (pitch_base >> 4) as u16;
            // another surface's sample on voice 0x18: `Voice_play`'s 25-tick release (0x4A9DA4)
            let call = LoopCall { slot, amp, word, bend: level as u8, pan, filter: self.filter };
            LoopVoice::drive(&mut voice, out, call, false, || self.clip(slot));
        }
        self.road = voice;
    }

    /// §4.5 wind — voice 0x1A, `SND_coll` slot 0x1F. `active` is 0x4AA050's `[ebp+0x13]` (see
    /// `EngineInput::attached_exterior`). Level and pitch are random walks stepped once per update.
    ///
    /// *(Corrected 2026-09-26, 0x4AA4AD…0x4AA5DF:)* the speed is `s = (int)(|car+0x35C| · 5)` —
    /// the speed estimate `speed_est` — and the wind plays while `s != 0`; the bend byte is the road
    /// layer's `min((V >> 1) + 0x40, 0x7F)` (`[ebp−0x10]`, [`road_v`] of `road_speed` on
    /// `nibble`). The port took `car+0xD60` for both, without the surface factor, and stopped under
    /// an invented 0.5 m/s.
    ///
    /// The call (0x4AA5CC…0x4AA5DF): `Voice_play(0x1A, 0x1F, level, [0x5D183C], vol, 0, 0x7F)` —
    /// the walk is the 16.16 pitch, the word `walk >> 4` (0xFA2…0x105D), `level` the bend byte on
    /// the voice, the pan angle 0 (the byte 0x40). The walk's four globals start as the image has
    /// them — level 0x28, pitch 0xFA24, both steps −1 — and only 0x4AA050 writes them (byte scan),
    /// so they carry over from race to race; the port starts them afresh with each bank load.
    /// *(Until 2026-09-27 the port started at 0x28 / +1 and 0x10000 / +10, and multiplied the
    /// float bend into the float walk.)*
    ///
    /// The walk's draws (0x4AA520, 0x4AA544, 0x4AA566, 0x4AA594) are **0x538040**'s — the front
    /// end's generator the race shares (`draw`, the caller's `fe_rng`): at most one for the level
    /// (`−1 − (r & 7)` at the top, `ebx` −1 on every path in, 0x4AA436 / 0x4AA449 / 0x4AA4AA;
    /// `(r & 7) + 1` at the bottom) and then at most one for the pitch (`−20 − r % 80` at the top,
    /// `r % 50 + 10` at the bottom; `div`, unsigned), in that order, only on the updates that hit a
    /// bound. *(Until 2026-09-27 an approximation took them from this bank's own LCG.)*
    pub fn wind(&mut self, out: &AudioOutput, active: bool, speed_est: f32, road_speed: f32, nibble: usize, draw: &mut impl FnMut() -> u32) {
        let master = (self.sfx_level, self.spl.sfx());
        let s = (speed_est.abs() * 5.0) as i32;
        let mut voice = self.wind;
        if !active || s == 0 {
            // 0x4AA5FF: 0x565DD0(h, 5, −1)
            LoopVoice::release(&mut voice, out);
        } else {
            // [0x5D1838] 0x28..0x7F, [0x5D183C] 0xFA24..0x105DC
            self.wind_walk.step(draw);
            let vol = ((s * self.wind_walk.level) >> 7).min(127);
            let level = (road_v(road_speed, nibble) / 2 + 64).clamp(0, 127);
            // `level` is the bend byte on top of the random-walk pitch
            let amp = byte_gain(category(master.0, master.1, vol));
            let word = (self.wind_walk.pitch >> 4) as u16;
            // one sample on the voice, so the slot never differs; the pan angle is 0
            let call = LoopCall { slot: SLOT_WIND, amp, word, bend: level as u8, pan: 0, filter: self.filter };
            LoopVoice::drive(&mut voice, out, call, false, || self.clip(SLOT_WIND));
        }
        self.wind = voice;
    }

    /// §4.6 turbo / blower — voice 0, `SND_coll` slot 0x0F. `boost` is the turbo state `car+0x54C`,
    /// `throttle` the raw pedal byte `car+0xD7F`, `base` the `.spl` aux volume for the view mode
    /// (0x4AD640) and `gain` the engine distance gain `[ebp−4]`. The shipped curve is silent below
    /// half the redline, jumps there and fades to nothing at 0.9 × redline — reproduced as written.
    ///
    /// `boost` not above 0.1 releases the voice (0x4AA72E…0x4AA744 → 0x4AA851); otherwise it is
    /// played every update with [`turbo_terms`]' volume and pitch — at volume 0 too: nothing but
    /// the boost test stops it. `Voice_play(0, 0xF, 0x40, pitch, vol, pan, 0x7F)` (0x4AA834…
    /// 0x4AA847): the word `pitch >> 4`, no bend. *(Until 2026-09-27 the port stopped the voice at
    /// volume 0, and its pitch was the float `(2.5 · rpm/redline + 0.5) · doppler` rounded to the
    /// word — where the original rounds the f32 to an int (`fistp`) before the `>> 4`.)*
    /// A zero redline (`specs[0x94]`, the `.sim`'s `+0x94`, 0x4AA7E5's `fdiv`) is **unreachable in the
    /// shipped data**: all 86 `.sim` files under the game directory hold a redline of 5500 or more
    /// (scanned 2026-09-30), and the same field is every gear-change and rpm rule's divisor
    /// (`ai.md` §8, 0x49DAF3…), so a car with 0 would not run at all. The guard below is only for a
    /// hand-made file; the original would there take the x87's infinity (every exception masked,
    /// `career.md` §11.4c) through `fistp` (0x80000000, `>> 4`) — not reproduced, not reachable.
    /// (2026-09-28, `career.md` §11.4c: the nine measured control words all have every exception
    /// mask bit set — IM DM ZM OM UM PM.)
    #[allow(clippy::too_many_arguments)]
    pub fn turbo(&mut self, out: &AudioOutput, boost: f32, throttle: u8, rpm: f32, redline: f32, base: i32, gain: i32, pitch_base: i32, pan: u16) {
        let master = (self.engine_level, self.spl.turbo());
        let mut voice = self.turbo;
        if boost <= 0.1 || redline <= 0.0 {
            // 0x4AA869: 0x565DD0(h, 5, −1)
            LoopVoice::release(&mut voice, out);
        } else {
            // 0x4AA70F…0x4AA72E: `min(auxVolume(mode) · gain >> 7, 0x7F)`
            let lvl = (base.wrapping_mul(gain) >> 7).min(127);
            let (vol, pitch) = turbo_terms(boost, throttle, rpm, redline, lvl, pitch_base);
            let word = (pitch >> 4) as u16;
            let amp = byte_gain(category(master.0, master.1, vol));
            // one sample on the voice, so the slot never differs
            let call = LoopCall { slot: SLOT_TURBO, amp, word, bend: BEND_ROOT, pan, filter: self.filter };
            LoopVoice::drive(&mut voice, out, call, false, || self.clip(SLOT_TURBO));
        }
        self.turbo = voice;
    }

    /// **The AI cars' queue drain, 0x4A73DE…0x4A7458 inside `Audio_updateAiCar` 0x4A7080** — read
    /// to the instruction 2026-09-26, CONFIRMED. For the car in audio slot `audio_slot`, the
    /// entries are walked **last-first** (`esi = car + 0x1120 + 0x18·(n−1)`, `sub esi, 0x18`) and
    /// each goes through 0x4AABD0 — a bare pass-through to 0x4A93C0 behind the audio-on flag
    /// `[0x5D1734]` (0x4AABD0…0x4AABFF) — as `(voice, idA, idB, mag · 65536, dist, pan)`: `voice`
    /// is **−1** for a one-shot (`chan ≤ −1`, 0x4A73FD) and **`slot + 0x22`** for anything else
    /// (0x4A7420) — the entry's `type` is never read, and front and rear share the one voice;
    /// `dist` is the car's own distance to the listener (`[esp+0x18]`, 0x4A7185…0x4A71A2) and `pan`
    /// the car's own angle (`edi`, 0x4A726D…0x4A72F0) for both kinds — no contact point.
    ///
    /// In 0x4A93C0 a one-shot is the player's (the level only gates, against the shared
    /// `[0x656860]`; voice 0x33 at the `512/(d+2)` attenuation). A loop on a voice that is not
    /// 0x12…0x15 ([`Self::ai_loop`]): magnitude ≤ 0 releases the voice (0x4A94F6…0x4A953D) and
    /// returns (0x4A954A → 0x4A985D; the `car+0x1108` store after it is the front voices 0x12 /
    /// 0x13's only, and write-only — `sfx.md` §5.3); otherwise the slot from §5.2's table, the
    /// bend byte `min(12.7 · mag, 127)` computed **only when the voice's slot changes** and
    /// latched in `0x5D17E8[voice]` (0x4A9654…0x4A968F), and the volume the plain
    /// `min(atten, 127)` (0x4A9821…0x4A9854) — not scaled by the slip as the player's is.
    #[allow(clippy::too_many_arguments)]
    pub fn drain_ai(&mut self, out: &AudioOutput, audio_slot: usize, queue: &SfxQueue, distance: f32, view_0c: bool, pan: u16) {
        if queue.entries.is_empty() {
            return;
        }
        let saved = (self.atten, self.pan, self.one_shot_pan);
        self.atten = atten_for(distance.max(0.0), view_0c);
        self.pan = pan;
        self.one_shot_pan = pan;
        for e in queue.entries.iter().rev() {
            tracing::debug!(audio_slot, event = ?e, distance, atten = self.atten, "ai sfx drain (0x4A73DE)");
            match *e {
                QueuedSfx::WallHit { j, class } => self.wall_hit(out, j, class),
                QueuedSfx::WallScrape { speed, class } => self.wall_scrape(out, speed, class),
                QueuedSfx::CarCar { effect } => self.car_car(out, effect),
                QueuedSfx::Smackable { effect, material } => self.car_smackable(out, effect, material),
                QueuedSfx::Prop { impact, class } => self.prop_hit(out, impact, class),
                QueuedSfx::Bush => self.bush_hit(out),
                QueuedSfx::Ground { effect, nibble } => self.ground_hit(out, effect, nibble),
                QueuedSfx::Landing { closing } => self.landing(out, closing, 1),
                QueuedSfx::Crash => self.crash(out),
                QueuedSfx::Loop { sample, v, .. } => self.ai_loop(out, audio_slot, sample, v),
            }
        }
        (self.atten, self.pan, self.one_shot_pan) = saved;
    }

    /// 0x4A93C0's loop arm for voice `slot + 0x22` (see [`Self::drain_ai`]).
    fn ai_loop(&mut self, out: &AudioOutput, audio_slot: usize, sample: usize, v: f32) {
        if v <= 0.0 {
            self.stop_ai_loop(out, audio_slot);
            return;
        }
        let amp = self.sfx_volume(self.atten.clamp(0, 127));
        // 0x4A9663…0x4A9687: `min(mag·127/10 >> 16, 127)` from the 16.16 magnitude, latched when
        // the voice starts (an update keeps the one it started with)
        let v16 = (v * 65536.0) as i64;
        let bend = ((v16 * 127 / 10) >> 16).min(127) as i32;
        let mut voice = self.ai_loops.get(&audio_slot).copied();
        let call = LoopCall { slot: sample, amp, word: WORD_UNIT, bend: bend as u8, pan: self.pan, filter: self.filter };
        LoopVoice::drive(&mut voice, out, call, true, || {
            let clip = self.clip(sample);
            if clip.is_some() {
                tracing::debug!(audio_slot, sample, bend, amp, "ai loop start (voice slot + 0x22)");
            }
            clip
        });
        match voice {
            Some(v) => self.ai_loops.insert(audio_slot, v),
            None => self.ai_loops.remove(&audio_slot),
        };
    }

    /// 0x4A9890 on voice `slot + 0x22`: a slot car's loop released — by a stop entry, or by
    /// 0x4A7DC0 when the slot loses its car (0x4A7FF8 / 0x4A80BD).
    pub fn stop_ai_loop(&mut self, out: &AudioOutput, audio_slot: usize) {
        if let Some(v) = self.ai_loops.remove(&audio_slot) {
            tracing::debug!(audio_slot, "ai loop release (voice slot + 0x22)");
            // 0x4A98B4 / 0x4A9521: 0x565DD0(h, 5, −1)
            LoopVoice::release(&mut Some(v), out);
        }
    }

    pub fn stop_loops(&mut self, out: &AudioOutput) {
        for voice in [&mut self.front, &mut self.rear, &mut self.wind, &mut self.turbo, &mut self.road] {
            LoopVoice::stop(voice, out);
        }
        for (_, id) in self.sirens.drain() {
            out.stop(id);
        }
        for (_, v) in self.ai_loops.drain() {
            LoopVoice::stop(&mut Some(v), out);
        }
    }
}

#[cfg(test)]
mod wind_tests {
    use super::WindWalk;

    #[test]
    fn the_wind_walk_draws_the_shared_generator_at_its_bounds_level_first() {
        // from the image's values both walks step below their floors on the first update: the
        // level's draw, then the pitch's
        let mut w = WindWalk::default();
        let seq = [0x0000_0105u32, 0x0000_0077];
        let mut i = 0;
        let mut draw = || {
            i += 1;
            seq[i - 1]
        };
        w.step(&mut draw);
        assert_eq!(i, 2);
        // (0x105 & 7) + 1 = 6; 0x77 % 50 + 10 = 29
        assert_eq!(w, WindWalk { level: 0x28, level_step: 6, pitch: 0xFA24, pitch_step: 29 });
        // no bound: no draw
        let mut n = 0;
        w.step(&mut || {
            n += 1;
            0
        });
        assert_eq!(n, 0);
        assert_eq!((w.level, w.pitch), (0x2E, 0xFA24 + 29));
        // at the top: `−1 − (r & 7)` and `−20 − r % 80`, the `div` unsigned
        let mut w = WindWalk { level: 0x7F, level_step: 3, pitch: 0x1_05DC, pitch_step: 40 };
        let seq = [0xFFFF_FFFFu32, 0xFFFF_FFFF];
        let mut i = 0;
        w.step(&mut || {
            i += 1;
            seq[i - 1]
        });
        assert_eq!(w, WindWalk { level: 0x7F, level_step: -8, pitch: 0x1_05DC, pitch_step: -20 - (0xFFFF_FFFFu32 % 80) as i32 });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_road_layer_halves_its_speed_in_the_chase_views() {
        // 30 m/s on gravel: V = 90; A = 200 at 10 m with the rpm boost
        assert_eq!(super::road_v(30.4, 7), 91, "the factor inside the truncation");
        assert_eq!(super::road_v(30.4, 1), 30);
        assert_eq!(super::road_volume(200, 90, false), 117);
        assert_eq!(super::road_volume(200, 90, true), 46);
        assert_eq!(super::road_volume(400, 90, false), 122, "capped at 122");
        assert_eq!(super::road_volume(400, 30, true), 0, "V/2 = 15 is silent");
    }

    #[test]
    fn the_category_gain_matches_the_integer_mix() {
        // (127 · 107 · 127 · 85) >> 21 = 69 of 127 for a full default-category sound
        assert_eq!(super::category(127, 107, 127), 69);
        assert_eq!(super::category(0, 107, 127), 0);
        // truncated, not rounded: 100 · 107 · 100 · 85 / 2²¹ = 43.36 and 64 · 110 · 90 · 85 / 2²¹ = 25.68
        assert_eq!(super::category(100, 107, 100), 43);
        assert_eq!(super::category(64, 110, 90), 25);
        // the byte survives the mixer's `round(v · 127)`
        for b in 0..=127i32 {
            assert_eq!(((b as f32 / 127.0) * 127.0).round() as i32, b);
        }
    }

    #[test]
    fn the_turbo_rounds_its_pitch_as_an_f32_then_shifts_it() {
        // 4500 of a 6000 redline, full throttle and boost, lvl 100, no doppler: g = (5400 − 4500)
        // / 5400 (0.9 as the f32 0.89999998), f0 = 255 · f32(1/127.5) = 2.0000001; the pitch
        // (4500 · 2.5 / 6000 + 0.5) · 65536 = 155648 → the word 9728 (2.375)
        let (vol, pitch) = super::turbo_terms(1.0, 255, 4500.0, 6000.0, 100, 0x1_0000);
        assert_eq!(pitch, 155648);
        assert_eq!(pitch >> 4, 9728);
        let g = (6000.0 * 0.9f32 as f64 - 4500.0) / (6000.0 * 0.9f32 as f64);
        assert_eq!(vol, ((100.0 * (255.0 * (1.0f32 / 127.5) as f64 * g)) as f32).round_ties_even() as i32);
        assert_eq!(vol, 33);
        // at or under half the redline the low branch is 0: silent, but the voice plays on
        assert_eq!(super::turbo_terms(1.0, 255, 3000.0, 6000.0, 100, 0x1_0000).0, 0);
        // (1000 · 2.5 / 6000 + 0.5) · 65536 = 60074.67: `fistp` rounds to 60075, the `>> 4` 3754
        let (_, p) = super::turbo_terms(1.0, 255, 1000.0, 6000.0, 100, 0x1_0000);
        assert_eq!((p, p >> 4), (60075, 3754));
    }

    #[test]
    fn siren_fades_out_by_seventy_metres() {
        // (0x1324 − min(m², 0x1324)) · 127 / 0x1324 over whole metres (0x4A72F3, 0x4A7607)
        assert_eq!(super::siren_volume(0.0), 127);
        assert_eq!(super::siren_volume(35.9), (4900 - 35 * 35) * 127 / 4900);
        assert_eq!(super::siren_volume(70.0), 0);
        assert_eq!(super::siren_volume(1.0e6), 0);
        // odd audio slots take sample 6, even ones 8; the 0x40 cop kind 9 (0x4AAF1A…0x4AAF29)
        assert_eq!((super::siren_slot(1, false), super::siren_slot(2, false), super::siren_slot(2, true)), (6, 8, 9));
        // the bend byte of pitchBase: 0x40 still, toward zero, never negative, capped
        assert_eq!(super::siren_bend(0x1_0000), 0x40);
        assert_eq!(super::siren_bend(0x1_03FF), 0x40);
        assert_eq!(super::siren_bend(0x1_0400), 0x41);
        assert_eq!(super::siren_bend(0x2_0000), 0x7F);
        assert_eq!(super::siren_bend(-0x1_0000), 0);
    }

    use super::*;

    fn input() -> LoopInput {
        LoopInput { cos_tilt: 1.0, body_height: 0.0, speed_est: 20.0, front_slip: 0.0, rear_slip: 0.0, racer_distance: 0.0, tyre_class: 2, surface_nibble: 1, wet: false, human: false }
    }

    /// 0x40FBC0: the front and the rear are independent — a front slide with the rear gripping
    /// still pushes the front loop — and a stop is pushed once, when its bit was set.
    #[test]
    fn squeal_producer_pushes_and_stops() {
        let mut q = SfxQueue::default();
        q.produce_loops(&LoopInput { front_slip: 0.5, ..input() }, || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x12, sample: 0x2E, v: 0.5 }]);
        q.clear();
        q.produce_loops(&LoopInput { rear_slip: 12.0, ..input() }, || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x12, sample: 0x2E, v: 0.0 }, QueuedSfx::Loop { chan: 0x14, sample: 0x2E, v: 5.0 }]);
        q.clear();
        q.produce_loops(&input(), || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x14, sample: 0x2E, v: 0.0 }]);
        q.clear();
        q.produce_loops(&input(), || 0);
        assert!(q.entries().is_empty(), "nothing is running, nothing to stop");
        // a car 60 m or more from every racer (`car+0x78`) does not squeal
        q.produce_loops(&LoopInput { front_slip: 1.0, racer_distance: 60.0, ..input() }, || 0);
        assert!(q.entries().is_empty());
    }

    /// 0x40FFF0 on its roof: the CarBody × surface loop on the rear channel, `v·(1 − r/3)`.
    #[test]
    fn roof_scrape_producer() {
        let mut q = SfxQueue::default();
        q.produce_loops(&LoopInput { cos_tilt: -1.0, ..input() }, || 0x8000);
        let QueuedSfx::Loop { chan, sample, v } = q.entries()[0] else { panic!() };
        assert_eq!((chan, sample), (0x14, 0x28));
        assert!((v - 5.0 * (1.0 - 0.5 / 3.0)).abs() < 1e-5, "{v}");
        // upright again: the stop, then the squeal producer
        q.clear();
        q.produce_loops(&input(), || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x14, sample: 0x28, v: 0.0 }]);
    }

    /// 0x40FFF0's side / roof arm pushes only the rear channel (0x410142…0x4101CB, 0x4101E5…
    /// 0x410269) and does not call 0x40FBC0: a front squeal running when the car goes over gets
    /// no entry — neither a stop nor an update — until it is upright again.
    #[test]
    fn the_roof_scrape_leaves_the_front_channel_alone() {
        let mut q = SfxQueue::default();
        q.produce_loops(&LoopInput { front_slip: 2.0, human: true, ..input() }, || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x12, sample: 0x2E, v: 2.0 }]);
        q.clear();
        // over in one tick, scraping: the rear scrape only
        q.produce_loops(&LoopInput { cos_tilt: 0.1, front_slip: 2.0, human: true, ..input() }, || 0);
        assert!(q.entries().iter().all(|e| matches!(e, QueuedSfx::Loop { chan: 0x14, .. })), "{:?}", q.entries());
        q.clear();
        // on its side, too slow to scrape: the rear stop only
        q.produce_loops(&LoopInput { cos_tilt: 0.1, speed_est: 0.2, human: true, ..input() }, || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x14, sample: 0x28, v: 0.0 }]);
        q.clear();
        // upright but tilted past 0x40FBC0's 0.5: the front's stop comes from the squeal producer
        q.produce_loops(&LoopInput { cos_tilt: 0.4, human: true, ..input() }, || 0);
        assert_eq!(q.entries(), &[QueuedSfx::Loop { chan: 0x12, sample: 0x2E, v: 0.0 }]);
    }

    /// 0x40F020 keeps 7 entries; 0x40F740 queues an impact only above 1.5.
    #[test]
    fn queue_cap_and_impact_gate() {
        let mut q = SfxQueue::default();
        q.push_impact(QueuedSfx::CarCar { effect: 1.5 });
        q.push_impact(QueuedSfx::WallHit { j: 0.4, class: 0 });
        assert!(q.entries().is_empty());
        for _ in 0..9 {
            q.push_impact(QueuedSfx::CarCar { effect: 9.0 });
        }
        assert_eq!(q.entries().len(), SfxQueue::CAP);
    }

    /// `main.rs`'s player collision pass pushes the tick's one-shots — `Car_ground`'s landing /
    /// roll-over first, then `Car_postImpact`'s collision one-shot (`sfx.md` §4) — into the SAME
    /// queue the loop producer (`produce_loops`) appends to in the following audio update, ahead
    /// of `CollisionSfx::drain_player`'s single last-first walk (0x4A7AF8…0x4A7C97). *(Until
    /// 2026-09-27 the player's one-shots were never queued at all — each was played straight from
    /// the collision pass, against `car+0x440` as it stood at its own call, not the tick's final
    /// contact; `sfx.md`'s "The drain's distance and pan".)* This pins the queue's contract: the
    /// one-shots keep push order, the loops land after them, and a last-first walk (the drain's)
    /// takes a loop before any one-shot and the tick's earliest one-shot last.
    #[test]
    fn the_players_queue_holds_one_shots_then_loops_in_push_order() {
        let mut q = SfxQueue::default();
        q.push(QueuedSfx::Landing { closing: 10.0 });
        q.push(QueuedSfx::Crash);
        q.push_impact(QueuedSfx::WallHit { j: 5.0, class: 0 });
        q.push_impact(QueuedSfx::CarCar { effect: 4.0 });
        q.produce_loops(&LoopInput { front_slip: 1.0, ..input() }, || 0);
        let entries = q.entries().to_vec();
        assert_eq!(
            entries[..4],
            [
                QueuedSfx::Landing { closing: 10.0 },
                QueuedSfx::Crash,
                QueuedSfx::WallHit { j: 5.0, class: 0 },
                QueuedSfx::CarCar { effect: 4.0 },
            ]
        );
        assert!(matches!(entries[4], QueuedSfx::Loop { .. }), "{entries:?}");
        // the drain's walk (`.iter().rev()`): the loop plays first, this tick's landing last
        let reversed: Vec<_> = entries.iter().rev().copied().collect();
        assert!(matches!(reversed[0], QueuedSfx::Loop { .. }));
        assert_eq!(reversed[reversed.len() - 1], QueuedSfx::Landing { closing: 10.0 });
    }

    #[test]
    fn carbody_row() {
        use super::carbody_group as g;
        // a cone (0x10 Pylon), a car, the ground kinds and the gaps (0x4A8FF8…0x4A9302)
        assert_eq!(g(16), Some(0x56));
        assert_eq!(g(3), Some(0x30));
        assert_eq!(g(5), Some(0x3C));
        assert_eq!(g(10), Some(0x36));
        assert_eq!(g(12), Some(0x39));
        assert_eq!(g(6), None);
        assert_eq!(g(1), None);
    }

    #[test]
    fn slot_rules() {
        assert_eq!(squeal_slot(0, surface_material(1)), 0x2A, "road tyres on asphalt");
        assert_eq!(squeal_slot(1, surface_material(1)), 0x2E, "race tyres on asphalt");
        assert_eq!(squeal_slot(1, surface_material(2)), 0x29, "gravel");
        assert_eq!(squeal_slot(0, surface_material(6)), 0x2D, "wet asphalt");
        // 0x40FD0C's column map: the raw `carInfo+0x1C0` is not the column
        assert_eq!(tyre_column(0), 0);
        assert_eq!(tyre_column(1), 2, "class 1 takes the RainTires column");
        assert_eq!(tyre_column(2), 1, "a race car takes the RaceTires column");
        assert_eq!(squeal_slot(tyre_column(2), surface_material(1)), 0x2E, "race car on asphalt");
        // 0x4A8E8A's Tires arm: the landing group is the surface's, and two surfaces are silent
        assert_eq!(landing_group(Material::Asphalt), Some(0x40));
        assert_eq!(landing_group(Material::Gravel), Some(0x46));
        assert_eq!(landing_group(Material::Snow), Some(0x43));
        assert_eq!(landing_group(Material::Ice), Some(0x50));
        assert_eq!(landing_group(Material::Earth), Some(0x4C));
        assert_eq!(landing_group(Material::Rock), None, "not in the arm at all");
        assert_eq!(landing_group(Material::WetAsphalt), None);
        // §5.2's CarBody column, what the roof scrape plays
        assert_eq!(body_scrape_slot(Material::Asphalt), 0x28);
        assert_eq!(body_scrape_slot(Material::Rock), 0x28);
        assert_eq!(body_scrape_slot(Material::Gravel), 0x29);
        assert_eq!(body_scrape_slot(Material::Ice), 0x2B);
        assert_eq!(body_scrape_slot(Material::Earth), 0x2C);
        assert_eq!(surface_material(4), Material::Rock);
    }
}
