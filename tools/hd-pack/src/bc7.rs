//! `hd-pack bc7`: the PNG pack to a KTX2 / BC7 pack, the alpha policy applied and the game's mip chain made.

use crate::common::{kind_of, list_pngs, par_for, png_bytes, read_png, write_atomic, Run};
use anyhow::{bail, Result};
use intel_tex_2::{bc7, RgbaSurface};
use nfs_assets::hdpack::{self, class, AlphaClass, Manifest};
use nfs_gfxdata::MipAlpha;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};


fn settings(quality: &str, alpha: bool) -> Result<bc7::EncodeSettings> {
    Ok(match (quality, alpha) {
        ("ultrafast", false) => bc7::opaque_ultra_fast_settings(),
        ("ultrafast", true) => bc7::alpha_ultra_fast_settings(),
        ("fast", false) => bc7::opaque_fast_settings(),
        ("fast", true) => bc7::alpha_fast_settings(),
        ("basic", false) => bc7::opaque_basic_settings(),
        ("basic", true) => bc7::alpha_basic_settings(),
        ("slow", false) => bc7::opaque_slow_settings(),
        ("slow", true) => bc7::alpha_slow_settings(),
        (q, _) => bail!("unknown quality {q} (ultrafast, fast, basic, slow)"),
    })
}

/// BC7-encode one level (`w x h` RGBA8); a side that is not a multiple of 4 is padded by replicating the edge texels.
fn encode_level(rgba: &[u8], w: u32, h: u32, s: &bc7::EncodeSettings) -> Vec<u8> {
    let (pw, ph) = (w.next_multiple_of(4), h.next_multiple_of(4));
    let padded: Vec<u8>;
    let data: &[u8] = if (pw, ph) == (w, h) {
        rgba
    } else {
        let mut v = vec![0u8; (pw * ph * 4) as usize];
        for y in 0..ph {
            for x in 0..pw {
                let (sx, sy) = (x.min(w - 1), y.min(h - 1));
                v[((y * pw + x) * 4) as usize..][..4].copy_from_slice(&rgba[((sy * w + sx) * 4) as usize..][..4]);
            }
        }
        padded = v;
        &padded
    };
    bc7::compress_blocks(s, &RgbaSurface { data, width: pw, height: ph, stride: pw * 4 })
}

#[derive(Default)]
struct Totals {
    kept_png: AtomicUsize,
    png_in: AtomicUsize,
    out: AtomicUsize,
    texels: AtomicUsize,
}

pub fn run(inp: &Path, out: &Path, m: &Manifest, threads: usize, quality: &str) -> Result<()> {
    settings(quality, true)?;
    let scale = hdpack::read_scale(inp).unwrap_or(4);
    let files = list_pngs(inp)?;
    std::fs::create_dir_all(out)?;
    write_atomic(&out.join("pack.txt"), hdpack::pack_txt(scale, &format!("format=ktx2/bc7 (png kept where a side is not a multiple of 4), hd-pack bc7 quality={quality}")).as_bytes())?;
    let t = Totals::default();
    let run = Run::new(files.len());
    par_for(&files, threads, |_, (key, path)| {
        let (k2, png) = (out.join(format!("{key}.ktx2")), out.join(format!("{key}.png")));
        if k2.is_file() || png.is_file() {
            run.skip();
            return;
        }
        let r = (|| -> Result<()> {
            let Some(row) = m.rows.get(key.as_str()) else { bail!("{key}: not in the manifest") };
            if !row.hd {
                bail!("{key}: a listed-only row (hd=no) has no place in a pack");
            }
            let (w, h, mut rgba) = read_png(path)?;
            if (w, h) != (row.w * scale, row.h * scale) {
                bail!("{key}: {w}x{h}, the game wants {}x{} (scale {scale} of the original)", row.w * scale, row.h * scale);
            }
            let kind = kind_of(Some(row), &rgba);
            kind.apply(&mut rgba);
            t.png_in.fetch_add(std::fs::metadata(path)?.len() as usize, Ordering::Relaxed);
            if w % 4 != 0 || h % 4 != 0 {
                // a BC texture's base level must be a whole number of blocks: keep the (policy-applied) PNG
                let b = png_bytes(w, h, &rgba)?;
                t.out.fetch_add(b.len(), Ordering::Relaxed);
                t.kept_png.fetch_add(1, Ordering::Relaxed);
                return write_atomic(&png, &b);
            }
            let has_alpha = kind != AlphaClass::Opaque;
            let s = settings(quality, has_alpha)?;
            // the chain the game would have made from the PNG: Test(10) for a texture with alpha (coverage kept), Blend otherwise;
            // the sky's layers are alpha-blended: Blend
            let levels_px: Vec<Vec<u8>> = if class::wants_chain(&row.class) {
                let mode = MipAlpha::for_texture(has_alpha && row.class == class::TRACK);
                std::iter::once(rgba.clone()).chain(nfs_gfxdata::mip_chain(&rgba, w, h, mode)).collect()
            } else {
                vec![rgba]
            };
            let (mut lw, mut lh) = (w, h);
            let mut blocks = Vec::with_capacity(levels_px.len());
            for px in &levels_px {
                blocks.push(encode_level(px, lw, lh, &s));
                t.texels.fetch_add((lw * lh) as usize, Ordering::Relaxed);
                lw = (lw / 2).max(1);
                lh = (lh / 2).max(1);
            }
            let file = nfs_assets::ktx2::write_bc7(w, h, &blocks);
            t.out.fetch_add(file.len(), Ordering::Relaxed);
            write_atomic(&k2, &file)
        })();
        run.report(key, r);
    });
    let secs = run.secs();
    let mib = |v: &AtomicUsize| v.load(Ordering::Relaxed) as f64 / 1048576.0;
    println!(
        "bc7: {} written ({} kept as PNG), {} already there, {} failed; PNG in {:.1} MiB -> out {:.1} MiB; {:.1} Mtexel (all levels) in {:.1}s = {:.1} Mtexel/s, {} threads -> {}",
        run.done(),
        t.kept_png.load(Ordering::Relaxed),
        run.skipped(),
        run.failed(),
        mib(&t.png_in),
        mib(&t.out),
        t.texels.load(Ordering::Relaxed) as f64 / 1e6,
        secs,
        t.texels.load(Ordering::Relaxed) as f64 / 1e6 / secs.max(1e-3) as f64,
        threads,
        out.display()
    );
    run.finish()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn bc7_round_trips_a_gradient_closely() {
        let (w, h) = (16u32, 16u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                px[((y * w + x) * 4) as usize..][..4].copy_from_slice(&[(x * 16) as u8, (y * 16) as u8, 128, 255]);
            }
        }
        let blocks = encode_level(&px, w, h, &settings("basic", false).unwrap());
        let back = nfs_assets::ktx2::decode_level(&blocks, w, h);
        let worst = px.iter().zip(&back).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 6, "worst channel error {worst}");
        assert!(back.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn a_two_by_two_level_pads_and_decodes() {
        let px = flat(2, 2, [40, 80, 120, 255]);
        let blocks = encode_level(&px, 2, 2, &settings("fast", false).unwrap());
        assert_eq!(blocks.len(), 16);
        let back = nfs_assets::ktx2::decode_level(&blocks, 2, 2);
        assert_eq!(back.len(), 16);
        assert!(back.chunks_exact(4).all(|p| (p[0] as i32 - 40).abs() <= 2 && (p[2] as i32 - 120).abs() <= 2));
    }
}

