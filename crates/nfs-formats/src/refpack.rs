//! RefPack (EA "QFS" / `10 FB`) compression.
//!
//! Status: **CONFIRMED** — decoding `alps.crp`, `356_1.crp`, `alps.fsh`, `skidpad.crp` yields
//! exactly the declared uncompressed size and well-formed inner containers (see `RE/FORMATS.md`).
//!
//! Stream layout (all multi-byte sizes are big-endian):
//! ```text
//! +0  u8  flags   bit7 (0x80): sizes are 4 bytes instead of 3
//!                 bit0 (0x01): a compressed-size field precedes the uncompressed size
//! +1  u8  0xFB
//! +2  [compressed size]  (only if flags & 1)
//!     uncompressed size
//!     ... command stream ...
//! ```
//! Command bytes:
//! - `0x00..=0x7F` (2 bytes): literals = b0&3, copy len = ((b0>>2)&7)+3, offset = ((b0&0x60)<<3)+b1+1
//! - `0x80..=0xBF` (3 bytes): literals = b1>>6, copy len = (b0&0x3F)+4, offset = ((b1&0x3F)<<8)+b2+1
//! - `0xC0..=0xDF` (4 bytes): literals = b0&3, copy len = ((b0&0x0C)<<6)+b3+5, offset = ((b0&0x10)<<12)+(b1<<8)+b2+1
//! - `0xE0..=0xFB` (1 byte): literals = ((b0&0x1F)<<2)+4, no copy
//! - `0xFC..=0xFF` (1 byte): literals = b0&3, then end of stream

use crate::FormatError;

/// The most output one stream byte can produce (a 4-byte command copies up to 1028 bytes): the
/// header's size is only trusted for the up-front allocation up to this bound.
const MAX_EXPANSION: usize = 260;

/// The second header byte.
pub const MARKER: u8 = 0xFB;
/// Header flag: the sizes are four bytes instead of three.
pub const FLAG_LONG_SIZES: u8 = 0x80;
/// Header flag: a compressed-size field precedes the uncompressed size.
pub const FLAG_HAS_COMPRESSED_SIZE: u8 = 0x01;
/// The flag bits that must read `0x10` for [`is_refpack`] (bit 7, the size width, and bit 0 are free).
const FLAGS_MASK: u8 = 0x3E;
const FLAGS_VALUE: u8 = 0x10;

/// Returns `true` if `data` starts with a RefPack header.
pub fn is_refpack(data: &[u8]) -> bool {
    data.len() >= 2 && data[1] == MARKER && (data[0] & FLAGS_MASK) == FLAGS_VALUE
}

/// Header information without decoding the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefPackHeader {
    pub flags: u8,
    pub compressed_size: Option<u32>,
    pub uncompressed_size: u32,
    /// Offset of the first command byte.
    pub payload_offset: usize,
}

pub fn read_header(data: &[u8]) -> Result<RefPackHeader, FormatError> {
    if data.len() < 2 || data[1] != MARKER {
        return Err(FormatError::UnknownMagic {
            what: "RefPack",
            offset: 0,
            expected: vec![FLAGS_VALUE, MARKER],
            found: data.iter().take(2).copied().collect(),
        });
    }
    let flags = data[0];
    let n = if flags & FLAG_LONG_SIZES != 0 { 4 } else { 3 };
    let mut pos = 2;
    let read_size = |pos: usize| -> Result<u32, FormatError> {
        let b = data
            .get(pos..pos + n)
            .ok_or(FormatError::Truncated { what: "RefPack header", offset: pos, needed: n })?;
        Ok(b.iter().fold(0u32, |acc, &x| (acc << 8) | x as u32))
    };
    let compressed_size = if flags & FLAG_HAS_COMPRESSED_SIZE != 0 {
        let s = read_size(pos)?;
        pos += n;
        Some(s)
    } else {
        None
    };
    let uncompressed_size = read_size(pos)?;
    pos += n;
    Ok(RefPackHeader { flags, compressed_size, uncompressed_size, payload_offset: pos })
}

/// Decompress a complete RefPack stream. Fails if the output size does not match the header.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, FormatError> {
    let hdr = read_header(data)?;
    let mut out: Vec<u8> = Vec::with_capacity((hdr.uncompressed_size as usize).min(data.len().saturating_mul(MAX_EXPANSION)));
    let mut pos = hdr.payload_offset;

    let byte = |p: usize| -> Result<u8, FormatError> {
        data.get(p).copied().ok_or(FormatError::RefPack { offset: p, reason: "command stream truncated" })
    };

    loop {
        let b0 = byte(pos)?;
        let (literals, copy_len, copy_off, consumed) = if b0 < 0x80 {
            let b1 = byte(pos + 1)?;
            (
                (b0 & 3) as usize,
                (((b0 & 0x1C) >> 2) + 3) as usize,
                (((b0 as usize & 0x60) << 3) + b1 as usize + 1),
                2,
            )
        } else if b0 < 0xC0 {
            let b1 = byte(pos + 1)?;
            let b2 = byte(pos + 2)?;
            (
                ((b1 >> 6) & 3) as usize,
                ((b0 & 0x3F) + 4) as usize,
                (((b1 as usize & 0x3F) << 8) + b2 as usize + 1),
                3,
            )
        } else if b0 < 0xE0 {
            let b1 = byte(pos + 1)?;
            let b2 = byte(pos + 2)?;
            let b3 = byte(pos + 3)?;
            (
                (b0 & 3) as usize,
                (((b0 as usize & 0x0C) << 6) + b3 as usize + 5),
                (((b0 as usize & 0x10) << 12) + ((b1 as usize) << 8) + b2 as usize + 1),
                4,
            )
        } else if b0 < 0xFC {
            ((((b0 & 0x1F) as usize) << 2) + 4, 0, 0, 1)
        } else {
            // terminator
            let lit = (b0 & 3) as usize;
            let src = data
                .get(pos + 1..pos + 1 + lit)
                .ok_or(FormatError::RefPack { offset: pos, reason: "terminator literals truncated" })?;
            out.extend_from_slice(src);
            break;
        };
        pos += consumed;
        let src = data
            .get(pos..pos + literals)
            .ok_or(FormatError::RefPack { offset: pos, reason: "literal run truncated" })?;
        out.extend_from_slice(src);
        pos += literals;
        if copy_len > 0 {
            if copy_off > out.len() {
                return Err(FormatError::RefPack { offset: pos, reason: "back-reference before start of output" });
            }
            let start = out.len() - copy_off;
            if copy_off >= copy_len {
                // the source run ends before the destination begins: one block copy
                out.extend_from_within(start..start + copy_len);
            } else {
                // Overlapping copy semantics: byte-by-byte.
                for i in 0..copy_len {
                    let b = out[start + i];
                    out.push(b);
                }
            }
        }
    }

    if out.len() != hdr.uncompressed_size as usize {
        return Err(FormatError::SizeMismatch {
            what: "RefPack",
            declared: hdr.uncompressed_size as usize,
            actual: out.len(),
        });
    }
    Ok(out)
}

/// Decompress if the buffer is RefPack-compressed; otherwise return it unchanged.
pub fn maybe_decompress(data: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, FormatError> {
    if is_refpack(data) {
        Ok(std::borrow::Cow::Owned(decompress(data)?))
    } else {
        Ok(std::borrow::Cow::Borrowed(data))
    }
}

/// [`maybe_decompress`] for an owned buffer: the same `Vec` back (no copy) when it is not RefPack.
pub fn decompress_vec(data: Vec<u8>) -> Result<Vec<u8>, FormatError> {
    if is_refpack(&data) {
        decompress(&data)
    } else {
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_constants_keep_their_literal_values() {
        assert_eq!([MARKER, FLAG_LONG_SIZES, FLAG_HAS_COMPRESSED_SIZE, FLAGS_MASK, FLAGS_VALUE], [0xFB, 0x80, 0x01, 0x3E, 0x10]);
        assert!(is_refpack(&[0x10, 0xFB]) && is_refpack(&[0x91, 0xFB]) && !is_refpack(&[0x20, 0xFB]) && !is_refpack(&[0x10, 0xFA]));
    }

    #[test]
    fn literal_only_stream() {
        // flags=0x10, 0xFB, 3-byte size = 5, then 0xE0 => 4 literals, then 0xFD => 1 literal + stop.
        let s = [0x10, 0xFB, 0, 0, 5, 0xE0, b'a', b'b', b'c', b'd', 0xFD, b'e'];
        assert_eq!(decompress(&s).unwrap(), b"abcde");
    }

    #[test]
    fn short_backreference() {
        // "abab": 2 literals "ab", then copy 2? min copy len for 2-byte cmd is 3 -> "ababa"
        // b0 = literals 2 | len (3-3)=0<<2 | off hi 0 => 0x02 ; b1 = off-1 = 1
        let s = [0x10, 0xFB, 0, 0, 5, 0x02, 0x01, b'a', b'b', 0xFC];
        assert_eq!(decompress(&s).unwrap(), b"ababa");
    }

    #[test]
    fn overlapping_run_repeats_the_last_byte() {
        // "ab" then a copy of length 3 from offset 1: each byte copied is the one just written.
        let s = [0x10, 0xFB, 0, 0, 5, 0x02, 0x00, b'a', b'b', 0xFC];
        assert_eq!(decompress(&s).unwrap(), b"abbbb");
    }

    #[test]
    fn non_overlapping_copy_is_a_block_copy() {
        // 4 literals "abcd" (0xE0), then copy length 3 from offset 4 (cmd 0x00, b1 = off - 1).
        let s = [0x10, 0xFB, 0, 0, 7, 0xE0, b'a', b'b', b'c', b'd', 0x00, 0x03, 0xFC];
        assert_eq!(decompress(&s).unwrap(), b"abcdabc");
    }

    #[test]
    fn decompress_vec_is_maybe_decompress_without_the_copy() {
        let packed = vec![0x10, 0xFB, 0, 0, 5, 0xE0, b'a', b'b', b'c', b'd', 0xFD, b'e'];
        assert_eq!(decompress_vec(packed.clone()).unwrap(), maybe_decompress(&packed).unwrap().into_owned());
        let plain = b"SHPI plain bytes".to_vec();
        assert_eq!(decompress_vec(plain.clone()).unwrap(), plain);
    }

    #[test]
    fn size_mismatch_is_error() {
        let s = [0x10, 0xFB, 0, 0, 9, 0xE0, b'a', b'b', b'c', b'd', 0xFC];
        assert!(matches!(decompress(&s), Err(FormatError::SizeMismatch { .. })));
    }
}
