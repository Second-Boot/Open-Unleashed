//! Diagnostic: every page source the car page builder 0x443F60 reads, over every shipped car
//! (`GameData/CarModel/*.tpg`) and front-end room (`FEData/Models/*.tpg`) — its SHPI format, its
//! page's `.tpg` `bpp`, its tint flag (0x439EE0: `[fileN.details] tint`, default 1, overridden by
//! the entry's own section) — and, for an 8-bit source whose flag is set (0x444130's palette path),
//! whether any texel uses index 255, the palette entry that path never writes (`car-model.md` §5.4).
//! `pagesrc` prints a tally, every such index-255 user and every source that is neither 8-bit nor
//! 32-bit (the 16-bit 0x78 / 0x7E / 0x6D paths, and 24-bit 0x7F).
use nfs_assets::{car::CarAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    const GLOBALS: [&str; 5] = ["Shadow.fsh", "INTGLASS.FSH", "Suit.fsh", "Head.fsh", "Cabrio.fsh"];
    let mut tally: BTreeMap<(String, i64, bool), usize> = BTreeMap::new();
    for dir in [paths.car_model_dir(), paths.fe_data().join("Models")] {
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.to_lowercase().ends_with(".tpg"))
            .map(|n| n[..n.len() - 4].to_string())
            .collect();
        names.sort();
        for model in names {
            let Ok(car) = CarAssets::load_in(&dir, &model) else {
                println!("skip {model} (does not load)");
                continue;
            };
            let numfiles = car.tpg.section("header").and_then(|h| h.get_i64("numfiles")).unwrap_or(0);
            let embedded = car.crp.embedded_texture_files(&car.crp_data).unwrap_or_default();
            let sf: Vec<u16> = car.crp.material_entries.iter().filter(|e| e.is_part(b"sf")).map(|e| e.part_index()).collect();
            for file_no in 1..=numfiles {
                let Some(det) = car.tpg.section(&format!("file{file_no}.details")) else { continue };
                let Some(page) = det.get_i64("tpage") else { continue };
                let bpp = car.tpg.section(&format!("tpage{page}.details")).and_then(|d| d.get_i64("bpp")).unwrap_or(1555);
                let owned;
                let data: &[u8] = if let Some(g) = det.get_i64("global").filter(|g| *g > 0) {
                    let Some(p) = GLOBALS.get(g as usize - 1).and_then(|n| GamePaths::find_file(&paths.car_model_dir(), n)) else { continue };
                    owned = nfs_formats::refpack::maybe_decompress(&std::fs::read(p).unwrap()).unwrap().into_owned();
                    &owned
                } else {
                    match sf.iter().position(|&i| i as i64 + 1 == file_no) {
                        Some(k) => embedded[k],
                        None => continue,
                    }
                };
                let file_tint = det.get_i64("tint").unwrap_or(1) != 0;
                let Ok(fsh) = nfs_formats::fsh::Fsh::parse(data) else { continue };
                for (ei, e) in fsh.entries.iter().enumerate() {
                    let tint = car.tpg.section(&format!("file{file_no}.{}", e.name.trim_end())).and_then(|s| s.get_i64("tint")).map(|t| t != 0).unwrap_or(file_tint);
                    let b = &e.blocks[0];
                    let fmt = format!("{:#04x}", b.format_code());
                    *tally.entry((fmt.clone(), bpp, tint)).or_default() += 1;
                    if b.is_compressed() {
                        println!("{model} file{file_no} {} compressed source {fmt} (code {:#04x})", e.name.trim(), b.code);
                    }
                    if !matches!(b.format_code(), 0x7B | 0x7D) {
                        println!("{model} file{file_no} {} source {fmt} page bpp {bpp}", e.name.trim());
                    }
                    if b.format_code() == 0x7B && tint {
                        let len = b.width as usize * b.height as usize;
                        let off = b.offset + 16;
                        let px: Vec<u8> = if b.is_compressed() {
                            let n = fsh.image_block_len(data.len(), ei);
                            nfs_formats::refpack::decompress(&data[off..b.offset + n]).unwrap()
                        } else {
                            data[off..off + len].to_vec()
                        };
                        let n255 = px[..len.min(px.len())].iter().filter(|&&i| i == 255).count();
                        let pal = e.blocks.iter().find(|b| matches!(b.code, 0x2A | 0x29 | 0x2D)).map(|p| p.width as usize * p.height.max(1) as usize);
                        println!("{model} file{file_no} {} 8-bit page bpp {bpp}: {n255} texels of index 255, palette {pal:?} colours", e.name.trim());
                        if pal.is_none() {
                            let chain: Vec<String> = e.blocks.iter().map(|b| format!("{:#04x}@{:#x}+{:#x}", b.code, b.offset, b.size)).collect();
                            let names: Vec<String> = fsh.entries.iter().map(|x| format!("{}@{:#x}", x.name.trim(), x.offset)).collect();
                            println!("    no palette block: chain {chain:?}; file {:#x} bytes, entries {names:?}", data.len());
                            let used = &px[..len.min(px.len())];
                            let max = used.iter().copied().max().unwrap_or(0);
                            let past = used.iter().filter(|&&i| i >= 192).count();
                            println!("    indices: max {max}, {past} texels at 192 or above (dword past a 256 x 3-byte palette)");
                        }
                    }
                }
            }
        }
    }
    for ((fmt, bpp, tint), n) in tally {
        println!("source {fmt} page {bpp} tint {tint}: {n} entries");
    }
}
