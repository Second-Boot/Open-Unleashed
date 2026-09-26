//! Parsers for the original *Need for Speed: Porsche Unleashed* data formats.
//!
//! Every parser here is derived from evidence recorded in `RE/FORMATS.md`. Fields whose meaning
//! is not established are exposed under `unknown_*` names rather than guessed. Malformed input
//! produces an explicit [`FormatError`] naming the file offset, structure, expected and actual
//! values; nothing is silently skipped.

pub mod ais;
pub mod animdefs;
pub mod bigf;
pub mod bnk;
pub mod career;
pub mod cop;
pub mod crdl;
pub mod crp;
pub mod ctr;
pub mod edg;
pub mod error;
pub mod expr;
pub mod fedata;
pub mod fetxt;
pub mod flare;
pub mod particle_dat;
pub mod fnt;
pub mod fsh;
pub mod ini;
pub mod key;
pub mod lay;
pub mod loc;
pub mod locale;
pub mod mad;
pub mod refpack;
pub mod regfile;
pub mod schl;
pub mod scn;
pub mod sav;
pub mod scripts;
pub mod sim;
pub mod trafcfg;
pub mod sky;
pub mod spl;
pub mod stl_sort;

pub use error::FormatError;

/// Read a little-endian `u32` at `off`, or fail with an [`FormatError::Truncated`].
pub(crate) fn u32_le(data: &[u8], off: usize, what: &'static str) -> Result<u32, FormatError> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(FormatError::Truncated { offset: off, needed: 4, what })
}

pub(crate) fn u16_le(data: &[u8], off: usize, what: &'static str) -> Result<u16, FormatError> {
    data.get(off..off + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(FormatError::Truncated { offset: off, needed: 2, what })
}

pub(crate) fn u32_be(data: &[u8], off: usize, what: &'static str) -> Result<u32, FormatError> {
    data.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(FormatError::Truncated { offset: off, needed: 4, what })
}

pub(crate) fn f32_le(data: &[u8], off: usize, what: &'static str) -> Result<f32, FormatError> {
    Ok(f32::from_bits(u32_le(data, off, what)?))
}

/// Decode a fixed-size, NUL-padded ASCII/Latin-1 field.
pub(crate) fn fixed_str(data: &[u8], off: usize, len: usize, what: &'static str) -> Result<String, FormatError> {
    let bytes = data
        .get(off..off + len)
        .ok_or(FormatError::Truncated { offset: off, needed: len, what })?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
    Ok(bytes[..end].iter().map(|&b| b as char).collect())
}
