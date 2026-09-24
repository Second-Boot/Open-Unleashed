//! Diagnostic: per embedded car texture, how many texels fall in each paint class — the tint slots,
//! and the untinted slots 0 / 10 / 20 (α ≈ 255 / 128 / 0, `car-model.md` §5) with their mean colour.
use nfs_assets::{car::CarAssets, texture::decode_pack, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for m in std::env::args().skip(1) {
        let Ok(c) = CarAssets::load(&paths, &m) else { continue };
        let crp_path = paths.car_model_dir().join(format!("{m}.crp"));
        for data in c.crp.embedded_texture_files(&c.crp_data).unwrap_or_default() {
            let Ok(images) = decode_pack(&crp_path, data) else { continue };
            for img in images {
                let px = &img.mips[0];
                let mut n = [0usize; 4];
                let mut sum = [[0u64; 3]; 3];
                for p in px.chunks_exact(4) {
                    let slot = 20 - (((p[3] as u32 + 1) * 20) >> 8);
                    let k = match slot { 0 => 0, 10 => 1, 20 => 2, _ => 3 };
                    n[k] += 1;
                    if k < 3 {
                        for i in 0..3 {
                            sum[k][i] += p[i] as u64;
                        }
                    }
                }
                let mean = |k: usize| if n[k] > 0 { [sum[k][0] / n[k] as u64, sum[k][1] / n[k] as u64, sum[k][2] / n[k] as u64] } else { [0; 3] };
                if std::env::var("SLOTS").is_ok_and(|w| w == img.name.trim()) {
                    let mut per = [(0usize, 0u64); 21];
                    for p in px.chunks_exact(4) {
                        let slot = (20 - (((p[3] as u32 + 1) * 20) >> 8)) as usize;
                        per[slot].0 += 1;
                        per[slot].1 += p[0] as u64;
                    }
                    for (k, (n, g)) in per.iter().enumerate().filter(|(_, (n, _))| *n > 0) {
                        println!("  slot {k}: {n} texels, mean grey {}", g / *n as u64);
                    }
                }
                println!("{m} {:<6} {}x{} slot0 {} {:?} slot10 {} {:?} slot20 {} {:?} tinted {}", img.name.trim(), img.width, img.height, n[0], mean(0), n[1], mean(1), n[2], mean(2), n[3]);
            }
        }
    }
}
