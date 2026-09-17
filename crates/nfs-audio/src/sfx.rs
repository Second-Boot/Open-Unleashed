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

    /// §4.4 road / tyre roll — voice 0x18, `SND_coll` slots 0x11..0x1C picked by the surface nibble
    /// (or 0x15 while the handbrake is locked). `gain` is the engine distance gain `A` of §4.1 and
    /// `doppler` the same factor the engine layers use.
    pub fn road_roll(&mut self, out: &AudioOutput, speed: f32, nibble: usize, handbrake: bool, gain: i32, doppler: f32, pan: f32, master: f32) {
        let slot = 0x11 + if handbrake { 4 } else { nibble.min(11) };
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
        assert_eq!(surface_material(4), Material::Rock);
    }
}
