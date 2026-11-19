//! ENHANCEMENT E2: generated mip chains for the textures the original uploads with a single level
//! (the cars' texture pages, particle / flare / weather sprites). The track's own textures keep the
//! mips their `.fsh` carries; nothing here touches them.
//!
//! The chain is a 2x2 box filter on the CPU, once per upload (the uploads are one-off, D7), with
//! the colour weighted by alpha so a transparent texel's colour does not bleed into its
//! neighbours, and — for an alpha-tested texture — the alpha rescaled per level so that the
//! share of texels the test keeps (`GREATER` the device's reference, 10 for the cars:
//! `RE/notes/thrash.md` §6d.4) stays the level-0 share, otherwise a cut-out thins out with
//! distance.

/// The device's alpha-test reference, `GREATER` 10 (`RE/notes/thrash.md` §6d.4): the texels with `alpha > 10` stay.
pub const ALPHA_TEST_REF: u8 = 10;

/// What the texture's alpha is used for, which decides how the chain treats it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MipAlpha {
    /// blended or opaque: plain box average of alpha, colour weighted by alpha
    Blend,
    /// alpha-tested (`alpha > reference` keeps the texel): coverage-preserving alpha
    Test(u8),
}

impl MipAlpha {
    /// The rule every user of a texture shares: an alpha-tested one keeps its coverage against the device's reference
    /// ([`ALPHA_TEST_REF`]), the rest is blended / opaque.
    pub fn for_texture(alpha_tested: bool) -> Self {
        if alpha_tested { MipAlpha::Test(ALPHA_TEST_REF) } else { MipAlpha::Blend }
    }
}

/// One level down from `src` (`w x h` RGBA8) to `(w/2).max(1) x (h/2).max(1)`.
fn halve(src: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let (x0, y0) = ((2 * x).min(w - 1), (2 * y).min(h - 1));
            let (x1, y1) = ((2 * x + 1).min(w - 1), (2 * y + 1).min(h - 1));
            let taps = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            let (mut pr, mut pg, mut pb) = (0u32, 0u32, 0u32);
            for (tx, ty) in taps {
                let i = ((ty * w + tx) * 4) as usize;
                let p = &src[i..i + 4];
                let al = p[3] as u32;
                r += p[0] as u32;
                g += p[1] as u32;
                b += p[2] as u32;
                a += al;
                pr += p[0] as u32 * al;
                pg += p[1] as u32 * al;
                pb += p[2] as u32 * al;
            }
            let o = ((y * nw + x) * 4) as usize;
            if a > 0 {
                // alpha-weighted colour
                out[o] = ((pr + a / 2) / a) as u8;
                out[o + 1] = ((pg + a / 2) / a) as u8;
                out[o + 2] = ((pb + a / 2) / a) as u8;
            } else {
                out[o] = ((r + 2) / 4) as u8;
                out[o + 1] = ((g + 2) / 4) as u8;
                out[o + 2] = ((b + 2) / 4) as u8;
            }
            out[o + 3] = ((a + 2) / 4) as u8;
        }
    }
    (nw, nh, out)
}

/// The share of texels whose alpha is above `reference`.
fn coverage(px: &[u8], reference: u8) -> f32 {
    let n = px.len() / 4;
    if n == 0 {
        return 0.0;
    }
    px.chunks_exact(4).filter(|p| p[3] > reference).count() as f32 / n as f32
}

/// Scale the alpha channel up so [`coverage`] of the level reaches `target` (a bisection over the
/// scale, 1 ..= 8; a level that cannot reach it keeps the nearest).
fn rescale_alpha(px: &mut [u8], reference: u8, target: f32) {
    if target <= 0.0 || coverage(px, reference) >= target {
        // nothing to keep, or already covered enough (never thin a level by rescaling down)
        return;
    }
    let share = |scale: f32| -> f32 {
        let n = px.len() / 4;
        px.chunks_exact(4).filter(|p| ((p[3] as f32 * scale).min(255.0)) as u32 > reference as u32).count() as f32 / n as f32
    };
    let (mut lo, mut hi) = (1.0f32, 8.0f32);
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        if share(mid) < target {
            lo = mid
        } else {
            hi = mid
        }
    }
    for p in px.chunks_exact_mut(4) {
        p[3] = (p[3] as f32 * hi).round().min(255.0) as u8;
    }
}

/// Levels 1.. of the chain for `base` (`w x h` RGBA8), down to 1x1. Level 0 is not repeated.
pub fn chain(base: &[u8], w: u32, h: u32, mode: MipAlpha) -> Vec<Vec<u8>> {
    let mut levels = Vec::new();
    let target = match mode {
        MipAlpha::Test(r) => Some((r, coverage(base, r))),
        MipAlpha::Blend => None,
    };
    let (mut cw, mut ch) = (w, h);
    let mut prev: Vec<u8> = base.to_vec();
    while cw > 1 || ch > 1 {
        let (nw, nh, mut next) = halve(&prev, cw, ch);
        if let Some((r, cov)) = target {
            rescale_alpha(&mut next, r, cov);
        }
        levels.push(next.clone());
        prev = next;
        cw = nw;
        ch = nh;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_count(w: u32, h: u32) -> u32 {
        32 - w.max(h).max(1).leading_zeros()
    }

    #[test]
    fn counts_and_sizes() {
        assert_eq!(level_count(256, 256), 9);
        assert_eq!(level_count(256, 64), 9);
        assert_eq!(level_count(1, 1), 1);
        let c = chain(&[200u8; 8 * 4 * 4], 8, 4, MipAlpha::Blend);
        assert_eq!(c.len() as u32, level_count(8, 4) - 1);
        assert_eq!(c[0].len(), 4 * 2 * 4);
        assert_eq!(c.last().unwrap().len(), 4);
    }

    #[test]
    fn transparent_colour_does_not_bleed() {
        // one opaque red texel among transparent blue ones: the result is red, alpha a quarter
        let mut px = [0u8, 0, 255, 0].repeat(4);
        px[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let c = chain(&px, 2, 2, MipAlpha::Blend);
        assert_eq!(c[0], vec![255, 0, 0, 64]);
    }

    #[test]
    fn cutout_coverage_is_kept() {
        // one opaque texel per 16x16 block (a fence wire seen from afar): at level 3 the plain
        // box filter has alpha 255/64 = 4, under the test's reference 10, and the cut-out is gone;
        // the rescaled chain keeps the dots
        let n = 128usize;
        let mut px = vec![0u8; n * n * 4];
        for y in (0..n).step_by(16) {
            for x in (0..n).step_by(16) {
                px[(y * n + x) * 4 + 3] = 255;
            }
        }
        let plain = chain(&px, 128, 128, MipAlpha::Blend);
        let kept = chain(&px, 128, 128, MipAlpha::Test(10));
        assert_eq!(coverage(&plain[2], 10), 0.0);
        assert!(coverage(&kept[2], 10) > 0.0);
        for (p, k) in plain.iter().zip(&kept) {
            assert!(coverage(k, 10) >= coverage(p, 10));
        }
    }
}
