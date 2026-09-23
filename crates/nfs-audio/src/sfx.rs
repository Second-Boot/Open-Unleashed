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

use crate::mixer::{AudioOutput, Clip, VoiceId};
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

pub struct CollisionSfx {
    bank: Bnk,
    data: Vec<u8>,
    clips: HashMap<usize, Arc<Clip>>,
    /// last random pick per group base (no-repeat rule 0x4A9380)
    last_pick: HashMap<usize, u8>,
    rng: u32,
    /// per-frame level gate `0x656860` (level >> 4)
    gate: i32,
    front: Option<(VoiceId, usize)>,
    rear: Option<(VoiceId, usize)>,
    /// road / tyre roll loop (voice 0x18, §4.4) and the slot it is playing
    road: Option<(VoiceId, usize)>,
    /// wind loop (voice 0x1A, slot 0x1F, §4.5)
    wind: Option<VoiceId>,
    /// `[0x5D1838]` / `[0x5D1840]` — the wind level random walk and its step
    wind_level: i32,
    wind_level_step: i32,
    /// `[0x5D183C]` / `[0x5D1844]` — the wind pitch random walk (16.16) and its step
    wind_pitch: i32,
    wind_pitch_step: i32,
    /// turbo / blower loop (voice 0, slot 0x0F, §4.6)
    turbo: Option<VoiceId>,
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

impl CollisionSfx {
    pub fn load(sounds_dir: &Path) -> anyhow::Result<Self> {
        let path = sounds_dir.join("SND_coll.bnk");
        let data = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let bank = Bnk::parse(&data)?;
        tracing::info!(slots = bank.slot_count(), "collision bank loaded");
        Ok(Self {
            bank,
            data,
            clips: HashMap::new(),
            last_pick: HashMap::new(),
            rng: 0x2545_F491,
            gate: 0,
            front: None,
            rear: None,
            road: None,
            wind: None,
            wind_level: 0x28,
            wind_level_step: 1,
            wind_pitch: 0x1_0000,
            wind_pitch_step: 10,
            turbo: None,
        })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&slot) {
            return Some(c.clone());
        }
        let pcm = self.bank.decode_slot(&self.data, slot).ok()?;
        let clip = Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: pcm.loop_range, gain: pcm.gain });
        self.clips.insert(slot, clip.clone());
        Some(clip)
    }

    fn rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(0x0001_9660).wrapping_add(0x3C6E_F35F);
        self.rng >> 8
    }

    /// `base + rand % 3`, never the previous pick of that group.
    fn pick(&mut self, base: usize) -> usize {
        let prev = self.last_pick.get(&base).copied();
        let mut r = (self.rand() % 3) as u8;
        if prev == Some(r) {
            r = (r + 1 + (self.rand() % 2) as u8) % 3;
        }
        self.last_pick.insert(base, r);
        base + r as usize
    }

    /// Call once per audio update: the one-shot gate decays by one.
    pub fn update(&mut self) {
        self.gate = (self.gate - 1).max(0);
    }

    fn play_one_shot(&mut self, out: &AudioOutput, slot: usize, level: f32, master: f32) {
        let level = level.clamp(0.0, 127.0);
        if (level as i32) >> 4 <= self.gate {
            return;
        }
        self.gate = (level as i32) >> 4;
        if let Some(clip) = self.clip(slot) {
            out.play(clip, level / 127.0 * master, 1.0, false);
        }
    }

    /// Wall hit with normal closing speed `j` (m/s) on a wall of material class `class`
    /// (0 metal, 1 wood, 2 snow, 3 ice, 4 earth, 5 rock): `vol = 3j/8`, level `64 + clamp(6.3·vol, 2, 63)`.
    pub fn wall_hit(&mut self, out: &AudioOutput, j: f32, class: u8, master: f32) {
        let mag = 3.0 * j;
        if mag <= 1.5 {
            return;
        }
        let base = [GROUP_CAR_CAR, GROUP_WALL_WOOD, GROUP_WALL_SNOW, GROUP_WALL_SNOW, GROUP_WALL_EARTH, GROUP_WALL_ROCK][(class as usize).min(5)];
        let slot = self.pick(base);
        let level = 64.0 + (mag * 0.125 * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level, master);
    }

    /// Wall scrape at speed `speed` (m/s) along a wall of material class `class`: `vol = speed/32`,
    /// level `clamp(76·vol, 2, 127)`.
    pub fn wall_scrape(&mut self, out: &AudioOutput, speed: f32, class: u8, master: f32) {
        let mag = speed * 0.25;
        if mag <= 1.5 {
            return;
        }
        let base = [GROUP_SCRAPE_METAL, 0x63, 0x66, 0x66, 0x69, GROUP_SCRAPE_ROCK][(class as usize).min(5)];
        let slot = self.pick(base);
        let level = (mag * 0.125 * 127.0 * 0.6).clamp(2.0, 127.0);
        self.play_one_shot(out, slot, level, master);
    }

    /// Car–car contact with effect magnitude `effect` (`car+0x430`): `vol = effect` (×8 of the ⅛ rule).
    pub fn car_car(&mut self, out: &AudioOutput, effect: f32, master: f32) {
        if effect <= 1.5 {
            return;
        }
        let slot = self.pick(GROUP_CAR_CAR);
        let level = 64.0 + (effect * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level, master);
    }

    /// A track prop's hit (event kind 5 with no other car, `sfx.md` §2/§4): CarBody against
    /// `0x5CC534[class]` = WoodSign, MetalSign, Glass, WoodBox, Pylon, Tree for classes 0…5 —
    /// groups 0x70, 0x73, 0x76, 0x79, 0x56, 0x53 — at `vol = 0x430 · 0.125 · 8` over the 1.5 gate
    /// (0x40F740), the level as `car_car`'s.
    pub fn prop_hit(&mut self, out: &AudioOutput, impact: f32, class: u32, master: f32) {
        if impact <= 1.5 {
            return;
        }
        let Some(base) = [0x70usize, 0x73, 0x76, 0x79, 0x56, 0x53].get(class as usize).copied() else { return };
        let slot = self.pick(base);
        let level = 64.0 + (impact * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level, master);
    }

    /// Hard landing with closing speed `closing` (> 6 m/s), level `64 + clamp(6.3·closing, 2, 63)`,
    /// on the group [`landing_group`] gives the surface under the wheels.
    pub fn landing(&mut self, out: &AudioOutput, closing: f32, surface_nibble: usize, master: f32) {
        let Some(base) = landing_group(surface_material(surface_nibble)) else { return };
        let slot = self.pick(base);
        let level = 64.0 + (closing * 6.3).clamp(2.0, 63.0);
        self.play_one_shot(out, slot, level, master);
    }

    /// Pylon (cone) hit: CarBody × Pylon group 0x56, fixed magnitude 15 → level 75 (§4.3).
    pub fn pylon_hit(&mut self, out: &AudioOutput, master: f32) {
        let slot = self.pick(GROUP_PYLON);
        self.play_one_shot(out, slot, 64.0 + (15.0 * 0.125 * 6.3f32).clamp(2.0, 63.0), master);
    }

    /// Crash / glass one-shot (damage ≥ 30 on a corner, roll-over): slot 0x20 at full level.
    pub fn crash(&mut self, out: &AudioOutput, master: f32) {
        self.play_one_shot(out, SLOT_CRASH, 127.0, master);
    }

    /// §5.3's roof / side scrape, producer **0x40FFF0**: while the car is on its side or roof
    /// (`car+0x3A0 <= 0.2`) with the body near the ground (`car+0x424 < 0.2`) and `speedEst/4 > 0.1`,
    /// it pushes `{4, 0x14, CarBody, surf, speedEst/4 · (1 − rand/3), 0}` — so the **rear** squeal
    /// channel carries a fluttering CarBody × surface loop instead of the tyres'. Upright, the
    /// caller stops it and runs the squeal producer again.
    pub fn body_scrape(&mut self, out: &AudioOutput, surface_nibble: usize, vol: f32, master: f32) {
        let slot = body_scrape_slot(surface_material(surface_nibble));
        let flutter = 1.0 - (self.rand() as f32 / 65536.0) / 3.0;
        let level = (vol * flutter * 127.0).clamp(0.0, 127.0);
        if let Some((id, _)) = self.front.take() {
            out.stop(id);
        }
        match self.rear {
            Some((id, s)) if s == slot => out.set(id, level / 127.0 * master, 1.0),
            Some((id, _)) => {
                out.stop(id);
                self.rear = None;
                if let Some(clip) = self.clip(slot) {
                    self.rear = Some((out.play(clip, level / 127.0 * master, 1.0, true), slot));
                }
            }
            None => {
                if let Some(clip) = self.clip(slot) {
                    self.rear = Some((out.play(clip, level / 127.0 * master, 1.0, true), slot));
                }
            }
        }
    }

    /// Tyre squeal loops (§5): `front` / `rear` = clamped slip sums (0..10), surface nibble of the car.
    /// `no_patch` is `Tracker::wordAt == 0` — nothing drivable under the car. 0x40FCF3…0x40FCFC
    /// then turns an **Asphalt** surface into **WetAsphalt** (slot 0x2D), which is what the tyres
    /// sound like off the road.
    #[allow(clippy::too_many_arguments)]
    pub fn squeal(&mut self, out: &AudioOutput, tyre_class: usize, surface_nibble: usize, no_patch: bool, front: f32, rear: f32, master: f32) {
        let m = match surface_material(surface_nibble) {
            Material::Asphalt if no_patch => Material::WetAsphalt,
            other => other,
        };
        let slot = squeal_slot(tyre_column(tyre_class), m);
        let front = if rear > 0.0 { front.clamp(0.0, 10.0) } else { 0.0 };
        let rear = rear.clamp(0.0, 10.0);
        for (which, vol) in [(0usize, front), (1usize, rear * 0.5)] {
            let level = (vol * 127.0).min(127.0);
            let voice = if which == 0 { &mut self.front } else { &mut self.rear };
            match voice {
                Some((id, s)) if level <= 0.0 => {
                    out.stop(*id);
                    let _ = s;
                    *voice = None;
                }
                Some((id, s)) if *s == slot => out.set(*id, level / 127.0 * master, 1.0),
                Some((id, _)) => {
                    out.stop(*id);
                    *voice = None;
                    if level > 0.0 && let Some(clip) = self.clip(slot) {
                        let id = out.play(clip, level / 127.0 * master, 1.0, true);
                        if which == 0 { self.front = Some((id, slot)) } else { self.rear = Some((id, slot)) }
                    }
                }
                None if level > 0.0 => {
                    if let Some(clip) = self.clip(slot) {
                        let id = out.play(clip, level / 127.0 * master, 1.0, true);
                        if which == 0 { self.front = Some((id, slot)) } else { self.rear = Some((id, slot)) }
                    }
                }
                None => {}
            }
        }
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
    pub fn road_roll(&mut self, out: &AudioOutput, speed: f32, nibble: usize, gain: i32, doppler: f32, pan: f32, master: f32) {
        let slot = 0x11 + nibble.min(11);
        let v = (speed.abs() as i32) * if nibble == 1 { 1 } else { 3 };
        let level = (v / 2 + 64).clamp(0, 127);
        let vol = (gain * (v - 15).max(0) / 128).min(122);
        if vol == 0 {
            if let Some((id, _)) = self.road.take() {
                out.stop(id);
            }
            return;
        }
        let amp = level as f32 / 127.0 * (vol as f32 / 127.0) * master;
        match self.road {
            Some((id, s)) if s == slot => {
                out.set(id, amp, doppler);
                out.set_pan(id, pan);
            }
            _ => {
                if let Some((id, _)) = self.road.take() {
                    out.stop(id);
                }
                if let Some(c) = self.clip(slot) {
                    self.road = Some((out.play_panned(c, amp, doppler, pan, true), slot));
                }
            }
        }
    }

    /// §4.5 wind — voice 0x1A, `SND_coll` slot 0x1F. The original plays it only from the attached
    /// exterior view (mode 1); `active` is that condition. Level and pitch are random walks stepped
    /// once per update.
    pub fn wind(&mut self, out: &AudioOutput, active: bool, speed: f32, master: f32) {
        if !active || speed.abs() < 0.5 {
            if let Some(id) = self.wind.take() {
                out.stop(id);
            }
            return;
        }
        // [0x5D1838] 0x28..0x7F, [0x5D183C] 0xFA24..0x105DC
        self.wind_level += self.wind_level_step;
        if self.wind_level > 0x7F {
            self.wind_level = 0x7F;
            self.wind_level_step = -1 - (self.rand() & 7) as i32;
        } else if self.wind_level < 0x28 {
            self.wind_level = 0x28;
            self.wind_level_step = (self.rand() & 7) as i32 + 1;
        }
        self.wind_pitch += self.wind_pitch_step;
        if self.wind_pitch > 0x1_05DC {
            self.wind_pitch = 0x1_05DC;
            self.wind_pitch_step = -20 - (self.rand() % 80) as i32;
        } else if self.wind_pitch < 0x0_FA24 {
            self.wind_pitch = 0x0_FA24;
            self.wind_pitch_step = (self.rand() % 50) as i32 + 10;
        }
        let s = (speed.abs() * 5.0) as i32;
        let vol = (s * self.wind_level / 128).min(127);
        let level = ((speed.abs() as i32) / 2 + 64).clamp(0, 127);
        let amp = level as f32 / 127.0 * (vol as f32 / 127.0) * master;
        let pitch = self.wind_pitch as f32 / 65536.0;
        match self.wind {
            Some(id) => out.set(id, amp, pitch),
            None => {
                if let Some(c) = self.clip(0x1F) {
                    self.wind = Some(out.play(c, amp, pitch, true));
                }
            }
        }
    }

    /// §4.6 turbo / blower — voice 0, `SND_coll` slot 0x0F. `boost` is the turbo state 0..1,
    /// `throttle` the raw pedal 0..255, `base` the `.spl` aux volume for the view mode and `gain` the
    /// engine distance gain. The shipped curve is silent below half the redline, jumps there and fades
    /// to nothing at 0.9 × redline — reproduced as written.
    #[allow(clippy::too_many_arguments)]
    pub fn turbo(&mut self, out: &AudioOutput, boost: f32, throttle: u8, rpm: f32, redline: f32, base: i32, gain: i32, doppler: f32, pan: f32, master: f32) {
        if boost <= 0.1 || redline <= 0.0 {
            if let Some(id) = self.turbo.take() {
                out.stop(id);
            }
            return;
        }
        let lvl = (base * gain / 128).min(127);
        let f0 = throttle as f32 * boost / 127.5;
        let g = if rpm > 0.5 * redline {
            (0.9 * redline - rpm).max(0.0) / (0.9 * redline)
        } else {
            // the low branch can never be positive with the shipped constants
            (0.5 * rpm - 0.25 * redline).max(0.0) / (0.25 * redline)
        };
        let vol = ((lvl as f32 * f0 * g) as i32).clamp(0, 127);
        if vol == 0 {
            if let Some(id) = self.turbo.take() {
                out.stop(id);
            }
            return;
        }
        let pitch = (2.5 * rpm / redline + 0.5) * doppler;
        let amp = 0x40 as f32 / 127.0 * (vol as f32 / 127.0) * master;
        match self.turbo {
            Some(id) => {
                out.set(id, amp, pitch);
                out.set_pan(id, pan);
            }
            None => {
                if let Some(c) = self.clip(0x0F) {
                    self.turbo = Some(out.play_panned(c, amp, pitch, pan, true));
                }
            }
        }
    }

    pub fn stop_loops(&mut self, out: &AudioOutput) {
        for v in [self.front.take(), self.rear.take()].into_iter().flatten() {
            out.stop(v.0);
        }
        for id in [self.wind.take(), self.turbo.take()].into_iter().flatten() {
            out.stop(id);
        }
        if let Some((id, _)) = self.road.take() {
            out.stop(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
