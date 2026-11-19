//! ENHANCEMENT E28: the procedural lens-dirt mask (CPU image synthesis, no asset): [`generate_dirt`].

/// Width and height of the generated dirt texture (2:1; it is stretched over the screen).
pub(crate) const DIRT_SIZE: (u32, u32) = (1024, 512);

/// The dirt's layers (tuned by eye). Smudges: fractal value noise over `SMUDGE_CELLS` lattice cells, `SMUDGE_OCTAVES` octaves
/// (seeds from `SMUDGE_SEED`), normalised by the octaves' amplitude sum, thresholded by a smoothstep over `SMUDGE_EDGE`, times
/// `SMUDGE_GAIN`.
const SMUDGE_CELLS: [f32; 2] = [4.0, 2.0];
const SMUDGE_OCTAVES: u32 = 4;
const SMUDGE_SEED: u32 = 11;
const SMUDGE_NORM: f32 = 0.9375;
const SMUDGE_EDGE: [f32; 2] = [0.42, 0.78];
const SMUDGE_GAIN: f32 = 0.75;
/// A speck's disc fades over `DISC_EDGE` of its radius; its rim rises over `RIM_IN` and falls over `RIM_OUT`.
const DISC_EDGE: [f32; 2] = [0.85, 1.0];
const RIM_IN: [f32; 2] = [0.55, 0.9];
const RIM_OUT: [f32; 2] = [0.9, 1.05];
/// Greasy blobs: the count, the radius `BLOB_R + r BLOB_R_SPAN` (texels), the amplitude, the Gaussian's falloff `exp(-d^2 k)`,
/// the reach in radii.
const BLOBS: usize = 14;
const BLOB_R: f32 = 40.0;
const BLOB_R_SPAN: f32 = 70.0;
const BLOB_AMP: f32 = 0.12;
const BLOB_AMP_SPAN: f32 = 0.2;
const BLOB_FALLOFF: f32 = 3.0;
const BLOB_REACH: f32 = 2.0;
/// Dust specks and dried droplets: the count, the radius `SPECK_R + SPECK_R_SPAN r^3`, the amplitude, the rim of one wider
/// than `RIM_FROM_R` texels.
const SPECKS: usize = 420;
const SPECK_R: f32 = 1.3;
const SPECK_R_SPAN: f32 = 7.0;
const SPECK_AMP: f32 = 0.25;
const SPECK_AMP_SPAN: f32 = 0.75;
const RIM_FROM_R: f32 = 3.5;
const SPECK_RIM: f32 = 0.6;
/// A few big dried droplets: the count, radius, amplitude, rim.
const BIG_DROPS: usize = 6;
const BIG_R: f32 = 14.0;
const BIG_R_SPAN: f32 = 12.0;
const BIG_AMP: f32 = 0.30;
const BIG_AMP_SPAN: f32 = 0.25;
const BIG_RIM: f32 = 0.8;
/// Wipe arcs: the count, the arc's radius, sweep (rad), line width (texels), amplitude.
const ARCS: usize = 5;
const ARC_R: f32 = 150.0;
const ARC_R_SPAN: f32 = 300.0;
const ARC_SWEEP: f32 = 0.4;
const ARC_SWEEP_SPAN: f32 = 0.7;
const ARC_WIDTH: f32 = 2.0;
const ARC_WIDTH_SPAN: f32 = 4.0;
const ARC_AMP: f32 = 0.15;
const ARC_AMP_SPAN: f32 = 0.2;
/// The rim of the lens collects more: the gain `EDGE_GAIN + EDGE_GAIN_SPAN * min(r^2 / 2, 1)` over the screen's radius `r`.
const EDGE_GAIN: f32 = 0.6;
const EDGE_GAIN_SPAN: f32 = 0.9;
/// The generator's seed, its step (the golden ratio's 32-bit constant) and `2^24` (a 24-bit sample to 0..1).
const DIRT_SEED: u32 = 0x1d17_c0de;
const RNG_STEP: u32 = 0x9e37_79b9;
const U24: f32 = 16_777_216.0;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = hash(self.0.wrapping_add(RNG_STEP));
        (self.0 >> 8) as f32 / U24
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    nfs_math::smooth01(t)
}

/// Smooth value noise on an integer lattice, 0..1.
fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (nfs_math::smooth01(fx), nfs_math::smooth01(fy));
    let h = |dx: i32, dy: i32| (hash(seed ^ hash((xi as i32 + dx) as u32).wrapping_add(hash((yi as i32 + dy) as u32).wrapping_mul(31))) >> 8) as f32 / U24;
    let (a, b, c, d) = (h(0, 0), h(1, 0), h(0, 1), h(1, 1));
    (a + (b - a) * ux) + ((c + (d - c) * ux) - (a + (b - a) * ux)) * uy
}

/// ENHANCEMENT E28: a procedural lens-dirt mask, `width x height` values (0 = clean, 1 = the dirtiest
/// spot): soft smudges (fractal noise, thresholded), a few greasy blobs, many dust specks and dried
/// droplets (a faint disc with a brighter rim), and some wipe arcs, a little stronger toward the edges
/// (a lens collects its dirt at the rim). Deterministic: the same texture on every run.
pub(crate) fn generate_dirt(width: u32, height: u32) -> Vec<f32> {
    dirt_layers(width, height, true)
}

/// [`generate_dirt`]; `all` false = only the soft layers (smudges and greasy blobs), for the test that they have no
/// straight edge.
fn dirt_layers(width: u32, height: u32, all: bool) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let mut img = vec![0.0f32; w * h];
    let mut rng = Rng(DIRT_SEED);
    // smudges: thresholded fractal noise
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32 * SMUDGE_CELLS[0], y as f32 / h as f32 * SMUDGE_CELLS[1]);
            let mut f = 0.0;
            let mut a = 0.5;
            let mut fr = 1.0;
            for o in 0..SMUDGE_OCTAVES {
                f += a * vnoise(u * fr, v * fr, SMUDGE_SEED + o);
                a *= 0.5;
                fr *= 2.0;
            }
            let m = smoothstep(SMUDGE_EDGE[0], SMUDGE_EDGE[1], f / SMUDGE_NORM);
            img[y * w + x] = SMUDGE_GAIN * m * m;
        }
    }
    let put = |img: &mut [f32], cx: f32, cy: f32, r: f32, amp: f32, rim: f32| {
        let (x0, x1) = (((cx - r - 1.0).floor().max(0.0)) as usize, ((cx + r + 1.0).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - r - 1.0).floor().max(0.0)) as usize, ((cy + r + 1.0).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                // a faint disc and a brighter rim just inside its edge
                let disc = (1.0 - smoothstep(DISC_EDGE[0], DISC_EDGE[1], d)) * (1.0 - rim);
                let ring = smoothstep(RIM_IN[0], RIM_IN[1], d) * (1.0 - smoothstep(RIM_OUT[0], RIM_OUT[1], d)) * rim;
                let v = amp * (disc + ring);
                let p = &mut img[y * w + x];
                *p = p.max(v);
            }
        }
    };
    // greasy blobs (large, faint, soft)
    for _ in 0..BLOBS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = BLOB_R + rng.next() * BLOB_R_SPAN;
        let amp = BLOB_AMP + rng.next() * BLOB_AMP_SPAN;
        // ENH-SUNEDGE: the blob is written over 2 r round its centre, where the Gaussian is already below half a byte
        // (exp(-12) x 0.32 x 1.5 x 255 < 0.001). It was cut at 1 r, a square box: at its sides the mask jumped by
        // exp(-3) x amp (up to 6 of 255), and the bloom's last add multiplies the mask by DIRT_GAIN (70) x the glow,
        // so round the HDR sun a blob near the screen's left / right edge drew a hard straight-sided square
        let reach = BLOB_REACH * r;
        let (x0, x1) = (((cx - reach).floor().max(0.0)) as usize, ((cx + reach).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - reach).floor().max(0.0)) as usize, ((cy + reach).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                let g = (-d * d * BLOB_FALLOFF).exp() * amp;
                let p = &mut img[y * w + x];
                *p = (*p + g).min(1.0);
            }
        }
    }
    if !all {
        return img;
    }
    // dust specks and dried droplets
    for _ in 0..SPECKS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = SPECK_R + SPECK_R_SPAN * rng.next().powi(3);
        let amp = SPECK_AMP + SPECK_AMP_SPAN * rng.next();
        let rim = if r > RIM_FROM_R { SPECK_RIM } else { 0.0 };
        put(&mut img, cx, cy, r, amp, rim);
    }
    // a few big dried droplets
    for _ in 0..BIG_DROPS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        put(&mut img, cx, cy, BIG_R + rng.next() * BIG_R_SPAN, BIG_AMP + rng.next() * BIG_AMP_SPAN, BIG_RIM);
    }
    // wipe arcs: soft curved lines
    for _ in 0..ARCS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let rad = ARC_R + rng.next() * ARC_R_SPAN;
        let a0 = rng.next() * std::f32::consts::TAU;
        let sweep = ARC_SWEEP + rng.next() * ARC_SWEEP_SPAN;
        let width_px = ARC_WIDTH + rng.next() * ARC_WIDTH_SPAN;
        let amp = ARC_AMP + rng.next() * ARC_AMP_SPAN;
        let steps = (rad * sweep) as i32;
        for s in 0..steps {
            let a = a0 + sweep * s as f32 / steps as f32;
            let fade = (std::f32::consts::PI * s as f32 / steps as f32).sin();
            put(&mut img, cx + rad * a.cos(), cy + rad * a.sin(), width_px, amp * fade, 0.0);
        }
    }
    // a little stronger toward the edges
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32 * 2.0 - 1.0, y as f32 / h as f32 * 2.0 - 1.0);
            let e = EDGE_GAIN + EDGE_GAIN_SPAN * ((u * u + v * v) * 0.5).min(1.0);
            let p = &mut img[y * w + x];
            *p = (*p * e).clamp(0.0, 1.0);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dirt_is_deterministic_and_sparse() {
        let a = generate_dirt(DIRT_SIZE.0, DIRT_SIZE.1);
        assert_eq!(a, generate_dirt(DIRT_SIZE.0, DIRT_SIZE.1));
        assert_eq!(a.len(), (DIRT_SIZE.0 * DIRT_SIZE.1) as usize);
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        // mostly clean glass with a few smudges and specks: neither empty nor a veil
        assert!(mean > 0.01 && mean < 0.25, "mean {mean}");
        assert!(a.iter().any(|&v| v > 150.0 / 255.0), "some spot is dirty");
        assert!(a.iter().filter(|&&v| v < 8.0 / 255.0).count() > a.len() / 3, "much of it is clean");
    }

    #[test]
    fn the_soft_dirt_has_no_box_edge() {
        // ENH-SUNEDGE: the greasy blobs were cut by a square of 1 r: at its sides the mask jumped by exp(-3) x amp
        // (>= 0.006), a kink in the otherwise smooth soft layers (smudges, blobs) that ran straight for 2 r (80..220
        // pixels), which the dirt's gain round the HDR sun showed as a straight-sided glow. A jump is a large second
        // difference across the edge; none may run straight for 24 pixels
        let (w, h) = (DIRT_SIZE.0 as usize, DIRT_SIZE.1 as usize);
        let a = dirt_layers(DIRT_SIZE.0, DIRT_SIZE.1, false);
        let at = |x: usize, y: usize| a[y * w + x];
        let kink = |a0: f32, a1: f32, a2: f32, a3: f32| ((a2 - a1) - 0.5 * ((a1 - a0) + (a3 - a2))).abs() > 0.004;
        for x in 2..w - 1 {
            let mut run = 0;
            for y in 0..h {
                run = if kink(at(x - 2, y), at(x - 1, y), at(x, y), at(x + 1, y)) { run + 1 } else { 0 };
                assert!(run < 24, "a straight vertical edge at x {x}, y {y}");
            }
        }
        for y in 2..h - 1 {
            let mut run = 0;
            for x in 0..w {
                run = if kink(at(x, y - 2), at(x, y - 1), at(x, y), at(x, y + 1)) { run + 1 } else { 0 };
                assert!(run < 24, "a straight horizontal edge at x {x}, y {y}");
            }
        }
    }
}
