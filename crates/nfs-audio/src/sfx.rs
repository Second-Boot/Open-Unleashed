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

/// Tyre squeal loop slot (§5.2) for a tyre class (0 road, 1 race, 2 rain) on a surface.
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
    horn: Option<VoiceId>,
}

impl CollisionSfx {
    pub fn load(sounds_dir: &Path) -> anyhow::Result<Self> {
        let path = sounds_dir.join("SND_coll.bnk");
        let data = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let bank = Bnk::parse(&data)?;
        tracing::info!(slots = bank.slot_count(), "collision bank loaded");
        Ok(Self { bank, data, clips: HashMap::new(), last_pick: HashMap::new(), rng: 0x2545_F491, gate: 0, front: None, rear: None, horn: None })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&slot) {
            return Some(c.clone());
        }
        let pcm = self.bank.decode_slot(&self.data, slot).ok()?;
        let clip = Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: pcm.loop_range });
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

    /// Hard landing with closing speed `closing` (> 6 m/s): Tires × Asphalt, level `64 + clamp(6.3·closing, 2, 63)`.
    pub fn landing(&mut self, out: &AudioOutput, closing: f32, master: f32) {
        let slot = self.pick(GROUP_LANDING);
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

    /// Tyre squeal loops (§5): `front` / `rear` = clamped slip sums (0..10), surface nibble of the car.
    pub fn squeal(&mut self, out: &AudioOutput, tyre_class: usize, surface_nibble: usize, front: f32, rear: f32, master: f32) {
        let slot = squeal_slot(tyre_class, surface_material(surface_nibble));
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

    /// Player horn (§4.6): the looped slot 5 while the button is held (slots 3/4 are per-car
    /// one-shot variants chosen by a car-record flag that is not modelled — MEDIUM).
    pub fn horn(&mut self, out: &AudioOutput, on: bool, master: f32) {
        match (on, self.horn) {
            (true, None) => {
                if let Some(clip) = self.clip(5) {
                    self.horn = Some(out.play(clip, master, 1.0, true));
                }
            }
            (false, Some(id)) => {
                out.stop(id);
                self.horn = None;
            }
            _ => {}
        }
    }

    pub fn stop_loops(&mut self, out: &AudioOutput) {
        for v in [self.front.take(), self.rear.take()].into_iter().flatten() {
            out.stop(v.0);
        }
        if let Some(id) = self.horn.take() {
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
        assert_eq!(surface_material(4), Material::Rock);
    }
}
