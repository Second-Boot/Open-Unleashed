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
//! The stereo `pan ± spread` pair of §3.4 is played as two voices, each at the layer's full level
//! (0x4AC660 starts the second from the same request, level byte `bl`; 0x4AC69F sets it `ebx`), and
//! `spread` also moves the mono layers dead-behind, as the original does. *(Until 2026-09-27 the
//! port halved both voices of a pair — no source.)* The tunnel reverb of §4.7 lives in
//! `reverb.rs`. The road, wind and turbo layers of §4.4–§4.6 live in `sfx.rs`.

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
    /// the car's 16-bit pan angle (`mixer::pan_angle`; used in the detached modes only)
    pub pan: u16,
    /// metres to the listener
    pub distance: f32,
    /// `340 / (340 + closing speed)`, taken as the 16.16 `pitchBase` word `(doppler · 65536)` truncated (0x4A781D `_ftol`)
    pub doppler: f32,
    /// detail setting; above 1 the directional blend is dropped and the mono layers are muted
    pub detail: u8,
    /// `[0x655F34]` 0..128 — widens the in-car spread
    pub crowding: u8,
    /// `[0x5D1748]` EngineLevel 0..127
    pub engine_level: u8,
    /// `(shift timer, specs[0x54] shift ticks)` while an upshift is in progress
    pub shift: Option<(u32, u32)>,
    /// `[ebp+0x13]` of 0x4AA050: `0x443EE0()`'s answer, stored **only** on the jump table's
    /// default arm (views 1, 5…14, > 15) when `byte[0x627F43] & 8` (viewId <= 1) — so view 1 alone
    /// (0x4AA264, 0x4AA28E…0x4AA2B9; views 0 and 15 take the mode-1 arm 0x4AA279 and leave it 0).
    /// It halves the engine gain (0x4AA884…0x4AA89C) and gates the wind (0x4AA4D8). MEDIUM only in
    /// what `0x443EE0` means.
    pub attached_exterior: bool,
    /// view 0x0C **during a replay's playback** (`[0x5E99F4] >= 2`, 0x4AA072…0x4AA083): the
    /// distance gain is `2 · 20000/(d+15)` instead of `2048/(d+2)` (§4.1)
    pub view_0c_replay: bool,
    /// `carInfo+0x0C == 1`, the automatic gearbox — the rpm level's divisor is `redline + 1000`
    /// for it and `redline + 2000` for a manual (0x4A79B7…0x4A79E3)
    pub automatic: bool,
    /// `word car+0x418` (`Vehicle::airborne_count`): above 5 the divisor loses 500 (0x4A79E9)
    pub airborne_count: u16,
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
            pan: 0,
            distance: 0.0,
            doppler: 1.0,
            detail: 1,
            crowding: 0,
            engine_level: 127,
            shift: None,
            attached_exterior: false,
            view_0c_replay: false,
            automatic: false,
            airborne_count: 0,
        }
    }
}

pub struct EngineSound {
    spl: Spl,
    /// the `engineGain` of the last update (after the exterior halving: the engine layers' master)
    last_gain: i32,
    /// the `engineGain` before the exterior halving (0x4AA884), which the turbo layer scales by
    turbo_gain: i32,
    /// `A` of §4.1 — the distance gain with its rpm boost — which the road layer scales by
    last_a: i32,
    /// `game.spl[6]` — the engine trim of the global settings block
    game_trim: u32,
    layers: Vec<Option<LayerDesc>>,
    clips: Vec<Option<Arc<Clip>>>,
    /// `levelState[32]`, the slew target
    level: Vec<i32>,
    voices: Vec<Option<VoiceId>>,
    /// the second half of a stereo layer's `pan ± spread` pair (§3.4)
    voices_b: Vec<Option<VoiceId>>,
    /// the first voice's pan and level as the mixer last gave them (`[esi]`, `[esi−4]` of
    /// 0x4AC290's layer record, `+0x458` / `+0x454` + 0xC·layer of the engine object) — kept
    /// through a stop ([`layer_pan_level`]). Their first value is whatever the heap block held:
    /// the object comes from 0x531CA0 (`0x4ACC64`: name 0x5D1B94, 0x758 bytes, flags 0), which
    /// clears nothing — it links a free block and writes its 16-byte header (0x5320B0; a guard
    /// or name only under flags 0x200 / 0x100) — and the init loop 0x4ACCB5…0x4ACCE1 writes the
    /// record's handle (`+0x45C`, −1) but not these two. APPROXIMATION of that undefined start:
    /// (0, 0), the plain arm of [`layer_pan_level`] (no silent pass on the first rise).
    kept: Vec<(u16, i32)>,
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
    /// Engine bank slots 3, 4, 5: the horns (§6 — `0x5D14EC[0x2B] = 3` routes the horn voice to the
    /// car's own engine bank, not to `SND_coll`; this corrects `sfx.md` §4.6).
    horn_clips: [Option<Arc<Clip>>; 3],
    /// the playing horn and its slot
    horn_voice: Option<(VoiceId, usize)>,
    /// `game.spl[8]` — the horns' mix level
    horn_trim: u32,
}

/// The horn `Horn_start` 0x4AAC00 picks for the view `word[0x627F3C]`: slot 4 on view 0 or 0x0F,
/// otherwise 3 when `[0x627F43] & 8` (`viewId <= 1`) and 5 for the rest (0x4AAC7D…0x4AACBA).
/// What `Engine_mixLayers` 0x4AC290 does with one voice of a layer this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerStep {
    /// no handle and nothing to play
    Idle,
    /// no handle (−1): `Voice_start` (0x4AC572 / 0x4AC660), the handle kept whatever it is
    Start,
    /// a handle: `Voice_setVolume` / `Voice_setPanAngle` / `Voice_setPitch` on it when the value
    /// changed (0x4AC5BB…0x4AC61B, 0x4AC692…0x4AC70C)
    Update,
    /// the level is 0 (or the second voice is not wanted): `0x565D60` and the handle back to −1
    /// (0x4AC73F…0x4AC76D, 0x4AC718…0x4AC730)
    Stop,
}

/// **A stolen or refused voice stays in its layer until the level falls to 0.** 0x4AC290 tests
/// the handle it keeps (`[esi+4]`, the second voice's `[esi+0x184]`) only against −1
/// (0x4AC537, 0x4AC645): it stores whatever `Voice_start` answered, −9 for a refusal (0x589BE9)
/// included, and never asks whether the voice still plays. A voice the allocator 0x5898F0 took
/// for a later start keeps its old handle, which 0x589D20 no longer matches (`voice+0 != handle`,
/// → −8), so the per-pass `set` calls fail silently; the layer is restarted only after its level
/// has been 0 once (the stop sets −1) and rises again. A handle-less pass (`Some` of a dead id in
/// the port) is therefore an update, never a restart.
pub fn layer_step(wanted: bool, voice: Option<VoiceId>) -> LayerStep {
    match (wanted, voice) {
        (false, Some(_)) => LayerStep::Stop,
        (false, None) => LayerStep::Idle,
        (true, Some(_)) => LayerStep::Update,
        (true, None) => LayerStep::Start,
    }
}

/// The pans and the first voice's level of one layer this pass (0x4AC4D4…0x4AC521): a stereo
/// layer (`slot >= 0x40`) takes `pan − spread` / `pan + spread` (16-bit wrap); a mono one 0x8000
/// while the in-car spread is open (`[edi+0xC8] > 0`, signed). With the spread closed, a mono
/// layer whose first voice was last given pan 0x8000 (`cmp word [esi], 0x8000`) at a level above
/// 0 (`[esi−4]`, signed) gets **pan 0x8000 at level 0 for this pass** (0x4AC501…0x4AC518: `xor ebx,
/// ebx`) — after the level test that stops a voice (0x4AC4CE), so the voice is set silent, not
/// stopped, and is started silent when it has no handle; the next pass, its kept level being 0,
/// it takes its pan and level. So the pass after the spread closes is silent, and a mono layer
/// whose own pan is 0x8000 (dead behind) with the spread closed is silent every other pass.
/// Answers `((pan_a, level_a), pan_b)`; `kept` is `(pan, level)` of [`EngineSound::kept`].
pub fn layer_pan_level(slot: u8, pan: u16, spread: i32, level: i32, kept: (u16, i32)) -> ((u16, i32), u16) {
    if slot >= 0x40 {
        ((pan.wrapping_sub(spread as u16), level), pan.wrapping_add(spread as u16))
    } else if spread > 0 {
        ((0x8000, level), 0x8000)
    } else if kept.0 == 0x8000 && kept.1 as i8 > 0 {
        ((0x8000, 0), 0x8000)
    } else {
        ((pan, level), pan)
    }
}

pub fn horn_slot(view: u16) -> usize {
    if view == 0 || view == 0x0F {
        4
    } else if view <= 1 {
        3
    } else {
        5
    }
}

/// `rpmLvl`, 0x4A79B7…0x4A7A07: `(int)(car+0xDB4 · 127 / (redline + K))` with `K` 1000 for an
/// automatic (`carInfo+0x0C == 1`) and 2000 otherwise, 500 less while `word car+0x418 > 5`
/// (unsigned, `jbe`). The port had `redline + 2000` for every car until 2026-09-26.
pub fn rpm_level(rpm: f32, redline: f32, automatic: bool, airborne_count: u16) -> i32 {
    let mut div = redline as f64 + if automatic { 1000.0 } else { 2000.0 };
    if airborne_count > 5 {
        div -= 500.0;
    }
    (rpm as f64 * 127.0 / div) as i32
}

/// `Horn_start`'s volume (0x4AAC10…0x4AAC5E): `0x8000000 / (d + 2 m)` in 16.16, i.e. `2048/(d+2)`,
/// or `4 · 2048/(d+1)` on view 0x0C.
/// The `· 4` is inside the `>> 16` (0x4AAC31), so it is `floor(4 · 2048/(d+1))` (`crate::fix_atten`).
pub fn horn_volume(view: u16, distance: f32) -> i32 {
    if view == 0x0C { crate::fix_atten(0x800_0000, 0x1_0000, 2, distance) } else { crate::fix_atten(0x800_0000, 0x2_0000, 0, distance) }
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
            bank.decode_slot(bank_bytes, slot).ok().map(|p| Arc::new(Clip::from_pcm(p)))
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
        let game_trim = nfs_formats::spl::GameSpl::load(sounds_dir).engine();
        let start_clips = [decode(6), decode(7), decode(8)];
        let stop_clips = [decode(9), decode(10), decode(11)];
        let n = layers.iter().filter(|l| l.is_some()).count();
        tracing::info!(set, sim, layers = n, spl = ?spl.0, game_trim, "engine sound loaded");
        Ok(Self {
            spl,
            last_gain: 0,
            turbo_gain: 0,
            last_a: 0,
            game_trim,
            layers,
            clips,
            level: vec![0; 32],
            voices: vec![None; 32],
            voices_b: vec![None; 32],
            kept: vec![(0, 0); 32],
            pedal: 0,
            start_clips,
            stop_clips,
            start_voice: None,
            stop_voice: None,
            started: false,
            shift_click: decode(2).map(|c| Arc::new(Clip { loop_range: None, ..(*c).clone() })),
            // `Horn_start` restarts the voice whenever it has finished, so a sample without a loop
            // (3 and 4) repeats; the port loops it whole
            horn_clips: [3, 4, 5].map(|n| {
                decode(n).map(|c| {
                    let frames = (c.samples.len() / c.channels.max(1) as usize) as u32;
                    Arc::new(Clip { loop_range: c.loop_range.or(Some((0, frames))), ..(*c).clone() })
                })
            }),
            horn_voice: None,
            horn_trim: nfs_formats::spl::GameSpl::load(sounds_dir).horn(),
        })
    }

    /// Stop every voice (leaving the race).
    pub fn stop(&mut self, out: &AudioOutput) {
        for v in self.voices.iter_mut().chain(self.voices_b.iter_mut()).chain([&mut self.start_voice, &mut self.stop_voice]) {
            if let Some(id) = v.take() {
                out.stop(id);
            }
        }
        if let Some((id, _)) = self.horn_voice.take() {
            out.stop(id);
        }
        self.level.iter_mut().for_each(|l| *l = 0);
    }

    /// Gear-shift click (engine bank slot 2). The original fires it on the **last** tick of the shift.
    ///
    /// `Bank_playOneShot(engineBank, 2, pan 0, volume 0x7F, bend byte 0x40, 0x10000, 0x7F)`, and
    /// slot 2 of the player banks mixes as `min(SfxLevel · vol, 127)` (0x4A9994…0x4A99BE) — full
    /// volume for any slider above 0. The port had a fixed 0.6 until 2026-09-24.
    pub fn play_shift(&self, out: &AudioOutput, sfx_level: u8) {
        if let Some(c) = &self.shift_click {
            let vol = (sfx_level as i32 * 0x7F).min(0x7F);
            out.play(c.clone(), vol as f32 / 127.0, 1.0, false);
        }
    }

    /// The player's horn while `car+0xD86` is set: 0x4A7CBF → `Horn_start(idx, dist, 0x40,
    /// pitchBase, pan)` → `Voice_play(0x2B, horn_slot(view), bend 0x40, pitch, horn_volume, pan, 0x7F)`,
    /// mixed as slots 3…5 of the engine bank, `SfxLevel · game.spl[8]` (0x4A99C7…0x4A99E1). The
    /// voice is only (re)started when idle, so its volume is the one it started with. This port had
    /// slot 5 on every view at a fixed 0.6 until 2026-09-24.
    #[allow(clippy::too_many_arguments)]
    pub fn horn(&mut self, out: &AudioOutput, on: bool, view: u16, distance: f32, pan: u16, sfx_level: u8) {
        let slot = horn_slot(view);
        // a playing voice is left alone — 0x4AACBF…0x4AACD6 returns before `Voice_play` while
        // 0x566380 says it plays — so a view change mid-honk keeps the slot it started with (the
        // port restarted on the new view's slot until 2026-09-26)
        if let Some((id, _)) = self.horn_voice
            && (!on || !out.is_playing(id))
        {
            out.stop(id);
            self.horn_voice = None;
        }
        if !on || self.horn_voice.is_some() {
            return;
        }
        let vol = horn_volume(view, distance).min(0x7F);
        let mixed = ((sfx_level as i32 * self.horn_trim as i32 * vol * 85) >> 21).clamp(0, 127);
        // bend byte 0x40: no bend
        let amp = mixed as f32 / 127.0;
        if let Some(c) = &self.horn_clips[slot - 3] {
            self.horn_voice = Some((out.play_panned(c.clone(), amp, 1.0, pan, true), slot));
        }
    }

    /// `Engine_auxVolume(mode)` 0x4AD640 — the base level the road and turbo layers start from
    /// (`engine-audio.md` §4.6).
    pub fn aux_volume(&self, mode: u8) -> i32 {
        self.spl.aux_volume(mode) as i32
    }

    /// The `engineGain` the last [`Self::update`] computed.
    pub fn gain(&self) -> i32 {
        self.last_gain
    }

    /// `engineGain` — the distance attenuation with its rpm boost and the shift dip (§4.1, §4.3).
    /// Returns `(A, gain for the turbo, engineGain)`.
    fn engine_gain(&self, inp: &EngineInput) -> (i32, i32, i32) {
        // 0x4AA072…0x4AA0C8: `fixdiv(0x8000000, d + 2)`, or on view 0x0C in a replay's playback
        // `fixdiv(20000, d + 15) << 1`, of the fractional distance (`crate::fix_atten`). The port
        // had only the first, of whole metres, until 2026-09-26.
        let atten = if inp.view_0c_replay {
            crate::fix_atten(0x4E20_0000, 0xF_0000, 1, inp.distance)
        } else {
            crate::fix_atten(0x800_0000, 0x2_0000, 0, inp.distance)
        };
        let rpm_lvl = rpm_level(inp.rpm, inp.redline, inp.automatic, inp.airborne_count);
        let a = atten.max(atten * (rpm_lvl.clamp(0, 150) + 40) / 128);
        let mut gain = a * 127 / 128;
        if let Some((timer, ticks)) = inp.shift {
            // 1.0 at the start of the shift falling to 0.5 at its end (0x4AA64B…0x4AA65F)
            gain = (gain + timer as i32 * gain / ticks.max(1) as i32) / 2;
        }
        let turbo_gain = gain;
        // §4.3: an attached exterior view is half as loud — after the dip and after the turbo
        // layer took its gain (0x4AA884…0x4AA89C; the port halved first until 2026-09-26)
        if inp.attached_exterior {
            gain /= 2;
        }
        (a, turbo_gain, gain)
    }

    /// `A` of §4.1 from the last [`Self::update`] — the road layer's distance gain (0x4AA36B…0x4AA38A).
    pub fn road_gain(&self) -> i32 {
        self.last_a
    }

    /// The `engineGain` the turbo layer takes: after the shift dip, before the exterior halving.
    pub fn turbo_gain(&self) -> i32 {
        self.turbo_gain
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

        // §3.3 in the record's integers: `rpmFrac` the 16.16 word of 0x4AA8B7 (`_ftol`), the bin
        // and the pitch word from it (`Spl::bin`, `Spl::pitch_word`); `pitchBase` 16.16 as well.
        // The word goes unchanged to the voice's `+0x6C` (the start's request `+0xC`, 0x4AC545;
        // `setPitch` 0x565F80 at 0x4AC608 / 0x4AC6F9) — relative to the sample's root: 0x590420
        // starts every layer at `(0x3C − root)·100 + fine` cents (`mixer::base_cents`) and 0x589F40
        // makes `+0x6E = +0x6C · cents_word >> 12`. `word / 4096` is exact in an f32, so the
        // mixer's `resample::pitch_word` gives the word back. *(Until 2026-09-27 the port took the
        // float `spl[8]·(rpm/redline + 0.2)·doppler/64`, a word off at the rounding — 3584 for
        // 3583 at half the redline — and floored it at 0.05, which has no source.)*
        let rpm_frac = nfs_formats::spl::rpm_frac_16(inp.rpm, inp.redline);
        let bin = self.spl.bin(rpm_frac);
        let blend = self.spl.blend(inp.mode, inp.cos);
        let pitch_word = self.spl.pitch_word(rpm_frac, (inp.doppler as f64 * 65536.0) as i32);
        let pitch = pitch_word as f32 / 4096.0;
        let spread = if inp.mode < 2 { (inp.crowding as i32 * 0x3FFF) >> 7 } else { 0 };
        let pan = if inp.mode < 2 { 0 } else { inp.pan };

        // §4.1 master volume, §3.6 duck while the start sample plays
        (self.last_a, self.turbo_gain, self.last_gain) = self.engine_gain(&inp);
        let master_vol = (((inp.engine_level as i32 * self.game_trim as i32 * 85) >> 7) * self.last_gain) >> 14;
        let mut rec_volume = ((self.spl.record_volume(inp.mode) as i32 * master_vol) >> 6).min(0x7FF);
        rec_volume = (rec_volume * self.start_stop(out, &inp, pan, rec_volume)) >> 9;

        tracing::trace!(rpm = inp.rpm, bin, load, blend, rec_volume, pitch_word, mode = inp.mode, audible = self.level.iter().filter(|l| **l > 0).count(), "engine record");
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
            // §3.4's voice management: a stereo layer (`slotId >= 0x40`) plays **two** voices at
            // `pan ± spread` whenever the in-car spread is open; a mono one goes dead-behind
            // instead. The pans are the 16-bit angles: `pan − spread` / `pan + spread`, wrapping
            // (0x4AC4D8…0x4AC4EA, 16-bit `sub` / `add`), and 0x8000 for a mono layer while the
            // spread is open (0x4AC4F8) — the mixer folds each into its pan byte. (Until
            // 2026-09-27 the port took the pair as `sin(θ ∓ spread·π/0x8000)` of its sine pan.)
            let stereo_pair = d.slot >= 0x40 && spread != 0;
            // the mono layer's silent pass after the spread closes ([`layer_pan_level`])
            let ((pan_a, lvl_a), pan_b) = layer_pan_level(d.slot, pan, spread, lvl, self.kept[i]);
            // a voice the budget took (or refused) keeps its id until the level falls to 0
            // ([`layer_step`]); the updates on it do nothing, as 0x589D20 fails the old handle
            for (k, slot, wanted, pan_x, lvl_x) in [(0, &mut self.voices[i], lvl > 0, pan_a, lvl_a), (1, &mut self.voices_b[i], lvl > 0 && stereo_pair, pan_b, lvl)] {
                let vol = lvl_x as f32 / 127.0;
                match (layer_step(wanted, *slot), *slot) {
                    (LayerStep::Stop, Some(v)) => {
                        out.stop(v);
                        *slot = None;
                    }
                    (LayerStep::Update, Some(v)) => {
                        out.set(v, vol, pitch);
                        out.set_pan(v, pan_x);
                        if k == 0 {
                            self.kept[i] = (pan_x, lvl_x);
                        }
                    }
                    (LayerStep::Start, _) => {
                        if let Some(c) = &self.clips[i] {
                            *slot = Some(out.play_panned(c.clone(), vol, pitch, pan_x, true));
                            // 0x4AC57E / 0x4AC581: the start keeps its level and pan
                            if k == 0 {
                                self.kept[i] = (pan_x, lvl_x);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// §3.6: the engine start / stop one-shots (0x4AC97F…0x4ACB5F). Returns the duck factor
    /// `ebx` the record's volume is scaled by (`· ebx >> 9`, 0x4ACB54…0x4ACB5F): 0x200 once the
    /// start has played (0x4AC9B1) or finished (0x4ACA7B); `0x200 − progress` while it plays
    /// (0x4ACA53…0x4ACA74; a negative one → 0, `js`); and **0** (0x4ACB52 `xor ebx, ebx`) on the
    /// update that starts it and on every update with the engine off — the loop bed is silent
    /// then. Both one-shots take the level `min(EngineLevel, record volume)` (`cmp; jg`, the
    /// record's word before the duck; 0x4AC9C8…0x4AC9DD / 0x4ACAC3…0x4ACAD8) and the record's pan
    /// at their start, and **every update while they play** `setVolume` of the same min and
    /// `setPan` (0x4ACA22…0x4ACA45, 0x4ACB1A…0x4ACB3D). *(Until 2026-09-27 the port gave them
    /// `EngineLevel` alone, set once, and a duck of 512 on the starting and the engine-off
    /// updates.)*
    fn start_stop(&mut self, out: &AudioOutput, inp: &EngineInput, pan: u16, rec_volume: i32) -> i32 {
        let class = (inp.mode as usize).min(2);
        let level = (inp.engine_level as i32).min(rec_volume) as f32 / 127.0;
        if inp.running {
            // 0x4AC990…0x4AC9A8: a stop sample still playing is cut and the start armed again
            if let Some(v) = self.stop_voice.take() {
                out.stop(v);
                self.started = false;
            }
            if self.started {
                return start_duck(true, true, None);
            }
            match self.start_voice {
                None => {
                    match &self.start_clips[class] {
                        Some(c) => self.start_voice = Some(out.play_panned(c.clone(), level, 1.0, pan, false)),
                        // a start that fails leaves an error handle, whose status (0x566380) ends
                        // it on the next update at 0x200 — the same as `started` now
                        None => self.started = true,
                    }
                    start_duck(true, false, None)
                }
                Some(v) if out.is_playing(v) => {
                    out.set(v, level, 1.0);
                    out.set_pan(v, pan);
                    start_duck(true, false, Some(Some(out.progress(v).unwrap_or(1.0))))
                }
                Some(_) => {
                    self.start_voice = None;
                    self.started = true;
                    start_duck(true, false, Some(None))
                }
            }
        } else {
            if let Some(v) = self.start_voice.take() {
                out.stop(v);
                self.started = true;
            }
            if self.started {
                match self.stop_voice {
                    None => match &self.stop_clips[class] {
                        Some(c) => self.stop_voice = Some(out.play_panned(c.clone(), level, 1.0, pan, false)),
                        // the failed start's error handle ends it on the next update; every update
                        // answers 0 either way
                        None => self.started = false,
                    },
                    Some(v) if out.is_playing(v) => {
                        out.set(v, level, 1.0);
                        out.set_pan(v, pan);
                    }
                    Some(_) => {
                        self.stop_voice = None;
                        self.started = false;
                    }
                }
            }
            start_duck(false, self.started, None)
        }
    }
}

/// The duck `ebx` of one update of §3.6 (0x4AC97F…0x4ACB54): engine off → 0 (every path of
/// 0x4ACA8B… ends at 0x4ACB52); the start already played → 0x200 (0x4AC9B1); the start voice
/// `start` = `None` being started this update → 0 (0x4ACA10); `Some(Some(progress))` playing →
/// `0x200 − progress·0x200`, 0 when negative (0x4ACA53…0x4ACA74, `js`); `Some(None)` just
/// finished → 0x200 (0x4ACA7B).
fn start_duck(running: bool, started: bool, start: Option<Option<f32>>) -> i32 {
    if !running {
        return 0;
    }
    if started {
        return 0x200;
    }
    match start {
        None => 0,
        Some(Some(p)) => (0x200 - (p * 512.0) as i32).max(0),
        Some(None) => 0x200,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_rpm_level_divisor_follows_the_gearbox() {
        // 7000 rpm at a 7000 redline: 127·7000/9000 = 98 manual, /8000 = 111 automatic,
        // /7500 = 118 for an automatic with more than five airborne ticks
        assert_eq!(super::rpm_level(7000.0, 7000.0, false, 0), 98);
        assert_eq!(super::rpm_level(7000.0, 7000.0, true, 0), 111);
        assert_eq!(super::rpm_level(7000.0, 7000.0, true, 6), 118);
        assert_eq!(super::rpm_level(7000.0, 7000.0, false, 5), 98);
    }

    #[test]
    fn the_horn_follows_the_view_and_the_distance() {
        // slot 4 on views 0 and 0x0F, 3 on view 1, 5 otherwise (0x4AAC7D…0x4AACBA)
        assert_eq!((horn_slot(0), horn_slot(0x0F), horn_slot(1), horn_slot(2)), (4, 4, 3, 5));
        // 2048/(d+2), four times 2048/(d+1) on view 0x0C
        assert_eq!(horn_volume(2, 30.0), 64);
        assert_eq!(horn_volume(0x0C, 31.0), 256);
    }

    use super::*;

    #[test]
    fn a_mono_layer_is_silent_for_one_pass_after_the_spread_closes() {
        // in-car, spread open: a mono layer dead behind at its level
        assert_eq!(layer_pan_level(0x10, 0, 0x2000, 90, (0, 0)), ((0x8000, 90), 0x8000));
        // the spread closes: kept 0x8000 at 90 → this pass 0x8000 at level 0
        assert_eq!(layer_pan_level(0x10, 0x1234, 0, 90, (0x8000, 90)), ((0x8000, 0), 0x8000));
        // the next pass (kept level 0) takes its own pan and level
        assert_eq!(layer_pan_level(0x10, 0x1234, 0, 90, (0x8000, 0)), ((0x1234, 90), 0x1234));
        // its own pan dead behind: silent every other pass
        assert_eq!(layer_pan_level(0x10, 0x8000, 0, 90, (0x8000, 90)), ((0x8000, 0), 0x8000));
        assert_eq!(layer_pan_level(0x10, 0x8000, 0, 90, (0x8000, 0)), ((0x8000, 90), 0x8000));
        // a stereo layer is never touched: `pan ∓ spread` with its level
        assert_eq!(layer_pan_level(0x40, 0x8000, 0, 90, (0x8000, 90)), ((0x8000, 90), 0x8000));
        assert_eq!(layer_pan_level(0x41, 0x100, 0x200, 90, (0, 0)), ((0xFF00, 90), 0x300));
    }

    #[test]
    fn a_stolen_layer_voice_waits_for_the_level_to_fall() {
        let dead = Some(VoiceId(7));
        // the handle is only compared with −1: a stolen / refused one is updated, not restarted
        assert_eq!(layer_step(true, dead), LayerStep::Update);
        // the level at 0 stops it and frees the slot (−1) ...
        assert_eq!(layer_step(false, dead), LayerStep::Stop);
        assert_eq!(layer_step(false, None), LayerStep::Idle);
        // ... and the next rise starts a new voice
        assert_eq!(layer_step(true, None), LayerStep::Start);
    }

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
    fn the_start_sample_ducks_the_bed() {
        use super::start_duck;
        // engine off: the bed is silent whatever the one-shots do
        assert_eq!(start_duck(false, true, None), 0);
        assert_eq!(start_duck(false, false, Some(Some(0.5))), 0);
        // the update that starts the start sample is silent, then it fades in with the progress
        assert_eq!(start_duck(true, false, None), 0);
        assert_eq!(start_duck(true, false, Some(Some(0.25))), 384);
        assert_eq!(start_duck(true, false, Some(Some(1.0))), 0);
        // finished, and every update after
        assert_eq!(start_duck(true, false, Some(None)), 512);
        assert_eq!(start_duck(true, true, None), 512);
    }

    #[test]
    fn the_bin_and_pitch_match_the_shipped_parameters() {
        let s = Spl::default();
        let f = |x: f32| nfs_formats::spl::rpm_frac_16(x, 1.0);
        // a layer window of [259, 512) only opens above 0.62 redline
        assert!(s.bin(f(0.6)) < 259);
        assert!(s.bin(f(0.65)) >= 259);
        // one global word for every layer, 4096 = 1.0 (0.6 of the redline truncates to 4095)
        assert_eq!(s.pitch_word(f(0.6), 0x10000), 4095);
        // the tiny car's doubled base doubles the word, to the rounding
        assert_eq!(s.pitch_word(f(0.6), 0x20000), 8191);
    }
}
