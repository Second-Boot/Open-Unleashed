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
    /// Code 0x06: the priority the voice allocator 0x5898F0 is given.
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
        if data.get(off..off + 2) != Some(b"PT") {
            return Err(FormatError::UnknownMagic {
                what: "PT header",
                offset: off,
                expected: b"PT".to_vec(),
                found: data.get(off..).unwrap_or(&[]).iter().take(2).copied().collect(),
            });
        }
        let mut h = PtHeader { platform: u16_le(data, off + 2, "PT platform")?, channels: 1, sample_rate: 22050, ..Default::default() };
        let mut p = off + 4;
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
                c if c < 0x80 => h.patch_params.push((c, v()?)),
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
        Ok(h)
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
