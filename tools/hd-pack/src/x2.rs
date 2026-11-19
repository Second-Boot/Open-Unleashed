//! `hd-pack x2`: the x2 pack from the x4 one (a deterministic 2:1 Lanczos-3 downscale).

use crate::common::{kind_of, list_pngs, par_for, png_bytes, read_png, write_atomic, Run};
use anyhow::{bail, Result};
use nfs_assets::hdpack::{self, AlphaClass, Manifest};
use std::path::Path;


fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f32::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// The 12 taps of a 2:1 Lanczos-3: output texel `o` covers source texels `2o` and `2o + 1` (centre at the edge between them),
/// the kernel stretched by the scale 2; tap `k` is source texel `2o + k - 5`.
fn taps() -> [f32; 12] {
    let mut w = [0f32; 12];
    for (k, t) in w.iter_mut().enumerate() {
        let i = k as f32 - 5.0;
        *t = lanczos3((i + 0.5 - 1.0) / 2.0);
    }
    let sum: f32 = w.iter().sum();
    w.iter_mut().for_each(|t| *t /= sum);
    w
}

/// One separable 2:1 pass over `nch` interleaved planes along x (`horizontal`) or y; edges wrap or clamp.
fn pass(src: &[f32], w: usize, h: usize, nch: usize, horizontal: bool, wrap: bool, taps: &[f32; 12]) -> (Vec<f32>, usize, usize) {
    let (ow, oh) = if horizontal { (w / 2, h) } else { (w, h / 2) };
    let mut out = vec![0f32; ow * oh * nch];
    let n = if horizontal { w } else { h } as i64;
    for y in 0..oh {
        for x in 0..ow {
            let o = (y * ow + x) * nch;
            let c = if horizontal { x } else { y } as i64 * 2;
            for (k, &t) in taps.iter().enumerate() {
                let mut i = c + k as i64 - 5;
                i = if wrap { i.rem_euclid(n) } else { i.clamp(0, n - 1) };
                let s = if horizontal { (y * w + i as usize) * nch } else { (i as usize * w + x) * nch };
                for ch in 0..nch {
                    out[o + ch] += t * src[s + ch];
                }
            }
        }
    }
    (out, ow, oh)
}

/// 2:1 downscale of an RGBA8 image by the alpha kind (see [`AlphaClass`]). Channels filtered: 0..3 "weighted" colour, 3 weight / alpha, 4..7
/// plain colour (the fallback where the weight vanishes).
fn downscale2(src: &[u8], w: usize, h: usize, kind: AlphaClass, wrap: [bool; 2]) -> Vec<u8> {
    const N: usize = 7;
    let mut planes = vec![0f32; w * h * N];
    for (i, p) in src.chunks_exact(4).enumerate() {
        let (r, g, b, a) = (p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32 / 255.0);
        let wgt = match kind {
            AlphaClass::Opaque => 1.0,
            AlphaClass::OneBit => (p[3] >= 128) as u8 as f32,
            AlphaClass::Smooth => a,
            AlphaClass::Levels(_) | AlphaClass::Slot => 1.0,
        };
        let o = i * N;
        planes[o..o + 7].copy_from_slice(&[r * wgt, g * wgt, b * wgt, if kind == AlphaClass::OneBit { wgt } else { a }, r, g, b]);
    }
    let t = taps();
    let (p, w1, _) = pass(&planes, w, h, N, true, wrap[0], &t);
    let (p, w2, h2) = pass(&p, w1, h, N, false, wrap[1], &t);
    let mut out = vec![0u8; w2 * h2 * 4];
    let q = |v: f32| (v + 0.5).floor().clamp(0.0, 255.0) as u8;
    for i in 0..w2 * h2 {
        let f = &p[i * N..i * N + N];
        let (rgb, a): ([f32; 3], u8) = match kind {
            AlphaClass::Opaque | AlphaClass::Levels(_) | AlphaClass::Slot => ([f[4], f[5], f[6]], if kind == AlphaClass::Opaque { 255 } else { q(f[3] * 255.0) }),
            AlphaClass::OneBit => {
                let vis = f[3] >= 0.5;
                // the weighted colour where something visible is under the footprint, the plain one otherwise
                let c = if f[3] > 1e-3 { [f[0] / f[3], f[1] / f[3], f[2] / f[3]] } else { [f[4], f[5], f[6]] };
                (c, if vis { 255 } else { 0 })
            }
            AlphaClass::Smooth => {
                let a = f[3].clamp(0.0, 1.0);
                let c = if a > 1e-3 { [f[0] / f[3], f[1] / f[3], f[2] / f[3]] } else { [f[4], f[5], f[6]] };
                (c, q(a * 255.0))
            }
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&[q(rgb[0]), q(rgb[1]), q(rgb[2]), a]);
    }
    out
}

pub fn run(inp: &Path, out: &Path, m: &Manifest, threads: usize) -> Result<()> {
    if let Some(s) = hdpack::read_scale(inp)
        && s != 4
    {
        bail!("{} is a scale={s} pack: x2 is made from the x4 one", inp.display());
    }
    let files = list_pngs(inp)?;
    std::fs::create_dir_all(out)?;
    write_atomic(&out.join("pack.txt"), hdpack::pack_txt(2, "source=x4 downscaled 2:1 (Lanczos-3, alpha by the original's policy), hd-pack x2").as_bytes())?;
    let run = Run::new(files.len());
    par_for(&files, threads, |_, (key, path)| {
        let target = out.join(format!("{key}.png"));
        if target.is_file() {
            run.skip();
            return;
        }
        let r = (|| -> Result<()> {
            let (w, h, rgba) = read_png(path)?;
            if w % 2 != 0 || h % 2 != 0 {
                bail!("{key}: {w}x{h} is not even");
            }
            if let Some(row) = m.rows.get(key)
                && (w, h) != (row.w * 4, row.h * 4)
            {
                bail!("{key}: {w}x{h} is not 4x the manifest's {}x{} (not an x4 pack image)", row.w, row.h);
            }
            let row = m.rows.get(key);
            let wrap = row.map_or([false, false], |r| match r.wrap.as_str() {
                "wrap/wrap" => [true, true],
                _ => [false, false],
            });
            let small = downscale2(&rgba, w as usize, h as usize, kind_of(row, &rgba), wrap);
            write_atomic(&target, &png_bytes(w / 2, h / 2, &small)?)
        })();
        run.report(key, r);
    });
    println!("x2: {} written, {} already there, {} failed, {:.1}s -> {}", run.done(), run.skipped(), run.failed(), run.secs(), out.display());
    run.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn a_flat_image_stays_flat() {
        let out = downscale2(&flat(8, 8, [10, 200, 90, 255]), 8, 8, AlphaClass::Opaque, [false, false]);
        assert_eq!(out.len(), 4 * 4 * 4);
        assert!(out.chunks_exact(4).all(|p| p == [10, 200, 90, 255]));
    }

    #[test]
    fn taps_are_normalised_and_symmetric() {
        let t = taps();
        assert!((t.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for k in 0..6 {
            assert!((t[k] - t[11 - k]).abs() < 1e-6);
        }
    }

    #[test]
    fn one_bit_alpha_stays_one_bit_and_colour_does_not_bleed() {
        // left half opaque red, right half transparent with a green colour under it: the visible texels stay red
        let mut src = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let p = if x < 4 { [255, 0, 0, 255] } else { [0, 255, 0, 0] };
                src[(y * 8 + x) * 4..][..4].copy_from_slice(&p);
            }
        }
        let out = downscale2(&src, 8, 8, AlphaClass::OneBit, [false, false]);
        for p in out.chunks_exact(4) {
            assert!(p[3] == 0 || p[3] == 255);
            if p[3] == 255 {
                assert!(p[0] > 250 && p[1] < 5, "visible texel keeps its colour: {p:?}");
            }
        }
        assert_eq!(out[3], 255);
        assert_eq!(out[(3) * 4 + 3], 0);
    }

    #[test]
    fn soft_alpha_is_premultiplied() {
        // an opaque white texel next to a transparent black one: the blended texel is white at half alpha, not grey
        let mut src = vec![0u8; 2 * 2 * 4];
        for (i, p) in [[255u8, 255, 255, 255], [0, 0, 0, 0], [255, 255, 255, 255], [0, 0, 0, 0]].iter().enumerate() {
            src[i * 4..i * 4 + 4].copy_from_slice(p);
        }
        // a 2x2 image is too small for the 12-tap window to see anything but clamped edges: tile it to 8x8
        let mut big = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                big[(y * 8 + x) * 4..][..4].copy_from_slice(&src[((y % 2) * 2 + x % 2) * 4..][..4]);
            }
        }
        let out = downscale2(&big, 8, 8, AlphaClass::Smooth, [true, true]);
        let p = &out[(2 * 4 + 2) * 4..][..4];
        assert!(p[3] > 100 && p[3] < 160, "half alpha: {p:?}");
        assert!(p[0] > 250, "colour stays white: {p:?}");
    }
}
