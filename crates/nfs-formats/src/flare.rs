//! `GameData/Render/flare.dat` — the flare definitions a track's kind-2 `ef` parts name
//! (`RE/notes/crp-geometry.md` §8). The loader 0x4522B0 / 0x452120 keeps them in `[0x61BCC0]`,
//! searched by 0x452510 on the id.
//!
//! A record is **0x8C** bytes: `+0` the id (a FourCC dword, compared as stored), `+4` / `+8` the
//! two layers' textures — FourCCs stored byte-reversed, which 0x452120 swaps (`KIPS` → `SPIK`)
//! before looking the texture up, `"null"` for none (0x452132) — `+0x1C` / `+0x20` the layers'
//! colours (ARGB), `+0x34` / `+0x38` their sizes (the draw takes a tenth, 0x451BFA), `+0x4C` the
//! flags, `+0x50` the blink phase, `+0x54` the ticks on and `+0x58` the ticks off.

pub const RECORD: usize = 0x8C;

#[derive(Debug, Clone, PartialEq)]
pub struct FlareDef {
    /// `+0`, the id as the file stores it (the `ef` part's `+0x1C` is compared against it)
    pub id: u32,
    /// the two layers' texture names, already swapped the loader's way; `None` for `"null"`
    pub textures: [Option<String>; 2],
    pub colours: [u32; 2],
    pub sizes: [f32; 2],
    pub flags: u32,
    pub phase: i32,
    pub on: i32,
    pub off: i32,
}

use crate::dword;

/// Every whole record of the file, in file order.
pub fn parse(data: &[u8]) -> Vec<FlareDef> {
    data.chunks_exact(RECORD)
        .map(|r| {
            let tex = |o: usize| {
                let v = dword(r, o);
                // 0x452132: the stored dword "llun" read little-endian is 0x6E756C6C
                if v == 0x6E75_6C6C {
                    return None;
                }
                let b = v.swap_bytes().to_le_bytes();
                Some(b.iter().map(|&c| c as char).collect::<String>())
            };
            FlareDef {
                id: dword(r, 0),
                textures: [tex(4), tex(8)],
                colours: [dword(r, 0x1C), dword(r, 0x20)],
                sizes: [f32::from_bits(dword(r, 0x34)), f32::from_bits(dword(r, 0x38))],
                flags: dword(r, 0x4C),
                phase: dword(r, 0x50) as i32,
                on: dword(r, 0x54) as i32,
                off: dword(r, 0x58) as i32,
            }
        })
        .collect()
}

impl FlareDef {
    /// 0x4519B3…0x4519E7: a flare with an on or off time shows while
    /// `(phase + (flags & 0x10 ? the part's own phase : 0) + gGameTick) mod (on + off) ≤ on`
    /// (an unsigned division, a signed compare).
    pub fn lit(&self, part_phase: u32, tick: u32) -> bool {
        if self.on == 0 && self.off == 0 {
            return true;
        }
        let own = if self.flags & 0x10 != 0 { part_phase } else { 0 };
        let t = (self.phase as u32).wrapping_add(own).wrapping_add(tick);
        let period = (self.on as u32).wrapping_add(self.off as u32);
        if period == 0 {
            return true;
        }
        ((t % period) as i32) <= self.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_blink() {
        let mut r = vec![0u8; RECORD];
        r[0..4].copy_from_slice(b"ypml");
        r[4..8].copy_from_slice(b"RALG");
        r[8..12].copy_from_slice(b"llun");
        r[0x1C..0x20].copy_from_slice(&0xB5FF_FFFFu32.to_le_bytes());
        r[0x34..0x38].copy_from_slice(&8.7f32.to_le_bytes());
        r[0x4C] = 4;
        r[0x54] = 21;
        r[0x58] = 50;
        let d = &parse(&r)[0];
        assert_eq!(d.id, u32::from_le_bytes(*b"ypml"));
        assert_eq!(d.textures, [Some("GLAR".to_string()), None]);
        assert_eq!((d.colours[0], d.sizes[0], d.flags), (0xB5FF_FFFF, 8.7, 4));
        assert!(d.lit(0, 0) && d.lit(0, 21) && !d.lit(0, 22) && !d.lit(0, 70) && d.lit(0, 71));
    }
}
