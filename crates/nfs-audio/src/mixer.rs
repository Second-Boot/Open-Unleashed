use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::resample::{self, Cursor, Gains, Interp};
use crate::rt::{self, Consumer, Producer, SampleRing};
use std::sync::atomic::{AtomicI8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u32);

/// A `Send + Sync` handle to the game thread's side of the mixer, without the output stream
/// itself — see [`AudioOutput::stop_handle`].
#[derive(Clone)]
pub struct StopHandle(Arc<Tx>);

impl StopHandle {
    pub fn stop(&self, id: VoiceId) {
        self.0.stop(id);
    }
}

/// A decoded mono/stereo clip at its native rate.
#[derive(Debug, Clone)]
pub struct Clip {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
    pub loop_range: Option<(u32, u32)>,
    /// The patch's own volume (`PcmSample::gain`, `RE/notes/audio.md` §2): the sound library
    /// multiplies the level the game asks for by it, so it belongs to the clip, not the call.
    pub gain: f32,
    /// The patch's bend range in cents (`PcmSample::bend_cents`, code 0x0A · 100) — `voice+0x66`, which
    /// scales the caller's bend byte `voice+0x3F` ([`Voice::pitch_word`]).
    pub bend_cents: i32,
    /// The patch's effect send, code 0x13 → `voice+0x44` (`PcmSample::fx_send`).
    pub fx_send: u8,
    /// The patch's voice priority, code 0x06 → `voice+0x1A` (`PcmSample::priority`): below
    /// [`STEAL_BELOW`] the voice can be taken by a later start when the budget is full.
    pub priority: u8,
    /// The patch's pan byte, code 0x0C (`PcmSample::pan`) — `voice+0x3E`.
    pub pan: u8,
    /// The patch's pan scale, code 0x25 (`PcmSample::pan_scale`) — `voice+0x43`.
    pub pan_scale: i8,
    /// The patch's records (`PcmSample::records`): what the start tests and draws from
    /// ([`crate::librand::patch_start`]). Empty for a clip that is not a bank patch.
    pub records: Vec<nfs_formats::bnk::PatchRecord>,
    /// For a patch of more than one record, each record's own clip (`PcmSample::parts`), in
    /// record order: 0x590420 starts every matching record as a voice of its own. Empty for a
    /// one-record patch, whose sample is this clip's.
    pub layers: Vec<Arc<Clip>>,
    /// The records' tables (`PcmSample::tables`), in record order: the envelope, the volume and
    /// pitch step tables and the level curve the library service walks ([`Shared::service`]).
    pub tables: Vec<nfs_formats::bnk::RecordTables>,
}

impl Clip {
    /// A bank patch's decoded sample as a clip, its records' own samples as its layers.
    pub fn from_pcm(p: nfs_formats::bnk::PcmSample) -> Clip {
        Clip {
            rate: p.rate,
            channels: p.channels,
            samples: p.samples,
            loop_range: p.loop_range,
            gain: p.gain,
            bend_cents: p.bend_cents,
            fx_send: p.fx_send,
            priority: p.priority,
            pan: p.pan,
            pan_scale: p.pan_scale,
            records: p.records,
            layers: p.parts.into_iter().map(|q| Arc::new(Clip::from_pcm(q))).collect(),
            tables: p.tables,
        }
    }

    /// The clip record `i` plays: its layer, or the clip itself for a one-record patch.
    fn layer(self: &Arc<Self>, i: usize) -> Arc<Clip> {
        self.layers.get(i).cloned().unwrap_or_else(|| Arc::clone(self))
    }
}

/// **0x565BB0**, the game's `atan2` on a 1024-step circle, of `x` (the side) and `y` (forward):
/// the octant from the signs and the larger magnitude, then `0x5E24AC[(x·256 + y/2) / y]` (x ≤ y,
/// `div`, so rounded) — the table is `round(atan(i/256) · 1024/2π)` for `i` in 0…256 (checked
/// against all 257 bytes of the file: every one within 0.4999) — or, for `x ≥ 0x800000`, the
/// 64-bit quotient `(x << 32) / y` rounded at bit 23 (0x565C18…0x565C3E). `x == y` is 0x80.
pub fn atan2_1024(x: i32, y: i32) -> i32 {
    let (mut esi, mut ecx) = (y, x);
    let (mut neg, mut base) = (false, 0i32);
    if esi < 0 {
        esi = esi.wrapping_neg();
        neg = true;
        base = 0x200;
    }
    if ecx < 0 {
        neg = !neg;
        ecx = ecx.wrapping_neg();
        base = -base;
    }
    if ecx > esi {
        std::mem::swap(&mut ecx, &mut esi);
        base += if neg { -0x100 } else { 0x100 };
        neg = !neg;
    }
    let t = if ecx == esi {
        0x80
    } else {
        let i = if ecx as u32 & 0xFF80_0000 != 0 {
            let q = ((ecx as u32 as u64) << 32) / esi as u32 as u64;
            (q >> 24) as u32 + ((q >> 23) & 1) as u32
        } else {
            ((esi as u32 >> 1) + ((ecx as u32) << 8)) / esi as u32
        };
        ((i as f64 / 256.0).atan() * 1024.0 / std::f64::consts::TAU).round() as i32
    };
    if neg { base - t } else { base + t }
}

/// **0x4A6FE0**'s angle of a sound source: `side` and `fwd` are the offset from the listener
/// dotted with the listener's row `+0xC` (`[0x655E5C]`) and row `+0x24` (`[0x655E74]`), each
/// `· 65536` and `_ftol`'d (0x5A0F98, toward zero), `sar 8`, then
/// `(atan2_1024(side, fwd) & 0x3FF) << 6` — a full-circle 16-bit angle, 0x4000 on the `+side`
/// hand, 0x8000 behind. `MIRROR` `[0x65742C]` negates `side` (0x4A704D): the caller passes it
/// negated. Without 3-D positioning (`[0x5D1760] == 0`, never in the shipped `fe.txt`) it is 0.
pub fn pan_angle(side: f32, fwd: f32) -> u16 {
    let sx = (side * 65536.0) as i32 >> 8;
    let sy = (fwd * 65536.0) as i32 >> 8;
    ((atan2_1024(sx, sy) & 0x3FF) << 6) as u16
}

/// The **pan byte** `Voice_play` 0x4A98E0 stores in the request's `+7` (0x4A9B4B…0x4A9B84) and
/// the loops' update hands to `Voice_setPan8` 0x566100 (0x4A9F3D…0x4A9F79, 0x4AB0F1…0x4AB13D),
/// because `[0x655D40]` — the 3-D pan path, `Voice_setPanAngle` — is always 0 (only 0x4A66B0's dead
/// branch sets it): the rear half folds onto the front, `(0xBFFF − a) >> 8` for
/// `0x3FFF < a < 0xC000`, else `((a + 0x4000) >> 8) & 0xFF` — 0x40 ahead, 0x7F at 0x4000 (and just
/// behind it), 0 at 0xC000, 0x3F dead behind.
pub fn pan_byte(a: u16) -> u8 {
    let a = a as i32;
    if a > 0x3FFF && a < 0xC000 { ((0xBFFF - a) >> 8) as u8 } else { (((a + 0x4000) >> 8) & 0xFF) as u8 }
}

/// The library's pan for a voice from the caller's byte (0x590560…0x5905A3 at the start,
/// 0x566171…0x5661A2 on `Voice_setPan8`): scaled about 0x40 by the patch's code 0x25 unless 1,
/// plus the patch's code 0x0C less 0x40, clamped to 0…127. The angle the backend is handed is then
/// `(p − 0x40) << 8` (0x59070C…0x590716, `(p + 0xC0) << 8` at 0x5661A4) — the byte back on the
/// front half circle, 0x4000 at 0x80. `patch_pan` is the start's `voice+0x3E`, code 0x0C plus
/// its random spread (code 0x0D, [`crate::librand::patch_start`]).
pub fn voice_pan(byte: u8, patch_pan: u8, scale: i8) -> u8 {
    let mut p = byte as i8 as i32;
    if scale != 1 {
        p = (p - 0x40) * scale as i32 + 0x40;
    }
    (p + patch_pan as i32 - 0x40).clamp(0, 127) as u8
}

/// The sound library's software voice count per `AUDIOQUALITY` `[0x657C98]`: `0x5B4B08[q]` = 48 /
/// 24 / 12, a byte of the configuration 0x4A66B0 hands to 0x5656D0 (0x4A66F3, 0x4A6719), copied to
/// `[0x6B48DD]` (0x5656E6) and capped at the library's 128 (`[0x6B48C6]` = 0x80, 0x58A545; the cap
/// 0x58A731…0x58A742). At quality 3 the configuration is skipped (0x4A66E8) and the library's
/// default 24 stays (0x58A5B5) — the sound is closed at that setting anyway (0x4A6854 / 0x4A6A09).
///
/// The hardware-3D voices `[0x6B48DF]` (`min(device's, 0x5B4AF0[q])`, 0x4A6755…0x4A67A1) are never
/// configured: that branch needs `[0x657C9C]`, which both start-ups clear just before the call
/// (0x4A684C, 0x4A6A61), so the device's own voice count does not enter the budget.
/// `[0x6B49A4]` = software + hardware (0x58A786…0x58A7A8) is the voice table's length (0x565792).
pub fn voice_budget(audio_quality: u8) -> usize {
    match audio_quality {
        0 => 48,
        1 => 24,
        2 => 12,
        _ => 24,
    }
}

/// The priority a stream voice is started with — `push 0x65` at 0x5613B2 in the stream voice
/// start 0x5612A0 (the stream player 0x566FE3 / 0x567304: music, the movies' sound). It is not
/// below [`STEAL_BELOW`], so a stream is never taken.
pub const STREAM_PRIORITY: u8 = 0x65;

/// The allocator's second pass takes only voices whose `voice+0x1A` is below this (0x589A38
/// `cmp cl, 0x65 ; jae`, unsigned): a priority of 0x65 or more is never stolen.
pub const STEAL_BELOW: u8 = 0x65;

/// **0x5898F0(n, priority, &handle, lo, hi)**, the voice allocator, for a start that needs `need`
/// voices (the patch's channel count, code 0x82 → layer `+3`, 0x5904CD) out of `budget`, with the
/// live voices given as `(priority, channels, stamp)` — `stamp` is [`Voice::stamp`], the service
/// tick `[0x6B49A8]` the voice started at ([`Shared::service_tick`]):
///
/// * pass 1 (0x589943…0x5899CA) takes free voices (`voice+0xF == 0`);
/// * pass 2 (0x5899EF…0x589AA8, read to the instruction 2026-09-27) takes, one at a time, the voice
///   with the **lowest** priority below [`STEAL_BELOW`] (0x589a3b…0x589a48: `jge` skips a
///   candidate whose priority is not strictly lower than the best so far, so on equal priority
///   the tie goes to the **smallest** stamp (0x589a51…0x589a58, `jae` — unsigned, so the oldest);
///   on an exact tie (equal priority *and* stamp) neither replaces the earlier one, so the
///   **lowest scanned voice-table slot** wins, whatever its stamp. The request's own priority is
///   **not** compared: any start takes a stealable voice;
/// * fewer than `need` found → −9 (0x589BE9) and nothing is stopped (the victims are only
///   stopped, 0x565D60 on their handle, after the count matched, 0x589B26).
///
/// Returns the indices of the voices to stop, or `None` for the refusal. A stolen stereo voice
/// frees both its channels (0x565D60 stops the whole handle). `sort_by_key` is stable, so `live`'s
/// own order (push order into [`Shared::voices`], not necessarily the original's slot order) is
/// this port's stand-in for "lowest slot" on an exact tie. *(Until 2026-09-27 `stamp` was the
/// port's own start-order counter (`VoiceId`), not the service tick — so two voices started in
/// different ticks but close together could tie here when the original's wouldn't, and vice
/// versa; withdrawn — [`Shared::service_tick`] now stamps a start with `[0x6B49A8]` exactly as
/// 0x589B3F…0x589B54 does. The remaining gap — an exact tie's winner depending on the original's
/// free-list history, which decides which physical slot a voice starts in — is unmodeled and,
/// short of tracking the 128-slot table itself, unmodelable from the port's `Vec<Voice>`.)*
pub fn pick_victims(live: &[(u8, u16, u32)], budget: usize, need: usize) -> Option<Vec<usize>> {
    let (mut order, mut victims) = (Vec::new(), Vec::new());
    pick_victims_into(live, budget, need, &mut order, &mut victims).then_some(victims)
}

/// [`pick_victims`] into the caller's buffers (the callback's allocator allocates nothing): `true`
/// for a start that is served, with the victims in `victims`. The stable sort of the original
/// order is the unstable one with the index as the last key — the same total order, no scratch
/// allocation.
pub fn pick_victims_into(live: &[(u8, u16, u32)], budget: usize, need: usize, order: &mut Vec<usize>, victims: &mut Vec<usize>) -> bool {
    victims.clear();
    let used: usize = live.iter().map(|&(_, ch, _)| ch.max(1) as usize).sum();
    let mut free = budget.saturating_sub(used);
    if free >= need {
        return true;
    }
    order.clear();
    order.extend((0..live.len()).filter(|&i| live[i].0 < STEAL_BELOW));
    order.sort_unstable_by_key(|&i| (live[i].0, live[i].2, i));
    for &i in order.iter() {
        victims.push(i);
        free += live[i].1.max(1) as usize;
        if free >= need {
            return true;
        }
    }
    false
}

/// The port's `byte / 127` volume back to the library's 16.16 `voice+0x2C`: `round(volume ·
/// 127)` (exact for every caller's byte, all in 0…127; clamped there) `<< 16`.
fn volume_16(volume: f32) -> i32 {
    ((volume * 127.0).round().clamp(0.0, 127.0) as i32) << 16
}

/// The ramps a voice is released with — **0x565DD0(handle, ticks, target)**: [`RELEASE_TICKS`]
/// from 0x4A9890 and every direct release of the audio update (0x4A9521, 0x4A956F, 0x4A97EF,
/// 0x4AA454, 0x4AA489, 0x4AA5FF, 0x4AA869, 0x4AA9E4, 0x4AAA2D, 0x4AAA86, 0x4AAD46, 0x4AAF9F), all
/// to target −1; [`REPLACE_TICKS`] from `Voice_play` 0x4A9C10 when a voice slot is given another
/// sample (0x4A9D86…0x4A9DA4: the slot's id `+4` differs from the call's) before the new start.
/// The caller forgets the handle at once (`0x656180[slot] = −1`), so nothing calls the old voice
/// again: no re-trigger cancels its ramp (a restart takes a new voice; the siren's 0x4AAF50 keeps
/// the handle, but the next start 0x4AAE90 overwrites it, 0x4AAEEE / 0x4AAF33). The voice stays
/// in the budget until the ramp stops it.
pub const RELEASE_TICKS: i32 = 5;
/// `Voice_play`'s release of a slot's old sample (0x4A9DA1 `push 0x19`), see [`RELEASE_TICKS`].
pub const REPLACE_TICKS: i32 = 0x19;
/// The release target (`push −1`, 0x4A98AF …): its goal `−1 << 16` is below 0, so the ramp
/// ends in the stop.
pub const RELEASE_TARGET: i32 = -1;

/// A voice's volume ramp `voice+0x24` (the step) / `+0x28` (the goal), **0x565DD0** and
/// **0x5895E1…0x589619**. The call: `ticks` ≤ 0 → 1 (0x565DFF), goal `= target << 16`, step
/// `= (goal − voice+0x2C) / ticks` (`idiv`, toward zero), for every voice of the handle's group
/// (0x590340: state 1, a handle) each from its own `+0x2C`. The service, once a tick, when the
/// step is not 0: `+0x2C += step`; past the goal (`jl` / `jg` by the step's sign, signed) it is
/// the goal and the step 0; then `+0x2C < 0` **stops the handle** (0x565D60 — the whole group,
/// a held anchor too) and the voice's envelope is not stepped that tick (0x589619 → 0x58969E).
/// `release(5, −1)` from 127: step `(−0x10000 − 0x7F0000) / 5` = −1677721; the volume word
/// `>> 16` (`sar`) runs 101, 75, 50, 24, and the fifth tick's −65533 stops the voice — four
/// ticks of fading sound, then silence, where the port stopped at once until 2026-09-27.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ramp {
    pub step: i32,
    pub goal: i32,
}

impl Ramp {
    /// 0x565E22…0x565E54 on one voice of volume `vol16`.
    pub fn start(vol16: i32, ticks: i32, target: i32) -> Self {
        let ticks = if ticks > 0 { ticks } else { 1 };
        let goal = target.wrapping_shl(16);
        Ramp { step: goal.wrapping_sub(vol16) / ticks, goal }
    }

    /// 0x5895E1…0x58960F: one service tick on `vol16`; `true` when the voice is to be stopped.
    pub fn tick(&mut self, vol16: &mut i32) -> bool {
        if self.step == 0 {
            return false;
        }
        let v = vol16.wrapping_add(self.step);
        *vol16 = v;
        let past = if self.step >= 0 { v >= self.goal } else { v <= self.goal };
        if past {
            *vol16 = self.goal;
            self.step = 0;
        }
        *vol16 < 0
    }
}

struct Voice {
    clip: Arc<Clip>,
    /// where the resampler is in the voice's input stream ([`resample::Cursor`]) — for a loop
    /// the stream runs on past the end (`0…hi`, then `lo…hi` again, the looped decoder 0x59CBA0)
    cur: Cursor,
    /// the channel gains and their ramp (0x598700 / 0x5980C0)
    gains: Gains,
    /// a call of the strip's volume or pan hook (0x5969C0 / 0x596890 → 0x598700) since the last
    /// block: 0x598700 raises the ramp flag `[node+1]` on **every** call, changed value or not
    /// (0x598721), so the block's pull runs the ramp ([`Gains::touch`]). Raised by the service tick
    /// (0x589699), `setVolume` on a changed volume (0x56631C) and the pan call (0x5661F0 → 0x590CC0).
    hook: bool,
    /// `voice+0x6C`, the caller's pitch word: 4096 is 1.0, unchanged ([`resample::PITCH_UNIT`]) —
    /// every start/update call hands it directly ([`AudioOutput::play_word`] / `set_word`, audio
    /// port 8; there is no float pitch left in the port, matching the original, which never took
    /// one either).
    word: u16,
    /// `voice+0x2C`, the caller's volume in 16.16 (`setVolume`'s int `<< 16`, 0x5662E3; the
    /// request's `+8 << 16` at the start, 0x59055D) — the level byte takes `>> 16`
    vol16: i32,
    /// the release ramp `voice+0x24` / `+0x28` ([`Ramp`], 0x565DD0)
    ramp: Ramp,
    /// the caller's pan byte `voice+0x1F` ([`pan_byte`]; 0x40, the request's default from
    /// 0x561960, until one is given) — the patch's pan is applied at the mix ([`voice_pan`])
    pan: u8,
    looping: bool,
    finished: bool,
    /// `voice+0x45`: the request's `+0xA`, 0x7F unless the caller passes an aux byte — 0x561960
    /// fills every request with 0x7F (0x56199F) and 0x4A9A5F writes `0x7F − aux` for the
    /// collision one-shots (slots from 0x30). 0x5906D7 copies it into the voice.
    fx_scale: u8,
    /// the channel's reverb send `+4` (0x599800; `None` until the voice's start has set it) and
    /// its wet gain `+8` (0x5980A0)
    send: Option<f32>,
    wet: f32,
    /// `voice+0x20` at the start (0x589B54): [`Shared::service_tick`] `[0x6B49A8]` as it stood at
    /// the start — the library service's own tick count, not a start order. *(Until 2026-09-27
    /// this was the port's `VoiceId`, its own start order — see [`pick_victims`].)*
    stamp: u32,
    /// `voice+0x3E`, the patch pan with its spread drawn at the start ([`voice_pan`])
    patch_pan: u8,
    /// `voice+0x3C`: the patch volume with its spread, times the request's byte `+6` over 127
    /// (0x5905D0) — a factor of the voice's level byte ([`Voice::level_byte`], 0x589E1C)
    level: i8,
    /// `voice+0x68`, the voice's own cents: [`base_cents`] plus the draws of codes 0x24 / 0x11
    /// (0x59052E, 0x590549)
    cents: i16,
    /// the envelope (`voice+0x30`…`+0x41`, [`Envelope`])
    env: Envelope,
    /// `voice+0x4A` / `+0x4B`: the steps of the volume and pitch tables (drawn at the start,
    /// stepped by the service)
    volume_phase: u8,
    pitch_phase: u8,
    /// the resampler has made its first pull (0x59BE00's `+0x1C`)
    pulled: bool,
    /// `voice+0x1B`: the last record started by a patch start of more than one (0x590A5C) — the
    /// voice whose handle the start answers; it outlives the group's others ([`Shared::free`])
    anchor: bool,
    /// the anchor's state 2 (0x589CF8): its own sample has ended, and it holds its voice of the
    /// budget until the last other voice of the group ends; it mixes nothing
    held: bool,
    /// `voice+0x3F`, the caller's **bend byte**: the request's `+9` at the start (0x5905FE;
    /// 0x561960's default 0x40), `Voice_setBend` 0x5660C0 → 0x566020 after (the byte, then the
    /// pitch word made again). 0x589F75…0x589F90 add `((i8)byte − 0x40) · (i16)voice+0x66 >> 6`
    /// cents ([`Clip::bend_cents`], code 0x0A · 100) to the voice's cents ([`Voice::pitch_word`]).
    bend: i8,
    /// the software strip's one-pole low-pass (0x5998E0's node, [`LowPass`]), one per channel
    lowpass: Option<LowPass>,
    /// Whether the start this voice came from is one [`Shared::silence`] reaches: either the
    /// voice table `0x656180…0x6563C8` the pause 0x4AB150 walks — `Voice_play` 0x4A9C10's write
    /// `[voice*8+0x656180] = handle` at a (re)start (0x4a9de9), for every voice number but 0x33 —
    /// or the speech phrase's own hard stop `0x4AF900`, folded in here for the same observed
    /// effect (the speech's `[0x6573A8]`/`[0x6573AC]` are not in the table either, but the pause
    /// silences them by that separate call). `false` for a voice a bare `Voice_start` 0x565E70
    /// began with no voice number at all: the HUD's `fe_sound` (0x4AADB0), the front end's
    /// `fesfx`, and — within the race — the checkpoint chime, the wrong-way cue, the pylon-hit cue
    /// and the cop siren's start (0x4AAE30/0x4AAE50/0x4AAE70/0x4AAE90, straight to
    /// `Bank_playOneShot` 0x4A98E0 with no `Voice_play` call at all) and the collision one-shots
    /// (voice 0x33: `wall_hit`, `car_car`, `prop_hit`, `bush_hit`, `landing`, `crash`, … —
    /// `Voice_play(0x33, …)` 0x4A9C10 takes the `ebx == 0x33` branch at 0x4a9cd4, which calls
    /// `Bank_playOneShot` and reads the table back at 0x4a9d0b for its answer, but never writes
    /// it — one voice number shared by every concurrent hit, so tracking a single handle for it
    /// would make each hit steal the last one's). See [`AudioOutput::silence_voices`].
    table: bool,
}

/// **The strip hook `+0xE0`** of the software path — `Voice_setFilter` 0x565F00(handle, value) →
/// 0x590C20(voice, value) → `strip+0xE0` = **0x596AC0** (set by the strip build 0x589FE0 on the
/// software arm, flag 4, 0x58A2BE; the hardware arms set 0x596D80 or nothing), for each channel
/// of the strip (`strip+0xB0`: a count byte, then the channel words): `f = value / (mix rate ·
/// 0.5)` (`fild`, `fmul` 0.5, `fdivp`, stored f32), then **0x5998E0**(channel, f): `f < 1` puts a
/// 0x1C-byte node (0x59A170: its pull 0x59A0F0, state `+0xC` = 0) at the head of the channel's
/// chain (0x596CD0, word `+8` = 0: after the resampler (2) and the optional biquad (1), before
/// the channel gain) and sets `+0x14 = f`, `+0x10 = 1 − f` (0x59A150); `f ≥ 1` takes the node out
/// and frees it (0x596D10, the state gone). The node's pull (0x59A0F0 → 0x59C170) runs, per sample
/// of the resampled block, `y = y · (1 − f) + f · x` and hands `y` on — a **one-pole low-pass**
/// with its corner at `value` Hz. Read to the instruction (0x59C170, 2026-10-02): `fld [edi]` loads
/// `y` (f32) once, each sample is `fmul [edi+4]` · `fld [edi+8]` `fmul [x]` · `faddp`, `fst dword`
/// stores the sample (the f32 handed on) and `y` itself stays on the FPU stack, only `fstp [edi]`
/// at the block's end rounds it to f32. So `y` runs in the x87's precision inside a block (53-bit,
/// the f64 reading of the audio path, see `resample.rs`) and is an f32 between blocks:
/// [`LowPass::begin_block`] / [`LowPass::end_block`] bracket a block, `acc` is the register.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LowPass {
    /// `+0x10`, `1 − f`
    pub a: f32,
    /// `+0x14`, `f`
    pub b: f32,
    /// `+0xC` per channel
    pub y: [f32; 2],
    /// the FPU register of 0x59C170 inside a block (loaded at `begin_block`, stored f32 at
    /// `end_block`)
    acc: [f64; 2],
}

impl LowPass {
    /// 0x59C170's `fld dword [edi]`: the stored f32 into the register.
    pub fn begin_block(&mut self) {
        self.acc = [self.y[0] as f64, self.y[1] as f64];
    }

    /// 0x59C170's `fstp dword [edi]`: the register rounded to f32 at the block's end.
    pub fn end_block(&mut self) {
        self.y = [self.acc[0] as f32, self.acc[1] as f32];
    }

    /// `f = value / (rate · 0.5)` as 0x596AC0 makes it; `None` for `f ≥ 1` (0x5998E9 `fcomp` 1.0,
    /// `test ah, 1`).
    pub fn coefficient(value: i32, mix_rate: u32) -> Option<f32> {
        let f = (value as f64 / (mix_rate as f64 * 0.5)) as f32;
        (f < 1.0).then_some(f)
    }

    /// 0x5998E0 on a voice's node: made at state 0 when there is none, its `f` set.
    pub fn set(node: &mut Option<LowPass>, value: i32, mix_rate: u32) {
        match Self::coefficient(value, mix_rate) {
            Some(f) => {
                let n = node.get_or_insert(LowPass { a: 0.0, b: 0.0, y: [0.0; 2], acc: [0.0; 2] });
                n.b = f;
                n.a = (1.0f64 - f as f64) as f32;
            }
            None => *node = None,
        }
    }

    /// 0x59C170 on one sample of channel `ch`.
    /// `fmul`/`faddp` in the register's precision, `fst dword` the f32 handed on.
    pub fn run(&mut self, ch: usize, x: f32) -> f32 {
        self.acc[ch] = self.acc[ch] * self.a as f64 + self.b as f64 * x as f64;
        self.acc[ch] as f32
    }
}

/// The value `Voice_play`'s update hands 0x565F00 (0x4A9FA8…0x4AA03B): nothing (`None`) for the
/// player's engine banks (`[0x6564A0]` / `[0x6564B0]`, kinds 0 and 1) or at `AUDIOQUALITY`
/// `[0x657C98]` ≥ 2; else **0xBB8** (3000 Hz) when `[0x627F43] & 8` (view ≤ 1), `0x443EE0` says
/// not exterior and the view word `[0x627F3C]` is neither 0 nor 0xF — the cockpit — and **0xFFFF**
/// (no filter) otherwise. Only the update of a playing voice runs it: a start returns before
/// (0x4A9DC9…0x4A9E03).
pub fn voice_play_filter(player_engine_bank: bool, audio_quality: u8, cockpit: bool) -> Option<i32> {
    if player_engine_bank || audio_quality >= 2 {
        return None;
    }
    Some(if cockpit { 0xBB8 } else { 0xFFFF })
}

/// A voice's envelope, **0x590607…0x590655** at the start and **0x58961E…0x589687** every
/// service tick: a 16.16 level `voice+0x34`, its step `+0x30`, the ticks left `+0x38`, the
/// segment `+0x41` of `+0x40` (code 0x09) over the record's `{ticks, target}` table (code 0x19,
/// [`nfs_formats::bnk::RecordTables::envelope`]). Code 0x08 (`+0xE` → `voice+0x42`) is stored and
/// never read (a byte scan of the library finds no reader), so there is no release: a segment of
/// negative ticks — 0x7FFFFFFF — holds for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Envelope {
    level: i32,
    step: i32,
    left: i32,
    seg: i8,
    segments: i8,
}

impl Envelope {
    /// The start: `level = (i8)code 0x1C << 16`, the first segment's ticks (negative →
    /// 0x7FFFFFFF) and step `((target << 16) − level) / ticks` (`idiv`, toward zero).
    fn start(level: i8, segments: i8, table: &[(i32, i32)]) -> Self {
        let mut e = Envelope { level: (level as i32) << 16, step: 0, left: 0, seg: 0, segments };
        e.load(table.first().copied().unwrap_or((0x7FFF_FFFF, 0)));
        e
    }

    fn load(&mut self, (ticks, target): (i32, i32)) {
        self.left = if ticks < 0 { 0x7FFF_FFFF } else { ticks };
        // a segment of 0 ticks divides by zero in the original; no shipped table has one
        self.step = if self.left == 0 { 0 } else { target.wrapping_shl(16).wrapping_sub(self.level) / self.left };
    }

    /// One service tick: the level takes its step; with no tick left the next segment is loaded
    /// — or, past the last (`jl`, signed), the voice is to be stopped (`true`); then a tick is
    /// counted off (`dec`). The step is taken before the count is looked at, so a segment runs
    /// its ticks plus one.
    fn tick(&mut self, table: &[(i32, i32)]) -> bool {
        if self.step != 0 {
            self.level = self.level.wrapping_add(self.step);
        }
        if self.left == 0 {
            self.seg = self.seg.wrapping_add(1);
            if self.seg >= self.segments {
                return true;
            }
            self.load(table.get(self.seg as usize).copied().unwrap_or((0x7FFF_FFFF, 0)));
        }
        self.left = self.left.wrapping_sub(1);
        false
    }
}

/// The record's tuning, the start of the pitch word `voice+0x68` (0x5904EC…0x590528):
/// `(note − root) · 100 + fine` cents, the note the request's byte `+5` (0x3C, 0x561983, which no
/// caller of the game changes), the root code 0x07 (default 60, 0x5991A2), the fine tune code
/// 0x10; a 16-bit word (`add ax, [edi+4]`, `mov word [esi+0x68], ax`). So a sample recorded
/// with its root off middle C plays that many semitones off unless the caller's pitch
/// makes up for it — the engine banks (`sd*.viv`) carry roots of 28…72 (most 50…66) and fine
/// tunes of −80…113 cents.
/// *(Until 2026-09-27 the port left both out: "decoded but not yet applied".)*
pub fn base_cents(r: &nfs_formats::bnk::PatchRecord) -> i16 {
    ((crate::librand::REQUEST_NOTE as i32 - r.root as i32) * 100 + r.fine as i32) as i16
}

/// `x / 127³` as 0x589E29…0x589E3B makes it: `imul` by 0x4186143D, the high word `sar 0x13`,
/// the sign bit added back — toward zero.
fn div_127_cubed(x: i32) -> i32 {
    let q = ((x as i64 * 0x4186_143D) >> 32) as i32 >> 19;
    q + ((q as u32) >> 31) as i32
}

/// `x / 127` as 0x589E53…0x589E64 makes it: `imul` by 0x81020409, `add edx, x`, `sar 6`, the
/// sign bit added back — toward zero.
fn div_127(x: i32) -> i32 {
    let q = (((x as i64 * 0x8102_0409u32 as i32 as i64) >> 32) as i32).wrapping_add(x) >> 6;
    q + ((q as u32) >> 31) as i32
}

/// 0x5E1D3C…0x5E1E3B, the image's 256 bytes 0x589E80 reads: the mantissa of `2^(i/256)`, less
/// one, in 256ths (not exactly any rounding of it — the bytes as they are).
const CENTS_MANTISSA: [u8; 256] = [
    0, 1, 2, 2, 3, 4, 4, 5, 6, 7, 7, 8, 9, 9, 10, 11, 12, 12, 13, 14, 14, 15, 16, 17, 17, 18, 19, 20, 20, 21, 22, 23, 23, 24, 25, 26, 26, 27, 28, 29, 30, 30,
    31, 32, 33, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 41, 42, 43, 44, 45, 45, 46, 47, 48, 49, 50, 50, 51, 52, 53, 54, 55, 55, 56, 57, 58, 59, 60, 61, 61,
    62, 63, 64, 65, 66, 67, 67, 68, 69, 70, 71, 72, 73, 74, 75, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96,
    97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 119, 120, 121, 122, 123, 124, 125, 126, 127,
    128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 154, 155, 156, 157, 158, 159, 160,
    161, 163, 164, 165, 166, 167, 168, 169, 171, 172, 173, 174, 175, 176, 178, 179, 180, 181, 182, 183, 185, 186, 187, 188, 189, 191, 192, 193, 194, 196, 197,
    198, 199, 200, 202, 203, 204, 205, 207, 208, 209, 210, 212, 213, 214, 216, 217, 218, 219, 221, 222, 223, 225, 226, 227, 229, 230, 231, 232, 234, 235, 236,
    238, 239, 240, 242, 243, 245, 246, 247, 249, 250, 251, 253, 254, 255,
];

/// **0x589E80**, cents to the library's 4096-based pitch word: whole octaves shift 0x1000
/// (`c ≥ 1200`: `c / 1200` by the magic 0x1B4E81B5, `shl` that many times; `c ≤ −1200` the
/// same on `−c`, `sar`), the rest `i = c · 13981 >> 16` (≈ `c · 256 / 1200`, `sar`), not below
/// −255, and the word `(mantissa[i] + 256) · w >> 8`, or for a negative `i`
/// `(mantissa[256 + i] + 256) · w >> 9` (the byte at 0x5E1E3C + i). The word is a u16
/// (`mov word [esi+0x6A], ax`).
pub fn cents_word(cents: i32) -> u16 {
    let mut c = cents;
    let mut w: i32 = 0x1000;
    if c >= 1200 {
        let q = c as u32 / 1200;
        c -= (q * 1200) as i32;
        for _ in 0..q.min(40) {
            w = w.wrapping_shl(1);
        }
    }
    if c <= -1200 {
        let q = c.wrapping_neg() as u32 / 1200;
        c += (q * 1200) as i32;
        for _ in 0..q.min(40) {
            w >>= 1;
        }
    }
    let i = (c.wrapping_mul(13981) >> 16).max(-255);
    let out = if i < 0 {
        ((CENTS_MANTISSA[(256 + i) as usize] as i32 + 256).wrapping_mul(w)) >> 9
    } else {
        ((CENTS_MANTISSA[i as usize] as i32 + 256).wrapping_mul(w)) >> 8
    };
    out as u16
}

impl Voice {
    fn new(clip: Arc<Clip>, word: u16, volume: f32, looping: bool, stamp: u32) -> Self {
        let (patch_pan, level) = (clip.pan, (clip.gain * 127.0).round() as i8);
        let env = match (clip.records.first(), clip.tables.first()) {
            (Some(r), Some(t)) => Envelope::start(r.env_level, r.env_segments, &t.envelope),
            _ => Envelope::start(0x7F, 1, &nfs_formats::bnk::DEFAULT_ENVELOPE),
        };
        Voice { clip, cur: Cursor::default(), gains: Gains::default(), hook: false, word, vol16: volume_16(volume), ramp: Ramp::default(), pan: 0x40, looping, finished: false, fx_scale: 0x7F, send: None, wet: 0.0, stamp, patch_pan, level, cents: 0, env, volume_phase: 0, pitch_phase: 0, pulled: false, anchor: false, held: false, bend: 0x40, lowpass: None, table: true }
    }

    /// **`Voice_setVolume` 0x566340 → 0x5662B0** on one voice: the int `<< 16` into `voice+0x2C`
    /// and the ramp `+0x24` zeroed (0x566303…0x566306) — so a `setVolume` cancels a ramp —
    /// unless `+0x2C` already holds that value, when nothing is written (0x5662FE `je`; the
    /// call then returns 0 without walking the rest of the group, 0x566338 — the group's voices
    /// share their volume, so the port writes each alike).
    fn set_volume(&mut self, volume: f32) {
        let v = volume_16(volume);
        if self.vol16 != v {
            self.vol16 = v;
            self.ramp.step = 0;
            // 0x56631C: `0x590D10` → the volume hook → 0x598700 per channel
            self.hook = true;
        }
    }

    /// **0x589DF0**, the voice's level byte `voice+0x3D`: `(envelope >> 16) · (volume >> 16) ·
    /// voice+0x3C · [0x6B499D] / 127³` (`imul`s, the magic 0x4186143D, `sar 0x13`, toward zero),
    /// stored as a byte; with a volume table, `· (i8)table[voice+0x4A] / 127` (0x81020409, toward
    /// zero); with a level curve, `curve[(i8)level]` (0x589E69…0x589E78). `[0x6B499D]` is the
    /// library's master level, 0x7F from its start (0x5657D7). Its only setter 0x5663C0 (the
    /// byte, then every playing voice's level made again, 0x5663E2…0x56641F) has four calls, all
    /// in the race audio's two resets — 0x4AB270 (called at 0x41523A, 0x415299, 0x4156A1, 0x415754,
    /// 0x41580D; `replay.md`) and 0x4AB360 (the race restart, 0x415721 / 0x4157F2): 0 at 0x4AB2B1 / 0x4AB3A2 before the
    /// voices are stopped, then 0, 1, …, 0x7F in a loop (0x4AB348…0x4AB35A, 0x4AB41B…0x4AB42D)
    /// within the same call. No option, slider, fade or front-end level touches it — the Audio
    /// options reach a voice through its `setVolume` byte (`sfx::category`) — so between those
    /// microseconds it is 0x7F, and the port keeps the constant. The volume word `voice+0x2C >> 16`
    /// (`sar`) is the caller's `setVolume` int (`<< 16`, 0x5662E3, not clamped) — or a release
    /// ramp's way down ([`Ramp`]); the port's callers hand `byte / 127`, which [`volume_16`]
    /// gives back exactly.
    fn level_byte(&self) -> i8 {
        const MASTER: i32 = 0x7F;
        let volume = self.vol16 >> 16;
        let x = (self.env.level >> 16).wrapping_mul(volume).wrapping_mul(self.level as i32).wrapping_mul(MASTER);
        let mut l = div_127_cubed(x) as i8;
        if let Some(t) = self.clip.tables.first() {
            if let Some(v) = &t.volume {
                let e = v.get(self.volume_phase as usize).copied().unwrap_or(0) as i32;
                l = div_127(e * l as i32) as i8;
            }
            if let Some(c) = &t.curve {
                l = c[(l as i32 + 128) as usize] as i8;
            }
        }
        l
    }

    /// **0x589F40**'s pitch word `voice+0x6E`: the cents `voice+0x68` plus, with a pitch table,
    /// `((i8)table[voice+0x4B] − 0x40) · (i16)depth >> 6` (0x589F92…0x589FAF, `sar`), through
    /// 0x589E80 ([`cents_word`]) into `voice+0x6A`, then `voice+0x6C · voice+0x6A >> 12` (0x589FBE…
    /// 0x589FD0) — `voice+0x6C` is [`Voice::word`], the caller's own pitch word, no longer a float
    /// (audio port 8). The bend term (0x589F60…0x589F90): `((i8)voice+0x3F − 0x40) · (i16)voice+0x66
    /// >> 6` (`sar`) when `+0x66` is not 0; through code 0x17's curve `+0x58` when set — no shipped
    /// bank has it. *(Until 2026-09-27 the bend was the caller's float factor `Clip::bend`,
    /// multiplied into the pitch — an octave's cents through `exp2` rather than the table 0x589E80
    /// walks.)*
    fn pitch_word(&self) -> u16 {
        let mut cents = self.cents as i32;
        if self.clip.bend_cents != 0 {
            cents += ((self.bend as i32 - 0x40) * self.clip.bend_cents as i16 as i32) >> 6;
        }
        if let (Some(r), Some(Some(t))) = (self.clip.records.first(), self.clip.tables.first().map(|t| t.pitch.as_ref())) {
            let e = t.get(self.pitch_phase as usize).copied().unwrap_or(0) as i32;
            cents += ((e - 0x40) * r.pitch_depth as i32) >> 6;
        }
        let word = cents_word(cents) as i32;
        ((self.word as i32).wrapping_mul(word) >> 12) as u16
    }

    /// The looped range the stream repeats, `(lo, hi)` in frames.
    fn range(&self) -> (usize, usize) {
        let nframes = self.clip.samples.len() / self.clip.channels.max(1) as usize;
        match (self.looping, self.clip.loop_range) {
            (true, Some((a, b))) if b > a && (b as usize) <= nframes => (a as usize, b as usize),
            _ => (0, nframes),
        }
    }

    /// The frame of the sample a stream index `k` reads: `k` up to the end, then round the loop;
    /// `None` before the start (the resampler's zeroed history) and past the end of a one-shot
    /// (the decoder's zero fill, 0x597BF2).
    fn frame_of(&self, k: i64) -> Option<usize> {
        let (lo, hi) = self.range();
        if k < 0 {
            return None;
        }
        let k = k as usize;
        if k < hi {
            Some(k)
        } else if self.looping && hi > lo {
            Some(lo + (k - hi) % (hi - lo))
        } else {
            None
        }
    }

    /// Whether this block's pull fails, which frees the voice (0x5987D2…0x5987DC): only a
    /// one-shot's decoder fails — the PCM one 0x597B60 when its position `+0x10` has reached the
    /// sample's end `+0x14` (`jb`, unsigned; it zero-fills past the end and moves on, 0x597BF2),
    /// the EA-XA one 0x597C40 when its decoded count `+0x28` has (`jl`), the looped one 0x59CBA0
    /// never. The resampler 0x59BE00 asks the decoder for `(step · n + frac) >> 16` samples a
    /// block (none, and no failure, when that is 0, 0x59BF88), the first time for its history
    /// less the zeroed look-behind on top (`+0x22 − +0x20`: 4 − 1 cubic, 2 − 0 linear,
    /// 0x59C0A0…0x59C0C2; 0x59BF4D…0x59BF53); so the decoder runs that many samples ahead of the
    /// resampler's position, and the pull fails on the first block that starts with the position
    /// that close to the end. The failure goes up the chain (0x59BFA5 → the negative return).
    /// *(Until 2026-09-27 the port ended a one-shot once its position passed the last sample —
    /// up to a block earlier, the voice's budget freed that much sooner.)*
    fn pull_fails(&self, step: u32, n: usize, interp: Interp) -> bool {
        if self.looping {
            return false;
        }
        let lead = match interp {
            Interp::Cubic => 3,
            Interp::Linear => 2,
        };
        let mut need = (step as u64 * n as u64 + self.cur.frac as u64) >> 16;
        let at = if self.pulled {
            self.cur.pos + lead
        } else {
            need += lead;
            0
        };
        let (_, hi) = self.range();
        need > 0 && at >= hi as u64
    }

    /// The position in the sample, in frames (fractional).
    fn position(&self) -> f64 {
        let whole = self.frame_of(self.cur.pos as i64).unwrap_or(self.cur.pos as usize);
        whole as f64 + self.cur.frac as f64 / 65536.0
    }
}

/// **0x589D70**: a voice's two effect sends are `e = voice+0x45 · voice+0x44` (both signed bytes)
/// times each environment bus over 127·127 — `0x82061029` then `add edx`, `sar 13` and the sign
/// bit added back: a divide by 16129 toward zero — stored as the bytes `voice+0x46` (bus `+0xC`,
/// `Mixer_setEnvParam(0x20, ·)`) and `voice+0x47` (bus `+0xD`, `(0x80, ·)`), set by 0x565CD0.
/// Recomputed at the voice start (0x5906DA) and on every bus change (0x565D28…0x565D51).
fn fx_send(fx_scale: u8, patch: u8, bus: i8) -> i32 {
    (fx_scale as i8 as i32 * patch as i8 as i32) * bus as i32 / 16129
}

/// The software path's reverb send for a voice (`[drv+0xD8]` = 0x596A10 when the voice start's
/// flag 0x10 is clear, 0x58A455…0x58A468): the byte `voice+0x46` as an int times `[0x5C1788]`
/// (an f32 1/127, `fild ; fmul ; fstp`, 0x596A1F…0x596A2E), which 0x599800 stores as the channel's
/// `+4` (0x596A3B). The DirectSound path (flag 0x10, 0x596400) sends `voice+0x47 / 127` — the
/// `0x80` bus — as the EAX buffer reverb mix (property set {4A4E6FC0-C341-11D1-B73A-
/// 444553540000}) instead. The game never takes that path: its only voice type is 0x24 (see
/// `pan_gains`).
fn reverb_input(fx_scale: u8, patch: u8, bus_c: i8) -> f32 {
    (fx_send(fx_scale, patch, bus_c) as f64 * (1.0f32 / 127.0) as f64) as f32
}

/// The channel's wet gain `+8` (0x5980A0): `volume · send · 1.5` (`[0x5B246C]`), the volume being
/// the channel's *current* `+0xC` — recomputed at every send change (0x599800) and, while a gain
/// ramp runs, at every sample (0x5981CA).
fn wet_gain(vol_cur: f32, send: f32) -> f32 {
    (vol_cur as f64 * send as f64 * 1.5) as f32
}

/// The software mixer's stereo law (`RE/notes/engine-audio.md`, "The pan law"): 0x58E0E0 builds a
/// 256-step table over the voice's angle (`[0x6B1820 + 4·(angle >> 8)]`, 0x58E290) with channel 0
/// at 0x4000 and channel 1 at 0xC000 (`[0x6B48FC]` / `[0x6B48FE]`, 0x58A5DF / 0x58A5E8); between
/// them the angle splits **linearly**, and each channel's share `x` (0…127, `idiv`) becomes
/// `f(x) = 2x − x²/127` (0x58E0B0), scaled by 1/127 (`[0x5C1788]`). Front and back mirror.
///
/// The angle is the library's `(p − 0x40) << 8` of the voice's pan byte `p` ([`voice_pan`]), so
/// the step is `(p − 0x40) & 0xFF` and the two shares come out as `127·p/128` for channel 0 and
/// `127·(128 − p)/128` for channel 1 — 0x7F: 127 / 0, 0x40: 95 / 95, 0: 0 / 127. Channel 0 is the
/// first of the stereo pair the buffer holds (`nChannels` 2, 0x58ED6A) — the port's first output
/// channel; a source on the `+side` hand of [`pan_angle`] is heard there.
///
/// This is the path of every voice the game starts: the hardware-3D voice count `[0x6B48DF]` is
/// 0 whatever the card — 0x4A66B0 configures it only when `[0x657C9C]` is set, and both start-ups
/// clear that just before the call (0x4A684C, 0x4A6A61) — so 0x58A7C4…0x58A7D6 drops every 3D
/// type and the one voice type left is the configuration's 0x24 (0x4A670B), never flag 0x10. A
/// hardware-3D voice would be positioned with `SetPosition` (0x596370) instead. *(Until
/// 2026-09-27 this was an approximation "for a card without DirectSound3D buffers".)*
/// (Until 2026-09-25 this was an equal-power cos / sin law of the port's own.)
/// (Until 2026-09-27 the port fed this law the sine of the angle, taken back to an angle and
/// rounded onto the grid, instead of the pan byte.)
fn pan_gains(p: u8) -> [f32; 2] {
    // the table's step and the angle it stands for
    let x = ((p as i32 - 0x40) & 0xFF) << 8;
    let (s0, s1) = (0x4000, 0xC000);
    // (distance to channel 0, distance to channel 1) along the arc between them (0x58E137…0x58E24C)
    let (d0, d1) = if x <= s0 {
        (s0 - x, x - s1 + 0x10000)
    } else if x >= s1 {
        (s0 - x + 0x10000, x - s1)
    } else {
        (x - s0, s1 - x)
    };
    // the byte · `[0x5C1788]` (f32 1/127) stored as an f32 (0x59694C)
    let f = |x: i32| ((2 * x - x * x / 127) as f64 * (1.0f32 / 127.0) as f64) as f32;
    // each channel's share grows with the distance to the other one
    [f(127 * d1 / (d0 + d1)), f(127 * d0 / (d0 + d1))]
}

#[derive(Default)]
struct Shared {
    voices: Vec<(VoiceId, Voice)>,
    streams: Vec<(VoiceId, StreamVoice)>,
    #[cfg(test)]
    next_id: u32,
    /// the environment buses of record 0, `[0x6B4A0C]` (`Mixer_setEnvParam(0x20, ·)`, the tunnel
    /// reverb's level) and `[0x6B4A0D]` (`(0x80, ·)`, half of it) — `engine-audio.md` §4.7
    bus_c: i8,
    bus_d: i8,
    tail: Option<Box<Reverb>>,
    /// the software voice count `[0x6B48DD]` ([`voice_budget`]); `None` until the quality is
    /// given — the library's default 24 (0x58A5B5)
    budget: Option<usize>,
    /// the most voices in use at once (a debug log line only)
    peak: usize,
    /// the mix rate `[0x6B48DA]` ([`mix_rate`]); `None` until the quality is given
    mix_rate: Option<u32>,
    /// the mixed frames on their way to the device's rate
    bridge: Bridge,
    /// the driver thread's writes ([`Pump`]) the mix is rendered in
    pump: Pump,
    /// the driver thread's `Sleep` `[0x6B48E1]` ([`pump_sleep_ms`]); `None` until the quality is
    /// given — the library's default 10 (0x58A570)
    sleep_ms: Option<u32>,
    /// the resampler's interpolator, `[0x6B48E5]` ([`Interp::for_quality`]); linear until the
    /// quality is given (the library's 0, 0x58A5D2)
    interp: Interp,
    /// the library's random generator 0x598FF0 ([`crate::librand`]), which every bank-patch start
    /// draws from
    rand: crate::librand::LibRand,
    /// the service ticks the mix is counted in (0x58FBC0)
    clock: ServiceClock,
    /// **`[0x6B49A8]`**, the library service's own tick count (0x589531…0x589538: `inc`ed at the
    /// top of every call to [`Shared::service`], before anything else): 0 at start-up (`mov
    /// [0x6B49A8], ebx` with `ebx` 0, 0x5657D1) and never reset — unlike [`ServiceClock::tick`]
    /// (`[0x6AD7AC]`, 0x58FBC0's own count of mixed frames, which wraps at 30000): the two
    /// increment in lockstep (0x58FBC0 calls the service once, then counts the next tick's frames,
    /// [`Shared::render`]) but are different variables, byte-scan confirmed (`bscan.py 0x6b49a8`
    /// finds no other writer). This is `voice+0x20`'s stamp ([`Voice::stamp`], [`pick_victims`]):
    /// every start between two service calls reads the same value, so two voices started in the
    /// same tick tie on it, as the original's do.
    service_tick: u32,
    /// the callback's reusable buffers (PERF F5: nothing in the device callback allocates)
    scratch: Scratch,
}

/// Buffers the mixer reuses from one call to the next instead of allocating each time; made
/// once ([`Shared::reserve_rt`]), emptied and refilled.
#[derive(Default)]
struct Scratch {
    /// the frames [`Shared::render_into`] mixes a driver pass in
    mix: Vec<[f32; 2]>,
    stopped: Vec<VoiceId>,
    stop: Vec<VoiceId>,
    failed: Vec<usize>,
    live: Vec<(u8, u16, u32)>,
    live_ids: Vec<VoiceId>,
    live_held: Vec<bool>,
    live_at: Vec<usize>,
    order: Vec<usize>,
    victims: Vec<usize>,
}

/// The most frames 0x598940 mixes at once (`cmp edi, 0x100`, 0x598979): a block, the unit every
/// voice's chain is pulled in (0x598760).
pub const BLOCK: usize = 256;

/// **0x58FBC0**'s count of the mix in service ticks: before each tick's frames the library
/// service runs (`call [0x6B49AC]` = 0x589520, set at 0x5657C2), and a tick is a hundredth of a
/// second of the mix rate — `[0x6AD7AC]` the ticks, `[0x6AD7B0]` the frames counted so far,
/// `rate · tick / 100` (`imul`, the magic 0x51EB851F, `sar 5`, toward zero) less those, rounded
/// down to 16 (`and edx, 0xFFFFFF0`); both reset after tick 30000 (`cmp esi, 0x7530 ; jle`).
/// `[0x6AD7A8]` is what is left of the tick (0x58FC5F). At 32000 Hz a tick is 320 frames; at
/// 22050 it is 208, 224, 224, … (220.5 on average); at 16000, 160.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceClock {
    pub tick: u32,
    pub counted: u32,
    pub left: u32,
}

impl ServiceClock {
    /// The next tick's frames (0x58FBF6…0x58FC47).
    pub fn next_tick(&mut self, mix_rate: u32) -> u32 {
        self.tick += 1;
        let total = (mix_rate as i64 * self.tick as i64 / 100) as u32;
        let frames = total.wrapping_sub(self.counted) & 0x0FFF_FFF0;
        self.counted = self.counted.wrapping_add(frames);
        if self.tick > 30000 {
            self.tick = 0;
            self.counted = 0;
        }
        frames
    }
}

/// The library's mixing rate per `AUDIOQUALITY` `[0x657C98]`: `0x5B4AE4[q]` = 32000 / 22050 /
/// 16000 (0x4A67AE), taken as the smaller of it and the caps word `[0x6B48C2]` — the library's
/// constant 48000 (0x58A555), so always the table's — into config `+0x1A` (0x4A67D0 / 0x4A67D7),
/// which 0x5656D0 copies to `[0x6B48DA]`; 0x58A703…0x58A72A clamps it to 11025…48000. At quality 3
/// the configuration is skipped and the library's 22050 stays (0x58A5C2).
///
/// The DirectSound driver opens its **primary** buffer at that rate (`SetFormat`, vtable `+0x38`:
/// PCM, 2 channels, `[0x6B48DA]` a second, 16 bits, 0x58ED4B…0x58ED88) and its mixing buffer
/// (`CreateSoundBuffer`, flags 0x18000) the same (0x58EDBC…0x58EE6B), and every software voice
/// steps through its sample at `sample rate / [0x6B48DA]` (0x596B60). So the bank samples are
/// resampled to this rate, and the card (or the system mixer) plays that.
pub fn mix_rate(audio_quality: u8) -> u32 {
    match audio_quality {
        0 => 32000,
        1 => 22050,
        2 => 16000,
        _ => 22050,
    }
}

/// The driver thread's `Sleep` between two passes, `[0x6B48E1]` (0x58F8AD…0x58F8B3, `Sleep` the
/// import at 0x5B220C): config `+0x21` from `0x5B4B14[q]` = 8 / 20 / 50 ms (0x4A67B5, stored at
/// 0x4A67C4 — `[esp+0x2D]` under the pushed `edi`; copied by 0x5656D0 from `+0x14` to 0x6B48D4),
/// clamped to 200 (0x58A855…0x58A86B); the library's default 10 (0x58A570) at quality 3, where the
/// configuration is skipped.
pub fn pump_sleep_ms(audio_quality: u8) -> u32 {
    match audio_quality {
        0 => 8,
        1 => 20,
        2 => 50,
        _ => 10,
    }
}

/// The DirectSound mixing buffer, in frames `[0x6AD7B4]`: `rate · 400 / 1000` (`lea ·5 ; lea ·5 ;
/// shl 4`, the magic 0x10624DD3 `sar 6`) rounded down to 16 (`and 0xFFFF0`, 0x58EDD5…0x58EE19) —
/// 0.4 s of the mix rate.
pub fn ring_frames(mix_rate: u32) -> u32 {
    (mix_rate * 400 / 1000) & 0xF_FFF0
}

/// **The driver thread's writes** (0x58F880 → 0x58F8E0 → 0x58F8F0): each pass takes the play
/// cursor (`GetCurrentPosition`, vtable `+0x10`, in frames, `and 0xFFFFF0`, 0x58F94D…0x58F985),
/// writes from the last write's end `[0x6AD78C]` up to `cursor + lead [0x6AD790]` rounded down to
/// 16 (0x58F9BE…0x58F9C7), wrapping at [`ring_frames`], and hands each of the (one or two) locked
/// parts to 0x58FBC0 (0x58FA40…0x58FADD), which mixes it in service ticks and blocks
/// ([`Shared::render`]); then it `Sleep`s [`pump_sleep_ms`]. So a write is the frames the cursor
/// moved in one pass, in multiples of 16 (at 22050 Hz and 20 ms: 432 or 448, 441 on average),
/// cut in two at the ring's end. APPROXIMATION: the cursor is taken to move exactly the sleep's
/// frames a pass (the sum of every pass's `sleep · rate / 1000`, `n · sleep · rate / 1000` after
/// `n` passes of one setting) — the thread's own work, the scheduler and the first pass's lead
/// fill are not modelled.
///
/// The cursor moves with time, so a pass after the sleep (or the rate) changed writes one pass of
/// the new setting, not the whole run again at it. *(Until 2026-10-09 the end was `passes ·` the
/// current sleep: the library's 10 ms during the start-up movies, then `AUDIOQUALITY` 1's 20 ms
/// from the front end's first sound — one pass wrote as many frames as had been played so far, the
/// whole intro again, mixed at once before anything the front end started: AUDIO-START, seconds of
/// silence in the menu after the movies.)*
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pump {
    pub passes: u64,
    /// the frames written so far (the write end, unwrapped)
    pub end: u64,
    /// `Σ sleep · rate` over the passes so far: the cursor, in thousandths of a frame
    pub moved: u64,
}

impl Pump {
    /// The next pass's parts: up to the ring's end, then the rest from its start.
    pub fn next_write(&mut self, mix_rate: u32, sleep_ms: u32) -> [u32; 2] {
        self.passes += 1;
        self.moved += sleep_ms as u64 * mix_rate as u64;
        let end = (self.moved / 1000) & !15;
        let w = end.saturating_sub(self.end) as u32;
        let ring = ring_frames(mix_rate).max(16);
        let pos = (self.end % ring as u64) as u32;
        self.end = end.max(self.end);
        let first = w.min(ring - pos);
        [first, w - first]
    }
}

/// From the mix rate to the output device's: the port mixes at [`mix_rate`] and interpolates
/// linearly between two mixed frames per device frame. APPROXIMATION of that one step: the
/// original hands the mix-rate buffer to DirectSound, and the conversion to the card's rate (if
/// any) is the card's or the system mixer's, not the game's.
#[derive(Default)]
struct Bridge {
    pending: std::collections::VecDeque<[f32; 2]>,
    /// the position between `pending[0]` and `pending[1]`, in mix frames
    t: f64,
}

impl Bridge {
    /// The next device frame. `step` is the mix rate over the device rate; `left` the device
    /// frames still wanted in this callback, which `render` is offered (the stream's renders one
    /// driver pass whatever it is offered, [`Shared::pump`]).
    fn next(&mut self, step: f64, left: usize, mut render: impl FnMut(usize, &mut std::collections::VecDeque<[f32; 2]>)) -> [f32; 2] {
        while self.pending.len() < 2 {
            let want = (left as f64 * step).ceil() as usize + 2;
            render(want, &mut self.pending);
        }
        let (a, b, t) = (self.pending[0], self.pending[1], self.t as f32);
        self.t += step;
        while self.t >= 1.0 && self.pending.len() > 1 {
            self.t -= 1.0;
            self.pending.pop_front();
        }
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }
}

impl Shared {
    /// The library's mix rate `[0x6B48DA]`: the table's once 0x4A66B0 has configured the sound
    /// ([`mix_rate`]), the library's own 22050 (`mov word [0x6B48DA], 0x5622`, 0x58A5C2) before.
    fn mix_rate(&self) -> u32 {
        self.mix_rate.unwrap_or(mix_rate(3))
    }

    /// One pass of the driver thread ([`Pump`]): its one or two parts, each rendered as 0x58FBC0
    /// renders it.
    #[cfg(test)]
    fn pump(&mut self, mix_rate: u32) -> Vec<[f32; 2]> {
        let mut out = std::collections::VecDeque::new();
        self.pump_to(mix_rate, &mut out);
        out.into()
    }

    /// [`Shared::pump`] appending to `dst` (the [`Bridge`]'s queue) — no allocation once the
    /// buffers have their size.
    fn pump_to(&mut self, mix_rate: u32, dst: &mut std::collections::VecDeque<[f32; 2]>) {
        let sleep = self.sleep_ms.unwrap_or(pump_sleep_ms(3));
        let mut buf = std::mem::take(&mut self.scratch.mix);
        for part in self.pump.next_write(mix_rate, sleep) {
            if part > 0 {
                self.render_into(&mut buf, part as usize, mix_rate);
                dst.extend(buf.iter().copied());
            }
        }
        self.scratch.mix = buf;
    }

    /// The software mixer's output for `n` stereo frames at `mix_rate`, as **0x58FBC0** drives it:
    /// whenever the frames of the last service tick are used up, the library service 0x589520
    /// runs ([`Shared::service`]) and the next tick's frames are counted ([`ServiceClock`]); the
    /// frames are mixed in blocks of at most [`BLOCK`] (0x598940 → 0x598760), never across a
    /// tick. Every voice is resampled to the mix rate by the library's kernel ([`resample`]: the
    /// 16.16 step of 0x596B60 / 0x596A50, the cubic or linear interpolator of `[0x6B48E5]`), mixed
    /// at its channel gains with their ramp (0x598760 / 0x5980C0) into floats of 16-bit scale,
    /// then the reverb, the streams and the 16-bit store (0x59A350). [`Bridge`] takes these frames
    /// on to the device's rate.
    ///
    /// 0x58FBC0 is handed one part of the driver thread's write, and a block ends where that part
    /// ends too: the port renders the same parts ([`Pump`], [`Shared::pump`]). *(Until 2026-09-27
    /// a block ended where the device callback's request did.)*
    #[cfg(test)]
    fn render(&mut self, n: usize, mix_rate: u32) -> Vec<[f32; 2]> {
        let mut out = Vec::new();
        self.render_into(&mut out, n, mix_rate);
        out
    }

    /// [`Shared::render`] into `out`, which is cleared and sized to `n` frames.
    fn render_into(&mut self, out: &mut Vec<[f32; 2]>, n: usize, mix_rate: u32) {
        out.clear();
        out.resize(n, [0.0f32; 2]);
        let mut at = 0;
        while at < n {
            if self.clock.left == 0 {
                // 0x58FBF6…0x58FC3A: the service first, then the tick's frames (0 for a tick
                // that rounds to nothing, which only runs the service again)
                self.service();
                self.clock.left = self.clock.next_tick(mix_rate);
                continue;
            }
            let len = (n - at).min(self.clock.left as usize).min(BLOCK);
            self.mix_block(&mut out[at..at + len], mix_rate);
            self.clock.left -= len as u32;
            at += len;
        }
        // the 16-bit store of the mix (0x59A350), back to the port's float scale
        for frame in out.iter_mut() {
            for s in frame.iter_mut() {
                *s = resample::to_i16(*s) as f32 / 32768.0;
            }
        }
    }

    /// **0x589520**, the library service, once a tick of mixed frames ([`ServiceClock`]), over
    /// every voice playing (state 1, a handle of its own, 0x589581…0x58958D), in this order:
    ///
    /// * a **pitch table** (`voice+0x60`): its step `+0x4B` moves on one, back to 0 at the
    ///   length `+0x49` (code 0x21, unsigned `jb`), and the pitch is made again (0x589593…0x5895B7);
    /// * a **volume table** (`+0x5C`): its step `+0x4A` likewise at `+0x48` (code 0x1E)
    ///   (0x5895BF…0x5895DE);
    /// * the ramp `voice+0x24` (0x5895E1…0x589619, [`Ramp`]) — below 0 it stops the handle; a
    ///   stream's volume ramp is [`Fade`];
    /// * the **envelope** ([`Envelope::tick`], 0x58961E…0x589687), which stops the voice's handle
    ///   — its group — once its segments have run (0x58964D → 0x565D60);
    /// * the level made again (0x589DF0) when any of these changed it.
    ///
    /// The port makes the level and the pitch word from the voice's state at every block
    /// ([`Voice::level_byte`], [`Voice::pitch_word`]), which is the same between two ticks —
    /// except that a caller's volume or pitch counts from the next block, where the library's
    /// `setVolume` ramp waits for the next tick.
    fn service(&mut self) {
        // 0x589531…0x589538, the very first thing the service does: `[0x6B49A8] += 1`
        self.service_tick = self.service_tick.wrapping_add(1);
        let mut stopped = std::mem::take(&mut self.scratch.stopped);
        stopped.clear();
        for (id, sv) in self.streams.iter_mut() {
            if let Some(f) = sv.fade.as_mut() {
                if f.tick() {
                    stopped.push(*id);
                }
                sv.volume = f.volume();
                if f.done() {
                    sv.fade = None;
                }
            }
            // **a stream whose song has been played is gone**: the stream's position callback
            // 0x566E40 adds the frames played to the item's `+0x14` and, once it reaches the
            // item's length `+0x18` (0x566E7C…0x566E84), takes the item off (0x566D80 at 0x566E8A),
            // so the stream object's item count `+0x16` (0x567550 adds one per queued song) drops
            // to 0 — the running flag 0x566A40 hands `MusicUpdate` (`+0x8C`), whose state 0 then
            // advances the playlist (0x4ADD6D). Here: the feeder has decoded the last block and
            // the mix has taken every queued frame. *(Until 2026-10-09 a drained stream stayed in
            // the mix for ever, so the music player never saw it end and the race went on in
            // silence after the first song — AUDIO-RACEMUSIC.)*
            if sv.src.finished.load(Ordering::Acquire) && sv.src.queue.len() < sv.src.channels.max(1) as usize && !stopped.contains(id) {
                stopped.push(*id);
            }
        }
        self.streams.retain(|(i, _)| !stopped.contains(i));
        self.scratch.stopped = stopped;
        let mut stop = std::mem::take(&mut self.scratch.stop);
        stop.clear();
        for (id, v) in self.voices.iter_mut() {
            // a voice whose group a stop earlier in this pass took is not in state 1 any more
            if v.finished || v.held || stop.contains(id) {
                continue;
            }
            let Some(r) = v.clip.records.first().copied() else {
                // not a bank patch: no tables, no envelope — the ramp only
                let ramping = v.ramp.step != 0;
                if v.ramp.tick(&mut v.vol16) {
                    stop.push(*id);
                } else {
                    v.hook |= ramping;
                }
                continue;
            };
            // 0x589598…0x58962D: the tick recomputes the level byte and runs the volume hook
            // 0x590D10 (0x589699) when `edi` was raised — a volume table (`+0x5C`, 0x5895C8), a
            // release ramp under way (`+0x24 ≠ 0`, 0x5895EB) or an envelope step `+0x30 ≠ 0`
            // (0x589628, tested before the segment is reloaded) — whether or not the byte changed
            let edi = v.clip.tables.first().is_some_and(|t| t.volume.is_some()) || v.ramp.step != 0 || v.env.step != 0;
            let tables = v.clip.tables.first();
            if tables.is_some_and(|t| t.pitch.is_some()) {
                v.pitch_phase = v.pitch_phase.wrapping_add(1);
                if v.pitch_phase >= r.pitch_table_len {
                    v.pitch_phase = 0;
                }
            }
            if tables.is_some_and(|t| t.volume.is_some()) {
                v.volume_phase = v.volume_phase.wrapping_add(1);
                if v.volume_phase >= r.volume_table_len as u8 {
                    v.volume_phase = 0;
                }
            }
            // the release ramp (0x5895E1…0x589619): below 0 stops the handle, and the envelope
            // is not stepped
            if v.ramp.tick(&mut v.vol16) {
                stop.push(*id);
                continue;
            }
            let Some(t) = tables else {
                v.hook |= edi;
                continue;
            };
            if v.env.tick(&t.envelope) {
                stop.push(*id);
            } else {
                v.hook |= edi;
            }
        }
        if !stop.is_empty() {
            self.voices.retain(|(i, _)| !stop.contains(i));
        }
        self.scratch.stop = stop;
    }

    /// One block of 0x598760 (≤ [`BLOCK`] frames): each voice pulls its chain once — a one-shot
    /// whose decoder has nothing left fails the pull and is freed before it mixes anything
    /// ([`Voice::pull_fails`], 0x5987DC → 0x598670 → 0x589C00) — then the streams.
    fn mix_block(&mut self, out: &mut [[f32; 2]], mix_rate: u32) {
        let n = out.len();
        let bus_c = self.bus_c;
        let interp = self.interp;
        let mut failed = std::mem::take(&mut self.scratch.failed);
        failed.clear();
        for (k, (_, v)) in self.voices.iter_mut().enumerate() {
            if v.finished || v.held {
                continue;
            }
            // the clip is only read here: a borrow of the voice's own `Arc`, no clone per voice
            // per block
            let c: &Clip = &v.clip;
            let cc = c.channels.max(1) as usize;
            // the 16.16 step, 0x596B60 → 0x596A50 → 0x599830
            let step = resample::step_16_16(v.pitch_word(), c.rate, mix_rate);
            if v.pull_fails(step, n, interp) {
                failed.push(k);
                continue;
            }
            v.pulled = true;
            // the level byte (0x589DF0) · `[0x5C1788]` (f32 1/127) as an f32 (0x59697A)
            let vol = (v.level_byte() as f64 * (1.0f32 / 127.0) as f64) as f32;
            let pg = pan_gains(voice_pan(v.pan, v.patch_pan, c.pan_scale));
            // N10 (0x598721): a hook call since the last block raised the ramp flag, changed or not
            if std::mem::take(&mut v.hook) {
                v.gains.touch();
            }
            v.gains.set_vol(vol);
            v.gains.set([resample::channel_gain(vol, pg[0]), resample::channel_gain(vol, pg[1])]);
            // the send (0x599800, at the voice start and at every bus change: 0x596A10) and the
            // wet gain from the volume as it is now. 0x598700 raises the ramp flag on every call —
            // the hooks' calls are tracked in `v.hook` (service tick, setVolume, pan), so the ramp
            // (and the wet gain from it) also runs for a call that changed nothing
            let send = reverb_input(v.fx_scale, c.fx_send, bus_c);
            if v.send != Some(send) {
                v.send = Some(send);
                v.wet = wet_gain(v.gains.vol[0], send);
            }
            let ramp = v.gains.ramp(n);
            // 0x59C170's `fld dword [edi]` for the block
            if let Some(lp) = v.lowpass.as_mut() {
                lp.begin_block();
            }
            for (fi, frame) in out.iter_mut().enumerate() {
                let at = |sc: usize| {
                    resample::interpolate(interp, v.cur.frac, v.cur.pos as i64, |k| {
                        v.frame_of(k).map_or(0.0, |f| c.samples[f * cc + sc] as f32)
                    })
                };
                let x0 = at(0);
                let mut xs = [x0, if cc > 1 { at(1) } else { x0 }];
                // the strip's low-pass at the chain's head, each channel of the voice its own
                // (a mono voice's one channel feeds both sides)
                if let Some(lp) = v.lowpass.as_mut() {
                    xs[0] = lp.run(0, xs[0]);
                    xs[1] = if cc > 1 { lp.run(1, xs[1]) } else { xs[0] };
                }
                let ramping = ramp.filter(|r| fi < r.len);
                for (ch, o) in frame.iter_mut().enumerate() {
                    let g = match ramping {
                        Some(r) => v.gains.ramp_next(&r, ch),
                        // a channel at gain 0 is skipped (0x59881E)
                        None if v.gains.cur[ch] == 0.0 => continue,
                        None => v.gains.cur[ch],
                    };
                    *o = resample::mix_into(*o, xs[ch], g);
                }
                // the reverb send: the voice's pulled buffer (`[0x6B1758]`, the one every channel
                // mixes from) at the wet gain into the reverb's input (0x598A90); in the ramp the
                // volume steps first, in the last channel's pass (0x5981BF…0x5981CF)
                if let Some(r) = ramping {
                    let vc = v.gains.ramp_vol_next(&r);
                    v.wet = wet_gain(vc, send);
                }
                if v.wet != 0.0 {
                    self.tail.get_or_insert_with(|| Box::new(Reverb::new(mix_rate))).send(fi, xs[0], v.wet);
                }
                if ramp.is_some_and(|r| fi + 1 == r.len) {
                    v.gains.ramp_done();
                }
                v.cur.advance(step);
            }
            // 0x59C170's `fstp dword [edi]`
            if let Some(lp) = v.lowpass.as_mut() {
                lp.end_block();
            }
        }
        // 0x598760 frees each failing strip as it comes to it (0x598670 → 0x589C00)
        for &k in &failed {
            self.free(k);
        }
        self.scratch.failed = failed;
        self.voices.retain(|(_, v)| !v.finished);
        // the reverb (0x598760 → [0x6B16B8] = 0x598AE0), after the voices and before the streams
        if let Some(rv) = self.tail.as_mut() {
            rv.process(out);
        }
        // streamed sources: a stream is a voice of the library too (0x5612A0 → 0x5898F0), so it
        // takes the same resampler, at the pitch word 4096: 0x5612A0 sets `voice+0x68` (cents)
        // from the descriptor's word `+0`, which 0x561900 clears and the stream header parser
        // 0x590F70 never writes, `voice+0x6C` from the request's `+0xC`, 0x561960's 0x1000 (the
        // music start 0x4AE087…0x4AE0A9 writes only `+8` and `+0xA`, the movie's 0x4E213B nothing
        // there), and the bend `+0x3F` from the request's 0x40 (0x561429…0x561458 → 0x589F40);
        // the library's `setPitch` 0x565F80 is called only by `Voice_play`'s update (0x4A9FA3) and
        // the engine mixer (0x4AC608, 0x4AC6F9), never on a stream. *(Until 2026-09-27 an
        // approximation "the stream player's own pitch call is not read".)* `window` holds
        // `s[P−1] … s[P+2]` of each channel
        for (_, sv) in self.streams.iter_mut() {
            let cc = sv.src.channels.max(1) as usize;
            let step = resample::step_16_16(4096, sv.src.rate, mix_rate);
            let q = &sv.src.queue;
            // the next source frame, zeros when the feeder is behind
            let pop = |q: &SampleRing| -> [f32; 2] { q.pop_frame(cc).unwrap_or([0.0, 0.0]) };
            if !sv.primed {
                sv.primed = true;
                for j in 1..4 {
                    let f = pop(q);
                    for c in 0..2 {
                        sv.window[c][j] = f[c];
                    }
                }
            }
            for frame in out.iter_mut() {
                for (ch, o) in frame.iter_mut().enumerate() {
                    let w = sv.window[ch];
                    let x = resample::interpolate(interp, sv.frac, 0, |k| w[(k + 1) as usize]);
                    *o = resample::mix_into(*o, x, sv.volume);
                }
                let f = sv.frac + step;
                sv.frac = f & 0xFFFF;
                for _ in 0..(f >> 16) {
                    let nf = pop(q);
                    for c in 0..2 {
                        sv.window[c].rotate_left(1);
                        sv.window[c][3] = nf[c];
                    }
                }
            }
        }
    }

    /// [`AudioOutput::silence_voices`]: only a voice tagged `table` — the pause's own walk never
    /// reaches the others.
    fn silence(&mut self) {
        for (_, v) in self.voices.iter_mut() {
            if !v.table {
                continue;
            }
            if v.ramp.step != 0 && v.ramp.goal < 0 {
                continue;
            }
            v.set_volume(0.0);
        }
    }

    /// [`AudioOutput::ramp`].
    fn ramp(&mut self, id: VoiceId, ticks: i32, target: i32) {
        for (_, v) in self.voices.iter_mut().filter(|(i, v)| *i == id && !v.finished && !v.held) {
            v.ramp = Ramp::start(v.vol16, ticks, target);
        }
    }

    /// [`AudioOutput::play_word`]'s start (0x5907B0). `tabled` is `false` only for the callers
    /// listed on the `table` field of [`Voice`] — a bare `Voice_start` 0x565E70 in the original,
    /// never `Voice_play` 0x4A9C10's voice-numbered table.
    #[cfg(test)]
    fn start_patch(&mut self, clip: Arc<Clip>, volume: f32, word: u16, looping: bool, sel: u8, tabled: bool) -> VoiceId {
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.start_patch_as(id, clip, volume, word, looping, sel, tabled);
        id
    }

    /// [`Shared::start_patch`] with the handle the game thread gave the start ([`AudioOutput`]
    /// numbers them in call order and queues the start, [`Cmd::Start`]).
    #[allow(clippy::too_many_arguments)]
    fn start_patch_as(&mut self, id: VoiceId, clip: Arc<Clip>, volume: f32, word: u16, looping: bool, sel: u8, tabled: bool) -> VoiceId {
        let g = self;
        if clip.records.is_empty() {
            // not a bank patch: one voice, no draw, the clip's own values scaled by `sel`
            if g.allocate(clip.channels.max(1) as usize, clip.priority, Some(id)) {
                let mut v = Voice::new(clip.clone(), word, volume, looping, g.service_tick);
                v.level = ((clip.gain * 127.0).round() as i32 * sel as i8 as i32 / 127) as i8;
                v.table = tabled;
                g.voices.push((id, v));
            }
            return id;
        }
        let patch_cents = crate::librand::patch_draw(&mut g.rand, &clip.records);
        let mut started = 0;
        for (i, r) in clip.records.iter().enumerate() {
            if !r.matches(sel as i8, crate::librand::REQUEST_NOTE) {
                continue;
            }
            let pan = crate::librand::record_pan(&mut g.rand, r);
            let layer = clip.layer(i);
            if !g.allocate(layer.channels.max(1) as usize, layer.priority, Some(id)) {
                // 0x5908F1 → 0x5909A6: every handle started so far is stopped again
                g.voices.retain(|(j, _)| *j != id);
                tracing::trace!(record = i, sel, "patch start refused");
                return id;
            }
            let d = crate::librand::record_draws(&mut g.rand, r, sel as i8, patch_cents, pan);
            let mut v = Voice::new(layer, word, volume, looping, g.service_tick);
            v.patch_pan = d.pan;
            v.level = d.level as i8;
            v.cents = base_cents(r).wrapping_add(d.cents as i16);
            (v.volume_phase, v.pitch_phase) = (d.volume_phase, d.pitch_phase);
            v.table = tabled;
            tracing::trace!(record = i, ?d, sel, "patch start");
            g.voices.push((id, v));
            started += 1;
        }
        if started > 1 {
            // 0x590A5C: the last one still standing is the anchor
            if let Some((_, v)) = g.voices.iter_mut().rev().find(|(j, _)| *j == id) {
                v.anchor = true;
            }
        }
        id
    }

    fn budget(&self) -> usize {
        self.budget.unwrap_or(24)
    }

    /// **0x589C00**, a voice freed when its sample has ended (the failed pull's callback,
    /// 0x58EC19): a voice outside a group (`voice+0x1E` 0, every one-record start) is freed.
    /// In a group — the voices of one patch start of several records (0x590A04…0x590A5C) — the
    /// live members are counted (0x589C39…0x589C79) and the anchor found (`voice+0x1B`):
    /// the anchor ending while others play is **held** (state 2, 0x589CF8) instead of freed; the
    /// second-last member ending beside a held anchor frees both (0x589CB6…0x589CE6); any other
    /// member is freed. So the group's handle — the anchor's — stays good, and its voice stays
    /// in the budget, until every record's sample has ended. Marks the freed voices `finished`.
    fn free(&mut self, k: usize) {
        let id = self.voices[k].0;
        // the group's live members: their count and the first anchor among them
        let live = |j: &usize| self.voices[*j].0 == id && !self.voices[*j].1.finished;
        let members = (0..self.voices.len()).filter(live).count();
        let anchor = (0..self.voices.len()).filter(live).find(|&j| self.voices[j].1.anchor);
        match anchor {
            Some(a) if members > 1 => {
                if self.voices[a].1.held && a != k && members == 2 {
                    self.voices[k].1.finished = true;
                    self.voices[a].1.finished = true;
                } else if !self.voices[a].1.held && a == k {
                    self.voices[a].1.held = true;
                } else {
                    self.voices[k].1.finished = true;
                }
            }
            _ => self.voices[k].1.finished = true,
        }
    }

    /// 0x5898F0 for a start needing `need` voices: `true` with the victims stopped, `false` for the
    /// refusal (−9) with nothing touched. A victim playing is stopped by its handle (0x589B26,
    /// 0x565D60), which stops its whole group; a held anchor (state 2) is taken without a stop
    /// (0x589B09 `cmp cl, 1 ; jne`), and so is a voice of the start in progress, whose group is not
    /// made yet (`voice+0x1E` is set after every record has started, 0x590A04).
    fn allocate(&mut self, need: usize, priority: u8, starting: Option<VoiceId>) -> bool {
        // the lists are the callback's reusable buffers ([`Scratch`])
        let sc = &mut self.scratch;
        let (mut live_ids, mut live_held, mut live, mut live_at) =
            (std::mem::take(&mut sc.live_ids), std::mem::take(&mut sc.live_held), std::mem::take(&mut sc.live), std::mem::take(&mut sc.live_at));
        let (mut order, mut victims) = (std::mem::take(&mut sc.order), std::mem::take(&mut sc.victims));
        live_ids.clear();
        live_held.clear();
        live.clear();
        live_at.clear();
        for (j, (i, v)) in self.voices.iter().enumerate().filter(|(_, (_, v))| !v.finished) {
            live_ids.push(*i);
            live_held.push(v.held);
            live.push((v.clip.priority, v.clip.channels, v.stamp));
            live_at.push(j);
        }
        live.extend(self.streams.iter().map(|(id, sv)| (STREAM_PRIORITY, sv.src.channels, id.0)));
        let served = pick_victims_into(&live, self.budget(), need, &mut order, &mut victims);
        let ok = served && {
            if !victims.is_empty() {
                // streams sit after the voices in `live` and are never victims; `live` lists the
                // voices in order, so a victim's index is its place among the unfinished voices
                self.steal(&victims, &live_ids, &live_held, &live_at, need, priority, starting);
            }
            let ch = |k: usize| live[k].1.max(1) as usize;
            let in_use = (0..live.len()).map(ch).sum::<usize>() - victims.iter().map(|&k| ch(k)).sum::<usize>() + need;
            if in_use > self.peak {
                self.peak = in_use;
                tracing::debug!(voices = in_use, budget = self.budget(), "voices in use: a new peak");
            }
            true
        };
        if !served {
            tracing::debug!(need, priority, budget = self.budget(), "voice start refused: no voice free or below 0x65");
        }
        let sc = &mut self.scratch;
        (sc.live_ids, sc.live_held, sc.live, sc.live_at, sc.order, sc.victims) = (live_ids, live_held, live, live_at, order, victims);
        ok
    }

    /// The victims of [`Shared::allocate`] stopped (0x589B26): see there.
    #[allow(clippy::too_many_arguments)]
    fn steal(&mut self, victims: &[usize], live_ids: &[VoiceId], live_held: &[bool], live_at: &[usize], need: usize, priority: u8, starting: Option<VoiceId>) {
        tracing::debug!(need, priority, stolen = victims.len(), "voice budget full: the lowest priority, oldest voice stolen");
        for &k in victims {
            let one = live_held[k] || Some(live_ids[k]) == starting;
            for j in 0..self.voices.len() {
                if (one && j == live_at[k]) || (!one && self.voices[j].0 == live_ids[k]) {
                    self.voices[j].1.finished = true;
                }
            }
        }
        self.voices.retain(|(_, v)| !v.finished);
    }
}

/// The reverb entries of the driver's default table (0x5E25B0, the only one the game selects:
/// 0x4AB470 → 0x565C70 with mode 0x28 → 0x5901B0 takes `0x5E25B0` for any mode but 0 and 1, and
/// mode 1's table `[0x6B4A10]` is never given): the three group counts `+4…+6` are 4 / 0 / 0, so
/// the network is four all-pass filters in series on one buffer (0x598AE0's group 0; the groups 1
/// and 2 with their extra buffers are empty), the gain byte `+7` is 0xFA, and the entries from `+8`
/// are `(delay word BE, coefficient c, damping d)`, delay in ms.
const REVERB_ENTRIES: [(i32, i32, i32); 4] = [(100, 0x40, 0), (70, 0x40, 95), (49, 0x4C, 0), (34, 0x4C, 95)];
const REVERB_GAIN_BYTE: i32 = 0xFA;

/// The smallest prime not below `n` — 0x598CD0's search (the smallest divisor ≥ 2 of `n` equals
/// `n`, else `n + 1`, and again).
fn next_prime(mut n: i64) -> i64 {
    loop {
        let mut d = 2;
        if n >= 2 {
            while d <= n && n % d != 0 {
                d += 1;
            }
        }
        if d == n {
            return n;
        }
        n += 1;
    }
}

/// One all-pass with a damped feedback, 0x598D10's state (0x24 bytes) and 0x59BD20's loop.
struct AllPass {
    buf: Vec<f32>,
    /// `+8`, the delay in samples
    len: usize,
    /// `+0` and `+4`, the write and the read counter, both counting down through the buffer: they
    /// start at 0 and `len` and the first pass resets the write one to `+8` (0x598B1E), so they
    /// stay equal — one counter
    left: usize,
    /// `+0x10` / `+0x14`, `c` and `−c`
    c: f32,
    nc: f32,
    /// `+0x1C` / `+0x20`, the feedback's damping and its complement
    g1: f32,
    g2: f32,
    /// `+0x18`, the damping's state
    state: f32,
}

impl AllPass {
    /// 0x598D10 for one entry. The x87 intermediates are f64 (`career.md` §11.4c: the control word
    /// read 0x027F, 53-bit precision).
    fn new(rate: u32, (ms, c, d): (i32, i32, i32)) -> Self {
        // 0x598D62…0x598DBB: `d · [0x5B4A78]` (an f32 1/127) stored, `1 − ` it (the unrounded
        // product), each times `(byte+7 + 100)` and `[0x5B73F4]` (an f32 1/350), stored
        let x = d as f64 * (1.0f32 / 127.0) as f64;
        let g1t = x as f32;
        let g2t = (1.0 - x) as f32;
        let k = f32::from_bits(0x3B3B_3EE7) as f64;
        let g1 = ((REVERB_GAIN_BYTE + 100) as f64 * g1t as f64 * k) as f32;
        let g2 = ((REVERB_GAIN_BYTE + 100) as f64 * g2t as f64 * k) as f32;
        let cf = (c as f64 * (1.0f32 / 127.0) as f64) as f32;
        // 0x598DD3…0x598DED: the delay word `· [0x5B3060]` (an f32 0.001) stored, `rate ·` it
        // truncated (0x5A0F98), then the next prime (0x598CD0)
        let secs = (ms as f64 * f32::from_bits(0x3A83_126F) as f64) as f32;
        let len = next_prime((rate as f64 * secs as f64).trunc() as i64) as usize;
        Self { buf: vec![0.0; len], len, left: 0, c: cf, nc: -cf, g1, g2, state: 0.0 }
    }

    /// 0x598AE0's chunk loop over `x`, in place, and 0x59BD20's per-sample body.
    fn run(&mut self, x: &mut [f32]) {
        let mut i = 0;
        while i < x.len() {
            if self.left < 1 {
                self.left = self.len;
            }
            let m = (x.len() - i).min(self.left);
            for j in 0..m {
                let p = self.left - 1 - j;
                let xin = x[i + j] as f64;
                let a = xin * self.nc as f64;
                let b = self.state as f64 * self.g1 as f64;
                let y = a + self.buf[p] as f64;
                x[i + j] = y as f32;
                let z = y * self.c as f64 + xin;
                let w = (z * self.g2 as f64 + b) as f32;
                self.buf[p] = w;
                self.state = w;
            }
            self.left -= m;
            i += m;
        }
    }
}

/// The library's software reverb — the DSP the driver runs itself (0x5901B0 → 0x598D10 sets it up,
/// 0x598A90 takes each voice's send, 0x598AE0 runs it after the voices of a block). Every voice's
/// pulled block times its wet gain is summed into one input buffer (`0x6B0A80`, 0x598A90); once a
/// block, the four all-passes run over it in place, and the result is added — gain 1.0, the same
/// mono signal — to both output channels (0x598B9B…0x598BC8). It runs for the 2000 blocks after
/// the last send (`[0x6B1680]`, set to 2000 at the setup 0x598A70, reset to 0 by a send, +1 per
/// block before the test, 0x598AE5 / 0x598AF2), so the tail rings out however the buses move.
struct Reverb {
    aps: [AllPass; 4],
    input: [f32; BLOCK],
    quiet: u32,
}

impl Reverb {
    fn new(rate: u32) -> Self {
        Self { aps: REVERB_ENTRIES.map(|e| AllPass::new(rate, e)), input: [0.0; BLOCK], quiet: 2000 }
    }

    /// 0x598A90 for one frame of a voice: `input[at] += x · wet`.
    fn send(&mut self, at: usize, x: f32, wet: f32) {
        self.input[at] = resample::mix_into(self.input[at], x, wet);
        self.quiet = 0;
    }

    /// 0x598AE0 for a block of `out.len()` frames.
    fn process(&mut self, out: &mut [[f32; 2]]) {
        let n = out.len();
        self.quiet += 1;
        if self.quiet >= 2000 {
            return;
        }
        for ap in self.aps.iter_mut() {
            ap.run(&mut self.input[..n]);
        }
        for (frame, r) in out.iter_mut().zip(self.input.iter()) {
            for o in frame.iter_mut() {
                *o = resample::mix_into(*o, *r, 1.0);
            }
        }
        self.input[..n].fill(0.0);
    }
}

/// What the game thread asks of the mixer — one command per [`AudioOutput`] call that changes
/// something, queued ([`rt::channel`]) and applied by the device callback ([`Core::drain`]) at the
/// top of its next run, in call order. That is where the mutex used to put them: a call took the
/// lock between two callbacks, so it counted from the next one's first block — and the library's
/// generator ([`Shared::rand`]) is still drawn by the start, on the callback's side, in call order.
enum Cmd {
    Quality { budget: usize, rate: u32, interp: Interp, sleep: u32, reverb: Option<Box<Reverb>> },
    FxAux { id: VoiceId, aux: u8 },
    Silence,
    Pan { id: VoiceId, pan: u16 },
    Start { id: VoiceId, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool, sel: u8, tabled: bool },
    SetWord { id: VoiceId, volume: f32, word: u16, bend: u8 },
    Ramp { id: VoiceId, ticks: i32, target: i32 },
    Filter { id: VoiceId, value: i32 },
    Stop(VoiceId),
    Env { p20: i32, p80: i32 },
    Fade { id: VoiceId, ms: i32, target: i32 },
    Stream { id: VoiceId, src: Arc<StreamSource>, volume: f32 },
}

/// A command queued and not yet seen applied (its sequence number is above the snapshot's
/// `applied`): what a status call on the game thread must already know of it.
#[derive(Clone, Copy)]
enum Pend {
    Start(VoiceId),
    Stream(VoiceId, f32),
    Stop(VoiceId),
}

impl Pend {
    fn id(self) -> VoiceId {
        match self {
            Pend::Start(i) | Pend::Stream(i, _) | Pend::Stop(i) => i,
        }
    }
}

/// One voice as the status calls read it ([`AudioOutput::progress`], [`AudioOutput::is_playing`]).
#[derive(Clone, Copy)]
struct VoiceStat {
    id: VoiceId,
    anchor: bool,
    held: bool,
    finished: bool,
    /// [`Voice::position`]
    position: f64,
    /// the clip's frames
    frames: usize,
    rate: u32,
}

/// The mixer's state as the callback last published it ([`Core::publish`]) — what the game
/// thread's status calls read instead of the voices themselves.
#[derive(Default)]
struct Snapshot {
    /// the commands applied when it was taken
    applied: u64,
    voices: Vec<VoiceStat>,
    /// `(handle, volume)` of the playing streams
    streams: Vec<(VoiceId, f32)>,
}

impl Snapshot {
    /// The voice a status call answers for — **settled 2026-09-27** (read to the instruction:
    /// 0x566910 whole, 0x590e20, 0x566d60): the call takes a handle and, via 0x590e20, searches
    /// the group's own small array for the entry whose *own* handle matches — never a sibling's —
    /// answering "ended" (state 3, `mov [ebp], 3` at 0x566a1e) only once that entry is gone from
    /// the array. The game only ever holds the handle 0x590420 answered for a start — the anchor's
    /// (or the one voice of a solo patch) — so the call always reads the **anchor's own** entry
    /// while the group has any; there is no reading "the first playing member" or "the lowest
    /// slot" instead, whatever this note's own preliminary framing said. Found, the entry's own
    /// remaining-frames field decides 0x566986's `jge`: negative — the held anchor, its sample
    /// already having run out while the array keeps its bookkeeping ([`Voice::held`], the group
    /// note) — answers **state 0** with every field left the zero the call started with
    /// (0x566921…0x56692a): position 0, not the anchor's real (finished) cursor; non-negative
    /// answers state 1 or 2 (0x566999's `[handle] == the found entry`, an array-index test with no
    /// bearing on which data is read) from the anchor's own fields either way. The port's `held`
    /// on the anchor already matches "kept, not freed, no strip" ([`Shared::free`]); this settles
    /// what a status call answers for it: 0, not a sibling's position and not `None`.
    fn status_voice(&self, id: VoiceId) -> Option<&VoiceStat> {
        let mut group = self.voices.iter().filter(|v| v.id == id);
        let first = group.clone().next();
        let v = group.find(|v| v.anchor).or(first)?;
        (!v.finished).then_some(v)
    }
}

/// What the last callback left queued for the device, in output frames (its delay to the playback
/// of the buffer plus the buffer), and when it ran (ns since `epoch`, plus 1; 0 before the first
/// callback) — for [`AudioOutput::latency_ms`].
struct Latency {
    epoch: std::time::Instant,
    frames: AtomicU64,
    at: AtomicU64,
}

/// The game thread's side: the command queue's producer and the bookkeeping the calls need
/// without asking the callback.
struct TxState {
    prod: Producer<Cmd>,
    /// what the callback gave back to be freed here (the reverb a rate change replaced)
    gc: Consumer<Box<Reverb>>,
    /// the next handle ([`VoiceId`]), numbered in call order as the library's start does
    next_id: u32,
    /// the commands queued so far
    seq: u64,
    /// the mix rate the queued commands leave the mixer at ([`Shared::mix_rate`]'s mirror)
    cfg_rate: Option<u32>,
    pending: Vec<(u64, Pend)>,
}

struct Tx {
    state: Mutex<TxState>,
    snap: Arc<Mutex<Snapshot>>,
    lat: Arc<Latency>,
    /// the buses `set_env` left, `[0x6B4A0C]` / `[0x6B4A0D]`
    bus: [AtomicI8; 2],
    /// TEST GLUE: `NFS_AUDIO_DUMP` — no device, the mixer runs on the calling thread
    offline: Option<Mutex<Offline>>,
}

/// The device callback's side: the mixer itself, which nothing else touches.
struct Core {
    shared: Shared,
    rx: Consumer<Cmd>,
    gc: Producer<Box<Reverb>>,
    applied: u64,
    snap: Arc<Mutex<Snapshot>>,
    lat: Arc<Latency>,
    device_rate: u32,
}

/// The command queue's size, in commands: a game frame sends a few dozen.
const QUEUE: usize = 4096;
/// What a push waits for a full queue (a callback that has stopped) before the command is dropped.
const PUSH_WAIT: std::time::Duration = std::time::Duration::from_millis(50);
/// The callback's buffers, made before it runs ([`Shared::reserve_rt`]).
const RT_VOICES: usize = 512;
const RT_FRAMES: usize = 1 << 15;

impl Shared {
    /// The state the status calls read, into `s` (whose buffers are reused).
    fn snapshot_into(&self, s: &mut Snapshot) {
        s.voices.clear();
        s.voices.extend(self.voices.iter().map(|(id, v)| VoiceStat {
            id: *id,
            anchor: v.anchor,
            held: v.held,
            finished: v.finished,
            position: v.position(),
            frames: v.clip.samples.len() / v.clip.channels.max(1) as usize,
            rate: v.clip.rate,
        }));
        s.streams.clear();
        s.streams.extend(self.streams.iter().map(|(id, sv)| (*id, sv.volume)));
    }

    /// Make the buffers the callback uses, so that it allocates nothing.
    fn reserve_rt(&mut self) {
        self.voices.reserve(RT_VOICES);
        self.streams.reserve(32);
        self.bridge.pending.reserve(RT_FRAMES);
        let sc = &mut self.scratch;
        sc.mix.reserve(RT_FRAMES);
        sc.stopped.reserve(32);
        sc.stop.reserve(RT_VOICES);
        sc.failed.reserve(RT_VOICES);
        sc.live.reserve(RT_VOICES + 32);
        sc.live_ids.reserve(RT_VOICES);
        sc.live_held.reserve(RT_VOICES);
        sc.live_at.reserve(RT_VOICES);
        sc.order.reserve(RT_VOICES);
        sc.victims.reserve(RT_VOICES);
        // the reverb the first send would make (at the default mix rate, before any quality is
        // given): a fresh one holds nothing and only runs once something is sent to it
        let rate = self.mix_rate();
        self.tail = Some(Box::new(Reverb::new(rate)));
    }
}

impl Core {
    fn apply(&mut self, cmd: Cmd) {
        let g = &mut self.shared;
        match cmd {
            Cmd::Quality { budget, rate, interp, sleep, reverb } => {
                g.sleep_ms = Some(sleep);
                if g.interp != interp {
                    g.interp = interp;
                    tracing::info!(?interp, "mix interpolator");
                }
                if g.budget != Some(budget) {
                    g.budget = Some(budget);
                    tracing::info!(budget, "voice budget");
                }
                if g.mix_rate != Some(rate) {
                    g.mix_rate = Some(rate);
                    // the reverb's delay lines are sized in frames of the mix rate
                    if let Some(old) = std::mem::replace(&mut g.tail, reverb) {
                        // freed on the game thread; a full return queue frees it here
                        let _ = self.gc.push(old);
                    }
                    tracing::info!(mix_rate = rate, device_rate = self.device_rate, "mix rate");
                }
            }
            Cmd::FxAux { id, aux } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.fx_scale = 0x7F - aux.min(0x7F);
                }
            }
            Cmd::Silence => g.silence(),
            Cmd::Pan { id, pan } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.pan = pan_byte(pan);
                    // 0x5661F0 stores the angle and calls 0x590CC0 unconditionally (no compare)
                    v.hook = true;
                }
            }
            Cmd::Start { id, clip, volume, word, bend, pan, looping, sel, tabled } => {
                g.start_patch_as(id, clip, volume, word, looping, sel, tabled);
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.bend = bend as i8;
                    v.pan = pan_byte(pan);
                }
            }
            Cmd::SetWord { id, volume, word, bend } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.set_volume(volume);
                    v.word = word;
                    v.bend = bend as i8;
                }
            }
            Cmd::Ramp { id, ticks, target } => g.ramp(id, ticks, target),
            Cmd::Filter { id, value } => {
                let rate = g.mix_rate();
                for (_, v) in g.voices.iter_mut().filter(|(i, v)| *i == id && !v.finished && !v.held) {
                    LowPass::set(&mut v.lowpass, value, rate);
                }
            }
            Cmd::Stop(id) => {
                g.voices.retain(|(i, _)| *i != id);
                g.streams.retain(|(i, _)| *i != id);
            }
            Cmd::Env { p20, p80 } => {
                g.bus_c = p20 as i8;
                g.bus_d = p80 as i8;
            }
            Cmd::Fade { id, ms, target } => {
                if let Some((_, sv)) = g.streams.iter_mut().find(|(i, _)| *i == id) {
                    sv.fade = Some(Fade::new(sv.volume, ms, target));
                }
            }
            Cmd::Stream { id, src, volume } => {
                if g.allocate(src.channels.max(1) as usize, STREAM_PRIORITY, None) {
                    g.streams.push((id, StreamVoice { src, volume, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: None }));
                }
            }
        }
        self.applied += 1;
    }

    /// Everything queued so far, in order.
    fn drain(&mut self) {
        while let Some(cmd) = self.rx.pop() {
            self.apply(cmd);
        }
    }

    /// Hand the game thread the state its status calls read. Never waits: if the game thread is
    /// reading the last one at this instant, this one is skipped and the next callback's goes.
    fn publish(&mut self) {
        let Ok(mut s) = self.snap.try_lock() else { return };
        s.applied = self.applied;
        self.shared.snapshot_into(&mut s);
    }

    /// The device callback's run for `out` (interleaved, `channels` wide) at `out_rate`: the mix at
    /// the library's rate `[0x6B48DA]` into a buffer of that rate, and from there to the device's
    /// ([`Bridge`]).
    fn fill(&mut self, out: &mut [f32], channels: usize, out_rate: f64) {
        let g = &mut self.shared;
        let mix_rate = g.mix_rate();
        let step = mix_rate as f64 / out_rate;
        let n = out.len() / channels.max(1);
        let mut bridge = std::mem::take(&mut g.bridge);
        for (k, frame) in out.chunks_exact_mut(channels.max(1)).enumerate() {
            let lr = bridge.next(step, n - k, |_, pending| g.pump_to(mix_rate, pending));
            // a mono device takes the left channel, a third and later one the right
            for (ch, o) in frame.iter_mut().enumerate() {
                *o = lr[ch.min(1)];
            }
        }
        g.bridge = bridge;
    }
}

/// TEST GLUE (`NFS_AUDIO_DUMP=<file>`): no device. The mixer runs on the calling thread: every
/// call that changes something is applied at once, and every 4th renders 441 frames at 44100 Hz
/// stereo, appended as f32 LE to the file — a deterministic function of the game's calls, to
/// compare the mix of one build against another (`RE/notes/PERF.md`, F5).
struct Offline {
    core: Core,
    n: u32,
}

static DUMP: std::sync::OnceLock<Mutex<std::fs::File>> = std::sync::OnceLock::new();

impl Offline {
    fn step(&mut self) {
        self.core.drain();
        self.n += 1;
        if self.n.is_multiple_of(4) {
            let mut buf = vec![0.0f32; 441 * 2];
            self.core.fill(&mut buf, 2, 44100.0);
            let bytes: Vec<u8> = buf.iter().flat_map(|x| x.to_le_bytes()).collect();
            use std::io::Write;
            // TEST GLUE: a muted run (`AudioOutput::new`, no `NFS_AUDIO_DUMP`) mixes the same way and writes nothing
            if let Some(dump) = DUMP.get() {
                dump.lock().unwrap().write_all(&bytes).unwrap();
            }
        }
        self.core.publish();
    }
}

impl Tx {
    /// The two halves of a mixer. With `offline` the callback's half is the glue's, and none is
    /// handed back.
    fn new(device_rate: u32, offline: bool) -> (Arc<Tx>, Option<Core>) {
        let (prod, rx) = rt::channel::<Cmd>(QUEUE);
        let (gc_tx, gc) = rt::channel::<Box<Reverb>>(8);
        let snap = Arc::new(Mutex::new(Snapshot { applied: 0, voices: Vec::with_capacity(RT_VOICES), streams: Vec::with_capacity(32) }));
        let lat = Arc::new(Latency { epoch: std::time::Instant::now(), frames: AtomicU64::new(0), at: AtomicU64::new(0) });
        let mut shared = Shared::default();
        shared.reserve_rt();
        let core = Core { shared, rx, gc: gc_tx, applied: 0, snap: snap.clone(), lat: lat.clone(), device_rate };
        let state = Mutex::new(TxState { prod, gc, next_id: 0, seq: 0, cfg_rate: None, pending: Vec::with_capacity(256) });
        let mut tx = Tx { state, snap, lat, bus: [AtomicI8::new(0), AtomicI8::new(0)], offline: None };
        if offline {
            tx.offline = Some(Mutex::new(Offline { core, n: 0 }));
            return (Arc::new(tx), None);
        }
        (Arc::new(tx), Some(core))
    }

    /// Queue `cmd` (and what a status call must know of it until it is applied).
    fn push(&self, st: &mut TxState, cmd: Cmd, pend: Option<Pend>) {
        // what the callback gave back is freed here, off its thread
        while let Some(old) = st.gc.pop() {
            drop(old);
        }
        let mut cmd = cmd;
        let t0 = std::time::Instant::now();
        loop {
            match st.prod.push(cmd) {
                Ok(()) => break,
                Err(c) => {
                    if t0.elapsed() > PUSH_WAIT {
                        tracing::warn!("audio command queue full: command dropped");
                        return;
                    }
                    cmd = c;
                    std::thread::yield_now();
                }
            }
        }
        st.seq += 1;
        if let Some(p) = pend {
            if st.pending.len() >= QUEUE {
                st.pending.remove(0);
            }
            st.pending.push((st.seq, p));
        }
        if let Some(o) = &self.offline {
            o.lock().unwrap().step();
        }
    }

    /// Queue `cmd` for a call that has no handle to give back.
    fn send(&self, cmd: Cmd) {
        let mut st = self.state.lock().unwrap();
        self.push(&mut st, cmd, None);
    }

    fn stop(&self, id: VoiceId) {
        let mut st = self.state.lock().unwrap();
        self.push(&mut st, Cmd::Stop(id), Some(Pend::Stop(id)));
    }

    /// The last queued, not yet applied command about `id`, if any — it is newer than the
    /// snapshot, which is returned locked with it.
    fn overlay(&self, id: VoiceId) -> (Option<Pend>, std::sync::MutexGuard<'_, Snapshot>) {
        let mut st = self.state.lock().unwrap();
        let snap = self.snap.lock().unwrap();
        st.pending.retain(|(seq, _)| *seq > snap.applied);
        let last = st.pending.iter().rev().map(|(_, p)| *p).find(|p| p.id() == id);
        (last, snap)
    }
}

/// The device buffer the stream asks for, in frames: `NFS_AUDIO_BUFFER` (0 = the device's own
/// default), else 256 on macOS (CoreAudio's default is 512), clamped to what the device supports;
/// other systems keep their default buffer.
// PORT-DEVIATION D2 (RE/notes/PORT-DEVIATIONS.md): a fixed 256-frame device buffer on macOS; the original's sound layer opens at 0x4A66B0 with 0x565720([0x655D44], 0x40000, 3).
fn device_buffer_frames(supported: &cpal::SupportedStreamConfig) -> Option<u32> {
    let asked = std::env::var("NFS_AUDIO_BUFFER").ok().and_then(|v| v.parse::<u32>().ok());
    let n = asked.unwrap_or(if cfg!(target_os = "macos") { 256 } else { 0 });
    match (n, supported.buffer_size()) {
        (0, _) => None,
        (n, cpal::SupportedBufferSize::Range { min, max }) => Some(n.clamp(*min, *max)),
        (_, cpal::SupportedBufferSize::Unknown) => None,
    }
}

pub struct AudioOutput {
    _stream: Option<cpal::Stream>,
    tx: Arc<Tx>,
    pub rate: u32,
}

impl AudioOutput {
    pub fn new() -> anyhow::Result<Self> {
        if let Ok(path) = std::env::var("NFS_AUDIO_DUMP") {
            DUMP.get_or_init(|| Mutex::new(std::fs::File::create(&path).expect("NFS_AUDIO_DUMP")));
            let (tx, _) = Tx::new(44100, true);
            tracing::warn!(path, "audio: offline dump (NFS_AUDIO_DUMP), no device");
            return Ok(Self { _stream: None, tx, rate: 44100 });
        }
        // TEST GLUE: scripted / test runs are silent — `NFS_MUTE=1`, or any `NFS_NO_FOCUS` run (the scripted
        // runs' marker) unless `NFS_SOUND=1` asks for the device (the audio tasks). No device is opened: the
        // mixer runs offline on the calling thread exactly as under `NFS_AUDIO_DUMP`, only nothing is written,
        // so the game's calls into the mixer are the same as with a device.
        let var = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
        if !var("NFS_SOUND") && (var("NFS_MUTE") || var("NFS_NO_FOCUS")) {
            let (tx, _) = Tx::new(44100, true);
            tracing::info!("audio: muted test run (NFS_MUTE / NFS_NO_FOCUS; NFS_SOUND=1 for the device), no device");
            return Ok(Self { _stream: None, tx, rate: 44100 });
        }
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
        let supported = device.default_output_config()?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        // a smaller device buffer lowers the output latency and changes nothing in the mix: the
        // driver passes ([`Pump`]) and the bridge's queue run on the stream's own clock, whatever
        // size the callback asks for (`the_mix_does_not_depend_on_the_callback_size`)
        let mut config: cpal::StreamConfig = supported.config();
        let mut attempts = vec![cpal::BufferSize::Default];
        if let Some(n) = device_buffer_frames(&supported) {
            attempts.insert(0, cpal::BufferSize::Fixed(n));
        }
        let mut last = None;
        for size in attempts {
            config.buffer_size = size;
            let (tx, core) = Tx::new(rate, false);
            let core = core.expect("a device mixer has its callback half");
            match Self::open(&device, &config, channels, core) {
                Ok(stream) => {
                    stream.play()?;
                    tracing::info!(rate, channels, buffer = ?config.buffer_size, "audio output started");
                    return Ok(Self { _stream: Some(stream), tx, rate });
                }
                Err(e) => {
                    tracing::warn!(error = %e, buffer = ?config.buffer_size, "audio stream refused");
                    last = Some(e);
                }
            }
        }
        Err(last.expect("an attempt was made").into())
    }

    /// TEST GLUE: the offline mixer of a muted run (no device, nothing written), for the tests of
    /// the modules that drive it.
    #[cfg(test)]
    pub(crate) fn offline() -> Self {
        let (tx, _) = Tx::new(44100, true);
        Self { _stream: None, tx, rate: 44100 }
    }

    /// The output stream whose callback is `core`'s: what the game thread queued since the last
    /// run, then the mix — nothing here takes a lock or allocates.
    fn open(device: &cpal::Device, config: &cpal::StreamConfig, channels: usize, mut core: Core) -> Result<cpal::Stream, cpal::Error> {
        let out_rate = config.sample_rate as f64;
        device.build_output_stream(
            config.clone(),
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                core.drain();
                let ts = info.timestamp();
                // saturating to zero when the playback instant is not after the callback's (cpal 0.18)
                let delay = ts.playback.duration_since(ts.callback);
                let frames = (out.len() / channels.max(1)) as u64 + (delay.as_secs_f64() * out_rate).round() as u64;
                core.lat.frames.store(frames, Ordering::Relaxed);
                core.lat.at.store(core.lat.epoch.elapsed().as_nanos() as u64 + 1, Ordering::Release);
                core.fill(out, channels, out_rate);
                core.publish();
            },
            |e| tracing::error!(error = %e, "audio stream"),
            None,
        )
    }

    /// What 0x4A66B0 configures for `AUDIOQUALITY` when the sound opens (both start-ups 0x4A6840 /
    /// 0x4A6A00; the latter reopens on a quality change): the software voice count
    /// ([`voice_budget`]) and the mix rate ([`mix_rate`]). Voices already playing are left alone;
    /// later starts obey the new count, and the mix goes on at the new rate.
    pub fn set_audio_quality(&self, audio_quality: u8) {
        let budget = voice_budget(audio_quality);
        let rate = mix_rate(audio_quality);
        let interp = Interp::for_quality(audio_quality);
        let sleep = pump_sleep_ms(audio_quality);
        let mut st = self.tx.state.lock().unwrap();
        // a new mix rate drops the reverb (its delay lines are sized in frames of the mix rate)
        // and the next send makes a fresh one — made here, off the callback, and handed over
        let reverb = (st.cfg_rate != Some(rate)).then(|| Box::new(Reverb::new(rate)));
        st.cfg_rate = Some(rate);
        self.tx.push(&mut st, Cmd::Quality { budget, rate, interp, sleep, reverb }, None);
    }

    /// The aux byte of `Voice_play` for a collision one-shot: `voice+0x45 = 0x7F − aux`
    /// (0x4A9A5F…0x4A9A6C → 0x5906D7), so a loud hit sends less to the reverb.
    pub fn set_fx_aux(&self, id: VoiceId, aux: u8) {
        self.tx.send(Cmd::FxAux { id, aux });
    }

    /// `Voice_setVolume(h, 0)` **0x566340** over the voice table `0x656180…0x6563C8` (stride 8)
    /// — the race pause 0x4AB150 (0x4AB16A…0x4AB18A) and `I_SFX` (0x4AB4FB…0x4AB527) — and over
    /// the speech phrase's voices `[0x6573A8]` / `[0x6573AC]` (0x4AF900(0, 0), 0x4AF91D…0x4AF944).
    /// The voices keep running, silent: 0x5662B0 writes `+0x2C = 0` and zeroes the ramp step
    /// `+0x24` (0x566303…0x566306). The streams (music) are not in the table. **A released voice
    /// is not either**: every release forgets its handle (`0x656180[slot] = −1`, [`RELEASE_TICKS`]),
    /// so a voice on its way out ([`Ramp`] toward −1) runs its ramp to the stop, heard. The voices
    /// the audio update drives come back with its next `setVolume`; a one-shot, the speech phrase
    /// and a held horn stay silent to their end.
    ///
    /// **Settled 2026-09-27** (read to the instruction: 0x4AB150, 0x4A9C10 whole, 0x4A98E0 whole,
    /// 0x565E70/0x565EA0, 0x4AADB0, 0x4AAE30/0x4AAE50/0x4AAE70/0x4AAE90): the table's entry is 8
    /// bytes, `+0` the handle (or −1), `+4` the record/patch id last started in that slot, indexed
    /// by the **voice number** (`Voice_play`'s first argument, 0..0x7F) — not the bank slot. Only
    /// `Voice_play` 0x4A9C10 ever writes it, at 0x4a9de9 on a genuine (re)start, and only for a
    /// voice number other than 0x33: at 0x4a9cd4 `cmp ebx, 0x33 ; jne` sends 0x33 (the collision
    /// one-shot) to `Bank_playOneShot` too but reads the table back for its answer (0x4a9d0b)
    /// without writing it, since 0x33 is the one voice number every concurrent collision hit
    /// shares — writing it would make each hit steal the last one's handle. `Bank_playOneShot`
    /// 0x4A98E0 itself never touches the table (confirmed end to end); called straight from a
    /// caller with no voice number — the checkpoint chime 0x4AAE30, the wrong-way cue 0x4AAE50,
    /// the pylon-hit cue 0x4AAE70, the cop siren's start 0x4AAE90 — it is exactly as untracked as
    /// `Voice_start` 0x565E70's HUD (`fe_sound` 0x4AADB0) and front-end (`fesfx`) one-shots, which
    /// go through 0x565E70/0x565EA0's own voice pool and never call 0x4A9C10 or 0x4A98E0 at all.
    /// The port's `Voice` now carries this as `table` (see its field doc), set at every start and
    /// read by [`Shared::silence`], so the pause silences exactly the table's voices: the loops
    /// (engine layers, AI engines, road/wind/turbo, the squeal channels), the ambience and the
    /// player's horn, plus (folded in for the same effect as the speech's own 0x4AF900 stop) the
    /// announcer phrase — and leaves the HUD/front-end one-shots, the collision one-shots (wall,
    /// car-car, prop, bush, landing, crash) and the checkpoint chime / wrong-way cue / pylon-hit
    /// cue / cop siren sounding, as the original does. *(Until 2026-09-27 every voice, table or
    /// not, was silenced — a collision one-shot, a checkpoint chime or a front-end button click
    /// still ringing at the instant of the pause fell silent when it should not have.)*
    pub fn silence_voices(&self) {
        self.tx.send(Cmd::Silence);
    }

    /// A playing voice's pan from the caller's 16-bit angle: the pan byte ([`pan_byte`]) to
    /// `Voice_setPan8` 0x566100, which stores it in `voice+0x1F` (and does nothing when it is the
    /// same, 0x56616D).
    pub fn set_pan(&self, id: VoiceId, pan: u16) {
        self.tx.send(Cmd::Pan { id, pan });
    }

    /// Fraction of the clip already played (0..1), or None when the voice is gone; 0 for a held
    /// anchor ([`status_voice`](Self::status_voice)).
    pub fn progress(&self, id: VoiceId) -> Option<f32> {
        let (pend, snap) = self.tx.overlay(id);
        match pend {
            // started, not yet in the mix: at its beginning
            Some(Pend::Start(_)) => Some(0.0),
            Some(_) => None,
            None => snap.status_voice(id).map(|v| {
                if v.held {
                    return 0.0;
                }
                let n = v.frames.max(1) as f64;
                (v.position / n).clamp(0.0, 1.0) as f32
            }),
        }
    }

    /// The voice's position in ms — what the sound library's status call 0x566910 answers at `+4`:
    /// the frames played `· 1000 / rate` (0x5669B9…0x5669D5, a 64-bit multiply and divide), 0 for
    /// a held anchor ([`status_voice`](Self::status_voice)), or `None` once the voice has ended
    /// (its state 3, 0x566A1E).
    pub fn position_ms(&self, id: VoiceId) -> Option<i32> {
        let (pend, snap) = self.tx.overlay(id);
        match pend {
            Some(Pend::Start(_)) => Some(0),
            Some(_) => None,
            None => snap.status_voice(id).map(|v| if v.held { 0 } else { played_ms(v.position as u64, v.rate) }),
        }
    }

    /// **`Voice_play` 0x4A9C10's start** (`Voice_start` 0x565E70 → 0x5907B0 → 0x590420): the
    /// library units, no float in between — the pitch **word** `+0xC` ([`resample::PITCH_UNIT`] =
    /// 4096 the sample's root, `pitch16_16 >> 4`, 0x4A9AC5), the **bend byte** `+9` (`voice+0x3F`)
    /// and the pan byte `+7` ([`pan_byte`] of the angle `pan`), taken together. `sel` (0x7F here,
    /// the request's default) is `Voice_play`'s seventh argument (0x4A9B89…0x4A9B90): the start
    /// (0x5907B0) tests each record's codes 0x01 / 0x02 against it and multiplies the patch volume
    /// by `sel / 127` (0x5905D0), and draws from the library's generator
    /// ([`crate::librand::patch_start`]) in the original's order. `clip.channels` voices of the
    /// budget go through the allocator ([`pick_victims`]); a refused start still returns a handle,
    /// one that names no voice — the library's −9, on which every later call fails
    /// ([`is_playing`](Self::is_playing) is false).
    ///
    /// Every record whose ranges hold `sel` starts as a voice of its own, with its own sample
    /// ([`Clip::layers`]), its own allocation (0x590420 → 0x5898F0 with the record's channel count
    /// and priority) and its own draws, in record order (0x590841…0x590992). A refused allocation
    /// stops the records already started and the start fails (0x5909A6). Two or more started make
    /// a **group** (0x5909DB…0x590A5C): one id (`voice+0x1E` from 0x5903D0), the last started the
    /// anchor (`+0x1B`), and the start answers the anchor's handle (`[esp+0x14]`, 0x590420's last
    /// answer). Every later call on the handle walks the group (0x590340: stop 0x565D60, volume,
    /// pitch, pan), which here is every voice with the returned [`VoiceId`]. *(Until 2026-09-27 the
    /// port played one merged sample per patch, the last record's, with the last record's draws —
    /// an approximation; until 2026-09-27, later, audio port 8, every caller gave a float pitch
    /// factor here instead of the word, [`resample::PITCH_UNIT`]'s own doc.)*
    pub fn play_word(&self, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool) -> VoiceId {
        self.play_word_sel(clip, volume, word, bend, pan, looping, 0x7F)
    }

    /// [`play_word`](Self::play_word) with the request's byte `+6`, `sel` — the collision one-shot's
    /// own aux/gate byte, the only shipped caller of a `sel` other than 0x7F.
    pub fn play_word_sel(&self, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool, sel: u8) -> VoiceId {
        self.play_word_tabled(clip, volume, word, bend, pan, looping, sel, true)
    }

    /// [`play_word`](Self::play_word) for a start the original makes with a bare `Voice_start`
    /// 0x565E70, never `Voice_play` 0x4A9C10: the HUD's `fe_sound` (0x4AADB0), the front end's
    /// `fesfx`, the checkpoint chime, the wrong-way cue and the pylon-hit cue (straight to
    /// `Bank_playOneShot` 0x4A98E0, no voice number at all). Not walked by
    /// [`silence_voices`](Self::silence_voices).
    pub fn play_word_untabled(&self, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool) -> VoiceId {
        self.play_word_tabled(clip, volume, word, bend, pan, looping, 0x7F, false)
    }

    /// [`play_word_untabled`](Self::play_word_untabled) with the request's byte `+6`
    /// ([`play_word_sel`](Self::play_word_sel)): the collision one-shot's own path, voice 0x33 —
    /// `Voice_play` takes it to `Bank_playOneShot` (0x4a9cd4…0x4a9d0b) but never writes the table
    /// for it (see the `table` field of [`Voice`]).
    pub fn play_word_sel_untabled(&self, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool, sel: u8) -> VoiceId {
        self.play_word_tabled(clip, volume, word, bend, pan, looping, sel, false)
    }

    fn play_word_tabled(&self, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool, sel: u8, tabled: bool) -> VoiceId {
        let mut st = self.tx.state.lock().unwrap();
        let id = VoiceId(st.next_id);
        st.next_id += 1;
        self.tx.push(&mut st, Cmd::Start { id, clip, volume, word, bend, pan, looping, sel, tabled }, Some(Pend::Start(id)));
        id
    }

    /// `Voice_play`'s update of a playing voice (0x4A9E1F…0x4A9FA3): `setVolume`, `Voice_setBend`
    /// 0x5660C0 with the byte, `Voice_setPitch` 0x565F80 with the word (`pitch16_16 >> 4`, the low
    /// 16 bits into `voice+0x6C`, 0x565FEB) — the pan is [`set_pan`](Self::set_pan).
    pub fn set_word(&self, id: VoiceId, volume: f32, word: u16, bend: u8) {
        self.tx.send(Cmd::SetWord { id, volume, word, bend });
    }

    /// **0x565DD0(handle, ticks, target)** on every playing voice of the handle's group
    /// ([`Ramp`]); a held anchor (state 2) is not walked (0x590340 takes state 1 only).
    pub fn ramp(&self, id: VoiceId, ticks: i32, target: i32) {
        self.tx.send(Cmd::Ramp { id, ticks, target });
    }

    /// **0x565F00(handle, value)**, the strip hook `+0xE0` on every playing voice of the group
    /// ([`LowPass`]); `value` from [`voice_play_filter`].
    pub fn set_filter(&self, id: VoiceId, value: i32) {
        self.tx.send(Cmd::Filter { id, value });
    }

    /// `Voice_play`'s update tail (0x4A9FA8…0x4AA03B): [`set_filter`](Self::set_filter) with
    /// `value` when there is one.
    pub fn voice_play_filter(&self, id: VoiceId, value: Option<i32>) {
        if let Some(v) = value {
            self.set_filter(id, v);
        }
    }

    /// The audio update's release of a voice, 0x565DD0(h, [`RELEASE_TICKS`], −1) — 0x4A9890 and
    /// the direct calls ([`RELEASE_TICKS`]'s list). The caller drops the handle.
    pub fn release(&self, id: VoiceId) {
        self.ramp(id, RELEASE_TICKS, RELEASE_TARGET);
    }

    /// `Voice_play`'s release of a slot's previous sample, 0x565DD0(h, 0x19, −1) (0x4A9DA4).
    pub fn release_replaced(&self, id: VoiceId) {
        self.ramp(id, REPLACE_TICKS, RELEASE_TARGET);
    }

    pub fn stop(&self, id: VoiceId) {
        self.tx.stop(id);
    }

    /// A cheap, cloneable handle that can [`Self::stop`] a voice without keeping the whole
    /// `AudioOutput` (its `cpal::Stream`) alive — `FEMovie`'s own audio track (`mad.md` §14) needs
    /// to stop itself well after the code that started it has moved on: at a loop restart
    /// (0x4E2380's `0x567690`) and whenever its widget is torn down (the destructor was not traced;
    /// this is the port's own equivalent, so no voice a widget started is ever left playing once
    /// the widget is gone).
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.tx.clone())
    }

    /// `0x565CD0(id, level)`: bit 0x20 of the id sets the bus `[0x6B4A0C]`, bit 0x80
    /// `[0x6B4A0D]` (an id with none of 0xE0 sets all, 0x565CDD…0x565CE1), stored as bytes; the
    /// voices' sends follow at once (0x565D28…0x565D51 → 0x5902A0). `p20` / `p80` are the tunnel
    /// reverb's two parameters (`reverb::EnvParams`).
    pub fn set_env(&self, p20: i32, p80: i32) {
        self.tx.bus[0].store(p20 as i8, Ordering::Relaxed);
        self.tx.bus[1].store(p80 as i8, Ordering::Relaxed);
        self.tx.send(Cmd::Env { p20, p80 });
    }

    /// The two buses as `set_env` left them.
    pub fn env(&self) -> (i8, i8) {
        (self.tx.bus[0].load(Ordering::Relaxed), self.tx.bus[1].load(Ordering::Relaxed))
    }

    /// **0x56AFA0** → 0x592E40 → 0x58F5D0, the driver's latency in ms: the software mixer's write
    /// position `[0x6AD78C]` less the primary buffer's play cursor (`GetCurrentPosition`, bytes
    /// `/ 2 / channels`), wrapped by the buffer length `[0x6AD7B4]`, `· 1000 / rate` (`[0x6B48DA]`,
    /// `idiv`) — what has been mixed and not yet heard; 0 with no buffer or on a failed call. Here:
    /// what the last callback queued ([`latency_ms`]) less the time since.
    pub fn latency_ms(&self) -> i32 {
        let l = &self.tx.lat;
        let at = l.at.load(Ordering::Acquire);
        if at == 0 {
            return 0;
        }
        let since = l.epoch.elapsed().saturating_sub(std::time::Duration::from_nanos(at - 1));
        latency_ms(l.frames.load(Ordering::Relaxed), since, self.rate)
    }

    /// Is a one-shot / loop voice still playing?
    pub fn is_playing(&self, id: VoiceId) -> bool {
        let (pend, snap) = self.tx.overlay(id);
        match pend {
            Some(Pend::Start(_) | Pend::Stream(..)) => true,
            Some(Pend::Stop(_)) => false,
            None => snap.voices.iter().any(|v| v.id == id && !v.finished) || snap.streams.iter().any(|(i, _)| *i == id),
        }
    }
}

/// The samples a stream's ring holds (1 Mi: ~12 s of 44.1 kHz stereo).
const STREAM_QUEUE: usize = 1 << 20;

/// Streamed stereo/mono source fed from another thread (music).
pub struct StreamSource {
    pub rate: u32,
    pub channels: u16,
    queue: SampleRing,
    pub finished: std::sync::atomic::AtomicBool,
}

impl StreamSource {
    pub fn new(rate: u32, channels: u16) -> Arc<Self> {
        Arc::new(Self { rate, channels, queue: SampleRing::new(STREAM_QUEUE), finished: std::sync::atomic::AtomicBool::new(false) })
    }
    /// Queue decoded samples (the feeder thread). The queue is a lock-free ring the callback reads
    /// without waiting; the feeder keeps itself ~2 s ahead, far below its size.
    pub fn push(&self, samples: &[i16]) {
        let n = self.queue.push(samples);
        if n < samples.len() {
            tracing::warn!(dropped = samples.len() - n, "music stream queue full");
        }
    }
    pub fn queued_frames(&self) -> usize {
        self.queue.len() / self.channels.max(1) as usize
    }
}

impl AudioOutput {
    /// `0x566AE0(stream, ms, target)`: ramp a stream's volume to the byte `target` over `ms`
    /// ([`Fade`]).
    pub fn fade(&self, id: VoiceId, ms: i32, target: i32) {
        self.tx.send(Cmd::Fade { id, ms, target });
    }

    /// A stream's volume now (0x567850 answers the voice's current level, 0x591320), `None` when
    /// it is not playing.
    pub fn stream_volume(&self, id: VoiceId) -> Option<f32> {
        let (pend, snap) = self.tx.overlay(id);
        match pend {
            Some(Pend::Stream(_, v)) => Some(v),
            Some(_) => None,
            None => snap.streams.iter().find(|(i, _)| *i == id).map(|(_, v)| *v),
        }
    }

    /// Play a streamed source at its native rate (the voices' resampler, [`resample`]), returns a stop
    /// handle. Its channels take voices of the budget at [`STREAM_PRIORITY`] (0x5612A0), so it is
    /// never stolen; a refused start (the budget full of unstealable voices) returns a dead handle.
    pub fn play_stream(&self, src: Arc<StreamSource>, volume: f32) -> VoiceId {
        let mut st = self.tx.state.lock().unwrap();
        let id = VoiceId(st.next_id);
        st.next_id += 1;
        self.tx.push(&mut st, Cmd::Stream { id, src, volume }, Some(Pend::Stream(id, volume)));
        id
    }
}

pub(crate) struct StreamVoice {
    pub(crate) src: Arc<StreamSource>,
    pub(crate) volume: f32,
    /// the resampler's 16-bit fraction ([`resample::Cursor`])
    pub(crate) frac: u32,
    /// per channel `s[P−1], s[P], s[P+1], s[P+2]` around the stream's position, 16-bit scale
    pub(crate) window: [[f32; 4]; 2],
    /// the first three frames are in `window`
    pub(crate) primed: bool,
    pub(crate) fade: Option<Fade>,
}

/// **0x566AE0(stream, ms, target)**: the byte `stream+0xE4 = target` and **0x565DD0**(`stream+8`,
/// `ms / 10`, target) on the stream's voice — the same [`Ramp`] as a release: `ms / 10` toward
/// zero (`imul` 0x66666667, `sar 2`, the sign added), at least 1 (0x565DFF…0x565E03); the goal
/// `target << 16`, the step `(goal − voice+0x2C) / ticks` (`idiv`, toward zero), one step a
/// service tick, a hundredth of a second of the mix ([`ServiceClock`], 0x58FBC0), until the
/// volume is past the goal — then it is the goal and the step 0. The voice's level is the word
/// `>> 16` (0x589DF0), which 0x567850 answers (its `+0x3D`, 0x591320). So F9's
/// 0x566AE0(stream, 0x1F4, 0) (0x4AD80A) from level 80 steps 50 times by −104857 in 16.16: the
/// level runs 78, 76, 75 … 1, 0 — 0 on the fiftieth tick with 30 left in the word, the goal
/// on the fifty-first. A goal below 0 would stop the handle (0x5895E1…0x589619); no stream
/// caller passes one. *(Until 2026-09-27 the port stepped the float volume in exactly `ms / 10`
/// even steps, not byte levels.)*
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade {
    pub ramp: Ramp,
    /// the stream voice's `+0x2C`, 16.16
    pub vol16: i32,
}

impl Fade {
    /// `volume` the stream's current `byte / 127`, `target` the call's byte.
    pub fn new(volume: f32, ms: i32, target: i32) -> Self {
        let vol16 = volume_16(volume);
        Self { ramp: Ramp::start(vol16, ms / 10, target), vol16 }
    }

    /// One service tick: `true` when the ramp went below 0 and stops the handle.
    pub fn tick(&mut self) -> bool {
        self.ramp.tick(&mut self.vol16)
    }

    /// The level byte over 127, as the mix takes the stream's volume.
    pub fn volume(&self) -> f32 {
        (self.vol16 >> 16).max(0) as f32 / 127.0
    }

    /// The ramp has reached its goal.
    pub fn done(&self) -> bool {
        self.ramp.step == 0
    }
}

/// The frames still queued `since` the callback that left `frames` for the device, as 0x58F5D0
/// answers them: `frames · 1000 / rate`, toward zero, never below 0.
pub fn latency_ms(frames: u64, since: std::time::Duration, rate: u32) -> i32 {
    let gone = (since.as_secs_f64() * rate as f64) as u64;
    (frames.saturating_sub(gone) * 1000 / rate.max(1) as u64) as i32
}

/// `played · 1000 / rate`, truncated (0x590D80 / 0x590DF0 are the 64-bit multiply and unsigned
/// divide).
pub fn played_ms(frames: u64, rate: u32) -> i32 {
    (frames * 1000 / rate.max(1) as u64) as i32
}

#[cfg(test)]
mod fx_tests {
    #[test]
    fn the_send_is_the_two_bytes_over_127_squared() {
        // every voice's default 0x7F with the engine banks' 127: the whole bus level
        assert_eq!(super::fx_send(0x7F, 127, 100), 100);
        assert_eq!(super::fx_send(0x7F, 127, 8), 8);
        // a patch without code 0x13 is never in the reverb
        assert_eq!(super::fx_send(0x7F, 0, 100), 0);
        // a full-level collision (aux 127) is dry; a light one (aux 66): 61·127·100 / 16129 = 48.03
        assert_eq!(super::fx_send(0x7F - 127, 127, 100), 0);
        assert_eq!(super::fx_send(0x7F - 66, 127, 100), 48);
        // toward zero, not rounded: 61·127·68 / 16129 = 32.66 → 32; the 0x80 bus 34 → 16.33 → 16
        assert_eq!(super::fx_send(0x7F - 66, 127, 68), 32);
        assert_eq!(super::fx_send(0x7F - 66, 127, 34), 16);
        // the software path's send `send · (1/127 f32)` (0x596A10) and the wet gain
        // `volume · send · 1.5` (0x5980A0)
        assert!((super::reverb_input(0x7F, 127, 100) - 100.0 / 127.0).abs() < 1e-6);
        assert!((super::wet_gain(1.0, super::reverb_input(0x7F, 127, 100)) - 100.0 / 127.0 * 1.5).abs() < 1e-6);
        assert_eq!(super::wet_gain(0.5, 0.0), 0.0);
    }
}

#[cfg(test)]
mod reverb_tests {
    use super::{next_prime, AllPass, Reverb, BLOCK, REVERB_ENTRIES};

    #[test]
    fn the_delay_is_the_next_prime_of_the_truncated_frame_count() {
        assert_eq!(next_prime(0), 2);
        assert_eq!(next_prime(1), 2);
        assert_eq!(next_prime(13), 13);
        assert_eq!(next_prime(14), 17);
        // 100 / 70 / 49 / 34 ms at 22050 Hz: 2205 → 2207, 1543.5 → 1543, 1080.45 → 1087, 749.7 → 751
        let l: Vec<usize> = REVERB_ENTRIES.iter().map(|&e| AllPass::new(22050, e).len).collect();
        assert_eq!(l, [2207, 1543, 1087, 751]);
    }

    #[test]
    fn an_all_pass_returns_the_input_at_once_and_its_feedback_one_delay_later() {
        // entry 0: c = 64/127, no damping (g1 = 0, g2 = 1)
        let mut ap = AllPass::new(1000, REVERB_ENTRIES[0]);
        let len = ap.len;
        assert_eq!(len, 101);
        let c = ap.c;
        let mut x = vec![0.0f32; len + 1];
        x[0] = 1.0;
        ap.run(&mut x);
        // y = −c·x + (the empty line) on the way in, then the stored `x + c·y` comes back
        assert!((x[0] + c).abs() < 1e-6);
        assert!((x[len] - (1.0 - c * c)).abs() < 1e-5);
    }

    #[test]
    fn the_reverb_rings_for_two_thousand_blocks_after_the_last_send() {
        let mut r = Reverb::new(22050);
        let mut out = vec![[0.0f32; 2]; BLOCK];
        // nothing sent: silent, and the input is never touched
        r.process(&mut out);
        assert!(out.iter().all(|f| *f == [0.0, 0.0]));
        r.send(0, 1000.0, 1.0);
        r.process(&mut out);
        assert!(out[0][0] != 0.0 && out[0][0] == out[0][1]);
        for _ in 0..1997 {
            r.process(&mut out);
        }
        assert_eq!(r.quiet, 1998);
        r.process(&mut out);
        r.process(&mut out);
        assert_eq!(r.quiet, 2000);
        let mut quiet = vec![[0.0f32; 2]; BLOCK];
        r.process(&mut quiet);
        assert!(quiet.iter().all(|f| *f == [0.0, 0.0]));
    }
}

#[cfg(test)]
mod fade_tests {
    use super::Fade;

    #[test]
    fn the_f9_fade_is_the_library_ramp_in_byte_levels() {
        // F9's 0x566AE0(stream, 0x1F4, 0) on a stream at level 80: 500 / 10 = 50 ticks, step
        // (0 − 0x500000) / 50 = −104857 toward zero
        let mut f = Fade::new(80.0 / 127.0, 500, 0);
        assert_eq!((f.vol16, f.ramp.step, f.ramp.goal), (80 << 16, -104857, 0));
        let mut levels = Vec::new();
        while !f.done() {
            assert!(!f.tick(), "a goal of 0 never stops the stream");
            levels.push(f.vol16 >> 16);
        }
        assert_eq!(levels.len(), 51);
        assert_eq!(&levels[..3], &[78, 76, 75]);
        assert_eq!(&levels[48..], &[1, 0, 0]);
        // the fiftieth tick leaves 30 in the word (level 0), the fifty-first the goal
        assert_eq!(f.vol16, 0);
        assert_eq!(f.volume(), 0.0);
        // `ms / 10` toward zero, at least one tick: a 5 ms ramp (the song start's) is one step
        let mut f = Fade::new(0.0, 5, 51);
        assert!(!f.tick());
        assert!(f.done());
        assert_eq!(f.volume(), 51.0 / 127.0);
        // a negative goal stops the handle when the word goes below 0
        let mut f = Fade::new(1.0 / 127.0, 10, -1);
        assert!(f.tick());
    }
}

#[cfg(test)]
mod position_tests {
    #[test]
    fn the_latency_is_what_is_queued_and_not_yet_heard() {
        use std::time::Duration;
        // 1024 frames of buffer and 1024 of device delay at 48 kHz: 42 ms, toward zero
        assert_eq!(super::latency_ms(2048, Duration::ZERO, 48000), 42);
        // 10 ms later 480 frames have gone
        assert_eq!(super::latency_ms(2048, Duration::from_millis(10), 48000), 32);
        // never negative
        assert_eq!(super::latency_ms(2048, Duration::from_secs(1), 48000), 0);
    }

    #[test]
    fn the_position_is_whole_milliseconds() {
        assert_eq!(super::played_ms(22050, 22050), 1000);
        assert_eq!(super::played_ms(22049, 22050), 999);
        assert_eq!(super::played_ms(0, 22050), 0);
    }
}

#[cfg(test)]
mod pan_tests {
    use super::{atan2_1024, pan_angle, pan_byte, pan_gains, voice_pan};

    #[test]
    fn the_angle_is_the_games_atan2_on_the_listener_axes() {
        // (side, forward): ahead 0, the +side hand 0x100, behind 0x200, the other hand −0x100
        assert_eq!(atan2_1024(0, 256), 0);
        assert_eq!(atan2_1024(256, 0), 0x100);
        assert_eq!(atan2_1024(0, -256), 0x200);
        assert_eq!(atan2_1024(-256, 0), -0x100);
        assert_eq!(atan2_1024(256, 256), 0x80);
        // x/y = 0.5: step (100·256 + 100) / 200 = 128 → atan(0.5) in 1024ths = 75.56 → 76; the
        // 64-bit path for x ≥ 0x800000 lands on the same step
        assert_eq!(atan2_1024(100, 200), 76);
        assert_eq!(atan2_1024(0x90_0000, 0x120_0000), 76);
        // the 16-bit angle: ·65536, toward zero, sar 8, & 0x3FF, << 6
        assert_eq!(pan_angle(1.0, 0.0), 0x4000);
        assert_eq!(pan_angle(-1.0, 0.0), 0xC000);
        assert_eq!(pan_angle(0.0, -3.0), 0x8000);
        assert_eq!(pan_angle(2.0, 2.0), 0x2000);
    }

    #[test]
    fn the_pan_byte_folds_the_rear_onto_the_front() {
        // 0x4A9B4B…0x4A9B84: 0x40 ahead, 0x7F at the +side hand, 0 at the other
        assert_eq!(pan_byte(0), 0x40);
        assert_eq!(pan_byte(0x2000), 0x60);
        assert_eq!(pan_byte(0x3FFF), 0x7F);
        assert_eq!(pan_byte(0x4000), 0x7F);
        // dead behind is 0x3F, one step off the front's 0x40
        assert_eq!(pan_byte(0x8000), 0x3F);
        assert_eq!(pan_byte(0xC000), 0);
        assert_eq!(pan_byte(0xFFFF), 0x3F);
    }

    #[test]
    fn the_patch_scales_and_offsets_the_pan_byte() {
        // no code 0x0C / 0x25: the caller's byte as it is
        assert_eq!(voice_pan(0x7F, 64, 1), 0x7F);
        // SND_coll's pan 40 on a centred call: 0x40 + 40 − 0x40
        assert_eq!(voice_pan(0x40, 40, 1), 40);
        assert_eq!(voice_pan(0, 90, 1), 26);
        // fesfx's scale 4 about 0x40, clamped to 0…127
        assert_eq!(voice_pan(0x48, 64, 4), 96);
        assert_eq!(voice_pan(0x60, 64, 4), 127);
        assert_eq!(voice_pan(0x20, 64, 4), 0);
    }

    #[test]
    fn the_pan_law_is_the_software_mixers() {
        // one channel at 127/127, the other silent (0x58E0E0)
        assert_eq!(pan_gains(0x7F), [1.0, 0.0]);
        assert_eq!(pan_gains(0), [0.0, 1.0]);
        // centre: x = 63, f = 126 − 31 = 95 → 95/127 on both (0x58E0B0)
        let c = pan_gains(0x40);
        assert!((c[0] - 95.0 / 127.0).abs() < 1e-6 && (c[1] - 95.0 / 127.0).abs() < 1e-6);
        // 0x60 (angle 0x2000): shares 127·96/128 = 95 and 127·32/128 = 31 → f = 119 and 55
        let g = pan_gains(0x60);
        assert!((g[0] - 119.0 / 127.0).abs() < 1e-6 && (g[1] - 55.0 / 127.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::{pick_victims, voice_budget, STREAM_PRIORITY};

    #[test]
    fn the_budget_is_the_quality_tables() {
        // 0x5B4B08: 48 / 24 / 12; quality 3 keeps the library's 24 (0x58A5B5)
        assert_eq!([0, 1, 2, 3].map(voice_budget), [48, 24, 12, 24]);
    }

    #[test]
    fn a_free_voice_is_taken_first() {
        let live = vec![(100, 1, 0); 23];
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![]));
    }

    #[test]
    fn a_full_budget_steals_the_lowest_priority_then_the_oldest() {
        // 24 voices: engine layers at 100, a collision at 26 started late, one at 26 earlier
        let mut live: Vec<(u8, u16, u32)> = (0..22).map(|i| (100, 1, i)).collect();
        live.push((26, 1, 30));
        live.push((26, 1, 25));
        // the lowest priority goes first, of the two the older (stamp 25, index 23)
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![23]));
        // two needed: both 26s, oldest first
        assert_eq!(pick_victims(&live, 24, 2), Some(vec![23, 22]));
        // the request's own priority is not compared: any start takes a voice, and with only
        // 100s left the oldest of them
        let live: Vec<(u8, u16, u32)> = (0..24).map(|i| (100, 1, 50 - i)).collect();
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![23]));
    }

    #[test]
    fn an_exact_tie_of_priority_and_stamp_steals_the_lowest_scanned_slot() {
        // 0x589a51…0x589a58: on equal priority *and* equal stamp, `jae` skips the later
        // candidate, so the earlier-scanned slot (the port's earlier-pushed voice, its stand-in
        // for "lowest slot" — `pick_victims`'s doc) keeps the pick, in whatever order the ties
        // are given
        let live: Vec<(u8, u16, u32)> = vec![(26, 1, 5), (26, 1, 5), (26, 1, 5)];
        assert_eq!(pick_victims(&live, 3, 1), Some(vec![0]));
    }

    #[test]
    fn streams_and_priorities_from_0x65_are_never_stolen() {
        // a stereo stream (0x65) and 22 voices at 0x65 or above: nothing can be taken
        let mut live: Vec<(u8, u16, u32)> = vec![(STREAM_PRIORITY, 2, 0)];
        live.extend((1..23).map(|i| (if i % 2 == 0 { 0x65 } else { 0xC8 }, 1, i)));
        assert_eq!(pick_victims(&live, 24, 1), None);
        // one stealable voice among them is the one taken
        live.push((100, 1, 99));
        assert_eq!(pick_victims(&live, 26, 2), Some(vec![23]));
        // with no free voice beside it, one stealable mono voice cannot make room for a stereo start
        assert_eq!(pick_victims(&live, 25, 2), None);
    }

    #[test]
    fn a_refusal_stops_nothing_and_a_stereo_victim_frees_two() {
        // 12 voices (quality 2), ten mono at 100 and one stereo at 90: a stereo start needs 2
        let mut live: Vec<(u8, u16, u32)> = (0..10).map(|i| (100, 1, i)).collect();
        live.push((90, 2, 20));
        assert_eq!(pick_victims(&live, 12, 2), Some(vec![10]));
        // a stereo start into a budget of one voice: refused, not one voice given up
        assert_eq!(pick_victims(&[(0, 1, 0)], 1, 2), None);
    }
}

#[cfg(test)]
mod rate_tests {
    use super::{mix_rate, Bridge, Shared};
    use crate::resample;

    fn ramp(sign: f32) -> impl FnMut(usize) -> Vec<[f32; 2]> {
        let mut next = 0.0f32;
        move |n| {
            (0..n)
                .map(|_| {
                    next += 1.0;
                    [next, sign * next]
                })
                .collect()
        }
    }

    /// The device frames (44.1 kHz callbacks of 512) from a sound started right after the
    /// driver's sleep changed from the library's 10 ms to `AUDIOQUALITY` 1's 20 ms, `intro_s`
    /// seconds of output after the device opened, until the sound is heard.
    fn frames_until_heard_after_sleep_change(intro_s: f64) -> usize {
        let mut s = Shared::default();
        let mut bridge = Bridge::default();
        let step = 22050.0 / 44100.0;
        let callback = |s: &mut Shared, bridge: &mut Bridge| -> bool {
            let mut heard = false;
            for k in 0..512 {
                let lr = bridge.next(step, 512 - k, |_, pending| s.pump_to(22050, pending));
                heard |= lr[0] != 0.0;
            }
            heard
        };
        for _ in 0..(intro_s * 44100.0 / 512.0) as usize {
            callback(&mut s, &mut bridge);
        }
        // the front end's first `fe_music`: 0x4A66B0's configuration for AUDIOQUALITY 1 (20 ms)
        s.sleep_ms = Some(20);
        // one callback with it (the music stream opens silent, state 2), then a sound starts
        callback(&mut s, &mut bridge);
        let clip = std::sync::Arc::new(super::Clip { rate: 22050, channels: 1, samples: vec![1000; 22050], loop_range: Some((0, 22050)), gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() });
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, true, 0)));
        let mut n = 0;
        while !callback(&mut s, &mut bridge) {
            n += 512;
            assert!(n < 44100 * 120, "never heard");
        }
        n
    }

    #[test]
    fn a_driver_sleep_change_queues_no_backlog() {
        // AUDIO-START (2026-10-09): the intro opened the output with the library's 10 ms
        // pass, the front end's first `set_audio_quality` made it 20 ms, and a pass wrote
        // `passes · 20 ms` less what was written — the whole intro once more, mixed at once and
        // played before anything newer
        for intro in [3.0, 10.0, 30.0] {
            let n = frames_until_heard_after_sleep_change(intro);
            eprintln!("intro {intro} s: first sound after {n} device frames ({} ms)", n * 1000 / 44100);
            assert!(n <= 2 * 512, "intro {intro} s: {n} frames of backlog");
        }
    }

    #[test]
    fn the_driver_writes_a_sleep_of_frames_in_sixteens() {
        use super::{Pump, pump_sleep_ms, ring_frames};
        assert_eq!([0, 1, 2, 3].map(pump_sleep_ms), [8, 20, 50, 10]);
        // 0.4 s of the mix rate, in sixteens
        assert_eq!([32000, 22050, 16000].map(ring_frames), [12800, 8816, 6400]);
        // 22050 Hz, 20 ms: 441 frames a pass, written as 432 / 448 (the cursor's sixteens)
        let mut p = Pump::default();
        let w: Vec<u32> = (0..4).map(|_| p.next_write(22050, 20).iter().sum()).collect();
        assert_eq!(w, [432, 448, 432, 448]);
        // 32000 Hz, 8 ms: 256 a pass, the ring of 12800 = 50 passes, so no pass is cut
        let mut p = Pump::default();
        assert!((0..200).all(|_| p.next_write(32000, 8)[1] == 0));
        // a pass across the ring's end is two parts (0x58F9DB…0x58F9F5): the shipped qualities'
        // passes land on the ring's end (20 · 441 → 8816, 8 · 800 = 6400, 50 · 256 = 12800), the
        // library's 10 ms at 22050 does not — its 160th pass is 208 + 16
        let mut p = Pump::default();
        let parts: Vec<[u32; 2]> = (0..160).map(|_| p.next_write(22050, 10)).collect();
        assert_eq!(parts[159], [208, 16]);
        assert!(parts[..159].iter().all(|x| x[1] == 0));
        assert_eq!(parts.iter().map(|x| x[0] + x[1]).sum::<u32>(), (160 * 2205 / 10) & !15);
    }

    #[test]
    fn the_mix_is_rendered_in_the_driver_passes() {
        // each pass is one or two 0x58FBC0 calls: at 22050 Hz / 20 ms the first pass of 432
        // frames spans the first tick (208) and 224 of the second (224), so the service runs twice
        let mut s = Shared::default();
        s.sleep_ms = Some(20);
        assert_eq!(s.pump(22050).len(), 432);
        assert_eq!(s.clock.tick, 2);
        assert_eq!(s.clock.left, 0);
        assert_eq!(s.pump(22050).len(), 448);
    }

    #[test]
    fn the_service_tick_is_a_hundredth_of_a_second_in_sixteens() {
        use super::ServiceClock;
        // 0x58FBC0: rate · tick / 100 less what was counted, rounded down to 16
        let mut c = ServiceClock::default();
        let ticks: Vec<u32> = (0..6).map(|_| c.next_tick(22050)).collect();
        assert_eq!(ticks, [208, 224, 224, 224, 208, 224]);
        let mut c = ServiceClock::default();
        assert!((0..100).all(|_| c.next_tick(32000) == 320));
        // on average the rate: 100 ticks of 22050 are 22048 frames (the last 16 not yet due)
        let mut c = ServiceClock::default();
        assert_eq!((0..100).map(|_| c.next_tick(22050)).sum::<u32>(), 22048);
        // both counters start over after tick 30000
        let mut c = ServiceClock { tick: 30000, counted: 30000 * 160, left: 0 };
        assert_eq!(c.next_tick(16000), 160);
        assert_eq!((c.tick, c.counted), (0, 0));
    }

    #[test]
    fn a_one_shot_is_freed_at_the_first_failing_pull() {
        use crate::resample::Interp;
        // 1000 frames at the mix rate (step 0x10000), linear: the decoder runs 2 ahead
        let clip = std::sync::Arc::new(super::Clip { rate: 22050, channels: 1, samples: vec![100; 1000], loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() });
        let mut s = Shared::default();
        s.interp = Interp::Linear;
        s.voices.push((super::VoiceId(0), super::Voice::new(clip.clone(), resample::PITCH_UNIT, 1.0, false, 0)));
        // blocks: the ticks 208, 224, 224, 224, 208; before the fifth the position is 880:
        // 880 + 2 < 1000, so the fifth block (to 1088) pulls and mixes the tail and the zeros
        s.render(208 + 224 * 3 + 208, 22050);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].1.cur.pos, 1088);
        // the sixth block's pull fails: 1088 + 2 ≥ 1000 — the voice is gone before it mixes
        s.render(1, 22050);
        assert!(s.voices.is_empty());
        // a looped voice never fails
        let mut s = Shared::default();
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, true, 0)));
        s.render(5000, 22050);
        assert_eq!(s.voices.len(), 1);
    }

    #[test]
    fn a_stream_fade_steps_once_a_tick() {
        let src = super::StreamSource::new(22050, 1);
        let mut s = Shared::default();
        s.streams.push((super::VoiceId(0), super::StreamVoice { src, volume: 1.0, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: Some(super::Fade::new(1.0, 30, 0)) }));
        // the first render runs the service before its first frame: one of the three steps,
        // 127 << 16 less 2774357 — level 84
        s.render(1, 22050);
        assert_eq!(s.streams[0].1.volume, 84.0 / 127.0);
        // the rest of the first tick (208 frames) has no service; the next tick's has
        s.render(207, 22050);
        assert_eq!(s.streams[0].1.volume, 84.0 / 127.0);
        s.render(1, 22050);
        assert_eq!(s.streams[0].1.volume, 42.0 / 127.0);
    }

    #[test]
    fn the_mix_rate_is_the_quality_table() {
        // 0x5B4AE4: 32000 / 22050 / 16000; quality 3 keeps the library's 22050 (0x58A5C2)
        assert_eq!([0, 1, 2, 3].map(mix_rate), [32000, 22050, 16000, 22050]);
        assert_eq!(Shared::default().mix_rate(), 22050);
    }

    #[test]
    fn the_bridge_interpolates_the_mix_to_the_device_rate() {
        // a ramp mixed at 16000 heard at 48000: three device frames per mix frame
        let mut r = ramp(-1.0);
        let mut b = Bridge::default();
        let got: Vec<f32> = (0..7).map(|k| b.next(16000.0 / 48000.0, 7 - k, |n, p| p.extend(r(n)))[0]).collect();
        let want = [1.0, 4.0 / 3.0, 5.0 / 3.0, 2.0, 7.0 / 3.0, 8.0 / 3.0, 3.0];
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{got:?}");
        }
        // at the same rate the mix passes through unchanged
        let mut r = ramp(1.0);
        let mut b = Bridge::default();
        assert_eq!((0..4).map(|k| b.next(1.0, 4 - k, |n, p| p.extend(r(n)))[1]).collect::<Vec<_>>(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_voice_steps_through_its_sample_at_the_mix_rate() {
        // a 22050 Hz sample mixed at 16000 (quality 2): 22050/16000 sample frames a mix frame
        let clip = std::sync::Arc::new(super::Clip { rate: 22050, channels: 1, samples: vec![0; 100_000], loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() });
        let mut s = Shared::default();
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, false, 0)));
        s.render(16000, 16000);
        // the step 22050·16/16000 = 22.05 (an f32) · 4096 → 90316 (truncated): 16000 of them
        let want = 90316u64 * 16000;
        assert_eq!((s.voices[0].1.cur.pos, s.voices[0].1.cur.frac as u64), (want >> 16, want & 0xFFFF));
    }

    #[test]
    fn a_known_sample_through_the_kernel_the_gains_and_the_store() {
        use crate::resample::{channel_gain, cubic, linear, mix_into, to_i16, Interp};
        // an 11025 Hz sample at the 22050 mix: step 0x8000, half a sample a frame
        let samples: Vec<i16> = vec![1000, -2000, 3000, 12000, -500];
        let clip = std::sync::Arc::new(super::Clip { rate: 11025, channels: 1, samples: samples.clone(), loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() });
        let s_at = |k: i64| if (0..5).contains(&k) { samples[k as usize] as f32 } else { 0.0 };
        let g = channel_gain(1.0, super::pan_gains(0x40)[0]);
        for interp in [Interp::Cubic, Interp::Linear] {
            let mut s = Shared::default();
            s.interp = interp;
            s.voices.push((super::VoiceId(0), super::Voice::new(clip.clone(), resample::PITCH_UNIT, 1.0, false, 0)));
            let out = s.render(12, 22050);
            for (k, frame) in out.iter().enumerate() {
                let (p, frac) = ((k / 2) as i64, if k % 2 == 1 { 0x8000 } else { 0 });
                // past the end the decoder's zeros go on through the kernel (0x597BF2)
                let x = if interp == Interp::Cubic {
                    cubic(s_at(p - 1), s_at(p), s_at(p + 1), s_at(p + 2), frac)
                } else {
                    linear(s_at(p), s_at(p + 1), frac)
                };
                let want = to_i16(mix_into(0.0, x, g)) as f32 / 32768.0;
                assert_eq!(frame[0], want, "{interp:?} frame {k}");
                assert_eq!(frame[1], want);
            }
            // the zeros past the end were mixed, and the voice is still held: its decoder fails
            // only at the next block's pull (cubic: 6 + 3 ≥ 5 samples; linear: 6 + 2)
            assert_eq!(s.voices.len(), 1);
            // a one-frame block asks the decoder for nothing (half a sample, 0x59BF88): no failure
            s.render(1, 22050);
            assert_eq!(s.voices.len(), 1);
            let out = s.render(2, 22050);
            assert_eq!(out, [[0.0, 0.0]; 2]);
            assert!(s.voices.is_empty());
        }
        // the cubic's first half-step reads the zeroed s[−1]: (−2000 − 0)/2 · t …, not s0 again
        assert_ne!(cubic(0.0, 1000.0, -2000.0, 3000.0, 0x8000), linear(1000.0, -2000.0, 0x8000));
    }
}

#[cfg(test)]
mod group_tests {
    use super::{Clip, Shared, Voice, VoiceId};
    use crate::resample;
    use nfs_formats::bnk::PatchRecord;
    use std::sync::Arc;

    fn layer(frames: usize, priority: u8) -> Arc<Clip> {
        Arc::new(Clip { rate: 22050, channels: 1, samples: vec![100; frames], loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() })
    }

    /// A patch of three records like `SND_coll`'s 0xD34: 72…97, 98…115 and 0…78 — a level of
    /// 75 starts the first and the third.
    fn patch(frames: [usize; 3]) -> Arc<Clip> {
        let rec = |lo: i32, hi: i32| PatchRecord::from_codes(&[(0x01, lo), (0x02, hi)]);
        let layers: Vec<Arc<Clip>> = frames.iter().map(|&n| layer(n, 26)).collect();
        Arc::new(Clip { records: vec![rec(72, 97), rec(98, 115), rec(0, 78)], layers, ..(*layer(10, 26)).clone() })
    }

    fn start(s: &mut Shared, clip: &Arc<Clip>, sel: u8) -> VoiceId {
        s.start_patch(clip.clone(), 1.0, resample::PITCH_UNIT, false, sel, true)
    }

    #[test]
    fn every_matching_record_starts_its_own_voice() {
        let clip = patch([300, 5000, 2000]);
        let mut s = Shared::default();
        let id = start(&mut s, &clip, 75);
        // records 0 and 2, each on its own sample, the last the anchor
        let lens: Vec<usize> = s.voices.iter().map(|(_, v)| v.clip.samples.len()).collect();
        assert_eq!(lens, [300, 2000]);
        assert!(s.voices.iter().all(|(i, _)| *i == id));
        assert_eq!(s.voices.iter().map(|(_, v)| v.anchor).collect::<Vec<_>>(), [false, true]);
        // a level of 100 starts record 1 alone: no group
        let mut s = Shared::default();
        start(&mut s, &clip, 100);
        assert_eq!(s.voices.len(), 1);
        assert!(!s.voices[0].1.anchor);
        assert_eq!(s.voices[0].1.clip.samples.len(), 5000);
    }

    #[test]
    fn the_anchor_is_held_until_the_group_ends() {
        // the anchor (record 2, 300 frames) ends first: held, in the budget, silent
        let clip = patch([2000, 10, 300]);
        let mut s = Shared::default();
        s.interp = crate::resample::Interp::Linear;
        start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert_eq!(s.voices.len(), 2);
        assert!(s.voices[1].1.held && !s.voices[0].1.held);
        // the other one's end frees both (0x589CB6)
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
        // the other way round: the first record ends first and is freed alone
        let clip = patch([300, 10, 2000]);
        let mut s = Shared::default();
        start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert_eq!(s.voices.len(), 1);
        assert!(s.voices[0].1.anchor && !s.voices[0].1.held);
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
    }

    #[test]
    fn a_held_anchor_answers_zero_never_a_siblings_position() {
        // 0x566910: the call always reads the handle's own entry (0x590e20's self-match), never a
        // sibling's; a held anchor's own entry stays in the group's array (it is not freed) but
        // its remaining-frames field is negative — state 0, every field left the zero the call
        // started with — and `None` only once the group is fully gone (state 3, 0x566a1e)
        let clip = patch([2000, 10, 300]);
        let mut s = Shared::default();
        s.interp = crate::resample::Interp::Linear;
        let id = start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert!(s.voices[1].1.held && s.voices[1].1.anchor && !s.voices[0].1.held);
        let status = |s: &Shared| {
            let mut snap = super::Snapshot::default();
            s.snapshot_into(&mut snap);
            (snap.status_voice(id).copied(), snap)
        };
        let v = status(&s).0.expect("the group is still alive");
        assert!(v.held, "answers the held anchor, not the still-playing sibling at voices[0]");
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
        assert!(status(&s).0.is_none(), "state 3: gone");
    }

    #[test]
    fn a_refused_record_stops_the_ones_started() {
        // one voice free: record 0 takes it, record 2 finds only the patch's own voice at 26 —
        // stealable, so it takes it: the first is stopped by the steal, the second plays
        let clip = patch([300, 10, 2000]);
        let mut s = Shared::default();
        s.budget = Some(1);
        start(&mut s, &clip, 75);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].1.clip.samples.len(), 2000);
        // two voices, one held by a voice at 0x65: record 0 (at 0x65 too) takes the free one,
        // record 2 finds nothing it may steal — refused, and record 0 is stopped again
        let mut s = Shared::default();
        s.budget = Some(2);
        s.voices.push((VoiceId(900), Voice::new(layer(10, 0x65), resample::PITCH_UNIT, 1.0, true, 0)));
        let unstealable = Arc::new(Clip { layers: clip.layers.iter().map(|l| Arc::new(Clip { priority: 0x65, ..(**l).clone() })).collect(), ..(*clip).clone() });
        let id = start(&mut s, &unstealable, 75);
        // record 0 took the free voice, record 2 was refused: record 0 is stopped again
        assert!(s.voices.iter().all(|(i, _)| *i != id));
        assert_eq!(s.voices.len(), 1);
    }

    #[test]
    fn two_starts_before_the_next_service_tick_share_the_voice_steal_stamp() {
        // 0x589531…0x589538 increments [0x6B49A8] once per call to the service, not once per
        // start (0x589B3F…0x589B54 just reads it) — so every patch started between two service
        // calls carries the same `voice+0x20` stamp
        let clip = layer(10, 50);
        let mut s = Shared::default();
        start(&mut s, &clip, 0x7F);
        start(&mut s, &clip, 0x7F);
        assert_eq!(s.voices.iter().map(|(_, v)| v.stamp).collect::<Vec<_>>(), [0, 0]);
        s.service();
        start(&mut s, &clip, 0x7F);
        assert_eq!(s.voices[2].1.stamp, 1);
    }
}

#[cfg(test)]
mod service_tests {
    use super::{cents_word, div_127, div_127_cubed, Clip, Envelope, Shared, Voice, VoiceId};
    use crate::resample;
    use nfs_formats::bnk::{PatchRecord, RecordTables};
    use std::sync::Arc;

    #[test]
    fn cents_to_the_pitch_word() {
        // 0x589E80 against the image's table (worked in scratch `audio6/chk_tbl.py`)
        let want = [(0, 4096), (1, 4096), (100, 4336), (-100, 3864), (600, 5792), (-600, 2904), (1199, 8176), (1200, 8192), (-1200, 2048), (2400, 16384), (-1600, 1624), (-1, 4088), (5, 4112), (-5, 4080)];
        for (c, w) in want {
            assert_eq!(cents_word(c), w, "{c} cents");
        }
    }

    #[test]
    fn the_root_and_the_fine_tune_start_the_pitch() {
        use super::base_cents;
        let rec = |codes: &[(u8, i32)]| PatchRecord::from_codes(codes);
        // no code 0x07 / 0x10: middle C, 0 cents
        assert_eq!(base_cents(&rec(&[])), 0);
        // sd6a's first slot: root 66 → six semitones down
        assert_eq!(base_cents(&rec(&[(0x07, 66)])), -600);
        // SND_coll's wind: root 76; a fine tune of −12 (the byte 244) on top
        assert_eq!(base_cents(&rec(&[(0x07, 76), (0x10, -12)])), -1612);
        // the start adds the draws to it (a record without spreads draws nothing)
        let mut s = Shared::default();
        let c = Arc::new(Clip { records: vec![rec(&[(0x07, 48), (0x10, 25)])], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, resample::PITCH_UNIT, false, 0x7F, true);
        assert_eq!(s.voices[0].1.cents, 1225);
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(1225));
    }

    #[test]
    fn an_engine_layer_pitch_end_to_end() {
        use super::base_cents;
        use crate::resample;
        // sd7a's layer 0x51 (44100 Hz, root 61, fine −43) at half the redline, spl[8] = 80, no
        // doppler — worked from the image in scratch `audio7/worked.py`: the record's word 3583
        // (0x4AC7F6…0x4AC81F), −143 cents → 3776 (0x589E80), +0x6E = 3583 · 3776 >> 12 = 3303
        // (0x589FCA), the step 3303 · f32(44100 · 16 / 32000) truncated = 72831 (0x596A50)
        let spl = nfs_formats::spl::Spl::default();
        let word = spl.pitch_word(nfs_formats::spl::rpm_frac_16(3000.0, 6000.0), 0x10000);
        assert_eq!(word, 3583);
        let r = PatchRecord::from_codes(&[(0x07, 61), (0x10, -43)]);
        assert_eq!(base_cents(&r), -143);
        assert_eq!(cents_word(-143), 3776);
        let mut s = Shared::default();
        let c = Arc::new(Clip { rate: 44100, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, word, true, 0x7F, true);
        assert_eq!(s.voices[0].1.pitch_word(), 3303);
        assert_eq!(resample::step_16_16(3303, 44100, 32000), 72831);
    }

    #[test]
    fn an_ai_engine_loop_pitch_end_to_end() {
        use super::base_cents;
        use crate::resample;
        // aud1ldex.bnk slot 0 (16000 Hz, root 61, fine +44, bend range 700 at the byte 0x40) at
        // half the redline, no doppler — worked from the image in scratch `audio8/worked.py`:
        // 0x4AAAAC…0x4AAAD1 give 3583, `<< 4` to Voice_play and `>> 4` back into `voice+0x6C`;
        // −56 cents → 3968 (0x589E80), `+0x6E` = 3583 · 3968 >> 12 = 3471, the step
        // 3471 · f32(16000 · 16 / 32000) = 27768 (0x596A50) at 32 kHz
        let word = crate::ai_engine::ai_pitch_word(crate::ai_engine::ai_rpm_frac(3000.0, 6000.0), crate::ai_engine::PITCH_BASE_STILL);
        assert_eq!(word, 3583);
        let r = PatchRecord::from_codes(&[(0x07, 61), (0x10, 44)]);
        assert_eq!(base_cents(&r), -56);
        assert_eq!(cents_word(-56), 3968);
        let mut s = Shared::default();
        let c = Arc::new(Clip { rate: 16000, bend_cents: 700, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, word, true, 0x7F, true);
        assert_eq!(s.voices[0].1.bend, 0x40);
        assert_eq!(s.voices[0].1.pitch_word(), 3471);
        assert_eq!(resample::step_16_16(3471, 16000, 32000), 27768);
    }

    #[test]
    fn the_bend_byte_is_cents_on_the_voice() {
        // SND_coll 0x10 (root 57, bend range 1200 cents) at the AI road loop's byte 127 and the
        // word 0x1000: 300 + ((127 − 64) · 1200 >> 6) = 1481 cents → 9632 (0x589E80), and
        // `+0x6E` = 0x1000 · 9632 >> 12 (0x589FCA; scratch `audio8/worked.py`)
        let r = PatchRecord::from_codes(&[(0x07, 57)]);
        let mut s = Shared::default();
        let c = Arc::new(Clip { bend_cents: 1200, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, resample::PITCH_UNIT, true, 0x7F, true);
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(300), "0x40 is no bend");
        s.voices[0].1.bend = 127;
        assert_eq!(s.voices[0].1.pitch_word(), 9632);
        // (0 − 0x40) · 1200 >> 6 = −1200, an octave down; `sar` rounds a negative term down
        s.voices[0].1.bend = 0;
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(-900));
        s.voices[0].1.bend = 0x3F;
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(300 - 19));
    }

    #[test]
    fn the_magic_divides_go_toward_zero() {
        for x in [-5_000_000, -2_048_383, -2_048_382, -1, 0, 1, 2_048_382, 2_048_383, 16_387_064, 260_144_641] {
            assert_eq!(div_127_cubed(x), x / 2_048_383, "{x}");
        }
        for x in (-16129..=16129).step_by(7) {
            assert_eq!(div_127(x), x / 127, "{x}");
        }
    }

    #[test]
    fn the_envelope_steps_once_a_tick_and_runs_a_segment_one_tick_over() {
        // SND_coll's common table: 22 ticks to 127, 20 to 126, hold at 121, 24 to 0; from 0
        let table = [(22, 127), (20, 126), (0x7FFF_FFFF, 121), (24, 0)];
        let mut e = Envelope::start(0, 4, &table);
        assert_eq!((e.level, e.step, e.left), (0, 378_321, 22));
        for _ in 0..22 {
            assert!(!e.tick(&table));
        }
        assert_eq!(e.level >> 16, 126);
        // the 23rd tick steps once more — 132 — and then loads the second segment
        assert!(!e.tick(&table));
        assert_eq!((e.level >> 16, e.seg, e.step, e.left), (132, 1, ((126 << 16) - 8_701_383) / 20, 19));
        // the hold (0x7FFFFFFF ticks) keeps it for good: the step rounds to 0 there
        for _ in 0..200 {
            assert!(!e.tick(&table));
        }
        assert_eq!(e.seg, 2);
        // a table that runs out stops the voice: 2 ticks to 50, then 3 to 0, two segments
        let table = [(2, 50), (3, 0)];
        let mut e = Envelope::start(0, 2, &table);
        // (the first segment steps on ticks 1–3, the second is loaded on tick 3 and has run
        // out by tick 6)
        let stops: Vec<bool> = (0..6).map(|_| e.tick(&table)).collect();
        assert_eq!(stops, [false, false, false, false, false, true]);
        // the default (no code 0x19): 127 for good
        let mut e = Envelope::start(0x7F, 1, &nfs_formats::bnk::DEFAULT_ENVELOPE);
        assert_eq!((e.level, e.step), (127 << 16, 0));
        assert!(!e.tick(&nfs_formats::bnk::DEFAULT_ENVELOPE));
    }

    fn clip(r: PatchRecord, t: RecordTables) -> Arc<Clip> {
        Arc::new(Clip { rate: 22050, channels: 1, samples: vec![100; 50_000], loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: vec![r], layers: Vec::new(), tables: vec![t] })
    }

    fn tables(envelope: Vec<(i32, i32)>) -> RecordTables {
        RecordTables { envelope, ..Default::default() }
    }

    #[test]
    fn the_level_byte_takes_the_envelope_the_table_and_the_curve() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, volume_table_len: 3, ..PatchRecord::from_codes(&[]) };
        let t = RecordTables { volume: Some(vec![127, 64, -10]), ..tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()) };
        let mut v = Voice::new(clip(r, t.clone()), resample::PITCH_UNIT, 100.0 / 127.0, false, 0);
        v.level = 99;
        // 127 · 100 · 99 · 127 / 127³ = 77.95 → 77, then · 127 / 127
        assert_eq!(v.level_byte(), 77);
        v.volume_phase = 1;
        assert_eq!(v.level_byte() as i32, 77 * 64 / 127);
        v.volume_phase = 2;
        assert_eq!(v.level_byte() as i32, (77 * -10) / 127);
        // a curve maps the byte: entry `level + 128`
        let mut curve = vec![0u8; 256];
        curve[128 + 77] = 120;
        let t = RecordTables { volume: None, curve: Some(curve), ..t };
        let mut v = Voice::new(clip(r, t), resample::PITCH_UNIT, 100.0 / 127.0, false, 0);
        v.level = 99;
        assert_eq!(v.level_byte(), 120);
    }

    #[test]
    fn the_service_walks_the_tables_once_a_tick() {
        // a pitch table of 3 entries (depth 128: two cents a step from 0x40) and a volume table of 2
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, volume_table_len: 2, pitch_table_len: 3, pitch_depth: 128, ..PatchRecord::from_codes(&[]) };
        let t = RecordTables { volume: Some(vec![127, 0]), pitch: Some(vec![0x40, 0x40 + 50, 0x40 - 50]), ..tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(0), Voice::new(clip(r, t), resample::PITCH_UNIT, 1.0, false, 0)));
        let phases = |s: &Shared| (s.voices[0].1.volume_phase, s.voices[0].1.pitch_phase);
        assert_eq!(phases(&s), (0, 0));
        // the first render runs the service before its first frame
        s.render(1, 22050);
        assert_eq!(phases(&s), (1, 1));
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(100));
        assert_eq!(s.voices[0].1.level_byte(), 0);
        s.render(207 + 1, 22050);
        assert_eq!(phases(&s), (0, 2));
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(-100));
        s.render(224, 22050);
        assert_eq!(phases(&s), (1, 0));
        assert_eq!(s.voices[0].1.pitch_word(), 4096);
    }

    #[test]
    fn a_run_out_envelope_stops_the_group() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(3), Voice::new(clip(r, tables(vec![(1, 127)])), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices.push((VoiceId(3), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        // tick 1: one tick left → 0; tick 2: past the only segment → 0x565D60 on the handle
        s.render(1, 22050);
        assert_eq!(s.voices.len(), 2);
        s.render(208, 22050);
        assert!(s.voices.is_empty());
    }

    #[test]
    fn a_release_is_a_five_tick_ramp_below_zero() {
        use super::{Ramp, RELEASE_TARGET, RELEASE_TICKS};
        // 0x565DD0(h, 5, −1) from 127: step (−0x10000 − 0x7F0000) / 5 toward zero
        let mut v = 127 << 16;
        let mut r = Ramp::start(v, RELEASE_TICKS, RELEASE_TARGET);
        assert_eq!(r, Ramp { step: -1677721, goal: -0x10000 });
        let mut words = Vec::new();
        let mut stopped = false;
        for _ in 0..5 {
            stopped = r.tick(&mut v);
            words.push(v >> 16);
        }
        assert_eq!(words, [101, 75, 50, 24, -1]);
        assert!(stopped);
        assert_eq!(v, -65533);
        // from 40 the fifth tick lands past the goal: clamped to it, the step 0, still below 0
        let mut v = 40 << 16;
        let mut r = Ramp::start(v, 5, -1);
        assert!(!(0..4).any(|_| r.tick(&mut v)));
        assert!(r.tick(&mut v));
        // ticks ≤ 0 are 1 (0x565E03); an upward ramp stops at its goal and does nothing more
        let mut v = 0;
        let mut r = Ramp::start(v, 0, 100);
        assert!(!r.tick(&mut v));
        assert_eq!((v, r.step), (100 << 16, 0));
        assert!(!r.tick(&mut v));
    }

    #[test]
    fn a_released_voice_holds_its_budget_until_the_ramp_stops_it() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(4), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices.push((VoiceId(5), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.ramp(VoiceId(4), super::RELEASE_TICKS, super::RELEASE_TARGET);
        // ticks 1…4 fade it; the fifth (0x589611) stops the handle
        s.render(1, 22050);
        assert_eq!(s.voices[0].1.vol16 >> 16, 101);
        s.render(207 + 224 * 2 + 1, 22050);
        assert_eq!(s.voices.len(), 2);
        assert_eq!(s.voices[0].1.vol16 >> 16, 24);
        // tick 4 is 224 frames, one of them mixed
        s.render(223, 22050);
        assert_eq!(s.voices.len(), 2);
        s.render(1, 22050);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].0, VoiceId(5));
        // a setVolume (0x5662B0) cancels a ramp: the step zeroed with the new volume
        s.ramp(VoiceId(5), 5, -1);
        s.voices[0].1.set_volume(0.5);
        assert_eq!(s.voices[0].1.ramp.step, 0);
        assert_eq!(s.voices[0].1.vol16, 64 << 16);
    }

    #[test]
    fn the_pause_silences_the_table_but_not_a_released_voice() {
        // 0x4AB150 → 0x566340(h, 0) over 0x656180…0x6563C8: a released handle is not there any
        // more (the release forgets it), so its ramp runs on; every other voice goes to 0, its
        // ramp step zeroed (0x566303…0x566306), and keeps running
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        for id in 1..=3 {
            s.voices.push((VoiceId(id), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        }
        s.ramp(VoiceId(1), super::RELEASE_TICKS, super::RELEASE_TARGET);
        // an upward ramp (a stream's fade-in shape) is a table voice's: cancelled
        s.ramp(VoiceId(3), 5, 100);
        s.silence();
        assert_eq!(s.voices[0].1.vol16, 127 << 16);
        assert_eq!(s.voices[0].1.ramp.step, -1677721);
        assert_eq!((s.voices[1].1.vol16, s.voices[1].1.ramp.step), (0, 0));
        assert_eq!((s.voices[2].1.vol16, s.voices[2].1.ramp.step), (0, 0));
        // the released one ramps out and stops; the silenced ones still play
        s.render(22050, 22050);
        assert_eq!(s.voices.iter().map(|(i, _)| i.0).collect::<Vec<_>>(), [2, 3]);
        // the next update's setVolume brings a loop back at its level
        s.voices[0].1.set_volume(0.5);
        assert_eq!(s.voices[0].1.vol16, 64 << 16);
    }

    #[test]
    fn the_pause_leaves_an_untabled_voice_sounding() {
        // 0x4AB150 walks only 0x656180…0x6563C8; a bare Voice_start 0x565E70 voice — the HUD's
        // fe_sound, the front end's fesfx, the collision one-shots (voice 0x33), the checkpoint
        // chime / wrong-way cue / pylon-hit cue and the cop siren's start — is never in it
        // (`RE/notes/sfx.md` "Audit 2026-09-27, audio port 11")
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(1), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices[0].1.table = false;
        s.voices.push((VoiceId(2), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.silence();
        assert_eq!(s.voices[0].1.vol16, 127 << 16, "not in the table: untouched");
        assert_eq!(s.voices[1].1.vol16, 0, "in the table: silenced");
    }

    #[test]
    fn the_strip_hook_is_a_one_pole_low_pass_at_the_value_in_hz() {
        use super::{voice_play_filter, LowPass};
        // 0x596AC0: 3000 / (rate · 0.5); 0xFFFF is ≥ 1 at every mix rate — no node
        assert_eq!(LowPass::coefficient(0xBB8, 22050), Some((3000.0f64 / 11025.0) as f32));
        assert_eq!(LowPass::coefficient(0xBB8, 32000), Some(0.1875));
        assert_eq!(LowPass::coefficient(0xFFFF, 32000), None);
        assert_eq!(LowPass::coefficient(16000, 32000), None);
        // the node starts at 0 and follows `y = y·(1 − f) + f·x` (0x59C170)
        let mut n = None;
        LowPass::set(&mut n, 0xBB8, 32000);
        let lp = n.as_mut().unwrap();
        assert_eq!((lp.a, lp.b), (0.8125, 0.1875));
        lp.begin_block();
        assert_eq!(lp.run(0, 1000.0), 187.5);
        assert_eq!(lp.run(0, 1000.0), 187.5 * 0.8125 + 187.5);
        lp.end_block();
        assert_eq!(lp.y[1], 0.0);
        // a new value keeps the state; 0xFFFF takes the node away (0x596D10)
        LowPass::set(&mut n, 0xBB8, 22050);
        assert_eq!(n.unwrap().y[0], 187.5 * 0.8125 + 187.5);
        LowPass::set(&mut n, 0xFFFF, 22050);
        assert!(n.is_none());
        // 0x4A9FA8…0x4AA03B: the player's engine banks and AUDIOQUALITY ≥ 2 are left alone
        assert_eq!(voice_play_filter(true, 0, true), None);
        assert_eq!(voice_play_filter(false, 2, true), None);
        assert_eq!(voice_play_filter(false, 1, true), Some(0xBB8));
        assert_eq!(voice_play_filter(false, 0, false), Some(0xFFFF));
    }

    #[test]
    fn the_low_pass_keeps_y_in_the_fpu_register_inside_a_block() {
        use super::LowPass;
        // 0x59C170: `y` is loaded once (`fld dword [edi]`), every sample is computed in the
        // register's 53-bit precision (only the handed-on sample is an f32, `fst dword`), and
        // `fstp dword [edi]` rounds `y` to f32 at the block's end
        let mut n = None;
        LowPass::set(&mut n, 1234, 22050);
        let lp = n.as_mut().unwrap();
        let (a, b) = (lp.a as f64, lp.b as f64);
        let xs = [1000.37f32, -733.91, 12345.6, 0.001, 77.7];
        let mut y = 0.0f64;
        lp.begin_block();
        for &x in &xs {
            y = y * a + b * x as f64;
            assert_eq!(lp.run(0, x), y as f32);
        }
        lp.end_block();
        assert_eq!(lp.y[0], y as f32);
        // the next block starts from the f32, not from the register
        lp.begin_block();
        let y2 = (y as f32 as f64) * a + b * 5.0f64;
        assert_eq!(lp.run(0, 5.0), y2 as f32);
    }

    #[test]
    fn a_hook_call_raises_the_ramp_flag_even_for_an_unchanged_value() {
        use crate::resample::Gains;
        // 0x598721: `mov byte [eax+1], 1` before any compare
        let mut g = Gains::default();
        g.touch();
        assert!(!g.pending, "the voice's first set is taken at once, flag clear (0x598630)");
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        assert!(g.ramp(256).is_none());
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        assert!(g.ramp(256).is_none(), "no event, no ramp");
        g.touch();
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        let r = g.ramp(256).expect("a hook call ramps");
        assert_eq!((r.len, r.step, r.vol_step), (32, [0.0, 0.0], 0.0));
    }

    #[test]
    fn a_filtered_voice_mixes_its_low_passed_samples() {
        use super::LowPass;
        // a square wave at the Nyquist rate (step 0x10000): the one-pole filter smooths it
        let alt = Arc::new(Clip { samples: (0..4000).map(|i| if i % 2 == 0 { 8000 } else { -8000 }).collect(), rate: 22050, ..(*clip(PatchRecord::from_codes(&[]), tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()))).clone() });
        let peak = |filter: bool| {
            let mut s = Shared::default();
            s.interp = crate::resample::Interp::Linear;
            let mut v = Voice::new(alt.clone(), resample::PITCH_UNIT, 1.0, true, 0);
            if filter {
                LowPass::set(&mut v.lowpass, 0xBB8, 22050);
            }
            s.voices.push((VoiceId(1), v));
            s.render(2000, 22050)[1000..].iter().map(|f| f[0].abs()).fold(0.0f32, f32::max)
        };
        let (open, closed) = (peak(false), peak(true));
        assert!(open > 0.05, "{open}");
        assert!(closed < open * 0.25, "{closed} vs {open}");
    }
}

#[cfg(test)]
mod rt_tests;
