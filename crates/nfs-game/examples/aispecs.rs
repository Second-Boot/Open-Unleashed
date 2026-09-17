//! Diagnostic: the four `carSpecs` fields the AI reads (`RE/notes/ai.md` §9 item 2) across every car.
use nfs_assets::GamePaths;
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.game_data().join("Simulation").join("CarData");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("sim")).unwrap_or(false)).collect();
    files.sort();
    let (mut la, mut st, mut an, mut cl): (BTreeMap<String, usize>, BTreeMap<String, usize>, BTreeMap<String, usize>, BTreeMap<i32, usize>) = Default::default();
    let mut shown = 0;
    for p in &files {
        let Ok(d) = std::fs::read(p) else { continue };
        let Ok(s) = nfs_formats::sim::CarSim::parse(&d) else { continue };
        let t = &s.unknown_tail;
        let (a, b, c) = (t[19], t[20], t[21]);
        *la.entry(format!("{a:.3}")).or_default() += 1;
        *st.entry(format!("{b:.3}")).or_default() += 1;
        *an.entry(format!("{c:.3}")).or_default() += 1;
        *cl.entry(nfs_physics::specs::CarSpecs::from_sim(&s, false).class as i32).or_default() += 1;
        if (b - 1.0).abs() > 1e-3 || (c - 1.0).abs() > 1e-3 {
            println!("{:<16} 0x140 {b:>6.3}  0x144 {c:>6.3}", p.file_stem().unwrap().to_string_lossy());
        }
        if false && shown < 6 {
            println!("{:<16} 0x13C {a:>7.3}  0x140 {b:>7.3}  0x144 {c:>7.3}", p.file_stem().unwrap().to_string_lossy());
            shown += 1;
        }
    }
    println!("{} sims", files.len());
    println!("0x13C {la:?}");
    println!("0x140 {st:?}");
    println!("0x144 {an:?}");
    println!("class {cl:?}");
}
