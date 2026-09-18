//! Which `scripts.bin` actions the shipped file actually uses — `traffic.md` §30.
//!
//! The file is a flat `u8[9][7][8]`: nine trigger rows, seven scripts each, four action bytes then
//! four durations. This just counts the action bytes so the reactions that can never fire are
//! visible.
fn main() -> anyhow::Result<()> {
    let paths = nfs_assets::paths::GamePaths::locate()?;
    let p = paths.game_data().join("Simulation").join("AI").join("SCRIPTS.BIN");
    let data = std::fs::read(&p)?;
    let s = nfs_formats::scripts::Scripts::parse(&data)?;
    const NAMES: [&str; 13] = ["INIT", "END", "SPEED_BURST", "HONK_HORN", "HARD_BRAKE", "SWIPE", "ATTACK", "PAUSE", "REAR_END", "SWERVE", "MOVE_AWAY", "WIPE_OUT", "CRAZY_HORN"];
    let mut used = std::collections::BTreeMap::<u8, usize>::new();
    for r in 0..9usize {
        for sc in 0..7usize {
            let o = r * 56 + sc * 8;
            let acts: Vec<String> = (0..4).map(|a| format!("{}({})", NAMES.get(s.table[o + a] as usize).copied().unwrap_or("?"), s.table[o + 4 + a])).collect();
            println!("row {r} script {sc}: {}", acts.join(" "));
            for a in 0..4 {
                *used.entry(s.table[o + a]).or_default() += 1;
            }
        }
    }
    println!("\ntotals:");
    for (a, n) in used {
        println!("  action {a:2} {:14} × {n}", NAMES.get(a as usize).copied().unwrap_or("?"));
    }
    Ok(())
}
