//! `<track>_forward.key` / `_backwards.key` — per-slice render draw distance.
//!
//! `RdrKeys::ctor` 0x44FB40 / `valueAt` 0x44FCB0 (`RE/notes/race-logic.md` §6.1, HIGH): a header
//! `{u32 version = 1, u32 count}` followed by `count` keyframes `{i32 slice, f32 value}` (the file
//! reserves 400 slots; unused ones are `(-1, 400.0)`). `value_at` is a **step function**: the value of
//! the last key whose slice is not past the queried one. The game multiplies the result by the detail
//! setting (0.5 / 0.67 / 0.83 / 1.0, minimum 50 m) and uses it as the renderer's far distance.

use crate::{f32_le, u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawKey {
    pub slice: i32,
    pub distance: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrawKeys {
    pub keys: Vec<DrawKey>,
}

impl Default for DrawKeys {
    /// The fallback the original synthesises when the file is missing: 600 m everywhere.
    fn default() -> Self {
        Self { keys: vec![DrawKey { slice: 0, distance: 600.0 }, DrawKey { slice: 100_000, distance: 600.0 }] }
    }
}

impl DrawKeys {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let what = ".key";
        let version = u32_le(data, 0, what)?;
        if version != 1 {
            return Err(FormatError::UnsupportedVersion { what, found: version, supported: "1" });
        }
        let count = u32_le(data, 4, what)? as usize;
        if count > 4096 {
            return Err(FormatError::UnexpectedValue { what, offset: 4, expected: "<= 4096 keys".into(), found: count.to_string() });
        }
        let mut keys = Vec::with_capacity(count);
        for i in 0..count {
            let o = 8 + i * 8;
            let slice = u32_le(data, o, what)? as i32;
            let distance = f32_le(data, o + 4, what)?;
            if slice >= 0 {
                keys.push(DrawKey { slice, distance });
            }
        }
        keys.sort_by_key(|k| k.slice);
        if keys.is_empty() {
            return Ok(Self::default());
        }
        Ok(Self { keys })
    }

    /// Draw distance for a camera slice (step function, `valueAt` 0x44FCB0).
    pub fn value_at(&self, slice: usize) -> f32 {
        let s = slice as i32;
        let mut v = self.keys.first().map(|k| k.distance).unwrap_or(600.0);
        for k in &self.keys {
            if k.slice > s {
                break;
            }
            v = k.distance;
        }
        v
    }

    /// The value the renderer uses: `value_at × detail`, at least 50 m (0x44F1A0).
    pub fn far_distance(&self, slice: usize, detail: f32) -> f32 {
        (self.value_at(slice) * detail).max(50.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(keys: &[(i32, f32)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for (s, d) in keys {
            v.extend_from_slice(&s.to_le_bytes());
            v.extend_from_slice(&d.to_le_bytes());
        }
        v
    }

    #[test]
    fn step_function_and_detail() {
        let k = DrawKeys::parse(&file(&[(0, 500.0), (100, 320.0), (-1, 400.0), (250, 640.0)])).unwrap();
        assert_eq!(k.keys.len(), 3, "unused (-1) slots are dropped");
        assert_eq!(k.value_at(0), 500.0);
        assert_eq!(k.value_at(99), 500.0);
        assert_eq!(k.value_at(100), 320.0);
        assert_eq!(k.value_at(249), 320.0);
        assert_eq!(k.value_at(10_000), 640.0);
        assert_eq!(k.far_distance(100, 0.5), 160.0);
        assert_eq!(k.far_distance(100, 0.1), 50.0, "clamped to 50 m");
    }

    #[test]
    fn old_versions_are_rejected() {
        let mut d = file(&[(0, 500.0)]);
        d[0] = 2;
        assert!(matches!(DrawKeys::parse(&d), Err(FormatError::UnsupportedVersion { .. })));
    }
}
