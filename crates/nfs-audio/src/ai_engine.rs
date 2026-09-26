//! Engine sound for AI and traffic cars — `AiEngine` 0x4AA970 (`RE/notes/engine-audio.md` §5, CONFIRMED).
//!
//! An AI car does not get the 32-layer CRDl mixer. It is **two looped samples crossfaded front to rear**
//! plus one road-roll loop, and nothing else: no rpm window, no load/coast pair, no shift dip. The two
//! samples are the sound set's `<set>lden.bnk` and `<set>ldex.bnk` in `Sounds/zzzwzzz.viv`; the original
//! copies them into one bank at slots `2·carIdx + 1` and `2·carIdx + 2`, which is why §5 reads that way.
//!
//! The crossfade uses the same `0x5D1764` curve as the player, centred at index 0x40:
//! `blend = clamp(96·cos, −0x40, 0x40)` (or a fixed 0x3F at low detail), and the two voices take
//! `tbl[0x40 − blend]` and `tbl[0x40 + blend]`.

use crate::engine::WEIGHT_CURVE;
use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// `SND_coll` slot 0x10 — the AI road-roll loop.
pub const AI_ROAD_SLOT: usize = 0x10;

/// `SND_coll` slots 0xA / 0xB — the AI horn (`sfx.md` §4.6, §6). `0x4A754B` plays
/// `0xA + (*(u32*)car[0x558] == 0x30)` on voice `aiIndex + 0x27`, and §6's "slots 6, 10, 11 force
/// kind 3" routes that to `SND_coll` whatever the voice table says. No car record carries `0x30`
/// in its first dword (`.TYPE` is 11 for all of them, see `AiEngines::honk`), so it is slot 0xA.
pub const AI_HORN_SLOT: usize = 0xA;

/// The horn's bend byte `0x5D1478[.TYPE]` with `.TYPE` = 11: the table reads 88, 80, 70, 65, 60,
/// 50, 85, 90, 60, 45, 90, **50**, … — about −100 cents under the patch's range (`Clip::bend`).
const HORN_BEND: i32 = 50;

fn curve(i: i32) -> i32 {
    WEIGHT_CURVE[i.clamp(0, 128) as usize] as i32
}

/// `blend` of §5: `clamp((cos + cos/2) >> 10, −0x40, 0x40)` with `cos` in 16.16.
pub fn ai_blend(cos: f32, detail: u8) -> i32 {
    if detail >= 2 {
        return 0x3F;
    }
    let c = (cos.clamp(-1.0, 1.0) * 65536.0) as i32;
    ((c + c / 2) >> 10).clamp(-0x40, 0x40)
}

/// `Traffic_pickGear` **0x408130** — the gear `0x408360` (the traffic / cop drive) stores in
/// `car+0xD82` and turns into the engine speed `car+0xDB4 = carSpecs[0x60 + 4·gear] · car+0x35C`
/// (0x408495…0x4084BD) that 0x4A7080 divides by the redline `carSpecs[0x94]` for the engine loops:
///
/// * `car+0x35C` (the speed estimate `max(|vx|,|vz|) + ¼·min`) under 2 m/s → **1** (neutral);
/// * `car+0x104C == −1` (reversing out of trouble, `traffic.md` §27.2a) → **0** (reverse);
/// * otherwise **2**, raised to `k + 1` for every forward slot `k` in `2 .. carSpecs[0x50]` whose
///   top speed `carSpecs[0x170 + 4k] · redline` (m/s per rpm × rpm) is under the speed
///   (0x408165…0x40819D) — the gear the speed calls for.
///
/// `car+0x104C` is `+1` from every spawn (0x406CF9, 0x406DAC) and only the trouble oscillator
/// (0x40A701, 0x40A82B, 0x40A8D2) sets `−1`, so a moving AI car runs the scan; `traffic.md` §24's
/// "gear 0 almost always" read `car+0x104C` the other way round. `mps_per_rpm` is `carSpecs+0x170`
/// per slot and `gear_slots` `carSpecs[0x50]` (forward gears + 2). The answer can be `gear_slots`
/// itself (faster than the top gear at the redline): the original then reads the ratio slot past
/// the last gear (0 when the `.sim` has none there; with six forward gears slot 8 is `+0x80`, the
/// final drive) — the port reads its own table's slot, and 0 past slot 7 (APPROXIMATION there).
pub fn ai_gear(speed_est: f32, reversing: bool, mps_per_rpm: &[f32], gear_slots: usize, redline: f32) -> usize {
    if speed_est < 2.0 {
        return 1;
    }
    if reversing {
        return 0;
    }
    let mut g = 2;
    for k in 2..gear_slots.min(mps_per_rpm.len()) {
        if mps_per_rpm[k] * redline < speed_est {
            g = k + 1;
        }
    }
    g
}

/// The CRAZY_HORN patterns `(len, reload, repeats)` 0x4013D0 hands `HonkPattern` by
/// `(gGameTick >> 3) & 3` (0x40142A…0x40145F, jump table 0x401568).
pub const CRAZY_HORN_PATTERNS: [(i32, i32, i32); 4] = [(4, 0x10, 8), (2, 0x20, 0xC), (7, 0x28, 0x10), (1, 0x20, 0)];

/// The oncoming traffic car's random honk, `HonkPattern(car, 2, 0x20, 8)` (0x401230…0x401237).
pub const ONCOMING_HONK_PATTERN: (i32, i32, i32) = (2, 0x20, 8);

/// What one honk-timer step asks of the horn voice `slot + 0x27`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HonkStep {
    /// `Voice_play` (0x4A754B…0x4A75A4): the timer is still up
    Play,
    /// 0x4A9890 (0x4A7527…0x4A7532): the timer has just run out
    Stop,
    /// nothing voiced this update (the timer is 0, or counting up through a pause)
    Idle,
}

/// **The honk timers of an audio slot** — `+0x20` timer, `+0x24` length, `+0x28` reload, `+0x2C`
/// repeats of the slot record `0x655D60 + 0x38·i` — and the car's horn byte `car+0xD86`, which for
/// an AI car only this pass writes (0x4A752A / 0x4A754B; the input copy 0x499279…0x499330 runs for a
/// human, `car+0x52C & 4`, only). Read to the instruction 2026-09-26, CONFIRMED.
///
/// `Honk(car, on)` 0x4A6D90 and `HonkPattern(car, len, reload, repeats)` 0x4A6DE0 look the car up
/// in the slot table (`[slot] == car`) and program these fields — a car without a slot is not
/// touched; 0x4A7DC0 zeroes `+0x20` when it gives a slot a new car (0x4A801B) or empties it
/// (0x4A80E7). `Audio_updateAiCar` 0x4A7080 steps them once per audio update ([`Self::step`]).
///
/// The one caller that matters, `0x4013D0` (`traffic.md` §12.3), runs for a traffic car on every
/// even tick: HONK_HORN (mask 8) ⇒ `Honk(car, 1)`, **else when `car+0xD86` is set ⇒ `Honk(car, 0)`**;
/// then CRAZY_HORN (mask 0x1000) ⇒ `HonkPattern` with [`CRAZY_HORN_PATTERNS`]. Played through, a
/// pattern is therefore cut after its first audible update — the `Honk(car, 0)` of the next tick
/// sees the horn byte the step set and ends it — and the fourth pattern `(1, 0x20, 0)` never sounds
/// and never ends (its length 1 runs out on the first step, its repeat count 0 goes negative and
/// reloads for ever), which holds the timer non-zero so no later pattern is taken until the slot
/// changes hands or a HONK_HORN comes. That is the binary's behaviour, reproduced, not a guess.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HonkTimer {
    pub timer: i32,
    pub len: i32,
    pub reload: i32,
    pub repeats: i32,
    /// `car+0xD86`
    pub horn: bool,
}

impl HonkTimer {
    /// `Honk(car, on)` 0x4A6D90: on ⇒ `timer = 1, len = 0` (held: a zero length never counts down);
    /// off ⇒ when the timer runs, `timer = 1, len = 1, repeats = 1` — it ends on the next step.
    pub fn honk(&mut self, on: bool) {
        if on {
            self.timer = 1;
            self.len = 0;
        } else if self.timer != 0 {
            self.timer = 1;
            self.len = 1;
            self.repeats = 1;
        }
    }

    /// `HonkPattern(car, len, reload, repeats)` 0x4A6DE0: taken only while the timer is 0 —
    /// `timer = len = len`, `+0x28 = −reload`, `+0x2C = repeats` (0x4A6E03…0x4A6E14).
    pub fn pattern(&mut self, len: i32, reload: i32, repeats: i32) {
        if self.timer == 0 {
            self.timer = len;
            self.len = len;
            self.reload = -reload;
            self.repeats = repeats;
        }
    }

    /// One step of 0x4A750A…0x4A75BF (the `car+0x52C & 4` clear arm — every car but a human):
    /// ```text
    /// if timer > 0:
    ///     if len > 0: timer −= 1
    ///     if timer == 0: horn = 0 ; stop ; if −−repeats != 0: timer = reload      ; (a negative pause)
    ///     else:          horn = 1 ; Voice_play
    /// elif timer < 0:  if ++timer == 0: timer = len
    /// ```
    pub fn step(&mut self) -> HonkStep {
        if self.timer > 0 {
            if self.len > 0 {
                self.timer -= 1;
            }
            if self.timer == 0 {
                self.horn = false;
                self.repeats = self.repeats.wrapping_sub(1);
                if self.repeats != 0 {
                    self.timer = self.reload;
                }
                HonkStep::Stop
            } else {
                self.horn = true;
                HonkStep::Play
            }
        } else {
            if self.timer < 0 {
                self.timer += 1;
                if self.timer == 0 {
                    self.timer = self.len;
                }
            }
            HonkStep::Idle
        }
    }
}

#[derive(Default)]
struct CarVoices {
    near: Option<VoiceId>,
    far: Option<VoiceId>,
    road: Option<VoiceId>,
    horn: Option<VoiceId>,
    set: String,
}

pub struct AiEngines {
    /// `<set>` → (lden, ldex) samples
    sets: HashMap<String, (Option<Arc<Clip>>, Option<Arc<Clip>>)>,
    road: Option<Arc<Clip>>,
    horn: Option<Arc<Clip>>,
    cars: Vec<CarVoices>,
    /// the honk timers of each audio slot ([`HonkTimer`])
    honks: Vec<HonkTimer>,
    viv: Vec<u8>,
    /// `game.spl[12]` — the mix level of `SND_coll` slots 0x0A / 0x0B (the horn)
    horn_trim: u32,
    /// `game.spl[10]` / `[11]` — the AI engine bank's and the road loop's (slot 0x10) mix levels
    engine_trim: u32,
    road_trim: u32,
}

/// 0x4A9C10's mixer volume, `(categoryLevel · game.spl[k] · vol · 85) >> 21`, as a gain.
fn mix(category: u8, trim: u32, vol: i32) -> f32 {
    ((category as i32 * trim as i32 * vol * 85) >> 21).clamp(0, 127) as f32 / 127.0
}

impl AiEngines {
    pub fn load(sounds_dir: &Path) -> anyhow::Result<Self> {
        let viv = std::fs::read(sounds_dir.join("zzzwzzz.viv"))?;
        Bigf::parse(&viv)?; // fail early when the archive is unreadable
        let coll = std::fs::read(sounds_dir.join("SND_coll.bnk")).ok();
        let slot = |n: usize| -> Option<Arc<Clip>> {
            let d = coll.as_ref()?;
            let p = Bnk::parse(d).ok()?.decode_slot(d, n).ok()?;
            Some(Arc::new(Clip { rate: p.rate, channels: p.channels, samples: p.samples, loop_range: p.loop_range, gain: p.gain, bend_cents: p.bend_cents, fx_send: p.fx_send }))
        };
        let (road, horn) = (slot(AI_ROAD_SLOT), slot(AI_HORN_SLOT));
        let spl = nfs_formats::spl::GameSpl::load(sounds_dir);
        Ok(Self { sets: HashMap::new(), road, horn, cars: Vec::new(), honks: Vec::new(), viv, horn_trim: spl.slots_0a0b(), engine_trim: spl.ai_engine(), road_trim: spl.ai_road() })
    }

    fn samples(&mut self, set: &str) -> (Option<Arc<Clip>>, Option<Arc<Clip>>) {
        if let Some(s) = self.sets.get(set) {
            return s.clone();
        }
        let key = set.to_lowercase();
        let load = |name: &str| -> Option<Arc<Clip>> {
            let viv = Bigf::parse(&self.viv).ok()?;
            let bytes = viv.get(&self.viv, name)?;
            let bank = Bnk::parse(bytes).ok()?;
            let p = bank.decode_slot(bytes, 0).ok()?;
            let frames = (p.samples.len() / p.channels.max(1) as usize) as u32;
            Some(Arc::new(Clip { rate: p.rate, channels: p.channels, samples: p.samples, loop_range: p.loop_range.or(Some((0, frames))), gain: p.gain, bend_cents: p.bend_cents, fx_send: p.fx_send }))
        };
        let pair = (load(&format!("{key}lden.bnk")), load(&format!("{key}ldex.bnk")));
        if pair.0.is_none() && pair.1.is_none() {
            tracing::warn!(set, "no AI engine bank in zzzwzzz.viv");
        } else {
            tracing::debug!(set, near = pair.0.is_some(), far = pair.1.is_some(), "AI engine bank");
        }
        self.sets.insert(set.to_string(), pair.clone());
        pair
    }

    /// One update for AI car `index`. `rpm_frac` is rpm/redline clamped to 0..2, `distance` metres to
    /// the listener, `cos` the listener→car direction against the car's forward, `pan` −1..1,
    /// `speed` |v_long| m/s, `crowding` `[0x655F34]` 0..128.
    #[allow(clippy::too_many_arguments)]
    /// `spline_cam` is `word[0x627F3C] == 0x0B`: on that one view 0x4AA973…0x4AA99F divides
    /// `0x10000000` by `d + 1 m` instead of `0x8000000` by `d + 2 m`, so an AI car is about twice
    /// as loud there (`engine-audio.md` §5). This port had only the second form.
    ///
    /// The mix is 0x4A9C10's: the two engine loops (bank `[0x6564C0]`) as `EngineLevel ·
    /// game.spl[10]`, the road loop (slot 0x10) as `AmbientLevel · game.spl[11]`. The port had both
    /// at `EngineLevel / 255` with no trim until 2026-09-24.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, out: &AudioOutput, index: usize, set: &str, rpm_frac: f32, distance: f32, cos: f32, pan: f32, speed: f32, doppler: f32, detail: u8, crowding: u8, spline_cam: bool, engine_level: u8, ambient_level: u8) {
        if self.cars.len() <= index {
            self.cars.resize_with(index + 1, CarVoices::default);
        }
        // of the fractional distance (`crate::fix_atten`); the port took whole metres until 2026-09-26
        let atten = if spline_cam { crate::fix_atten(0x1000_0000, 0x1_0000, 0, distance) } else { crate::fix_atten(0x800_0000, 0x2_0000, 0, distance) };
        if atten == 0 {
            // 0x4AA9C0…0x4AAAA4 releases the engine voices `v`, `v + 4`, `v + 8` only
            let c = &mut self.cars[index];
            for v in [c.near.take(), c.far.take(), c.road.take()].into_iter().flatten() {
                out.stop(v);
            }
            return;
        }
        if self.cars[index].set != set {
            self.stop(out, index);
            self.cars[index].set = set.to_string();
        }
        let (near_clip, far_clip) = self.samples(set);
        let blend = ai_blend(cos, detail);
        let pitch = (rpm_frac.clamp(0.0, 2.0) + 0.2) * doppler * 1.25;
        let v_near = ((curve(0x40 - blend) * atten) >> 7).min(127);
        let v_far = ((curve(0x40 + blend) * atten) >> 7).min(127);
        let road_vol = if crowding == 0 {
            0
        } else {
            let s = speed.abs().min(40.0);
            ((((atten as f32 * s * 0.4 * 65536.0) as i32 >> 20) * crowding as i32) >> 7).min(127)
        };

        for (slot, clip, vol) in [(0usize, near_clip, v_near), (1, far_clip, v_far)] {
            let amp = mix(engine_level, self.engine_trim, vol);
            let voice = if slot == 0 { &mut self.cars[index].near } else { &mut self.cars[index].far };
            match (*voice, clip) {
                (Some(id), _) => {
                    out.set(id, amp, pitch.max(0.05));
                    out.set_pan(id, pan);
                }
                (None, Some(c)) => *voice = Some(out.play_panned(c, amp, pitch.max(0.05), pan, true)),
                (None, None) => {}
            }
        }
        // the road loop plays at the car's own pan; its third argument is `rpmFrac` (16.16), which
        // Voice_play clamps to 127 (0x4A9C1F) as the **bend byte** — so it sounds bent up by nearly
        // the patch's whole range once the engine turns at all. The port had read it as headroom.
        let road_bend = self.road.as_ref().map_or(1.0, |c| c.bend(((rpm_frac.clamp(0.0, 2.0) * 65536.0) as i32).min(127)));
        match (self.cars[index].road, road_vol, &self.road) {
            (Some(id), 0, _) => {
                out.stop(id);
                self.cars[index].road = None;
            }
            (Some(id), v, _) => {
                out.set(id, mix(ambient_level, self.road_trim, v), doppler * road_bend);
                out.set_pan(id, pan);
            }
            (None, v, Some(c)) if v > 0 => self.cars[index].road = Some(out.play_panned(c.clone(), mix(ambient_level, self.road_trim, v), doppler * road_bend, pan, true)),
            _ => {}
        }
    }

    /// The AI horn of `sfx.md` §4.6 — the loop an AI car sounds while its `scripts.bin` `HONK_HORN`
    /// action is running (`traffic.md` §12.3: `0x4013D0` calls `Honk(car, 1)` on mask 8, and
    /// `0x4A754B` keeps the loop going while the honk timer `audioSlot+0x20` has not run out).
    ///
    /// The volume is the siren's distance law (0x4A7554…0x4A7592: `(0x1324 − min(m², 0x1324)) · 127
    /// / 0x1324`, silent from 70 m) in the slot-0x0A/0x0B category `(AmbientLevel · game.spl[12] ·
    /// vol · 85) >> 21` of 0x4A9C10; the pitch is `pitchBase`, 1.0 in this build. This port had the
    /// engine loops' `2048/(d+2)` in the engine level until 2026-09-24. The bend byte is
    /// `0x5D1478[.TYPE]` and the slot `0xA + (.TYPE == 0x30)`; `.TYPE` (registry id 0x21F, the car
    /// record's `+0x00`) is **11** for every car — `fe.txt` sets `car[i].type=11`, 0x4EEDA0 streams
    /// `0x21F = 0xB` for each FE-configured car (0x4EEFAC…0x4EEFB5), and 0x4A17F0 builds the world
    /// props' records with `+0x00 = 0x0B` — so the bend is `0x5D1478[11]` = 50 and the slot 0xA.
    /// *(Until 2026-09-25 the port played the horn unbent, taking `.TYPE` for an unknown per-model id.)*
    pub fn honk(&mut self, out: &AudioOutput, index: usize, on: bool, distance: f32, pan: f32, ambient_level: u8) {
        if self.cars.len() <= index {
            self.cars.resize_with(index + 1, CarVoices::default);
        }
        let v = crate::sfx::siren_volume(distance);
        let vol = ((ambient_level as i32 * self.horn_trim as i32 * v * 85) >> 21).clamp(0, 127);
        let amp = vol as f32 / 127.0;
        let pitch = self.horn.as_ref().map_or(1.0, |c| c.bend(HORN_BEND));
        match (self.cars[index].horn, on && vol > 0, &self.horn) {
            (Some(id), true, _) => {
                out.set(id, amp, pitch);
                out.set_pan(id, pan);
            }
            (Some(id), false, _) => {
                out.stop(id);
                self.cars[index].horn = None;
            }
            (None, true, Some(c)) => self.cars[index].horn = Some(out.play_panned(c.clone(), amp, pitch, pan, true)),
            _ => {}
        }
    }

    /// The honk timers of audio slot `index`, for the `Honk` / `HonkPattern` calls of 0x4013D0.
    pub fn honk_timer(&mut self, index: usize) -> &mut HonkTimer {
        if self.honks.len() <= index {
            self.honks.resize_with(index + 1, HonkTimer::default);
        }
        &mut self.honks[index]
    }

    /// 0x4A7DC0 gives slot `index` to another car or empties it: its voices stop (0x4A7FC8…0x4A8005,
    /// 0x4A808B…0x4A80C6) and its honk timer is zeroed (0x4A801B / 0x4A80E7). The horn byte is the
    /// car's, but a stale one only ever meets a zero timer, where `Honk(car, 0)` does nothing.
    pub fn release(&mut self, out: &AudioOutput, index: usize) {
        self.stop(out, index);
        if let Some(h) = self.honks.get_mut(index) {
            *h = HonkTimer::default();
        }
    }

    pub fn stop(&mut self, out: &AudioOutput, index: usize) {
        let Some(c) = self.cars.get_mut(index) else { return };
        for v in [c.near.take(), c.far.take(), c.road.take(), c.horn.take()].into_iter().flatten() {
            out.stop(v);
        }
    }

    pub fn stop_all(&mut self, out: &AudioOutput) {
        for i in 0..self.cars.len() {
            self.stop(out, i);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_mix_is_the_category_formula() {
        // (level · game.spl[k] · vol · 85) >> 21: the shipped spl[10] = 60 puts a full AI loop at 39
        assert_eq!(mix(127, 60, 127), 39.0 / 127.0);
        assert_eq!(mix(0, 60, 127), 0.0);
    }

    use super::*;

    fn run(t: &mut HonkTimer, n: usize) -> String {
        (0..n).map(|_| match t.step() { HonkStep::Play => 'P', HonkStep::Stop => 's', HonkStep::Idle => '.' }).collect()
    }

    #[test]
    fn a_pattern_played_through_honks_len_minus_one_updates_then_pauses_reload() {
        // (4, 0x10, 8) with nothing cutting it: 3 updates on, the stop, 15 updates of pause, the
        // reload update, and again — 8 times
        let mut t = HonkTimer::default();
        t.pattern(4, 0x10, 8);
        assert_eq!((t.timer, t.len, t.reload, t.repeats), (4, 4, -0x10, 8));
        let cycle = format!("PPPs{}", ".".repeat(0x10));
        assert_eq!(run(&mut t, cycle.len() * 8), cycle.repeat(8));
        assert_eq!(t.timer, 0, "the eighth stop does not reload");
        // a pattern is taken only while the timer is 0
        t.honk(true);
        t.pattern(7, 0x28, 0x10);
        assert_eq!((t.timer, t.len), (1, 0));
    }

    #[test]
    fn honk_on_holds_and_honk_off_ends_on_the_next_step() {
        let mut t = HonkTimer::default();
        t.honk(true);
        assert_eq!(run(&mut t, 5), "PPPPP");
        assert!(t.horn);
        t.honk(false);
        assert_eq!(run(&mut t, 3), "s..");
        assert_eq!(t, HonkTimer { timer: 0, len: 1, reload: 0, repeats: 0, horn: false });
        // off with the timer at 0 does nothing
        t.honk(false);
        assert_eq!(t.timer, 0);
    }

    #[test]
    fn the_controls_handler_cuts_a_pattern_after_one_update() {
        // 0x4013D0 each tick before the step: HONK_HORN off ⇒ `Honk(car, 0)` when the horn byte is
        // set, then the pattern (the oncoming honk's (2, 0x20, 8) here)
        let mut t = HonkTimer::default();
        let mut out = String::new();
        for _ in 0..6 {
            if t.horn {
                t.honk(false);
            }
            let (l, r, n) = ONCOMING_HONK_PATTERN;
            t.pattern(l, r, n);
            out.push(match t.step() {
                HonkStep::Play => 'P',
                HonkStep::Stop => 's',
                HonkStep::Idle => '.',
            });
        }
        // one update on, one off, and the pattern taken again
        assert_eq!(out, "PsPsPs");
    }

    #[test]
    fn the_fourth_crazy_horn_pattern_is_silent_and_never_ends() {
        let mut t = HonkTimer::default();
        let (l, r, n) = CRAZY_HORN_PATTERNS[3];
        t.pattern(l, r, n);
        let s = run(&mut t, 200);
        assert!(!s.contains('P'), "{s}");
        assert_ne!(t.timer, 0, "the timer stays busy, so no other pattern is taken");
        assert!(t.repeats < 0);
    }

    #[test]
    fn the_ai_gear_follows_the_speed() {
        // slots: reverse, neutral, then four forward gears topping out at 10, 20, 30, 40 m/s at 5000 rpm
        let mps = [0.0, 0.1, 0.002, 0.004, 0.006, 0.008];
        assert_eq!(ai_gear(1.9, false, &mps, 6, 5000.0), 1, "neutral under 2 m/s");
        assert_eq!(ai_gear(15.0, true, &mps, 6, 5000.0), 0, "reverse while backing out");
        assert_eq!(ai_gear(5.0, false, &mps, 6, 5000.0), 2);
        assert_eq!(ai_gear(15.0, false, &mps, 6, 5000.0), 3);
        assert_eq!(ai_gear(35.0, false, &mps, 6, 5000.0), 5);
        // past the top gear's redline speed the scan runs one past the last slot
        assert_eq!(ai_gear(45.0, false, &mps, 6, 5000.0), 6);
    }

    #[test]
    fn the_crossfade_follows_the_listener() {
        // in front of the car (cos = −1) the near voice takes the whole level, behind it the far one
        assert_eq!(ai_blend(-1.0, 1), -0x40);
        assert_eq!(ai_blend(1.0, 1), 0x40);
        assert_eq!(ai_blend(0.0, 1), 0);
        // two thirds of the way round is already saturated (the slope is 96, not 64)
        assert_eq!(ai_blend(0.7, 1), 0x40);
        // low detail pins it just short of the end of the curve
        assert_eq!(ai_blend(-1.0, 2), 0x3F);
        let (a, b) = (curve(0x40 - ai_blend(-1.0, 1)), curve(0x40 + ai_blend(-1.0, 1)));
        assert!(a > b && b == 0, "near {a} far {b}");
    }
}
