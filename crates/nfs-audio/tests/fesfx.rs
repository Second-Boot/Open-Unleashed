//! The front end's one-shots decode from the shipped `fesfx.bnk` / `lilfesfx.bnk`, for every slot
//! a traced caller of 0x4AADB0 pushes (`nfs_audio::fesfx`).

use nfs_assets::GamePaths;
use nfs_formats::bnk::Bnk;

#[test]
fn every_traced_front_end_slot_decodes() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("game data not found; skipping");
        return;
    };
    let sounds = paths.game_data().join("Sounds");
    for name in ["fesfx.bnk", "lilfesfx.bnk"] {
        let data = std::fs::read(sounds.join(name)).expect(name);
        let bank = Bnk::parse(&data).expect(name);
        for slot in [5usize, 6, 8, 9, 0x0D, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35] {
            let pcm = bank.decode_slot(&data, slot).unwrap_or_else(|e| panic!("{name} slot {slot:#x}: {e}"));
            assert!(!pcm.samples.is_empty(), "{name} slot {slot:#x} is empty");
        }
    }
}
