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
/// CarBody × Bush (`sfx.md` §3): three one-shots from 0x7C.
pub const GROUP_BUSH: usize = 0x7C;

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
}

/// 0x4A98E0 / 0x4A9C10's mixer volume `(categoryLevel · game.spl[k] · vol · 85) >> 21`, as the
/// gain that multiplies `vol / 127`.
fn category(level: u8, trim: u32) -> f32 {
    level as f32 * trim as f32 * 85.0 / 2_097_152.0
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
            sirens: HashMap::new(),
            siren_trim: nfs_formats::spl::GameSpl::load(sounds_dir).loops_6564d0(),
            spl: nfs_formats::spl::GameSpl::load(sounds_dir),
            sfx_level: 127,
            engine_level: 127,
            reverb: 8,
            atten: 256,
        })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&slot) {
            return Some(c.clone());
        }
        let pcm = self.bank.decode_slot(&self.data, slot).ok()?;
        let clip = Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: pcm.loop_range, gain: pcm.gain, bend_cents: pcm.bend_cents });
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

    /// The sliders, the tunnel reverb level (`reverb.rs`) the categories read, and the listener's
    /// distance to the player car with whether the view is 0x0C, for the queue's attenuation.
    /// APPROXIMATION: the queue measures to each event's own point; the port uses the car's.
    pub fn set_levels(&mut self, sfx_level: u8, engine_level: u8, reverb: i32, distance: f32, view_0c: bool) {
        self.sfx_level = sfx_level;
        self.engine_level = engine_level;
        self.reverb = reverb;
        let d = distance.max(0.0);
        self.atten = if view_0c { (2048.0 / (d + 1.0)) as i32 * 4 } else { (512.0 / (d + 2.0)) as i32 };
    }

    /// The default category, `SfxLevel · game.spl[9]` — everything in this bank but the slots below.
    fn sfx_gain(&self) -> f32 {
        category(self.sfx_level, self.spl.sfx())
    }

    /// A one-shot's gain (0x4A98E0): the default category, and for slots from 0x30 (the collision
    /// groups) the volume further scaled by `(0x96 − reverb) / 150` (0x4A9A56…0x4A9A90), so a hit
    /// in a tunnel (reverb 100) is a third as loud as outside (8).
    fn one_shot_gain(&self, slot: usize) -> f32 {
        let g = self.sfx_gain();
        if slot >= 0x30 { g * (0x96 - self.reverb) as f32 / 150.0 } else { g }
    }

    /// Call once per audio update: the one-shot gate decays by one.
    pub fn update(&mut self) {
        self.gate = (self.gate - 1).max(0);
    }

    /// The collision one-shot of 0x4A93C0 (chan −1): `level` only decides the gate; the call is
    /// `Voice_play(0x33, slot, bend 0x40, 0x10000, vol = atten, pan, aux = level)` (0x4A94D5…0x4A94EA),
    /// so the loudness is the distance's, and `level` goes on as the aux byte, which 0x4A9A5F turns
    /// into `0x7F − level` in the request's `+0xA` → `voice+0x45` (0x5906D7; its consumer inside the
    /// mixer is not traced — not a volume: that is `voice+0x2C`). The port had `level` as the gain.
    fn play_one_shot(&mut self, out: &AudioOutput, slot: usize, level: f32) {
        let level = level.clamp(0.0, 127.0);
        if (level as i32) >> 4 <= self.gate {
            return;
        }
        self.gate = (level as i32) >> 4;
        let vol = self.atten.clamp(0, 127);
        if let Some(clip) = self.clip(slot) {
            out.play(clip, vol as f32 / 127.0 * self.one_shot_gain(slot), 1.0, false);
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
    /// on the group [`landing_group`] gives the surface under the wheels.
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
    /// of `SND_coll` straight through 0x4A98E0 at level 0x7F, pan 0x40, pitch 0x10000.
    pub fn trigger_chime(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(2) {
            out.play(clip, self.sfx_gain(), 1.0, false);
        }
    }

    /// The "wrong way" cue 0x4AAE70 (0x4A7744): slot 1 of `SND_coll` straight at level 0x28.
    pub fn wrong_way_cue(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(1) {
            out.play(clip, 0x28 as f32 / 127.0 * self.sfx_gain(), 1.0, false);
        }
    }

    /// The police siren for one audio slot (0x4A75C2…0x4A765C): a cop (`0x52C & 0x20`) with its
    /// lights on (`0xECC & 2`) starts [`siren_slot`] once (0x4AAE90 — volume 0, level byte
    /// `0x5D1878[slot]` = 0x20) and then every update 0x4AAFB0 restarts it if it has finished (the
    /// three samples loop, so it never does), sets the bend byte `min(pitchBase · 64 >> 16, 127)`
    /// — 64, no bend, since the listener velocity `[0x655E44]` is NULL and `pitchBase` is 1.0 — the volume
    /// `(AmbientLevel · game.spl[5] · v · 85) >> 21` with `v` = [`siren_volume`], and the pan.
    /// Lights off releases the voice (0x4AAF50, 0x4A9890).
    pub fn siren(&mut self, out: &AudioOutput, audio_slot: usize, on: bool, distance: f32, pan: f32, ambient_level: u8) {
        if !on {
            if let Some(id) = self.sirens.remove(&audio_slot) {
                out.stop(id);
            }
            return;
        }
        let v = siren_volume(distance);
        let vol = ((ambient_level as i32 * self.siren_trim as i32 * v * 85) >> 21).clamp(0, 127);
        // the bend byte is 64: no bend
        let amp = vol as f32 / 127.0;
        match self.sirens.get(&audio_slot) {
            Some(&id) => {
                out.set(id, amp, 1.0);
                out.set_pan(id, pan);
            }
            None => {
                // the start plays at volume 0; the first update brings the level in
                if let Some(c) = self.clip(siren_slot(audio_slot, false)) {
                    self.sirens.insert(audio_slot, out.play_panned(c, 0.0, 1.0, pan, true));
                }
            }
        }
    }

    /// Cone hit, from the HUD's cone count (`skill-tests.md` §8): **0x4AAE50** plays slot 0 of the
    /// kind-3 bank `[0x6564D0]` (`SND_coll.bnk`) straight through 0x4A98E0 at level 0x7F, pan 0x40,
    /// pitch 0x10000 — not through the collision queue, so no pair lookup and no one-shot gate.
    /// (The port had CarBody × Pylon, group 0x56, at the gated fixed-15 level.)
    pub fn pylon_hit(&mut self, out: &AudioOutput) {
        if let Some(clip) = self.clip(0) {
            out.play(clip, self.sfx_gain(), 1.0, false);
        }
    }

    /// Crash / glass one-shot (damage ≥ 30 on a corner, roll-over): slot 0x20 at full level.
    pub fn crash(&mut self, out: &AudioOutput) {
        self.play_one_shot(out, SLOT_CRASH, 127.0);
    }

    /// §5.3's roof / side scrape, producer **0x40FFF0**: while the car is on its side or roof
    /// (`car+0x3A0 <= 0.2`) with the body near the ground (`car+0x424 < 0.2`) and `speedEst/4 > 0.1`,
    /// it pushes `{4, 0x14, CarBody, surf, speedEst/4 · (1 − rand/3), 0}` — so the **rear** squeal
    /// channel carries a fluttering CarBody × surface loop instead of the tyres'. Upright, the
    /// caller stops it and runs the squeal producer again.
    pub fn body_scrape(&mut self, out: &AudioOutput, surface_nibble: usize, vol: f32) {
        let slot = body_scrape_slot(surface_material(surface_nibble));
        let flutter = 1.0 - (self.rand() as f32 / 65536.0) / 3.0;
        if let Some((id, _)) = self.front.take() {
            out.stop(id);
        }
        self.loop_voice(out, 1, slot, vol * flutter);
    }

    /// One squeal-channel loop (0x4A93C0, chan ≥ 0, voices 0x12…0x15): for a strength `v` (the
    /// clamped slip sum, or the scrape's `speedEst/4`) the bend byte is `min(12.7·v, 127)`
    /// (0x4A96BC…0x4A96E0) — the squeal rises in pitch as it gets harder — and the volume
    /// `atten · min(127·v, 127) / 128` (0x4A96FA…0x4A9724); `v ≤ 0` releases it. The port had
    /// `min(127·v, 127)` as a gain with no distance until 2026-09-24.
    fn loop_voice(&mut self, out: &AudioOutput, which: usize, slot: usize, v: f32) {
        let master = self.sfx_gain();
        let v16 = (v.max(0.0) * 65536.0) as i64;
        let bend = ((v16 * 127 / 10) >> 16).min(127) as i32;
        let level = ((v16 * 127) >> 16).min(127) as i32;
        let vol = ((self.atten * level) >> 7).clamp(0, 127);
        let voice = if which == 0 { &mut self.front } else { &mut self.rear };
        if v <= 0.0 {
            if let Some((id, _)) = voice.take() {
                out.stop(id);
            }
            return;
        }
        let amp = vol as f32 / 127.0 * master;
        match *voice {
            Some((id, s)) if s == slot => {
                let pitch = self.clips.get(&slot).map_or(1.0, |c| c.bend(bend));
                out.set(id, amp, pitch);
            }
            other => {
                if let Some((id, _)) = other {
                    out.stop(id);
                }
                let voice_slot = self.clip(slot).map(|clip| {
                    let pitch = clip.bend(bend);
                    (out.play(clip, amp, pitch, true), slot)
                });
                if which == 0 { self.front = voice_slot } else { self.rear = voice_slot }
            }
        }
    }


    /// Tyre squeal loops (§5): `front` / `rear` = clamped slip sums (0..10), surface nibble of the car.
    /// `no_patch` is `Tracker::wordAt == 0` — nothing drivable under the car. 0x40FCF3…0x40FCFC
    /// then turns an **Asphalt** surface into **WetAsphalt** (slot 0x2D), which is what the tyres
    /// sound like off the road.
    #[allow(clippy::too_many_arguments)]
    pub fn squeal(&mut self, out: &AudioOutput, tyre_class: usize, surface_nibble: usize, no_patch: bool, front: f32, rear: f32) {
        let m = match surface_material(surface_nibble) {
            Material::Asphalt if no_patch => Material::WetAsphalt,
            other => other,
        };
        let slot = squeal_slot(tyre_column(tyre_class), m);
        let front = if rear > 0.0 { front.clamp(0.0, 10.0) } else { 0.0 };
        let rear = rear.clamp(0.0, 10.0);
        self.loop_voice(out, 0, slot, front);
        self.loop_voice(out, 1, slot, rear * 0.5);
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
    pub fn road_roll(&mut self, out: &AudioOutput, speed: f32, nibble: usize, gain: i32, doppler: f32, pan: f32) {
        let master = category(self.sfx_level, self.spl.road_roll());
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
        // `level` is the bend byte: the roll rises in pitch with speed (`Clip::bend`)
        let amp = vol as f32 / 127.0 * master;
        match self.road {
            Some((id, s)) if s == slot => {
                let pitch = self.clips.get(&slot).map_or(1.0, |c| c.bend(level)) * doppler;
                out.set(id, amp, pitch);
                out.set_pan(id, pan);
            }
            _ => {
                if let Some((id, _)) = self.road.take() {
                    out.stop(id);
                }
                if let Some(c) = self.clip(slot) {
                    let pitch = c.bend(level) * doppler;
                    self.road = Some((out.play_panned(c, amp, pitch, pan, true), slot));
                }
            }
        }
    }

    /// §4.5 wind — voice 0x1A, `SND_coll` slot 0x1F. The original plays it only from the attached
    /// exterior view (mode 1); `active` is that condition. Level and pitch are random walks stepped
    /// once per update.
    pub fn wind(&mut self, out: &AudioOutput, active: bool, speed: f32) {
        let master = self.sfx_gain();
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
        // `level` is the bend byte on top of the random-walk pitch
        let amp = vol as f32 / 127.0 * master;
        let pitch = self.wind_pitch as f32 / 65536.0;
        match self.wind {
            Some(id) => {
                let bend = self.clips.get(&0x1F).map_or(1.0, |c| c.bend(level));
                out.set(id, amp, pitch * bend)
            }
            None => {
                if let Some(c) = self.clip(0x1F) {
                    let bend = c.bend(level);
                    self.wind = Some(out.play(c, amp, pitch * bend, true));
                }
            }
        }
    }

    /// §4.6 turbo / blower — voice 0, `SND_coll` slot 0x0F. `boost` is the turbo state 0..1,
    /// `throttle` the raw pedal 0..255, `base` the `.spl` aux volume for the view mode and `gain` the
    /// engine distance gain. The shipped curve is silent below half the redline, jumps there and fades
    /// to nothing at 0.9 × redline — reproduced as written.
    #[allow(clippy::too_many_arguments)]
    pub fn turbo(&mut self, out: &AudioOutput, boost: f32, throttle: u8, rpm: f32, redline: f32, base: i32, gain: i32, doppler: f32, pan: f32) {
        let master = category(self.engine_level, self.spl.turbo());
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
        // bend byte 0x40: no bend
        let amp = vol as f32 / 127.0 * master;
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
        for (_, id) in self.sirens.drain() {
            out.stop(id);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_category_gain_matches_the_integer_mix() {
        // (127 · 107 · 127 · 85) >> 21 = 69 of 127 for a full default-category sound
        let g = super::category(127, 107);
        assert_eq!((127.0 * g) as i32, (127 * 107 * 127 * 85) >> 21);
        assert_eq!(super::category(0, 107), 0.0);
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
    }

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
