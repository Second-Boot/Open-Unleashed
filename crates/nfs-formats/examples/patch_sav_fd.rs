//! One-off tool for a reference-capture pass (RE/notes/REFERENCE-2026-11-17-ISS4.md): build test
//! profiles the original and the port both load. TEST GLUE, not the original's.
//!
//! Usage: `patch_sav_fd <in.sav> <out.sav> [pass=N] [name=NAME] [cars=ID,ID,...] [seenfd=1] [list=<nfs5.car>]`
//!
//! * `pass=N` — missions 0..N marked passed: `+0xE8` (the last attempt's result, 0x51AF45) of the first N records of
//!   the `Missions` image set to 1, so 0x51AE00 answers the stage of record N (savegame.md §4.1, career.md §5).
//! * `name=NAME` — `PlayerInfo`'s player and driver names (`[0x691DC8]`, `[0x691DE8]`); the file stem is
//!   `<picture><NAME>` (0x4DEDA0), so name the output file to match.
//! * `cars=...` — the garage replaced by copies of the first car with the `nfs5.car` record of each id put at
//!   `+0x04` (`FECar::init` 0x4EEBDA copies the record there); needs `list=` for the catalogue path.
//! * `seenfd=1` — `seen_factory_welcome` / `seen_first_mission` set (`[0x691D94]`, `[0x691DF5]`).
use nfs_formats::sav::SaveFile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (in_path, out_path) = (&args[1], &args[2]);
    let opt = |k: &str| args[3..].iter().find_map(|a| a.strip_prefix(&format!("{k}=")).map(str::to_string));
    let data = std::fs::read(in_path).expect("read input .sav");
    let mut save = SaveFile::parse(&data).expect("parse .sav");
    if let Some(n) = opt("pass").map(|s| s.parse::<usize>().expect("pass=N")) {
        let m = save.missions.as_mut().expect("Missions module present");
        for i in 0..n {
            let o = i * 0xF0 + 0xE8;
            m[o..o + 2].copy_from_slice(&1u16.to_le_bytes());
        }
    }
    let player = save.player.as_mut().expect("PlayerInfo module present");
    if let Some(name) = opt("name") {
        player.player_name = name.clone();
        player.driver_name = name;
    }
    if opt("seenfd").is_some() {
        player.seen_factory_welcome = 1;
        player.seen_first_mission = 1;
    }
    if let Some(ids) = opt("cars") {
        let cat = std::fs::read(opt("list").expect("list=<nfs5.car>")).expect("read nfs5.car");
        let template = player.cars[0].clone();
        player.cars = ids
            .split(',')
            .map(|s| {
                let id: u16 = s.parse().expect("car id");
                let rec = cat.chunks_exact(nfs_formats::fedata::CAR_RECORD_SIZE).find(|r| u16::from_le_bytes([r[0], r[1]]) == id).expect("car id in nfs5.car");
                let mut c = template.clone();
                c.image[4..4 + rec.len()].copy_from_slice(rec);
                c.installed.clear();
                c.removed.clear();
                c
            })
            .collect();
    }
    let out = save.write();
    std::fs::write(out_path, &out).expect("write output .sav");
    let back = SaveFile::parse(&out).expect("re-parse written .sav");
    let p = back.player.unwrap();
    eprintln!("wrote {out_path}: name={:?} cars={:?} seen_fd={}/{}", p.player_name, p.cars.iter().map(|c| c.car_id()).collect::<Vec<_>>(), p.seen_factory_welcome, p.seen_first_mission);
}
