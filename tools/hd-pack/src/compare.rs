//! `hd-pack compare`: the quality of a BC7 pack against its PNGs.

use crate::common::{kind_of, list_pngs, read_png};
use anyhow::{bail, Context, Result};
use nfs_assets::hdpack::{AlphaClass, Manifest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};


pub fn run(pngs: &Path, ktx: &Path, m: &Manifest, limit: usize) -> Result<()> {
    let files: Vec<(String, PathBuf)> = list_pngs(pngs)?.into_iter().filter(|(k, _)| ktx.join(format!("{k}.ktx2")).is_file()).take(limit).collect();
    let (mut n, mut sum_psnr, mut worst) = (0usize, 0f64, (f64::MAX, String::new()));
    // per alpha kind: (textures, texels, texels that change side of the 128 test, max alpha error)
    let mut by_kind: HashMap<&str, (usize, usize, usize, i32)> = HashMap::new();
    for (key, path) in &files {
        let (w, h, mut a) = read_png(path)?;
        let row = m.rows.get(key.as_str());
        let kind = kind_of(row, &a);
        kind.apply(&mut a);
        let img = nfs_assets::ktx2::read_bc7(&std::fs::read(ktx.join(format!("{key}.ktx2")))?).with_context(|| key.clone())?;
        if (img.width, img.height) != (w, h) {
            bail!("{key}: size mismatch");
        }
        let b = nfs_assets::ktx2::decode_level(&img.levels[0], w, h);
        let (mut se, mut cnt) = (0f64, 0usize);
        let name = match kind {
            AlphaClass::Opaque => "opaque",
            AlphaClass::OneBit => "1-bit",
            AlphaClass::Smooth => "smooth",
            AlphaClass::Levels(_) | AlphaClass::Slot => "levels / slot",
        };
        let e = by_kind.entry(name).or_default();
        e.0 += 1;
        for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            // colour error where the texel is visible (an invisible texel's colour is never seen)
            if p[3] >= 128 || kind == AlphaClass::Opaque {
                for c in 0..3 {
                    let d = p[c] as f64 - q[c] as f64;
                    se += d * d;
                }
                cnt += 3;
            }
            e.1 += 1;
            e.3 = e.3.max((p[3] as i32 - q[3] as i32).abs());
            if (p[3] >= 128) != (q[3] >= 128) {
                e.2 += 1;
            }
        }
        let mse = se / cnt.max(1) as f64;
        let psnr = if mse < 1e-9 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
        sum_psnr += psnr;
        n += 1;
        if psnr < worst.0 {
            worst = (psnr, key.clone());
        }
    }
    println!("compare: {n} textures; mean PSNR {:.2} dB (colour of the visible texels); worst {:.2} dB ({})", sum_psnr / n.max(1) as f64, worst.0, worst.1);
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort();
    for (k, (t, texels, flips, maxerr)) in kinds {
        println!("  alpha {k}: {t} textures, {texels} texels, {flips} change side of the 128 test, max alpha error {maxerr}");
    }
    Ok(())
}

