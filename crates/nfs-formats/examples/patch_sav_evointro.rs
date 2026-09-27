//! One-off tool for a reference-capture pass (RE/notes/REFERENCE-2026-09-28.md): set
//! `PlayerInfo.seen_evolution_intro` (`[0x691DD5]`, savegame.md §4.7/§8) to 1 in a `.sav` so
//! `одиночная игра` routes straight past the `careerfirst` Evolution-intro movie.
//! Usage: `patch_sav_evointro <in.sav> <out.sav>`.
use nfs_formats::sav::SaveFile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (in_path, out_path) = (&args[1], &args[2]);
    let data = std::fs::read(in_path).expect("read input .sav");
    let mut save = SaveFile::parse(&data).expect("parse .sav");
    let player = save.player.as_mut().expect("PlayerInfo module present");
    eprintln!("before: seen_evolution_intro = {}", player.seen_evolution_intro);
    player.seen_evolution_intro = 1;
    let out = save.write();
    std::fs::write(out_path, &out).expect("write output .sav");
    // Round-trip check: re-parse and confirm the flag stuck.
    let back = SaveFile::parse(&out).expect("re-parse written .sav");
    eprintln!("after:  seen_evolution_intro = {}", back.player.unwrap().seen_evolution_intro);
    eprintln!("wrote {out_path} ({} bytes, was {} bytes)", out.len(), data.len());
}
