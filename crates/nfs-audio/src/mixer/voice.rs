//! The voice: ramp, envelope, low-pass, cents, the start / service math of 0x565xxx / 0x5898F0 / 0x58FBC0.

use super::*;

/// The port's `byte / 127` volume back to the library's 16.16 `voice+0x2C`: `round(volume ·
/// 127)` (exact for every caller's byte, all in 0…127; clamped there) `<< 16`.
pub(super) fn volume_16(volume: f32) -> i32 {
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

pub(super) struct Voice {
    pub(super) clip: Arc<Clip>,
    /// where the resampler is in the voice's input stream ([`resample::Cursor`]) — for a loop
    /// the stream runs on past the end (`0…hi`, then `lo…hi` again, the looped decoder 0x59CBA0)
    pub(super) cur: Cursor,
    /// the channel gains and their ramp (0x598700 / 0x5980C0)
    pub(super) gains: Gains,
    /// a call of the strip's volume or pan hook (0x5969C0 / 0x596890 → 0x598700) since the last
    /// block: 0x598700 raises the ramp flag `[node+1]` on **every** call, changed value or not
    /// (0x598721), so the block's pull runs the ramp ([`Gains::touch`]). Raised by the service tick
    /// (0x589699), `setVolume` on a changed volume (0x56631C) and the pan call (0x5661F0 → 0x590CC0).
    pub(super) hook: bool,
    /// `voice+0x6C`, the caller's pitch word: 4096 is 1.0, unchanged ([`resample::PITCH_UNIT`]) —
    /// every start/update call hands it directly ([`AudioOutput::play_word`] / `set_word`, audio
    /// port 8; there is no float pitch left in the port, matching the original, which never took
    /// one either).
    pub(super) word: u16,
    /// `voice+0x2C`, the caller's volume in 16.16 (`setVolume`'s int `<< 16`, 0x5662E3; the
    /// request's `+8 << 16` at the start, 0x59055D) — the level byte takes `>> 16`
    pub(super) vol16: i32,
    /// the release ramp `voice+0x24` / `+0x28` ([`Ramp`], 0x565DD0)
    pub(super) ramp: Ramp,
    /// the caller's pan byte `voice+0x1F` ([`pan_byte`]; 0x40, the request's default from
    /// 0x561960, until one is given) — the patch's pan is applied at the mix ([`voice_pan`])
    pub(super) pan: u8,
    pub(super) looping: bool,
    pub(super) finished: bool,
    /// `voice+0x45`: the request's `+0xA`, 0x7F unless the caller passes an aux byte — 0x561960
    /// fills every request with 0x7F (0x56199F) and 0x4A9A5F writes `0x7F − aux` for the
    /// collision one-shots (slots from 0x30). 0x5906D7 copies it into the voice.
    pub(super) fx_scale: u8,
    /// the channel's reverb send `+4` (0x599800; `None` until the voice's start has set it) and
    /// its wet gain `+8` (0x5980A0)
    pub(super) send: Option<f32>,
    pub(super) wet: f32,
    /// `voice+0x20` at the start (0x589B54): [`Shared::service_tick`] `[0x6B49A8]` as it stood at
    /// the start — the library service's own tick count, not a start order. *(Until 2026-09-27
    /// this was the port's `VoiceId`, its own start order — see [`pick_victims`].)*
    pub(super) stamp: u32,
    /// `voice+0x3E`, the patch pan with its spread drawn at the start ([`voice_pan`])
    pub(super) patch_pan: u8,
    /// `voice+0x3C`: the patch volume with its spread, times the request's byte `+6` over 127
    /// (0x5905D0) — a factor of the voice's level byte ([`Voice::level_byte`], 0x589E1C)
    pub(super) level: i8,
    /// `voice+0x68`, the voice's own cents: [`base_cents`] plus the draws of codes 0x24 / 0x11
    /// (0x59052E, 0x590549)
    pub(super) cents: i16,
    /// the envelope (`voice+0x30`…`+0x41`, [`Envelope`])
    pub(super) env: Envelope,
    /// `voice+0x4A` / `+0x4B`: the steps of the volume and pitch tables (drawn at the start,
    /// stepped by the service)
    pub(super) volume_phase: u8,
    pub(super) pitch_phase: u8,
    /// the resampler has made its first pull (0x59BE00's `+0x1C`)
    pub(super) pulled: bool,
    /// `voice+0x1B`: the last record started by a patch start of more than one (0x590A5C) — the
    /// voice whose handle the start answers; it outlives the group's others ([`Shared::free`])
    pub(super) anchor: bool,
    /// the anchor's state 2 (0x589CF8): its own sample has ended, and it holds its voice of the
    /// budget until the last other voice of the group ends; it mixes nothing
    pub(super) held: bool,
    /// `voice+0x3F`, the caller's **bend byte**: the request's `+9` at the start (0x5905FE;
    /// 0x561960's default 0x40), `Voice_setBend` 0x5660C0 → 0x566020 after (the byte, then the
    /// pitch word made again). 0x589F75…0x589F90 add `((i8)byte − 0x40) · (i16)voice+0x66 >> 6`
    /// cents ([`Clip::bend_cents`], code 0x0A · 100) to the voice's cents ([`Voice::pitch_word`]).
    pub(super) bend: i8,
    /// the software strip's one-pole low-pass (0x5998E0's node, [`LowPass`]), one per channel
    pub(super) lowpass: Option<LowPass>,
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
    pub(super) table: bool,
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
pub(super) struct Envelope {
    pub(super) level: i32,
    pub(super) step: i32,
    pub(super) left: i32,
    pub(super) seg: i8,
    pub(super) segments: i8,
}

impl Envelope {
    /// The start: `level = (i8)code 0x1C << 16`, the first segment's ticks (negative →
    /// 0x7FFFFFFF) and step `((target << 16) − level) / ticks` (`idiv`, toward zero).
    pub(super) fn start(level: i8, segments: i8, table: &[(i32, i32)]) -> Self {
        let mut e = Envelope { level: (level as i32) << 16, step: 0, left: 0, seg: 0, segments };
        e.load(table.first().copied().unwrap_or((0x7FFF_FFFF, 0)));
        e
    }

    pub(super) fn load(&mut self, (ticks, target): (i32, i32)) {
        self.left = if ticks < 0 { 0x7FFF_FFFF } else { ticks };
        // a segment of 0 ticks divides by zero in the original; no shipped table has one
        self.step = if self.left == 0 { 0 } else { target.wrapping_shl(16).wrapping_sub(self.level) / self.left };
    }

    /// One service tick: the level takes its step; with no tick left the next segment is loaded
    /// — or, past the last (`jl`, signed), the voice is to be stopped (`true`); then a tick is
    /// counted off (`dec`). The step is taken before the count is looked at, so a segment runs
    /// its ticks plus one.
    pub(super) fn tick(&mut self, table: &[(i32, i32)]) -> bool {
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
pub(super) fn div_127_cubed(x: i32) -> i32 {
    let q = ((x as i64 * 0x4186_143D) >> 32) as i32 >> 19;
    q + ((q as u32) >> 31) as i32
}

/// `x / 127` as 0x589E53…0x589E64 makes it: `imul` by 0x81020409, `add edx, x`, `sar 6`, the
/// sign bit added back — toward zero.
pub(super) fn div_127(x: i32) -> i32 {
    let q = (((x as i64 * 0x8102_0409u32 as i32 as i64) >> 32) as i32).wrapping_add(x) >> 6;
    q + ((q as u32) >> 31) as i32
}

/// 0x5E1D3C…0x5E1E3B, the image's 256 bytes 0x589E80 reads: the mantissa of `2^(i/256)`, less
/// one, in 256ths (not exactly any rounding of it — the bytes as they are).
pub(super) const CENTS_MANTISSA: [u8; 256] = [
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
    pub(super) fn new(clip: Arc<Clip>, word: u16, volume: f32, looping: bool, stamp: u32) -> Self {
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
    pub(super) fn set_volume(&mut self, volume: f32) {
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
    pub(super) fn level_byte(&self) -> i8 {
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
    pub(super) fn pitch_word(&self) -> u16 {
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
    pub(super) fn range(&self) -> (usize, usize) {
        let nframes = self.clip.samples.len() / self.clip.channels.max(1) as usize;
        match (self.looping, self.clip.loop_range) {
            (true, Some((a, b))) if b > a && (b as usize) <= nframes => (a as usize, b as usize),
            _ => (0, nframes),
        }
    }

    /// The frame of the sample a stream index `k` reads: `k` up to the end, then round the loop;
    /// `None` before the start (the resampler's zeroed history) and past the end of a one-shot
    /// (the decoder's zero fill, 0x597BF2).
    pub(super) fn frame_of(&self, k: i64) -> Option<usize> {
        let (lo, hi) = self.range();
        Self::frame_in(lo, hi, self.looping, k)
    }

    /// [`frame_of`](Self::frame_of) for a range taken once (`mix_block` reads it per block, not
    /// per tap).
    #[inline]
    pub(super) fn frame_in(lo: usize, hi: usize, looping: bool, k: i64) -> Option<usize> {
        if k < 0 {
            return None;
        }
        let k = k as usize;
        if k < hi {
            Some(k)
        } else if looping && hi > lo {
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
    pub(super) fn pull_fails(&self, step: u32, n: usize, interp: Interp) -> bool {
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
    pub(super) fn position(&self) -> f64 {
        let whole = self.frame_of(self.cur.pos as i64).unwrap_or(self.cur.pos as usize);
        whole as f64 + self.cur.frac as f64 / 65536.0
    }
}

/// **0x589D70**: a voice's two effect sends are `e = voice+0x45 · voice+0x44` (both signed bytes)
/// times each environment bus over 127·127 — `0x82061029` then `add edx`, `sar 13` and the sign
/// bit added back: a divide by 16129 toward zero — stored as the bytes `voice+0x46` (bus `+0xC`,
/// `Mixer_setEnvParam(0x20, ·)`) and `voice+0x47` (bus `+0xD`, `(0x80, ·)`), set by 0x565CD0.
/// Recomputed at the voice start (0x5906DA) and on every bus change (0x565D28…0x565D51).
pub(super) fn fx_send(fx_scale: u8, patch: u8, bus: i8) -> i32 {
    (fx_scale as i8 as i32 * patch as i8 as i32) * bus as i32 / 16129
}

/// The software path's reverb send for a voice (`[drv+0xD8]` = 0x596A10 when the voice start's
/// flag 0x10 is clear, 0x58A455…0x58A468): the byte `voice+0x46` as an int times `[0x5C1788]`
/// (an f32 1/127, `fild ; fmul ; fstp`, 0x596A1F…0x596A2E), which 0x599800 stores as the channel's
/// `+4` (0x596A3B). The DirectSound path (flag 0x10, 0x596400) sends `voice+0x47 / 127` — the
/// `0x80` bus — as the EAX buffer reverb mix (property set {4A4E6FC0-C341-11D1-B73A-
/// 444553540000}) instead. The game never takes that path: its only voice type is 0x24 (see
/// `pan_gains`).
pub(super) fn reverb_input(fx_scale: u8, patch: u8, bus_c: i8) -> f32 {
    (fx_send(fx_scale, patch, bus_c) as f64 * (1.0f32 / 127.0) as f64) as f32
}

/// The channel's wet gain `+8` (0x5980A0): `volume · send · 1.5` (`[0x5B246C]`), the volume being
/// the channel's *current* `+0xC` — recomputed at every send change (0x599800) and, while a gain
/// ramp runs, at every sample (0x5981CA).
pub(super) fn wet_gain(vol_cur: f32, send: f32) -> f32 {
    (vol_cur as f64 * send as f64 * 1.5) as f32
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
