//! Shared little-endian primitive readers.
//!
//! Every parser in this crate used to re-implement `from_le_bytes([b[o], b[o + 1], ..])` with
//! its own failure mode. The failure mode *is* format behaviour (a reader that zero-fills, one
//! that falls back to `-1`, one that errors), so this module offers the three flavours side by
//! side and each call site keeps the one it had:
//!
//! * `get_*` — checked, `None` past the end (the caller picks the fallback);
//! * `read_*` — for a record the caller has already length-checked: panics past the end, exactly
//!   like the indexing it replaces;
//! * [`Cursor`] — a sequential reader returning [`FormatError::Truncated`] (the shape of
//!   `sav.rs`'s reader), usable from other crates too.
//!
//! Floats are read as `f32::from_bits(u32)`, which is bit-identical to `f32::from_le_bytes`.

use crate::FormatError;

/// `N` bytes at `off`, or `None` when they run past the end.
#[inline]
pub fn get_array<const N: usize>(data: &[u8], off: usize) -> Option<[u8; N]> {
    data.get(off..off.checked_add(N)?).and_then(|b| b.try_into().ok())
}

#[inline]
pub fn get_u8(data: &[u8], off: usize) -> Option<u8> {
    data.get(off).copied()
}

#[inline]
pub fn get_u16(data: &[u8], off: usize) -> Option<u16> {
    get_array(data, off).map(u16::from_le_bytes)
}

#[inline]
pub fn get_i16(data: &[u8], off: usize) -> Option<i16> {
    get_array(data, off).map(i16::from_le_bytes)
}

#[inline]
pub fn get_u32(data: &[u8], off: usize) -> Option<u32> {
    get_array(data, off).map(u32::from_le_bytes)
}

#[inline]
pub fn get_i32(data: &[u8], off: usize) -> Option<i32> {
    get_array(data, off).map(i32::from_le_bytes)
}

#[inline]
pub fn get_f32(data: &[u8], off: usize) -> Option<f32> {
    get_u32(data, off).map(f32::from_bits)
}

#[inline]
pub fn get_u16_be(data: &[u8], off: usize) -> Option<u16> {
    get_array(data, off).map(u16::from_be_bytes)
}

#[inline]
pub fn get_u32_be(data: &[u8], off: usize) -> Option<u32> {
    get_array(data, off).map(u32::from_be_bytes)
}

/// `N` bytes at `off` of a record the caller has already length-checked (panics past the end).
#[inline]
pub fn read_array<const N: usize>(data: &[u8], off: usize) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&data[off..off + N]);
    a
}

/// A little-endian `u16` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(read_array(data, off))
}

/// A little-endian `i16` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_i16(data: &[u8], off: usize) -> i16 {
    i16::from_le_bytes(read_array(data, off))
}

/// A little-endian `u32` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(read_array(data, off))
}

/// A little-endian `i32` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_i32(data: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(read_array(data, off))
}

/// A little-endian `f32` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_f32(data: &[u8], off: usize) -> f32 {
    f32::from_bits(read_u32(data, off))
}

/// A big-endian `u32` at `off` of an already length-checked record (panics past the end).
#[inline]
pub fn read_u32_be(data: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(read_array(data, off))
}

/// A sequential little-endian reader over a byte slice. Every read past the end fails with
/// [`FormatError::Truncated`] carrying the cursor position, the bytes needed and the `what`
/// label given at construction; a failed read does not move the cursor.
#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    /// A cursor at offset 0 whose errors name `what`.
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, pos: 0, what }
    }

    /// A cursor at `pos` (may lie past the end; the first read then fails).
    pub fn at(data: &'a [u8], pos: usize, what: &'static str) -> Self {
        Self { data, pos, what }
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Bytes left after the cursor (0 when it lies past the end).
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// The [`FormatError::Truncated`] this cursor reports for a read of `needed` bytes here.
    pub fn truncated(&self, needed: usize) -> FormatError {
        FormatError::Truncated { what: self.what, offset: self.pos, needed }
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let b = self.pos.checked_add(n).and_then(|end| self.data.get(self.pos..end)).ok_or_else(|| self.truncated(n))?;
        self.pos += n;
        Ok(b)
    }

    /// Skip `n` bytes (fails like [`Cursor::take`] when they are not there).
    pub fn skip(&mut self, n: usize) -> Result<(), FormatError> {
        self.take(n).map(|_| ())
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let b = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(b);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, FormatError> {
        self.array().map(u16::from_le_bytes)
    }

    pub fn i16(&mut self) -> Result<i16, FormatError> {
        self.array().map(i16::from_le_bytes)
    }

    pub fn u32(&mut self) -> Result<u32, FormatError> {
        self.array().map(u32::from_le_bytes)
    }

    pub fn i32(&mut self) -> Result<i32, FormatError> {
        self.array().map(i32::from_le_bytes)
    }

    pub fn f32(&mut self) -> Result<f32, FormatError> {
        self.u32().map(f32::from_bits)
    }

    pub fn u32_be(&mut self) -> Result<u32, FormatError> {
        self.array().map(u32::from_be_bytes)
    }

    /// A NUL-terminated Latin-1 string (bytes mapped 1:1 to `char`); the cursor moves past the
    /// NUL. A missing NUL fails with `Truncated { offset: pos, needed: 1 }`.
    pub fn cstr(&mut self) -> Result<String, FormatError> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let end = rest.iter().position(|&b| b == 0).ok_or_else(|| self.truncated(1))?;
        let s: String = rest[..end].iter().map(|&b| b as char).collect();
        self.pos += end + 1;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_and_panicking_readers_agree() {
        let d = [0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x80, 0x3F];
        assert_eq!(get_u16(&d, 0), Some(0x0201));
        assert_eq!(get_u32(&d, 0), Some(0x0403_0201));
        assert_eq!(get_u32_be(&d, 0), Some(0x0102_0304));
        assert_eq!(get_f32(&d, 4), Some(1.0));
        assert_eq!(get_u32(&d, 5), None);
        assert_eq!(get_u32(&d, usize::MAX), None);
        assert_eq!(read_u32(&d, 0), 0x0403_0201);
        assert_eq!(read_f32(&d, 4), 1.0);
        assert_eq!(get_i16(&[0xFF, 0xFF], 0), Some(-1));
    }

    #[test]
    fn cursor_errors_carry_position_and_do_not_move() {
        let d = [1u8, 0, b'a', b'b', 0, 7];
        let mut c = Cursor::new(&d, "t");
        assert_eq!(c.u16().unwrap(), 1);
        assert_eq!(c.cstr().unwrap(), "ab");
        assert_eq!(c.pos(), 5);
        match c.u32() {
            Err(FormatError::Truncated { what: "t", offset: 5, needed: 4 }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(c.pos(), 5);
        assert_eq!(c.u8().unwrap(), 7);
        assert!(matches!(c.cstr(), Err(FormatError::Truncated { offset: 6, needed: 1, .. })));
    }
}
