//! `SCHl` streamed music (EA streaming audio) from `Music/zzzymus.viv`.
//!
//! Status: **CONFIRMED** (`RE/notes/audio.md` §3): block dispatcher `0x567170`, `SCHl` handler
//! `0x566EA0` (parses the PT header at `block+8`), history re-seed at `0x59C54E`
//! (`movsx word, movsx word+2, ptr += 4`). Cross-checked on all 28 streams: the per-block
//! sample counts sum exactly to the PT 0x85 total, and every `SCDl` block is laid out as below.
//!
//! ```text
//! block  { char[4] tag, u32 LE size incl. this 8-byte head }
//! SCHl   PT header (music: 2 ch, 32000 Hz, no 0xA0 => EA-XA, 0x85 = total samples per channel)
//! SCCl   u32 number of SCDl blocks that follow
//! SCDl   u32 samples per channel, u32 off[ch] relative to the end of the offset array, then per
//!        channel: i16 s[n-1], i16 s[n-2] (history seed) + 15-byte EA-XA frames (planar)
//! SCEl   end
//! ```
//! The seed is the encoder's reference history; the game re-seeds the predictor from it for every
//! block, so blocks decode independently (which is what makes seeking trivial).

use crate::bnk::{decode_ea_xa, interleave, Codec, EaXaState, PcmSample, PtHeader};
use crate::{u32_le, FormatError};

/// One `SCDl` block: `channels[c]` starts at that channel's two seed samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBlock<'a> {
    /// File offset of the block tag.
    pub offset: usize,
    pub samples_per_channel: u32,
    pub channels: Vec<&'a [u8]>,
}

impl<'a> DataBlock<'a> {
    /// Decode this block, appending interleaved samples to `out`.
    pub fn decode_into(&self, out: &mut Vec<i16>) -> Result<(), FormatError> {
        let n = self.samples_per_channel as usize;
        let mut planes = Vec::with_capacity(self.channels.len());
        for (c, chan) in self.channels.iter().enumerate() {
            if chan.len() < 4 {
                return Err(FormatError::Truncated { what: "SCDl channel seed", offset: self.offset, needed: 4 });
            }
            let s1 = i16::from_le_bytes([chan[0], chan[1]]);
            let s2 = i16::from_le_bytes([chan[2], chan[3]]);
            let mut plane = Vec::with_capacity(n);
            let padded = decode_ea_xa(chan, 4, n, &mut EaXaState::seeded(s1, s2), &mut plane, "SCDl EA-XA frames")?;
            if padded != 0 {
                return Err(FormatError::Truncated { what: "SCDl EA-XA frames", offset: self.offset, needed: padded });
            }
            debug_assert_eq!(plane.len(), n, "channel {c}");
            planes.push(plane);
        }
        out.extend(interleave(&planes));
        Ok(())
    }
}

/// A block of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block<'a> {
    /// `SCHl`: the PT header.
    Header(PtHeader),
    /// `SCCl`: declared number of `SCDl` blocks.
    Count(u32),
    /// `SCDl`.
    Data(DataBlock<'a>),
    /// `SCEl`.
    End,
}

/// Iterator over the blocks of a stream, for streaming playback / seeking.
pub struct Blocks<'a> {
    data: &'a [u8],
    pos: usize,
    channels: usize,
    done: bool,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Result<Block<'a>, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.data.len() {
            return None;
        }
        let r = parse_block(self.data, self.pos, self.channels);
        match &r {
            Ok((block, next)) => {
                if matches!(block, Block::End) {
                    self.done = true;
                }
                if let Block::Header(h) = block {
                    self.channels = h.channels as usize;
                }
                self.pos = *next;
            }
            Err(_) => self.done = true,
        }
        Some(r.map(|(b, _)| b))
    }
}

/// Parse the block at `pos`; returns it and the offset of the next block.
pub fn parse_block(data: &[u8], pos: usize, channels: usize) -> Result<(Block<'_>, usize), FormatError> {
    let tag = data.get(pos..pos + 4).ok_or(FormatError::Truncated { what: "SCxl block tag", offset: pos, needed: 4 })?;
    let size = u32_le(data, pos + 4, "SCxl block size")? as usize;
    if size < 8 || pos + size > data.len() {
        return Err(FormatError::InvalidOffset { what: "SCxl block size", offset: pos + size, len: data.len() });
    }
    let body = &data[pos + 8..pos + size];
    let block = match tag {
        b"SCHl" => Block::Header(PtHeader::parse(data, pos + 8)?),
        b"SCCl" => Block::Count(u32_le(data, pos + 8, "SCCl count")?),
        b"SCDl" => {
            let n = u32_le(data, pos + 8, "SCDl samples per channel")?;
            let base = 4 + 4 * channels; // end of the offset array, relative to body
            if body.len() < base {
                return Err(FormatError::Truncated { what: "SCDl offsets", offset: pos + 12, needed: 4 * channels });
            }
            let mut chans = Vec::with_capacity(channels);
            for c in 0..channels {
                let off = u32_le(data, pos + 12 + 4 * c, "SCDl channel offset")? as usize;
                let start = base + off;
                if start > body.len() {
                    return Err(FormatError::InvalidOffset { what: "SCDl channel data", offset: pos + 8 + start, len: pos + size });
                }
                chans.push(&body[start..]);
            }
            Block::Data(DataBlock { offset: pos, samples_per_channel: n, channels: chans })
        }
        b"SCEl" => Block::End,
        other => {
            return Err(FormatError::UnknownRecordType { what: "SCxl block", offset: pos, code: u32::from_le_bytes([other[0], other[1], other[2], other[3]]) });
        }
    };
    Ok((block, pos + size))
}

/// A music stream: the leading `SCHl` header and `SCCl` count, plus where the data blocks start.
#[derive(Debug, Clone)]
pub struct SchlStream {
    pub header: PtHeader,
    /// `SCCl` value, `None` if the stream has no `SCCl` block before the first `SCDl`.
    pub declared_blocks: Option<u32>,
    /// File offset of the first block after `SCHl`/`SCCl`.
    pub data_start: usize,
}

impl SchlStream {
    /// Parse the stream head (`SCHl`, optional `SCCl`). Data blocks are visited lazily by
    /// [`SchlStream::blocks`].
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(b"SCHl") {
            return Err(FormatError::UnknownMagic {
                what: "SCHl",
                offset: 0,
                expected: b"SCHl".to_vec(),
                found: data.iter().take(4).copied().collect(),
            });
        }
        let (block, mut pos) = parse_block(data, 0, 0)?;
        let Block::Header(header) = block else { unreachable!("tag checked above") };
        if header.sample_count.is_none() {
            return Err(FormatError::UnexpectedValue { what: "SCHl PT sample count (0x85)", offset: 8, expected: "present".into(), found: "absent".into() });
        }
        if header.codec()? != Codec::EaXa {
            return Err(FormatError::UnexpectedValue { what: "SCHl codec", offset: 8, expected: "EA-XA".into(), found: format!("{:?}", header.codec()?) });
        }
        let mut declared_blocks = None;
        if data.get(pos..pos + 4) == Some(b"SCCl") {
            let (block, next) = parse_block(data, pos, header.channels as usize)?;
            if let Block::Count(n) = block {
                declared_blocks = Some(n);
            }
            pos = next;
        }
        Ok(Self { header, declared_blocks, data_start: pos })
    }

    /// Iterate the blocks from the first data block to `SCEl`.
    pub fn blocks<'a>(&self, data: &'a [u8]) -> Blocks<'a> {
        Blocks { data, pos: self.data_start, channels: self.header.channels as usize, done: false }
    }

    /// Decode the whole stream. Verifies the `SCCl` count and the PT 0x85 total.
    pub fn decode_all(&self, data: &[u8]) -> Result<PcmSample, FormatError> {
        let total = self.header.sample_count.unwrap_or(0) as usize;
        let mut samples = Vec::with_capacity(total * self.header.channels as usize);
        let mut blocks = 0u32;
        let mut ended = false;
        for b in self.blocks(data) {
            match b? {
                Block::Data(d) => {
                    d.decode_into(&mut samples)?;
                    blocks += 1;
                }
                Block::End => ended = true,
                Block::Header(_) | Block::Count(_) => {
                    return Err(FormatError::UnexpectedValue { what: "SCxl block order", offset: 0, expected: "SCDl or SCEl".into(), found: "SCHl/SCCl after data".into() });
                }
            }
        }
        if !ended {
            return Err(FormatError::Truncated { what: "SCEl end block", offset: data.len(), needed: 8 });
        }
        if let Some(n) = self.declared_blocks
            && n != blocks
        {
            return Err(FormatError::SizeMismatch { what: "SCCl block count", declared: n as usize, actual: blocks as usize });
        }
        let per_channel = samples.len() / self.header.channels as usize;
        if per_channel != total {
            return Err(FormatError::SizeMismatch { what: "SCHl sample count (0x85)", declared: total, actual: per_channel });
        }
        // a music stream carries no patch volume, so it plays at the level the caller asks for
        Ok(PcmSample { rate: self.header.sample_rate, channels: self.header.channels, samples, loop_range: None, padded_tail: 0, gain: 1.0 })
    }
}

/// Parse and fully decode a stream in one call.
pub fn decode_all(data: &[u8]) -> Result<PcmSample, FormatError> {
    SchlStream::parse(data)?.decode_all(data)
}
