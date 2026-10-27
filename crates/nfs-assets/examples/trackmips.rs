//! Diagnostic (ENHANCEMENT E2): per track, how many of its `.fsh` / sky textures the data ships with
//! fewer levels than the original uploads (`texture::build_levels`) — those get a generated chain.
use nfs_assets::{
    texture::{build_levels, decode_pack_mipped},
    GamePaths,
};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for t in std::env::args().skip(1) {
        let dir = paths.track_dir();
        let Some(p) = GamePaths::find_file(&dir, &format!("{t}.fsh")) else {
            println!("{t}: no fsh");
            continue;
        };
        let data = nfs_formats::refpack::maybe_decompress(&std::fs::read(&p).unwrap()).unwrap().into_owned();
        let tex = decode_pack_mipped(&p, &data).unwrap();
        let single = tex.iter().filter(|x| x.mips.len() == 1 && x.width.max(x.height) > 1).count();
        let short = tex.iter().filter(|x| (x.mips.len() as u32) < build_levels(x.width)).count();
        let alpha1 = tex.iter().filter(|x| x.mips.len() == 1 && x.has_alpha).count();
        let mut dims: std::collections::BTreeMap<(u32, u32), u32> = Default::default();
        for x in tex.iter().filter(|x| x.mips.len() == 1) {
            *dims.entry((x.width, x.height)).or_insert(0) += 1;
        }
        if let Some(sp) = GamePaths::find_file(&dir.join("Sky"), &format!("{t}.fsh")) {
            let d = nfs_formats::refpack::maybe_decompress(&std::fs::read(&sp).unwrap()).unwrap().into_owned();
            let sky = nfs_assets::texture::decode_pack(&sp, &d).unwrap();
            let list: Vec<String> = sky.iter().map(|x| format!("{}:{}x{}", x.name.trim(), x.width, x.height)).collect();
            println!("   sky (single level, as the original uploads it): {}", list.join(" "));
        }
        println!("{t}: total {} single {single} (alpha {alpha1}) short-of-original {short} dims {dims:?}", tex.len());
    }
}
