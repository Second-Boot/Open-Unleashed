//! Diagnostic: per render method, how many car `mt` records carry the `R3DLightLayer` parameters
//! (`Material::light`) and how many do not (`RE/notes/lighting.md` §5.6).
use nfs_formats::refpack;

fn main() {
    let mut tally = std::collections::BTreeMap::<(String, bool), usize>::new();
    let list = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    for p in list.lines() {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let Ok(mats) = crp.materials(&data) else { continue };
        for m in mats {
            *tally.entry((m.render_method.clone(), m.light.is_some())).or_default() += 1;
        }
    }
    for ((rm, l), n) in tally {
        println!("{rm:14} light={l:5} x{n}");
    }
}
