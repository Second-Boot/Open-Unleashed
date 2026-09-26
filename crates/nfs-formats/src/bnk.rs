//! `BNKl` sample banks, the EA `PT` sample header and the EA-XA ADPCM codec.
//!
//! Status: **CONFIRMED** (`RE/notes/audio.md` §1–2): the `PT` TLV parser is `0x590F70`
//! (iterator `0x599760`, big-endian values via `0x59BD90`), the bank loader checks version 4 at
//! `0x4AFA3D`, the EA-XA decoders are `0x59A5A0` (x87) / `0x59A660` (SSE) / `0x59A730` (3DNow)
//! with the shift table built at `0x59B8D0` and coefficients at `0x5E3B28` / `0x5E3B38`.
//! Cross-checked against every bank in `Sounds/*.viv`, `Sounds/*.bnk` and `Speech/zzzxeng.viv`.
//!
//! ```text
//! BNKl bank
//! +0   "BNKl"        +4  u8 version (=4)     +5 u8 0         +6 u16 slot count
//! +8   u32 dir size (offset of first sample data)            +12 u32 data size
//! +16  u32 (always 0 in shipped data; role unknown)
//! +20  u32 offset[count]  — 0 = empty slot; else PT header at (0x14 + 4·i) + offset (self-relative)
//! sample data: absolute file offsets from PT code 0x88 (+ch)
//!
//! PT header
//! "PT" u16 platform, then TLV { u8 code, u8 size, BE value[size] } (size 0xFF => u32 BE size follows);
//! 0xFC = skip byte, 0xFD / 0xFE = section markers (no size), 0xFF = end.
//! ```
//!
//! Deviations from the notes, established on the shipped data (see the tests in `tests/audio.rs`):
//! * The `data size` field at +12 is **not** validated: in `Speech/zzzxeng.viv` it is unrelated to
//!   the real bank length (the BIGF entry is smaller than `dir + data`), while it matches exactly
//!   in every `Sounds/` bank. It is exposed as [`Bnk::data_size`].
//! * Speech banks omit the trailing partial EA-XA frame: 132 of 154 banks store `floor(n/28)`
//!   frames for `n` samples (1..27 samples missing). The decoder pads those samples with silence
//!   and reports the count in [`PcmSample::padded_tail`]; a shortfall of a whole frame or more is
//!   an error.

use crate::{u16_le, u32_le, FormatError};

/// Shift table entry, `0x59B8D0`: `(signext4(nib) << 12) >> shift` (arithmetic shift).
#[inline]
fn ea_xa_table(shift: u8, nib: u8) -> i32 {
    let s = ((nib as i32) << 28) >> 28; // sign-extend the 4-bit nibble
    (s << 12) >> shift
}

/// Predictor coefficients (`0x5E3B28`, `0x5E3B38`) = {0,240,460,392}/256 and {0,0,−208,−220}/256.
pub const EA_XA_K0: [f64; 4] = [0.0, 0.9375, 1.796875, 1.53125];
pub const EA_XA_K1: [f64; 4] = [0.0, 0.0, -0.8125, -0.859375];

/// Bytes per EA-XA frame and samples per frame (`0x590EF0`: k = 0x88/256 ≈ 15/28 bytes per sample).
pub const EA_XA_FRAME_BYTES: usize = 15;
pub const EA_XA_FRAME_SAMPLES: usize = 28;

/// EA-XA predictor history (`s[n-1]`, `s[n-2]`).
///
/// Kept as floats without intermediate rounding, as the notes describe the exe decoder
/// ("float, no rounding"); only the emitted samples are rounded and saturated to `i16`.
/// Whether the exe saturates the history itself is not established (HYPOTHESIS: it does not).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EaXaState {
    pub s1: f64,
    pub s2: f64,
}

impl EaXaState {
    /// History seeded from two stored `i16` samples (`SCDl` blocks: `s[n-1]` first, then `s[n-2]`).
    pub fn seeded(s1: i16, s2: i16) -> Self {
        Self { s1: s1 as f64, s2: s2 as f64 }
    }
}

#[inline]
fn saturate(v: f64) -> i16 {
    // fistp-style round-to-nearest then clamp; the clamp only matters for corrupt/hostile input.
    v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

/// Decode one 15-byte EA-XA frame (28 samples, high nibble first) into `out`.
///
/// `hdr` byte: hi nibble = predictor 0..3, lo nibble = shift 0..15;
/// `s = tbl[shift][nib] + K0[p]·s1 + K1[p]·s2`.
pub fn decode_ea_xa_frame(frame: &[u8; EA_XA_FRAME_BYTES], state: &mut EaXaState, out: &mut Vec<i16>) {
    let predictor = (frame[0] >> 4) as usize;
    let shift = frame[0] & 0x0F;
    let (k0, k1) = (EA_XA_K0[predictor], EA_XA_K1[predictor]);
    for &byte in &frame[1..] {
        for nib in [byte >> 4, byte & 0x0F] {
            let s = ea_xa_table(shift, nib) as f64 + k0 * state.s1 + k1 * state.s2;
            out.push(saturate(s));
            state.s2 = state.s1;
            state.s1 = s;
        }
    }
}

/// Decode `count` samples of one EA-XA channel starting at `data[off]`.
///
/// Returns the number of trailing samples that had to be padded with silence because the data
/// ends at a frame boundary before the last, partial frame (speech banks; see module docs).
/// Fails with [`FormatError::Truncated`] when a whole frame or more is missing.
pub fn decode_ea_xa(
    data: &[u8],
    off: usize,
    count: usize,
    state: &mut EaXaState,
    out: &mut Vec<i16>,
    what: &'static str,
) -> Result<usize, FormatError> {
    let frames_needed = count.div_ceil(EA_XA_FRAME_SAMPLES);
    let avail = data.len().saturating_sub(off);
    let frames_avail = (avail / EA_XA_FRAME_BYTES).min(frames_needed);
    let missing = count.saturating_sub(frames_avail * EA_XA_FRAME_SAMPLES);
    if missing >= EA_XA_FRAME_SAMPLES || (missing > 0 && !avail.is_multiple_of(EA_XA_FRAME_BYTES)) {
        return Err(FormatError::Truncated { what, offset: off + frames_avail * EA_XA_FRAME_BYTES, needed: EA_XA_FRAME_BYTES });
    }
    let start = out.len();
    for f in 0..frames_avail {
        let p = off + f * EA_XA_FRAME_BYTES;
        let frame: &[u8; EA_XA_FRAME_BYTES] = data[p..p + EA_XA_FRAME_BYTES].try_into().unwrap();
        decode_ea_xa_frame(frame, state, out);
    }
    out.truncate(start + count - missing);
    out.resize(start + count, 0);
    Ok(missing)
}

/// Sample codec, resolved from PT codes 0xA0 / 0x83 (see [`PtHeader::codec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// EA-XA ADPCM, 15-byte frames of 28 samples (codec byte 0x0A or 3; class 7).
    EaXa,
    /// Raw little-endian `i16` (codec byte 8 or 0; class 0 with type hint 0x10).
    Pcm16,
    /// Raw signed 8-bit (codec byte 9 or 2; class 8).
    Pcm8,
    /// Low-rate codec 4 (`0x59A800`, ≈1.6 bit/sample); unused in the shipped data, not decoded.
    Unknown4,
}

/// Decoded, interleaved PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmSample {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved samples, `channels * samples_per_channel` long.
    pub samples: Vec<i16>,
    /// `(loop start, loop end)` in samples when the PT header carries a non-empty loop
    /// (0x86 < 0x87); one-shot banks store both as size-0 values (0, 0), which is not a loop.
    pub loop_range: Option<(u32, u32)>,
    /// Samples per channel that were filled with silence because the encoded data stopped short
    /// of the declared count by less than one frame (speech banks). 0 for a complete clip.
    pub padded_tail: u32,
    /// The patch's own volume, `PtHeader::patch_volume() / 127` (`audio.md` §2). The library
    /// multiplies the caller's level by it, so a clip is never played at more than this.
    pub gain: f32,
    /// `voice+0x66`: code 0x0A · 100 cents (0x590604) — how far the caller's **bend byte** moves
    /// the pitch: `(byte − 0x40) · bend_cents >> 6` cents (0x589F75…0x589F90).
    pub bend_cents: i32,
    /// `voice+0x44`: code 0x13, default 0 (0x599188 clears its bucket, `[esp+0x74]`) — the patch's
    /// **effect send**. 0x589D70 multiplies it by `voice+0x45` and by each environment bus level
    /// (`[0x6B4A0C]` / `[0x6B4A0D]`, set by 0x565CD0 — the reverb of `engine-audio.md` §4.7) over
    /// 127·127 into the voice's two send bytes `+0x46` / `+0x47`, which the driver is handed
    /// (0x58A43E / 0x58A45A). A patch without it is never in the reverb.
    pub fx_send: u8,
    /// The voice allocator's priority: code 0x06, default 0 (the parser's bucket `[esp+0x3C]`
    /// cleared at 0x59919E), taken as a **byte** into the layer record `+0x0C` (0x5993C2) and handed
    /// to 0x5898F0 (0x5904C9), which stores it at `voice+0x1A`. A voice below 0x65 can be stolen
    /// when the budget is full, the lowest first (`nfs_audio::mixer::pick_victims`).
    pub priority: u8,
    /// The patch's pan byte: code 0x0C, default 64 (0x5991B2), a byte into layer `+0x11`
    /// (0x5993E9), read signed and clamped to 0…127 by 0x590470; the library adds it, less 0x40,
    /// to the caller's pan byte (0x590582) — `voice+0x3E`.
    pub pan: u8,
    /// The patch's pan scale: code 0x25, default 1 (0x59922A), a byte into layer `+0x1E`
    /// (0x5994DA) → `voice+0x43`: unless 1, the caller's pan byte becomes `(p − 0x40)·scale + 0x40`
    /// before the patch pan is added (0x590564…0x59057E, 0x566171…0x566187).
    pub pan_scale: i8,
    /// The patch's records ([`PtHeader::patch_records`]) — what the library's patch start
    /// 0x5907B0 / 0x590420 walks, the random draws of `nfs_audio::librand` included.
    pub records: Vec<PatchRecord>,
    /// Each record's own sample for a patch of more than one ([`PtHeader::parts`]), in record
    /// order; empty for a one-record patch, whose sample is this one.
    pub parts: Vec<PcmSample>,
    /// The records' tables ([`PtHeader::tables`]), in record order.
    pub tables: Vec<RecordTables>,
}

impl PcmSample {
    pub fn samples_per_channel(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// EA `PT` sample/patch header (TLV). Unknown codes are kept verbatim, never dropped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PtHeader {
    /// `u16` after "PT"; 0 = PC in all shipped files.
    pub platform: u16,
    /// Code 0x80 ("split"/count byte, `state+7`); always 1 in shipped data; absent in speech banks.
    pub split: Option<u32>,
    /// Code 0x81 type hint; the exe defaults it to 0x10 when absent.
    pub type_hint: Option<u32>,
    /// Code 0x82; defaults to 1.
    pub channels: u16,
    /// Code 0x83 codec class (speech banks: 7), consulted only when 0x80 is absent.
    pub codec_class: Option<u32>,
    /// Code 0x84; defaults to 22050.
    pub sample_rate: u32,
    /// Code 0x85 samples per channel.
    pub sample_count: Option<u32>,
    /// Codes 0x86 / 0x87 loop start / end sample (banks only).
    pub loop_start: Option<u32>,
    pub loop_end: Option<u32>,
    /// Codes 0x88 / 0x89: absolute file offset of channel 0 / 1 data (stereo is planar).
    pub data_offset: [Option<u32>; 2],
    /// Codes 0x8A / 0x8B: per-channel value, always 0 in shipped data; role unknown.
    pub unknown_8a: [Option<u32>; 2],
    /// Codes 0x8C / 0xA1 flag bytes (`state+8`).
    pub flags_8c: Option<u32>,
    pub flags_a1: Option<u32>,
    /// Code 0xA0 codec byte; the exe defaults it to 0x0A (EA-XA).
    pub codec_byte: Option<u32>,
    /// Patch parameters, codes < 0x80, in file order; see the table in `RE/notes/audio.md` §2 and
    /// the accessors below. Codes that are absent take the parser's defaults (0x599188).
    pub patch_params: Vec<(u8, u32)>,
    /// Any other code >= 0x80 with its raw big-endian value bytes, in file order.
    pub unknown: Vec<(u8, Vec<u8>)>,
    /// The patch's records as the library's parser 0x599100 buckets them: a new record after
    /// every 0xFE (0x599388 → `ebx = 1`, "more follow"; 0x5907B0 parses them one by one until the
    /// 0xFF), each holding its codes below 0x26 (0x5992B6) with **0x59BD90's** value: big-endian,
    /// sign-extended from 1, 2 or 3 bytes (`sub eax, 0x100` / `0x10000` / `0x1000000` past
    /// 0x7F / 0x7FFF / 0x7FFFFF), 4 bytes as they are. `patch_params` keeps them unsigned and
    /// merged. See [`PtHeader::patch_records`].
    pub records: Vec<Vec<(u8, i32)>>,
    /// Each record's tables ([`RecordTables`]), in record order, read from the bank at the
    /// self-relative pointers of codes 0x12, 0x19, 0x1D and 0x20 (`&value + value`, 0x599402…
    /// 0x5994A5 — the walker's `+0xC` is the value's address, 0x5997D6).
    pub tables: Vec<RecordTables>,
    /// Each record as a header of its own, for a patch of more than one record (empty
    /// otherwise): 0x5907B0 parses the records one at a time (0x599100 per record, its defaults
    /// written afresh each call, 0x599188…0x599299, the codes from 0x80 included), so every
    /// record has its own sample — its own count, offsets, rate and channels — and 0x590420
    /// starts each as a voice of its own. The merged fields above are the last record's.
    pub parts: Vec<PtHeader>,
    /// Whether the 0xFD / 0xFE section markers were seen.
    pub saw_fd: bool,
    pub saw_fe: bool,
    /// Header length in bytes including the terminating 0xFF.
    pub len: usize,
}

impl PtHeader {
    fn param(&self, code: u8, default: u32) -> u32 {
        self.patch_params.iter().rev().find(|(c, _)| *c == code).map(|(_, v)| *v).unwrap_or(default)
    }

    /// Code 0x0E, default 127: the patch's volume. `0x5905A5` builds the voice's level as
    /// `(volume + rand(volume_spread)) · callerLevel / 127`.
    pub fn patch_volume(&self) -> u32 {
        self.param(0x0E, 127)
    }
    /// Code 0x0F, default 0: a random amount added to the volume each time the patch starts.
    pub fn volume_spread(&self) -> u32 {
        self.param(0x0F, 0)
    }
    /// Code 0x0C, default 64 (centre): the patch's pan, offset against the caller's (0x590447).
    pub fn patch_pan(&self) -> u32 {
        self.param(0x0C, 64)
    }
    /// Code 0x0D, default 0: a random amount added to the pan.
    pub fn pan_spread(&self) -> u32 {
        self.param(0x0D, 0)
    }
    /// Code 0x25, default 1 (0x59922A): the scale of the caller's pan byte about 0x40 (layer
    /// `+0x1E` → `voice+0x43`, 0x590506). Only `fesfx.bnk` / `lilfesfx.bnk` carry it (4, one slot).
    pub fn pan_scale(&self) -> u32 {
        self.param(0x25, 1)
    }
    /// Code 0x07, default 60: the key the sample was recorded at; the playback pitch is
    /// `(key − root)·100 + fine_tune() + rand(pitch_spread())` cents (0x5904EC…0x590549).
    pub fn root_key(&self) -> u32 {
        self.param(0x07, 60)
    }
    /// Code 0x10, default 0 — cents.
    pub fn fine_tune(&self) -> u32 {
        self.param(0x10, 0)
    }
    /// Code 0x0A, default 0 — semitones, a signed byte: **not** part of the base pitch but the range
    /// of the caller's bend byte (`voice+0x66 = value · 100`, 0x5905EF…0x59060D; used at 0x589F75).
    pub fn coarse_tune(&self) -> u32 {
        self.param(0x0A, 0)
    }
    /// Code 0x11, default 0 — cents.
    pub fn pitch_spread(&self) -> u32 {
        self.param(0x11, 0)
    }
    /// Code 0x13, default 0: the effect send (`PcmSample::fx_send`, 0x5906CA → `voice+0x44`).
    pub fn fx_send(&self) -> u8 {
        self.param(0x13, 0).min(127) as u8
    }
    /// Code 0x06, default 0: the priority the voice allocator 0x5898F0 is given (`PcmSample::priority`).
    pub fn priority(&self) -> u32 {
        self.param(0x06, 0)
    }
    /// Codes 0x01/0x02 and 0x03/0x04, defaults `0..127`: a patch entry is used only when the
    /// requested key and velocity fall inside these ranges (0x590884).
    pub fn key_range(&self) -> (u32, u32) {
        (self.param(0x01, 0), self.param(0x02, 127))
    }
    pub fn velocity_range(&self) -> (u32, u32) {
        (self.param(0x03, 0), self.param(0x04, 127))
    }

    /// The patch's records ([`PtHeader::records`]) with the parser's defaults (0x599188…0x599299)
    /// and the byte / word the record keeps of each code (0x5993AC…0x5994F9).
    pub fn patch_records(&self) -> Vec<PatchRecord> {
        self.records.iter().map(|r| PatchRecord::from_codes(r)).collect()
    }
}

/// One record of a patch (0x599100's 0xA4-byte layer record), the fields the patch start
/// 0x5907B0 / 0x590420 tests and draws from. A patch is one record unless its header carries
/// 0xFE: 91 slots of `SND_coll` / `LilSND_coll` (most of them collision groups whose two or
/// three records split the level with codes 0x01 / 0x02), one each of `rain.bnk` and
/// `thunder_991.bnk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchRecord {
    /// Codes 0x01 / 0x02 → `+8` / `+9`, defaults 0 / 0x7F: the record starts only when the
    /// request's byte `+6` lies in `lo..=hi` (0x590884…0x59089E, `movsx`, signed `jl` / `jg`).
    /// That byte is `Voice_play`'s seventh argument (0x4A9B89…0x4A9B90) — the collision
    /// one-shot's level — or 0x7F (0x561960's default; every other start of the game).
    pub sel: (i8, i8),
    /// Codes 0x03 / 0x04 → `+0xA` / `+0xB`, defaults 0 / 0x7F: the same test on the request's
    /// byte `+5` (0x5908A4…0x5908BE), which no caller of the game writes: 0x3C (0x561983).
    pub note: (i8, i8),
    /// Code 0x0C → `+0x11`, default 0x40: the patch's pan byte.
    pub pan: i8,
    /// Code 0x0D → `+0x12`: the pan's random spread, `pan + rand(spread)` (0x590447…0x590487).
    pub pan_spread: i8,
    /// Code 0x0E → `+0x14`, default 0x7F: the patch's volume.
    pub volume: i8,
    /// Code 0x0F → `+0x15`: the volume's random spread (0x5905A5…0x5905CE).
    pub volume_spread: i8,
    /// Code 0x11 → `+6` (a word): the pitch's random spread in cents (0x590534…0x590549).
    pub pitch_spread: i16,
    /// Code 0x1E → `+0x1A` → `voice+0x48`: the length of the volume step table (code 0x1D).
    pub volume_table_len: i8,
    /// Code 0x1F → `+0x1B`: when set, the volume table's start step is `rand() % it`
    /// (0x590666…0x59067A, `movsx` then an unsigned `div`) — `voice+0x4A`.
    pub volume_table_phase: i8,
    /// Code 0x21 → `+0x1C` → `voice+0x49`: the length of the pitch step table (code 0x20).
    pub pitch_table_len: u8,
    /// Code 0x23 → `+0x1D`: when set, the pitch table's start step is `rand() % (code 0x21)`
    /// (0x590683…0x590699, the length as an unsigned byte) — `voice+0x4B`.
    pub pitch_table_phase: i8,
    /// Code 0x24 → `+0` (a word): the patch-wide pitch spread in cents, drawn once per start from
    /// the **first** record (0x590862…0x590878) and added to every record's pitch (0x59052E).
    pub patch_pitch_spread: i16,
    /// Code 0x07 → `+0xD`, default 60 (0x5991A2): the root key; the voice's pitch word starts at
    /// `(note − root) · 100 + fine` cents (0x5904EC…0x590528, the note the request's `+5`).
    pub root: i8,
    /// Code 0x10 → `+4` (a word), default 0: the fine tune in cents (0x59051F).
    pub fine: i16,
    /// Code 0x1C → `+0x19`, default 0x7F (0x5991EB): the envelope's start level,
    /// `voice+0x34 = level << 16` (0x590614…0x590628).
    pub env_level: i8,
    /// Code 0x09 → `+0xF`, default 1 (0x5991AA): the envelope's segment count → `voice+0x40`;
    /// the voice is stopped when its last segment has run (0x589646…0x58964D).
    pub env_segments: i8,
    /// Code 0x22 → `+0x54` (a word): the pitch table's depth → `voice+0x64`,
    /// `(entry − 0x40) · depth >> 6` cents (0x589FA2…0x589FAF).
    pub pitch_depth: i16,
}

/// The envelope 0x599100 gives a record without code 0x19: `[esp+0x8C]` = 0x5E3498
/// (0x5991E0), the image's `{0x7FFFFFFF, 127}, {0, 1}, {3, 7}, {15, 31}` — with the default
/// count 1 and start level 127, a level held at 127.
pub const DEFAULT_ENVELOPE: [(i32, i32); 4] = [(0x7FFF_FFFF, 127), (0, 1), (3, 7), (15, 31)];

/// A record's tables — what `voice+0x50`, `+0x5C`, `+0x60` and `+0x54` point at (0x590551…
/// 0x59065B), read from the bank.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordTables {
    /// Code 0x19 → `+0x28` → `voice+0x50`: the envelope's segments `{ticks, target}` (two
    /// little-endian words each, `[ptr + 8·k]`, 0x589657…0x58966D), code 0x09 of them (at least
    /// the first, which the start reads whatever the count, 0x590621); [`DEFAULT_ENVELOPE`] without.
    pub envelope: Vec<(i32, i32)>,
    /// Code 0x1D → `+0x2C` → `voice+0x5C`: the volume step table (signed bytes, `movsx`,
    /// 0x589E4C), code 0x1E long (at least one entry).
    pub volume: Option<Vec<i8>>,
    /// Code 0x20 → `+0x30` → `voice+0x60`: the pitch step table (signed bytes, 0x589F9E), code
    /// 0x21 long (at least one entry).
    pub pitch: Option<Vec<i8>>,
    /// Code 0x12 → `+0x20` → `voice+0x54`: the level curve, a byte per level byte — indexed by
    /// the signed level (`movsx`, 0x589E71), so held here from 128 before the pointer: entry
    /// `level + 128`.
    pub curve: Option<Vec<u8>>,
    /// Code 0x17 → `+0x24` → `voice+0x58`: the bend byte's curve (0x589F6D…0x589F79) — no
    /// shipped bank carries it; kept only as present or not.
    pub bend_curve: bool,
}

impl RecordTables {
    fn read(data: &[u8], codes: &[(u8, i32)], ptrs: &[(u8, i64)]) -> Self {
        let get = |code: u8, default: i32| codes.iter().rev().find(|(c, _)| *c == code).map_or(default, |(_, v)| *v);
        let ptr = |code: u8| ptrs.iter().rev().find(|(c, _)| *c == code).map(|(_, p)| *p);
        // a byte of the bank, 0 outside it (no shipped pointer leaves the file)
        let byte = |at: i64| if at >= 0 { data.get(at as usize).copied().unwrap_or(0) } else { 0 };
        let word = |at: i64| i32::from_le_bytes([byte(at), byte(at + 1), byte(at + 2), byte(at + 3)]);
        let segments = (get(0x09, 1) as i8).max(1) as i64;
        let envelope = match ptr(0x19) {
            Some(p) => (0..segments).map(|k| (word(p + 8 * k), word(p + 8 * k + 4))).collect(),
            None => DEFAULT_ENVELOPE[..(segments as usize).min(4)].to_vec(),
        };
        let bytes = |p: i64, n: i64| (0..n.max(1)).map(|k| byte(p + k) as i8).collect::<Vec<i8>>();
        RecordTables {
            envelope,
            volume: ptr(0x1D).map(|p| bytes(p, get(0x1E, 0) as u8 as i64)),
            pitch: ptr(0x20).map(|p| bytes(p, get(0x21, 0) as u8 as i64)),
            curve: ptr(0x12).map(|p| (-128..128).map(|k| byte(p + k)).collect()),
            bend_curve: ptr(0x17).is_some(),
        }
    }
}

impl PatchRecord {
    /// A record from its codes: the last of each code counts (0x5992C6 overwrites the bucket).
    pub fn from_codes(codes: &[(u8, i32)]) -> Self {
        let get = |code: u8, default: i32| codes.iter().rev().find(|(c, _)| *c == code).map_or(default, |(_, v)| *v);
        PatchRecord {
            sel: (get(0x01, 0) as i8, get(0x02, 0x7F) as i8),
            note: (get(0x03, 0) as i8, get(0x04, 0x7F) as i8),
            pan: get(0x0C, 0x40) as i8,
            pan_spread: get(0x0D, 0) as i8,
            volume: get(0x0E, 0x7F) as i8,
            volume_spread: get(0x0F, 0) as i8,
            pitch_spread: get(0x11, 0) as i16,
            volume_table_len: get(0x1E, 0) as i8,
            volume_table_phase: get(0x1F, 0) as i8,
            pitch_table_len: get(0x21, 0) as u8,
            pitch_table_phase: get(0x23, 0) as i8,
            patch_pitch_spread: get(0x24, 0) as i16,
            root: get(0x07, 60) as i8,
            fine: get(0x10, 0) as i16,
            env_level: get(0x1C, 0x7F) as i8,
            env_segments: get(0x09, 1) as i8,
            pitch_depth: get(0x22, 0) as i16,
        }
    }

    /// Whether 0x5907B0 starts this record for the request's bytes `+6` (`sel`) and `+5` (`note`).
    pub fn matches(&self, sel: i8, note: i8) -> bool {
        (self.sel.0..=self.sel.1).contains(&sel) && (self.note.0..=self.note.1).contains(&note)
    }
}

fn be_value(bytes: &[u8], off: usize, code: u8) -> Result<u32, FormatError> {
    if bytes.len() > 4 {
        return Err(FormatError::UnexpectedValue {
            what: "PT value size",
            offset: off,
            expected: format!("<= 4 bytes for code {code:#04x}"),
            found: format!("{} bytes", bytes.len()),
        });
    }
    Ok(bytes.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32))
}

impl PtHeader {
    /// Parse a `PT` header starting at `data[off]`.
    pub fn parse(data: &[u8], off: usize) -> Result<Self, FormatError> {
        let (mut h, _, _) = Self::parse_at(data, off, off + 4, false)?;
        if h.records.len() > 1 {
            // 0x5907B0: one 0x599100 call per record until one ends at the 0xFF (0x59098E)
            let mut p = off + 4;
            loop {
                let (r, next, more) = Self::parse_at(data, off, p, true)?;
                h.parts.push(r);
                if !more {
                    break;
                }
                p = next;
            }
        }
        Ok(h)
    }

    /// The TLV walk from `p` (the header at `off`); with `one_record` it stops after a 0xFE and
    /// answers where the next record starts and that one follows (0x599388 → `ebx = 1`).
    fn parse_at(data: &[u8], off: usize, mut p: usize, one_record: bool) -> Result<(Self, usize, bool), FormatError> {
        if data.get(off..off + 2) != Some(b"PT") {
            return Err(FormatError::UnknownMagic {
                what: "PT header",
                offset: off,
                expected: b"PT".to_vec(),
                found: data.get(off..).unwrap_or(&[]).iter().take(2).copied().collect(),
            });
        }
        let mut h = PtHeader { platform: u16_le(data, off + 2, "PT platform")?, channels: 1, sample_rate: 22050, records: vec![Vec::new()], ..Default::default() };
        let mut more = false;
        // per record: (code, the pointer's target) of the self-relative codes
        let mut ptrs: Vec<Vec<(u8, i64)>> = vec![Vec::new()];
        loop {
            let code = *data.get(p).ok_or(FormatError::Truncated { what: "PT code", offset: p, needed: 1 })?;
            p += 1;
            match code {
                0xFF => break,
                0xFC => continue,
                0xFD => {
                    h.saw_fd = true;
                    continue;
                }
                0xFE => {
                    h.saw_fe = true;
                    if one_record {
                        more = true;
                        break;
                    }
                    h.records.push(Vec::new());
                    ptrs.push(Vec::new());
                    continue;
                }
                _ => {}
            }
            let mut size = *data.get(p).ok_or(FormatError::Truncated { what: "PT size", offset: p, needed: 1 })? as usize;
            p += 1;
            if size == 0xFF {
                size = crate::u32_be(data, p, "PT extended size")? as usize;
                p += 4;
            }
            let val_off = p;
            let bytes = data.get(p..p + size).ok_or(FormatError::Truncated { what: "PT value", offset: p, needed: size })?;
            p += size;
            let v = || be_value(bytes, val_off, code);
            match code {
                c if c < 0x80 => {
                    let value = v()?;
                    if c < 0x26 {
                        let signed = match size {
                            1 if value > 0x7F => value as i32 - 0x100,
                            2 if value > 0x7FFF => value as i32 - 0x1_0000,
                            3 if value > 0x7F_FFFF => value as i32 - 0x100_0000,
                            _ => value as i32,
                        };
                        h.records.last_mut().expect("one record at least").push((c, signed));
                        if matches!(c, 0x12 | 0x17 | 0x19 | 0x1D | 0x20) {
                            ptrs.last_mut().expect("one record at least").push((c, val_off as i64 + signed as i64));
                        }
                    }
                    h.patch_params.push((c, value));
                }
                0x80 => h.split = Some(v()?),
                0x81 => h.type_hint = Some(v()?),
                0x82 => {
                    let ch = v()?;
                    if ch == 0 || ch > 2 {
                        return Err(FormatError::UnexpectedValue {
                            what: "PT channels (0x82)",
                            offset: val_off,
                            expected: "1 or 2".into(),
                            found: ch.to_string(),
                        });
                    }
                    h.channels = ch as u16;
                }
                0x83 => h.codec_class = Some(v()?),
                0x84 => h.sample_rate = v()?,
                0x85 => h.sample_count = Some(v()?),
                0x86 => h.loop_start = Some(v()?),
                0x87 => h.loop_end = Some(v()?),
                0x88 | 0x89 => h.data_offset[(code - 0x88) as usize] = Some(v()?),
                0x8A | 0x8B => h.unknown_8a[(code - 0x8A) as usize] = Some(v()?),
                0x8C => h.flags_8c = Some(v()?),
                0xA1 => h.flags_a1 = Some(v()?),
                0xA0 => h.codec_byte = Some(v()?),
                _ => h.unknown.push((code, bytes.to_vec())),
            }
        }
        h.len = p - off;
        h.tables = h.records.iter().zip(&ptrs).map(|(codes, ptrs)| RecordTables::read(data, codes, ptrs)).collect();
        Ok((h, p, more))
    }

    /// Resolve the codec: the 0xA0 codec byte (default 0x0A) unless 0x80 is absent and 0x83 is
    /// present, in which case `0x599720` maps (0x81 type hint, 0x83 class):
    /// (·,7) → EA-XA, (·,8) → PCM8, (·,9) → codec 4, (0x10,0) → PCM16.
    pub fn codec(&self) -> Result<Codec, FormatError> {
        if self.split.is_none()
            && let Some(class) = self.codec_class
        {
            return match (self.type_hint.unwrap_or(0x10), class) {
                (_, 7) => Ok(Codec::EaXa),
                (_, 8) => Ok(Codec::Pcm8),
                (_, 9) => Ok(Codec::Unknown4),
                (0x10, 0) => Ok(Codec::Pcm16),
                (t, c) => Err(FormatError::UnexpectedValue {
                    what: "PT codec class (0x81,0x83)",
                    offset: 0,
                    expected: "(·,7) | (·,8) | (·,9) | (0x10,0)".into(),
                    found: format!("({t:#x},{c})"),
                }),
            };
        }
        match self.codec_byte.unwrap_or(0x0A) {
            0x0A | 3 => Ok(Codec::EaXa),
            8 | 0 => Ok(Codec::Pcm16),
            9 | 2 => Ok(Codec::Pcm8),
            4 => Ok(Codec::Unknown4),
            other => Err(FormatError::UnexpectedValue {
                what: "PT codec byte (0xA0)",
                offset: 0,
                expected: "0x0A|3 (EA-XA), 8|0 (PCM16), 9|2 (PCM8), 4".into(),
                found: format!("{other:#x}"),
            }),
        }
    }

    /// Loop as `(start, end)` when the header carries a non-empty loop.
    pub fn loop_range(&self) -> Option<(u32, u32)> {
        match (self.loop_start, self.loop_end) {
            (Some(s), Some(e)) if s < e => Some((s, e)),
            _ => None,
        }
    }

    /// Decode the sample described by this header from `data` (the bank file), using the
    /// absolute per-channel offsets of codes 0x88 / 0x89. Planar stereo is interleaved.
    pub fn decode(&self, data: &[u8]) -> Result<PcmSample, FormatError> {
        let count = self.sample_count.ok_or(FormatError::UnexpectedValue {
            what: "PT sample count (0x85)",
            offset: 0,
            expected: "present".into(),
            found: "absent".into(),
        })? as usize;
        if let Some((s, e)) = self.loop_range()
            && e as usize > count
        {
            return Err(FormatError::UnexpectedValue {
                what: "PT loop end (0x87)",
                offset: 0,
                expected: format!("<= sample count {count} (start {s})"),
                found: e.to_string(),
            });
        }
        let codec = self.codec()?;
        let ch = self.channels as usize;
        let mut planes: Vec<Vec<i16>> = Vec::with_capacity(ch);
        let mut padded_tail = 0usize;
        for c in 0..ch {
            let off = self.data_offset[c].ok_or(FormatError::UnexpectedValue {
                what: "PT data offset (0x88+ch)",
                offset: 0,
                expected: format!("offset for channel {c}"),
                found: "absent".into(),
            })? as usize;
            if off > data.len() {
                return Err(FormatError::InvalidOffset { what: "PT sample data", offset: off, len: data.len() });
            }
            let mut plane = Vec::with_capacity(count);
            match codec {
                Codec::EaXa => {
                    padded_tail = padded_tail.max(decode_ea_xa(data, off, count, &mut EaXaState::default(), &mut plane, "EA-XA sample data")?);
                }
                Codec::Pcm16 => {
                    let bytes = data.get(off..off + count * 2).ok_or(FormatError::Truncated { what: "PCM16 sample data", offset: off, needed: count * 2 })?;
                    plane.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
                }
                Codec::Pcm8 => {
                    let bytes = data.get(off..off + count).ok_or(FormatError::Truncated { what: "PCM8 sample data", offset: off, needed: count })?;
                    plane.extend(bytes.iter().map(|&b| (b as i8 as i16) << 8));
                }
                Codec::Unknown4 => {
                    return Err(FormatError::UnexpectedValue {
                        what: "PT codec",
                        offset: off,
                        expected: "EA-XA, PCM16 or PCM8".into(),
                        found: "codec 4 (unsupported low-rate codec)".into(),
                    });
                }
            }
            planes.push(plane);
        }
        Ok(PcmSample {
            rate: self.sample_rate,
            channels: self.channels,
            samples: interleave(&planes),
            loop_range: self.loop_range(),
            padded_tail: padded_tail as u32,
            gain: self.patch_volume().min(127) as f32 / 127.0,
            bend_cents: self.coarse_tune() as u8 as i8 as i32 * 100,
            fx_send: self.fx_send(),
            priority: self.priority() as u8,
            pan: (self.patch_pan() as u8 as i8 as i32).clamp(0, 127) as u8,
            pan_scale: self.pan_scale() as u8 as i8,
            records: self.patch_records(),
            parts: self.parts.iter().map(|r| r.decode(data)).collect::<Result<_, _>>()?,
            tables: self.tables.clone(),
        })
    }
}

/// Interleave equally long planar channels.
pub fn interleave(planes: &[Vec<i16>]) -> Vec<i16> {
    match planes {
        [] => Vec::new(),
        [mono] => mono.clone(),
        _ => {
            let n = planes[0].len();
            let mut out = Vec::with_capacity(n * planes.len());
            for i in 0..n {
                for p in planes {
                    out.push(p[i]);
                }
            }
            out
        }
    }
}

/// A used bank slot: its PT header and where the header sits in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BnkSlot {
    pub header_offset: usize,
    pub header: PtHeader,
}

/// A parsed `BNKl` bank. Slot index = patch id (`CRDl` tables, speech, collision code).
#[derive(Debug, Clone)]
pub struct Bnk {
    pub version: u8,
    /// Byte at +5, 0 in shipped data.
    pub unknown_05: u8,
    /// Offset of the first sample data (= directory size).
    pub dir_size: u32,
    /// Declared data size; equals `len - dir_size` in `Sounds/` banks but is unrelated to the
    /// real length in speech banks, so it is not validated.
    pub data_size: u32,
    /// `u32` at +16, 0 in shipped data.
    pub unknown_10: u32,
    /// One entry per slot; `None` = empty slot (offset 0).
    pub slots: Vec<Option<BnkSlot>>,
}

impl Bnk {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(b"BNKl") {
            return Err(FormatError::UnknownMagic {
                what: "BNKl",
                offset: 0,
                expected: b"BNKl".to_vec(),
                found: data.iter().take(4).copied().collect(),
            });
        }
        let version = *data.get(4).ok_or(FormatError::Truncated { what: "BNKl version", offset: 4, needed: 1 })?;
        if version != 4 {
            return Err(FormatError::UnsupportedVersion { what: "BNKl", found: version as u32, supported: "4" });
        }
        let unknown_05 = data[5];
        let count = u16_le(data, 6, "BNKl slot count")? as usize;
        let dir_size = u32_le(data, 8, "BNKl dir size")?;
        let data_size = u32_le(data, 12, "BNKl data size")?;
        let unknown_10 = u32_le(data, 16, "BNKl +16")?;
        if dir_size as usize > data.len() || (dir_size as usize) < 0x14 + 4 * count {
            return Err(FormatError::InvalidOffset { what: "BNKl dir size", offset: dir_size as usize, len: data.len() });
        }
        let mut slots = Vec::with_capacity(count);
        for i in 0..count {
            let entry = 0x14 + 4 * i;
            let rel = u32_le(data, entry, "BNKl slot offset")?;
            if rel == 0 {
                slots.push(None);
                continue;
            }
            let header_offset = entry + rel as usize;
            if header_offset >= dir_size as usize {
                return Err(FormatError::InvalidOffset { what: "BNKl slot header", offset: header_offset, len: dir_size as usize });
            }
            let header = PtHeader::parse(data, header_offset)?;
            slots.push(Some(BnkSlot { header_offset, header }));
        }
        Ok(Self { version, unknown_05, dir_size, data_size, unknown_10, slots })
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// PT header of slot `i`, `None` for an empty or out-of-range slot.
    pub fn header(&self, i: usize) -> Option<&PtHeader> {
        self.slots.get(i).and_then(|s| s.as_ref()).map(|s| &s.header)
    }

    /// Indices of the used slots.
    pub fn used_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|_| i))
    }

    /// Decode slot `slot` of the bank whose bytes are `data` (the same buffer given to `parse`).
    pub fn decode_slot(&self, data: &[u8], slot: usize) -> Result<PcmSample, FormatError> {
        let s = self.slots.get(slot).and_then(|s| s.as_ref()).ok_or(FormatError::UnexpectedValue {
            what: "BNKl slot",
            offset: 0x14 + 4 * slot,
            expected: "a used slot".into(),
            found: if slot < self.slots.len() { "empty slot".into() } else { format!("index {slot} >= slot count {}", self.slots.len()) },
        })?;
        s.header.decode(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn records_split_at_fe() {
        // `SND_coll.bnk`'s header at 0xBAC: three records split over the level by codes 1 / 2
        let h = hex(
            "50 54 00 00 24 02 00 c8 01 01 4f 02 01 73 06 01 19 0a 01 04 11 01 28 13 01 7f fd 80 01 01 85 02 23 f2 86 00 87 00 \
             88 04 00 08 f6 90 8a 04 00 00 00 00 fe 02 01 4e 06 01 1a 0a 01 04 11 01 28 13 01 7f fd 80 01 01 85 02 17 7a 86 00 \
             87 00 88 04 00 09 09 e0 8a 04 00 00 00 00 fe 01 01 74 06 01 60 0a 01 04 11 01 28 13 01 7f fd 80 01 01 85 02 23 f2 \
             86 00 87 00 88 04 00 08 f6 90 8a 04 00 00 00 00 ff",
        );
        let p = PtHeader::parse(&h, 0).unwrap();
        let r = p.patch_records();
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].sel, r[1].sel, r[2].sel), ((79, 115), (0, 78), (116, 127)));
        assert_eq!((r[0].patch_pitch_spread, r[1].patch_pitch_spread), (200, 0));
        assert_eq!((r[0].pitch_spread, r[0].volume, r[0].pan), (40, 0x7F, 0x40));
        assert!(r[1].matches(40, 0x3C) && !r[0].matches(40, 0x3C) && r[2].matches(0x7F, 0x3C));
        // each record is a header of its own (0x599100 per record, defaults afresh): its own
        // sample count and offset, and only its own codes
        assert_eq!(p.parts.len(), 3);
        assert_eq!(p.parts.iter().map(|h| h.sample_count).collect::<Vec<_>>(), [Some(9202), Some(6010), Some(9202)]);
        assert_eq!(p.parts.iter().map(|h| h.data_offset[0]).collect::<Vec<_>>(), [Some(0x08F690), Some(0x0909E0), Some(0x08F690)]);
        for (i, h) in p.parts.iter().enumerate() {
            assert_eq!(h.patch_records(), vec![r[i]]);
            assert!(h.parts.is_empty());
        }
        // the merged view keeps the last record's sample, as before
        assert_eq!(p.sample_count, Some(9202));
        // a one-record header has no parts
        assert!(PtHeader::parse(&hex("50 54 00 00 11 01 96 ff"), 0).unwrap().parts.is_empty());
    }

    #[test]
    fn values_sign_extend_by_size() {
        // 0x59BD90: one byte 0x96 is −106, two bytes 0x0096 are 150, three bytes 0xFFFFFE are −2
        let h = hex("50 54 00 00 11 01 96 24 02 00 96 0d 03 ff ff fe ff");
        let p = PtHeader::parse(&h, 0).unwrap();
        assert_eq!(p.records, vec![vec![(0x11, -106), (0x24, 150), (0x0D, -2)]]);
        let r = p.patch_records()[0];
        assert_eq!((r.pitch_spread, r.patch_pitch_spread, r.pan_spread), (-106, 150, -2));
        // the unsigned merged view is unchanged
        assert_eq!(p.pitch_spread(), 0x96);
    }
}
