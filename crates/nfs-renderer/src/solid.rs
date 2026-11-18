//! ENHANCEMENT E1 (FOLIAGE-OVERDRAW): the smallest alpha a texture can give a sample, and whether a mesh's vertices are all alpha 1.
//!
//! An alpha-tested draw (the race's `GREATER 10`, a material with `device_states`) whose texture's alpha never comes near the test's
//! reference, whose vertices all carry alpha 1 and whose object opacity is 1 never fails its test, and E1's alpha-to-coverage gives it
//! full coverage everywhere: its draw is the plain opaque one. Drawn with the plain opaque pipeline instead (no coverage, no
//! `discard`) the picture is the same and the GPU keeps its hidden-surface removal (`Core::solid_draws`, [`solid_alpha`]). The answers
//! here are conservative: a lower minimum only costs the faster path.

/// Whether a draw whose texels' alpha is at least `min_alpha` (bytes; any filtered sample lies between the texels' alphas) and whose
/// vertex alpha and object opacity are 1 always gets full coverage from `fs_opaque_a2c` under the reference `reference` (`shadow_on.w`:
/// the reference + 1/2 in bytes; 0 none). Its coverage is `clamp((a - ref) / fwidth(a) + 1/2, 0, 1)`; with every sample's `a` in
/// `[lo, 255]` the two screen derivatives are at most `255 - lo` each, so `fwidth(a) <= 2 (255 - lo)` and the coverage is 1 when
/// `lo - ref >= 255 - lo`: `lo >= (255 + ref) / 2` (one byte more against rounding). That also keeps `lo` above the test's reference
/// (`fs_opaque` never discards) and above half (the shadow pass's `fs_cut` without a test keeps the fragment).
pub(crate) fn solid_alpha(min_alpha: u8, reference: f32) -> bool {
    f32::from(min_alpha) >= (255.0 + reference.max(0.0)) * 0.5 + 1.0
}

/// The smallest alpha byte of every RGBA8 level (255 for none).
pub(crate) fn rgba_levels_min_alpha(levels: &[Vec<u8>]) -> u8 {
    levels.iter().map(|l| rgba_min_alpha(l)).min().unwrap_or(255)
}

/// The smallest alpha byte of one RGBA8 level.
pub(crate) fn rgba_min_alpha(level: &[u8]) -> u8 {
    level.chunks_exact(4).map(|p| p[3]).min().unwrap_or(255)
}

/// The smallest alpha any texel of the BC7 levels can decode to (a bound: the smallest alpha endpoint of each block).
pub(crate) fn bc7_levels_min_alpha(levels: &[Vec<u8>]) -> u8 {
    levels.iter().flat_map(|l| l.chunks_exact(16)).map(|b| bc7_block_min_alpha(b.try_into().expect("16-byte block"))).min().unwrap_or(255)
}

/// `n` bits of the 128-bit little-endian block from bit `at`.
fn bits(v: u128, at: u32, n: u32) -> u32 {
    ((v >> at) & ((1u128 << n) - 1)) as u32
}

/// A `n`-bit endpoint value widened to 8 bits as BC7 does (the top bits repeated below).
fn widen(v: u32, n: u32) -> u8 {
    ((v << (8 - n)) | (v >> (2 * n - 8))) as u8
}

/// One BC7 block's smallest possible alpha (the format's mode table: the mode is the lowest set bit of the first byte). Modes 0-3
/// carry no alpha (255); the others interpolate between endpoints with weights in `[0, 64]`, so no texel's alpha is below the
/// smallest alpha endpoint of the block. Modes 4 / 5 may rotate a colour channel into the alpha: then that channel's endpoints
/// count. The reserved mode 8 decodes to zero.
fn bc7_block_min_alpha(b: &[u8; 16]) -> u8 {
    let v = u128::from_le_bytes(*b);
    let mode = b[0].trailing_zeros();
    match mode {
        0..=3 => 255,
        4 | 5 => {
            // mode 4: mode bits 5, rotation 2, index mode 1, R0 R1 G0 G1 B0 B1 (5 bits each), A0 A1 (6 bits each)
            // mode 5: mode bits 6, rotation 2, R0 R1 G0 G1 B0 B1 (7 bits each), A0 A1 (8 bits each)
            let (start, cbits, abits) = if mode == 4 { (8, 5, 6) } else { (8, 7, 8) };
            let rot = bits(v, mode + 1, 2);
            let (at, n) = match rot {
                0 => (start + 6 * cbits, abits),
                r => (start + 2 * (r - 1) * cbits, cbits),
            };
            widen(bits(v, at, n), n).min(widen(bits(v, at + n, n), n))
        }
        6 => {
            // mode bits 7, R0 R1 G0 G1 B0 B1 A0 A1 (7 bits each), P0 P1: an endpoint's alpha is `A << 1 | P`
            let (a, p) = (7 + 6 * 7, 7 + 8 * 7);
            (0..2).map(|k| ((bits(v, a + 7 * k, 7) << 1) | bits(v, p + k, 1)) as u8).min().unwrap_or(0)
        }
        7 => {
            // mode bits 8, partition 6, R0..R3 G0..G3 B0..B3 A0..A3 (5 bits each), P0..P3: an alpha is `A << 1 | P` in 6 bits
            let a = 8 + 6 + 12 * 5;
            let p = a + 4 * 5;
            (0..4).map(|k| widen((bits(v, a + 5 * k, 5) << 1) | bits(v, p + k, 1), 6)).min().unwrap_or(0)
        }
        _ => 0,
    }
}

/// Every vertex of `bytes` (whole `Vertex` records: the colour's alpha is the last f32) has alpha exactly 1.
pub(crate) fn vertex_bytes_alpha_one(bytes: &[u8]) -> bool {
    const STRIDE: usize = std::mem::size_of::<crate::Vertex>();
    bytes.chunks_exact(STRIDE).all(|v| f32::from_le_bytes(v[STRIDE - 4..].try_into().expect("4 bytes")) == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block of `mode` with the bit fields `(at, n, value)` set over a zero background.
    fn block(mode: u32, fields: &[(u32, u32, u32)]) -> [u8; 16] {
        let mut v: u128 = 1u128 << mode;
        for &(at, n, val) in fields {
            v &= !(((1u128 << n) - 1) << at);
            v |= (u128::from(val) & ((1u128 << n) - 1)) << at;
        }
        v.to_le_bytes()
    }

    #[test]
    fn bc7_modes_without_alpha_are_opaque_and_mode_8_is_zero() {
        for m in 0..4 {
            assert_eq!(bc7_block_min_alpha(&block(m, &[])), 255);
        }
        assert_eq!(bc7_block_min_alpha(&[0; 16]), 0);
    }

    #[test]
    fn bc7_alpha_endpoints_bound_the_alpha() {
        // the field layouts end where the index bits take the rest of the 128
        assert_eq!(5 + 2 + 1 + 30 + 12 + 31 + 47, 128);
        assert_eq!(6 + 2 + 42 + 16 + 31 + 31, 128);
        assert_eq!(7 + 56 + 2 + 63, 128);
        assert_eq!(8 + 6 + 60 + 20 + 4 + 30, 128);
        let a4 = 8 + 30;
        assert_eq!(bc7_block_min_alpha(&block(4, &[(a4, 6, 63), (a4 + 6, 6, 63)])), 255);
        assert_eq!(bc7_block_min_alpha(&block(4, &[(a4, 6, 63), (a4 + 6, 6, 32)])), 130);
        // rotation 1: the red endpoints are the alpha (5 bits each)
        assert_eq!(bc7_block_min_alpha(&block(4, &[(5, 2, 1), (8, 5, 31), (13, 5, 16), (a4, 12, 0xFFF)])), 132);
        let a5 = 8 + 42;
        assert_eq!(bc7_block_min_alpha(&block(5, &[(a5, 8, 254), (a5 + 8, 8, 255)])), 254);
        // rotation 3: the blue endpoints (7 bits each)
        assert_eq!(bc7_block_min_alpha(&block(5, &[(6, 2, 3), (8 + 28, 7, 127), (8 + 35, 7, 127), (a5, 16, 0)])), 255);
        let (a6, p6) = (7 + 42, 7 + 56);
        assert_eq!(bc7_block_min_alpha(&block(6, &[(a6, 7, 127), (a6 + 7, 7, 127), (p6, 2, 3)])), 255);
        assert_eq!(bc7_block_min_alpha(&block(6, &[(a6, 7, 127), (a6 + 7, 7, 64), (p6, 2, 3)])), 129);
        let (a7, p7) = (8 + 6 + 60, 8 + 6 + 80);
        assert_eq!(bc7_block_min_alpha(&block(7, &[(a7, 20, 0xF_FFFF), (p7, 4, 15)])), 255);
        assert_eq!(bc7_block_min_alpha(&block(7, &[(a7, 20, 0x7_FFFF), (p7, 4, 15)])), 125);
    }

    #[test]
    fn the_bound_keeps_full_coverage() {
        // the race's reference 10 (+ 1/2): at least 134
        assert!(solid_alpha(255, 10.5));
        assert!(solid_alpha(134, 10.5));
        assert!(!solid_alpha(133, 10.5));
        // no test: above half for the shadow pass's own cut
        assert!(solid_alpha(129, 0.0) && !solid_alpha(128, 0.0));
        // with `lo` that high the worst case still covers fully: a = lo, fwidth = 2 (255 - lo)
        let lo = 134.0f32;
        assert!((lo - 10.5) / (2.0 * (255.0 - lo)) + 0.5 >= 1.0);
    }

    #[test]
    fn rgba_and_vertices() {
        assert_eq!(rgba_levels_min_alpha(&[vec![1, 2, 3, 255, 4, 5, 6, 200], vec![0, 0, 0, 255]]), 200);
        assert_eq!(rgba_levels_min_alpha(&[]), 255);
        let v = |a: f32| crate::Vertex { position: [0.0; 3], normal: [0.0; 3], uv: [0.0; 2], color: [0.5, 0.5, 0.5, a] };
        assert!(vertex_bytes_alpha_one(bytemuck::cast_slice(&[v(1.0), v(1.0)])));
        assert!(!vertex_bytes_alpha_one(bytemuck::cast_slice(&[v(1.0), v(0.99)])));
    }
}
