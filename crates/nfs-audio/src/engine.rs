//! Player engine sound — the original's fixed 32-layer CRDl mixer
//! (`RE/notes/engine-audio.md` §2–§4, CONFIRMED unless marked).
//!
//! The engine is **not** "play every loop in the bank". `0x4ACC10` loads four `CRDl` files into one
//! object in a fixed order — `0..7 = .ect`, `8..15 = .elt`, `16..23 = .cct`, `24..31 = .clt` — and the
//! mixer callback `0x4AC290` gives each layer a level from three factors: the layer's own 512-bin
//! curve indexed by engine speed, a throttle/coast weight, and a front/rear weight. The cockpit half
//! (16..31) and the exterior half (0..15) are never audible at the same time.
//!
//! Ported here: the `.spl` parameters (§3.1), the four-file layer order and the `[start, end)` window
//! (§3.2), the ring record's `bin`/`blend`/`load`/`pitch` (§3.3), the per-layer level with the 129-entry
//! curve at `0x5D1764` and the ±8-per-update slew (§3.4), the single global playback ratio (§3.5), the
//! engine start/stop one-shots and their duck of the loop bed (§3.6), the smoothed pedal "load" and the
//! gear-shift dip (§4.3), and the distance gain with its rpm boost (§4.1).
//!
//! Not ported: the stereo `pan ± spread` pair for `slotId >= 0x40` layers (each layer plays one voice
//! at the record's pan; `spread` is computed and used only to move the mono layers dead-behind as the
//! original does), the tunnel reverb of §4.7, and the road/wind/turbo layers of §4.4–§4.6, which belong
//! to `sfx.rs`.

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk, crdl::Crdl, spl::Spl};
use std::path::Path;
use std::sync::Arc;

/// `0x5D1764[0..128]` — the fixed concave weight curve both crossfades use (§3.4).
pub const WEIGHT_CURVE: [u8; 129] = [
    0, 3, 7, 10, 13, 16, 19, 22, 24, 27, 29, 31, 33, 35, 37, 39, 41, 43, 45, 46, 48, 50, 51, 53, 54, 55, 57, 58, 60, 61, 62, 63, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84,
    84, 85, 86, 87, 88, 88, 89, 90, 91, 91, 92, 93, 94, 94, 95, 96, 96, 97, 98, 98, 99, 100, 100, 101, 101, 102, 103, 103, 104, 104, 105, 106, 106, 107, 107, 108, 108, 109, 109, 110, 110, 111, 111, 112, 112,
    113, 113, 114, 114, 115, 115, 116, 116, 117, 117, 118, 118, 119, 119, 119, 120, 120, 121, 121, 122, 122, 122, 123, 123, 124, 124, 125, 125, 125, 126, 127, 127,
];

fn weight(i: i32) -> i32 {
    WEIGHT_CURVE[i.clamp(0, 128) as usize] as i32
}

/// One layer's level before the slew (`0x4AC290`, §3.4). `a` is the layer's curve value at the current
/// bin, `coast` marks the `.ect`/`.cct` half, `stereo` the `slotId >= 0x40` group.
pub fn layer_level(a: i32, coast: bool, stereo: bool, load: i32, blend: i32, spread: i32, detail: u8, rec_volume: i32) -> i32 {
    if a == 0 {
        return 0;
    }
    let w1 = if coast { weight(0x80 - load) } else { weight(load) };
    let w2 = if stereo {
        if detail > 1 {
            127
        } else if spread != 0 {
            (weight(0x80 - blend) * 94) >> 7
        } else {
            weight(0x80 - blend)
        }
    } else if detail > 1 {
        0
    } else {
        weight(blend)
    };
    ((((w2 * w1 * a) >> 3) * rec_volume) >> 18).clamp(0, 127)
}

/// One of the 32 layer slots (`layerDesc[i]`, `obj+0x2D4 + 12·i`).
#[derive(Debug, Clone)]
struct LayerDesc {
    /// bank slot / patch id; `>= 0x40` marks the stereo group
    slot: u8,
    start: usize,
    end: usize,
    /// `tableA[start..end)` — the original copies only the window (§3.2)
    table: Vec<u8>,
}

/// Everything the mixer needs for one update.
#[derive(Debug, Clone, Copy)]
pub struct EngineInput {
    pub rpm: f32,
    pub redline: f32,
    /// `car+0xD7F` throttle pedal 0..255
    pub throttle: u8,
    /// `car+0xD80` brake pedal 0..255 (the load follows it while the auto box is reversing)
    pub brake: u8,
    /// `car+0xD98` — the auto box is in its reverse state
    pub reversing: bool,
    /// `car+0xDB0` — the engine is running
    pub running: bool,
    /// `car+0x548` — the rev limiter is cutting the ignition (forces full load)
    pub rev_cut: bool,
    /// 0 = cockpit, 1 = attached exterior, 2/3 = detached camera (§4.2)
    pub mode: u8,
    /// listener→car direction against the car's forward, −1 in front of the car, +1 behind
    pub cos: f32,
    /// stereo position −1..1 (detached modes only)
    pub pan: f32,
    /// metres to the listener
    pub distance: f32,
    /// `340 / (340 + closing speed)`
    pub doppler: f32,
    /// detail setting; above 1 the directional blend is dropped and the mono layers are muted
    pub detail: u8,
    /// `[0x655F34]` 0..128 — widens the in-car spread
    pub crowding: u8,
    /// `[0x5D1748]` EngineLevel 0..127
    pub engine_level: u8,
    /// `(shift timer, specs[0x54] shift ticks)` while an upshift is in progress
    pub shift: Option<(u32, u32)>,
}

impl Default for EngineInput {
    fn default() -> Self {
        Self {
            rpm: 0.0,
            redline: 7000.0,
            throttle: 0,
            brake: 0,
            reversing: false,
            running: true,
            rev_cut: false,
            mode: 3,
            cos: 0.0,
            pan: 0.0,
            distance: 0.0,
            doppler: 1.0,
            detail: 1,
            crowding: 0,
            engine_level: 127,
            shift: None,
        }
    }
}

pub struct EngineSound {
    spl: Spl,
    /// `game.spl[6]` — the engine trim of the global settings block
    game_trim: u32,
    layers: Vec<Option<LayerDesc>>,
    clips: Vec<Option<Arc<Clip>>>,
    /// `levelState[32]`, the slew target
    level: Vec<i32>,
    voices: Vec<Option<VoiceId>>,
    /// `[0x6563C8 + 4·idx]` — the smoothed pedal value
    pedal: i32,
    /// engine start / stop one-shots per view class (bank slots 6..8 / 9..11)
    start_clips: [Option<Arc<Clip>>; 3],
    stop_clips: [Option<Arc<Clip>>; 3],
    start_voice: Option<VoiceId>,
    stop_voice: Option<VoiceId>,
    /// `obj+0x40`: the start sample has finished, so a stop sample is owed
    started: bool,
    /// Engine bank slot 2: the gear-shift click (§4.3).
    shift_click: Option<Arc<Clip>>,
    /// Engine bank slot 5: the horn loop (§6 — `0x5D14EC[0x2B] = 3` routes the horn voice to the
    /// car's own engine bank, not to `SND_coll`; this corrects `sfx.md` §4.6).
    horn_clip: Option<Arc<Clip>>,
    horn_voice: Option<VoiceId>,
}

impl EngineSound {
    /// `set` = the catalogue sound set (`sd7a`), `sim` = the `.sim` name whose `.spl` holds the
    /// per-car parameters. `sounds_dir` is `GameData/Sounds`.
    pub fn load(out: &AudioOutput, sounds_dir: &Path, set: &str, sim: &str) -> anyhow::Result<Self> {
        let _ = out;
        let viv_path = sounds_dir.join(format!("{set}.viv"));
        let data = std::fs::read(&viv_path).map_err(|e| anyhow::anyhow!("{}: {e}", viv_path.display()))?;
        let viv = Bigf::parse(&data)?;
        let bank_bytes = viv.get(&data, &format!("{set}.bnk")).ok_or_else(|| anyhow::anyhow!("{set}.bnk missing in {}", viv_path.display()))?;
        let bank = Bnk::parse(bank_bytes)?;

        // §3.2: eight layers per file, in this order, whatever the files contain
        let mut layers: Vec<Option<LayerDesc>> = vec![None; 32];
        for (file, base) in [("ect", 0usize), ("elt", 8), ("cct", 16), ("clt", 24)] {
            let Some(t) = viv.get(&data, &format!("{set}.{file}")) else {
                tracing::warn!(set, file, "engine layer file missing");
                continue;
            };
            let crdl = match Crdl::parse(t) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(set, file, error = %e, "engine layer file unreadable");
                    continue;
                }
            };
            for (i, e) in crdl.entries.iter().take(8).enumerate() {
                if e.slot == 0xFF {
                    continue;
                }
                // the window is "first non-zero .. first zero after it", exactly as the loader scans
                let Some(start) = e.table_a.iter().position(|&v| v != 0) else { continue };
                let end = e.table_a[start..].iter().position(|&v| v == 0).map(|k| start + k).unwrap_or(512);
                layers[base + i] = Some(LayerDesc { slot: e.slot, start, end, table: e.table_a[start..end].to_vec() });
            }
        }
        let decode = |slot: usize| -> Option<Arc<Clip>> {
            bank.decode_slot(bank_bytes, slot).ok().map(|p| Arc::new(Clip { rate: p.rate, channels: p.channels, samples: p.samples, loop_range: p.loop_range }))
        };
        let clips: Vec<Option<Arc<Clip>>> = layers
            .iter()
            .map(|l| match l {
                Some(d) => {
                    let c = decode(d.slot as usize);
                    if c.is_none() {
                        tracing::warn!(set, slot = d.slot, "engine layer sample missing");
                    }
                    // the layer loops for as long as it is audible
                    c.map(|c| Arc::new(Clip { loop_range: c.loop_range.or(Some((0, (c.samples.len() / c.channels.max(1) as usize) as u32))), ..(*c).clone() }))
                }
                None => None,
            })
            .collect();

        let spl = std::fs::read(sounds_dir.join(format!("{sim}.spl"))).ok().and_then(|d| Spl::parse(&d).ok()).unwrap_or_default();
        // `game.spl[6]` = the engine trim of the global settings block (16 shipped)
        let game_trim = std::fs::read(sounds_dir.join("game.spl")).ok().and_then(|d| d.get(24..28).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))).unwrap_or(16);
        let start_clips = [decode(6), decode(7), decode(8)];
        let stop_clips = [decode(9), decode(10), decode(11)];
        let n = layers.iter().filter(|l| l.is_some()).count();
        tracing::info!(set, sim, layers = n, spl = ?spl.0, game_trim, "engine sound loaded");
        Ok(Self {
            spl,
            game_trim,
            layers,
            clips,
            level: vec![0; 32],
            voices: vec![None; 32],
            pedal: 0,
            start_clips,
            stop_clips,
            start_voice: None,
            stop_voice: None,
            started: false,
            shift_click: decode(2).map(|c| Arc::new(Clip { loop_range: None, ..(*c).clone() })),
            horn_clip: decode(5).map(|c| {
                let frames = (c.samples.len() / c.channels.max(1) as usize) as u32;
                Arc::new(Clip { loop_range: c.loop_range.or(Some((0, frames))), ..(*c).clone() })
            }),
            horn_voice: None,
        })
    }

    /// Stop every voice (leaving the race).
    pub fn stop(&mut self, out: &AudioOutput) {
        for v in self.voices.iter_mut().chain([&mut self.start_voice, &mut self.stop_voice, &mut self.horn_voice]) {
            if let Some(id) = v.take() {
                out.stop(id);
            }
        }
        self.level.iter_mut().for_each(|l| *l = 0);
    }

    /// Gear-shift click (engine bank slot 2). The original fires it on the **last** tick of the shift.
    pub fn play_shift(&self, out: &AudioOutput, master: f32) {
        if let Some(c) = &self.shift_click {
            out.play(c.clone(), master, 1.0, false);
        }
    }

    /// Horn — the car's own engine bank, slot 5 held while the button is down (§6).
    pub fn horn(&mut self, out: &AudioOutput, on: bool, master: f32) {
        match (on, self.horn_voice) {
            (true, None) => {
                if let Some(c) = &self.horn_clip {
                    self.horn_voice = Some(out.play(c.clone(), master, 1.0, true));
                }
            }
            (false, Some(id)) => {
                out.stop(id);
                self.horn_voice = None;
            }
            _ => {}
        }
    }

    /// `engineGain` — the distance attenuation with its rpm boost and the shift dip (§4.1, §4.3).
    fn engine_gain(&self, inp: &EngineInput) -> i32 {
        let atten = 2048 / (inp.distance.max(0.0) as i32 + 2);
        let rpm_lvl = (inp.rpm * 127.0 / (inp.redline + 2000.0)) as i32;
        let a = atten.max(atten * (rpm_lvl.clamp(0, 150) + 40) / 128);
        let mut gain = a * 127 / 128;
        if let Some((timer, ticks)) = inp.shift {
            // 1.0 at the start of the shift falling to 0.5 at its end
            gain = (gain + timer as i32 * gain / ticks.max(1) as i32) / 2;
        }
        gain
    }

    /// One mixer update (§3.3 record, §3.4 levels, §3.5 pitch, §3.6 start/stop).
    pub fn update(&mut self, out: &AudioOutput, inp: EngineInput) {
        // §4.3: the smoothed pedal, attack /2 and release /8
        let target = if inp.reversing { inp.brake as i32 } else { inp.throttle as i32 };
        if self.pedal < target {
            self.pedal += 1.max((target - self.pedal) / 2);
        } else if self.pedal > target {
            self.pedal += (-1i32).min((target - self.pedal) / 8);
        }
        self.pedal = self.pedal.min(255);
        let mut l = self.pedal;
        if let Some((timer, _)) = inp.shift {
            l = match timer {
                5 => l * 3 / 4,
                4 => l / 2,
                3 => l / 4,
                0..=2 => 0,
                _ => l,
            };
        }
        let load_word: i32 = if !inp.running {
            -1
        } else if inp.rev_cut {
            0x7F
        } else {
            l >> 1
        };
        // `load = clamp(x + x/64 + x/32, 0, 0x80)` — the 1.0469 trim of §3.3
        let x = load_word.max(0);
        let load = (x + x / 64 + x / 32).clamp(0, 0x80);

        let rpm_frac = if inp.redline > 0.0 { inp.rpm / inp.redline } else { 0.0 };
        let bin = self.spl.bin(rpm_frac);
        let blend = self.spl.blend(inp.mode, inp.cos);
        let pitch = self.spl.pitch(rpm_frac, inp.doppler).max(0.05);
        let spread = if inp.mode < 2 { (inp.crowding as i32 * 0x3FFF) >> 7 } else { 0 };
        let pan = if inp.mode < 2 { 0.0 } else { inp.pan };

        // §4.1 master volume, §3.6 duck while the start sample plays
        let master_vol = (((inp.engine_level as i32 * self.game_trim as i32 * 85) >> 7) * self.engine_gain(&inp)) >> 14;
        let mut rec_volume = ((self.spl.record_volume(inp.mode) as i32 * master_vol) >> 6).min(0x7FF);
        rec_volume = rec_volume * self.start_stop(out, &inp, pan) / 512;

        tracing::trace!(rpm = inp.rpm, bin, load, blend, rec_volume, pitch, mode = inp.mode, audible = self.level.iter().filter(|l| **l > 0).count(), "engine record");
        for i in 0..32 {
            let Some(d) = self.layers[i].clone() else { continue };
            // mode 0 plays the cockpit half, every other mode the exterior half
            let audible_half = (inp.mode > 0) == (i < 16);
            let level = if !audible_half || bin < d.start || bin >= d.end {
                0
            } else {
                let a = d.table[bin - d.start] as i32;
                // .ect/.cct are the coast tables, .elt/.clt the load tables
                let coast = (i < 8) || (16..24).contains(&i);
                layer_level(a, coast, d.slot >= 0x40, load, blend, spread, inp.detail, rec_volume)
            };
            // ±8 per update
            self.level[i] = level.clamp(self.level[i] - 8, self.level[i] + 8);
            let lvl = self.level[i];
            // mono layers go dead-behind while the in-car spread is open
            let voice_pan = if d.slot >= 0x40 { pan } else if spread != 0 { -1.0 } else { pan };
            match (lvl, self.voices[i]) {
                (0, Some(v)) => {
                    out.stop(v);
                    self.voices[i] = None;
                }
                (0, None) => {}
                (_, Some(v)) => {
                    out.set(v, lvl as f32 / 127.0, pitch);
                    out.set_pan(v, voice_pan);
                }
                (_, None) => {
                    if let Some(c) = &self.clips[i] {
                        self.voices[i] = Some(out.play_panned(c.clone(), lvl as f32 / 127.0, pitch, voice_pan, true));
                    }
                }
            }
        }
    }

    /// §3.6: the engine start / stop one-shots. Returns the duck factor (0..512) for the loop bed.
    fn start_stop(&mut self, out: &AudioOutput, inp: &EngineInput, pan: f32) -> i32 {
        let class = (inp.mode as usize).min(2);
        let level = inp.engine_level as f32 / 127.0;
        if inp.running {
            if let Some(v) = self.stop_voice.take() {
                out.stop(v);
            }
            if !self.started {
                match self.start_voice {
                    Some(v) if out.is_playing(v) => {
                        let progress = out.progress(v).unwrap_or(1.0);
                        return 512 - (progress * 512.0) as i32;
                    }
                    Some(_) => {
                        self.start_voice = None;
                        self.started = true;
                    }
                    None => {
                        if let Some(c) = &self.start_clips[class] {
                            self.start_voice = Some(out.play_panned(c.clone(), level, 1.0, pan, false));
                            return 512;
                        }
                        self.started = true;
                    }
                }
            }
        } else {
            if let Some(v) = self.start_voice.take() {
                out.stop(v);
            }
            if self.started {
                if self.stop_voice.is_none_or(|v| !out.is_playing(v)) {
                    match &self.stop_clips[class] {
                        Some(c) => self.stop_voice = Some(out.play_panned(c.clone(), level, 1.0, pan, false)),
                        None => self.started = false,
                    }
                    if self.stop_voice.is_some_and(|v| !out.is_playing(v)) {
                        self.started = false;
                    }
                }
            }
        }
        512
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weight_curve_is_monotonic_and_spans_the_range() {
        assert_eq!(WEIGHT_CURVE[0], 0);
        assert_eq!(WEIGHT_CURVE[128], 127);
        assert!(WEIGHT_CURVE.windows(2).all(|w| w[1] >= w[0]));
        // concave: the first half of the index range already reaches two thirds of the level
        assert!(WEIGHT_CURVE[64] > 80);
        assert_eq!(weight(-5), 0);
        assert_eq!(weight(400), 127);
    }

    #[test]
    fn load_and_coast_layers_trade_places_with_the_pedal() {
        // a mid-range curve value, listener in front of the car (blend 0), at a typical volume
        let (a, blend, rec) = (110, 0, 220);
        let on = |coast, load| layer_level(a, coast, true, load, blend, 0, 1, rec);
        // closed throttle: the coast layer carries the sound, the load layer is silent
        assert!(on(true, 0) > 100, "coast at zero load = {}", on(true, 0));
        assert_eq!(on(false, 0), 0);
        // full throttle: the other way round
        assert_eq!(on(true, 0x80), 0);
        assert!(on(false, 0x80) > 100);
        // half way both are audible and neither dominates
        let (c, l) = (on(true, 0x40), on(false, 0x40));
        assert!(c > 20 && l > 20 && (c - l).abs() < 20, "coast {c} load {l}");
        // a curve value of zero is always silent, whatever the weights
        assert_eq!(layer_level(0, false, true, 0x80, blend, 0, 1, rec), 0);
    }

    #[test]
    fn the_two_groups_face_opposite_ways() {
        let rec = 220;
        // blend is 0 with the listener in front of the car and 0x80 behind it (`Spl::blend`).
        // The stereo group (slot >= 0x40) is weighted by `tbl[0x80 - blend]`, so it is the half heard
        // from the front; the mono group is weighted by `tbl[blend]` and is heard from behind.
        assert!(layer_level(110, false, true, 0x80, 0, 0, 1, rec) > 100);
        assert_eq!(layer_level(110, false, false, 0x80, 0, 0, 1, rec), 0);
        assert_eq!(layer_level(110, false, true, 0x80, 0x80, 0, 1, rec), 0);
        assert!(layer_level(110, false, false, 0x80, 0x80, 0, 1, rec) > 100);
        // at low detail the mono half is dropped entirely and the stereo half loses the blend
        assert_eq!(layer_level(110, false, false, 0x80, 0x80, 0, 2, rec), 0);
        assert_eq!(layer_level(110, false, true, 0x80, 0, 0, 2, rec), layer_level(110, false, true, 0x80, 0x80, 0, 2, rec));
        // the in-car spread trims the stereo half by 94/128
        let (plain, trimmed) = (layer_level(60, false, true, 0x80, 0, 0, 1, rec), layer_level(60, false, true, 0x80, 0, 1, 1, rec));
        assert!(trimmed < plain && trimmed > 0, "plain {plain} trimmed {trimmed}");
    }

    #[test]
    fn the_bin_and_pitch_match_the_shipped_parameters() {
        let s = Spl::default();
        // a layer window of [259, 512) only opens above 0.62 redline
        assert!(s.bin(0.6) < 259);
        assert!(s.bin(0.65) >= 259);
        // one global ratio for every layer
        assert!((s.pitch(0.6, 1.0) - 1.0).abs() < 1e-6);
        // closing on the car raises the pitch
        assert!(s.pitch(0.6, 340.0 / (340.0 - 30.0)) > 1.0);
    }
}
