//! Diagnostic: the `Controller.*` bindings a profile `.sav` carries (six fields each).
fn main() {
    for p in std::env::args().skip(1) {
        let Ok(d) = std::fs::read(&p) else { continue };
        let Ok(s) = nfs_formats::sav::SaveFile::parse(&d) else { continue };
        println!("# {p}");
        for (name, f) in nfs_game::save::saved_controls(&s) {
            println!("{name:<28} {f:?}");
        }
    }
}
