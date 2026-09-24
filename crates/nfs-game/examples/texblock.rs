//! Diagnostic: the state block (`lod_bias`, `address`) of every track material naming a texture,
//! filtered by a substring of the texture's name, with the material's key dword `mt+0x24`; and the
//! key `(+0x18, +0x1C)` each kind-4 projector looks its texture up by (0x44B000 → 0x44D320, a
//! lookup only — `RE/notes/thrash.md` §7).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let f = a.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
    let dw = |d: &[u8], o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    for name in a[0].split(',') {
        let Ok(t) = TrackAssets::load(&paths, name) else { continue };
        let mts: Vec<_> = t.crp.material_entries.iter().filter(|e| e.is_part(b"mt")).collect();
        for (i, m) in t.crp.materials(&t.crp_data).unwrap().iter().enumerate() {
            if m.texture_name().to_lowercase().contains(&f) {
                let key = mts.get(i).and_then(|e| t.crp_data.get(e.data_offset()..e.data_offset() + e.data_len())).map(|b| dw(b, 0x24));
                println!("{name} mt{i} {:?} key {key:#x?} {} bias {} address {:?}", m.texture_name(), m.render_method, m.lod_bias, m.address);
            }
        }
        let mut keys = std::collections::BTreeMap::new();
        for art in t.crp.articles(&t.crp_data).unwrap_or_default() {
            for p in art.parts.iter().filter(|p| p.is_part(b"ef")) {
                let Ok(d) = t.crp.entry_data(&t.crp_data, p) else { continue };
                if d.len() >= 0x48 && dw(d, 0) == 4 {
                    let tex: String = d[0x1C..0x20].iter().map(|&c| c as char).collect();
                    *keys.entry((dw(d, 0x18), tex)).or_insert(0) += 1;
                }
            }
        }
        for ((k, tex), n) in keys {
            println!("{name} projector key {k:#x} {tex:?} x{n}");
        }
    }
}
