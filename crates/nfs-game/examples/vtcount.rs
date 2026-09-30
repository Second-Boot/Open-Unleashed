//! Census: the `vt` parts' own count field (`[part+8]`, the loop bound of 0x47B660's item
//! walk, 0x47B80A…0x47B868) against `data_len / 16`, and how many vertices the fitted tree's first-LOD
//! `vt` population holds per shipped track.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::crp::CrpEntry;

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in ["alps", "autobahn", "canyon", "castle", "coastal", "farmland", "foothills", "forest", "industrial", "monaco1", "monaco2", "monaco3", "monaco4", "monaco5", "skidpad"] {
        let Ok(t) = TrackAssets::load(&paths, name) else { continue };
        let arts = t.crp.articles(&t.crp_data).unwrap();
        let (mut parts, mut mismatched, mut total, mut items) = (0usize, 0usize, 0usize, 0usize);
        for a in &arts {
            for p in a.parts.iter().filter(|p| p.is_part(b"vt")) {
                parts += 1;
                if let CrpEntry::Part { b, .. } = p
                    && *b as usize * 16 != p.data_len()
                {
                    mismatched += 1;
                }
            }
            let Some(base) = a.level(b"Base").and_then(|e| t.crp.entry_data(&t.crp_data, e).ok()) else { continue };
            if base.len() < 0x70 {
                continue;
            }
            let flags = u32::from_le_bytes([base[0], base[1], base[2], base[3]]);
            if flags & 0x8000 != 0 {
                continue;
            }
            let k0 = u32::from_le_bytes([base[8], base[9], base[10], base[11]]) as usize;
            let o = 0x64 + 12 * k0;
            let Some(w) = base.get(o + 8..o + 10) else { continue };
            let lod = u16::from_le_bytes([w[0], w[1]]);
            if let Some(vt) = a.find(b"vt", lod) {
                items += 1;
                total += vt.data_len() / 16;
            }
        }
        println!("{name}: vt parts {parts}, count field != len/16: {mismatched}; items with a first-LOD vt {items}, vertices {total}");
    }
}
