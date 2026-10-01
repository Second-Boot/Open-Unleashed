//! `GameData/Render/particle.dat` — the particle system definitions (`RE/notes/particles.md` §6),
//! loaded by 0x455570: a u32 count, then per system a u32 tag and the 0x7C-byte definition the
//! system keeps at `[+0x24]`. Offsets below are the definition's own (the file's less the tag's 4).

pub const RECORD: usize = 0x80;

#[derive(Debug, Clone, PartialEq)]
pub struct ParticleDef {
    /// the system's tag, the `ef` part's `+0x28` names it
    pub tag: u32,
    /// `+0x00` — 1 a floor at the emitter's height, 2 emitting only once triggered, 4 streaks
    pub flags: u32,
    /// `+0x04`
    pub count: u32,
    /// `+0x08…+0x10`: `x` and `z` spread the birth point, `y` is the speed along the second
    /// direction
    pub emit: [f32; 3],
    /// `+0x14…+0x1C`: the random parts along the first direction, of the speed, and along the axis
    pub rand_emit: [f32; 3],
    /// `+0x20`
    pub air_friction: f32,
    /// `+0x24`, the texture's FourCC stored byte-reversed (0x4542CA swaps it)
    pub material: String,
    /// `+0x28`
    pub blend: u32,
    /// `+0x2C`
    pub zbias: i32,
    /// `+0x30…+0x33`
    pub frames: u8,
    pub frame_speed: u8,
    pub states: u8,
    pub overall_size: u8,
    /// `+0x3C`, `+0x40` — a tenth of the ini's sizes; the draw multiplies by 10
    pub start_size: f32,
    pub end_size: f32,
    /// `+0x44…+0x50`
    pub lifetime: u32,
    pub period_on: u32,
    pub period_off: u32,
    pub prime: u32,
    /// `+0x54`, `+0x58`
    pub weight: f32,
    pub rand_weight: f32,
    /// `+0x64`
    pub rand_rotation: f32,
    /// `+0x68` — the fraction of the draw distance the emission fades from
    pub volume: f32,
    /// `+0x6C…+0x78`
    pub colours: [u32; 4],
}

use crate::dword;

fn float(r: &[u8], o: usize) -> f32 {
    f32::from_bits(dword(r, o))
}

/// Every definition of the file, in its order.
pub fn parse(data: &[u8]) -> Vec<ParticleDef> {
    if data.len() < 4 {
        return Vec::new();
    }
    let n = dword(data, 0) as usize;
    data[4..]
        .chunks_exact(RECORD)
        .take(n)
        .map(|rec| {
            let r = &rec[4..];
            let m = dword(r, 0x24).swap_bytes().to_le_bytes();
            ParticleDef {
                tag: dword(rec, 0),
                flags: dword(r, 0),
                count: dword(r, 4),
                emit: [float(r, 8), float(r, 0xC), float(r, 0x10)],
                rand_emit: [float(r, 0x14), float(r, 0x18), float(r, 0x1C)],
                air_friction: float(r, 0x20),
                material: m.iter().map(|&c| c as char).collect(),
                blend: dword(r, 0x28),
                zbias: dword(r, 0x2C) as i32,
                frames: r[0x30],
                frame_speed: r[0x31],
                states: r[0x32],
                overall_size: r[0x33],
                start_size: float(r, 0x3C),
                end_size: float(r, 0x40),
                lifetime: dword(r, 0x44),
                period_on: dword(r, 0x48),
                period_off: dword(r, 0x4C),
                prime: dword(r, 0x50),
                weight: float(r, 0x54),
                rand_weight: float(r, 0x58),
                rand_rotation: float(r, 0x64),
                volume: float(r, 0x68),
                colours: [dword(r, 0x6C), dword(r, 0x70), dword(r, 0x74), dword(r, 0x78)],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `particle.ini`'s first system, `Sparks`, as the file stores it.
    #[test]
    fn sparks() {
        let mut d = 1u32.to_le_bytes().to_vec();
        let mut rec = vec![0u8; RECORD];
        rec[0..4].copy_from_slice(&1263488339u32.to_le_bytes());
        rec[8..12].copy_from_slice(&20u32.to_le_bytes());
        rec[0x10..0x14].copy_from_slice(&0.06f32.to_le_bytes());
        rec[0x28..0x2C].copy_from_slice(&1397772875u32.to_le_bytes());
        rec[0x34..0x38].copy_from_slice(&[1, 10, 2, 0]);
        rec[0x40..0x44].copy_from_slice(&0.03f32.to_le_bytes());
        rec[0x48..0x4C].copy_from_slice(&100u32.to_le_bytes());
        rec[0x70..0x74].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        d.extend(rec);
        let s = &parse(&d)[0];
        assert_eq!((s.tag, s.count, s.emit[1], s.states, s.start_size, s.lifetime), (1263488339, 20, 0.06, 2, 0.03, 100));
        assert_eq!(s.material, "SPRK");
        assert_eq!(s.colours[0], 0xFFFF_FFFF);
    }
}
