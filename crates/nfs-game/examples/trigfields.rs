//! Diagnostic: the values the shipped scenes give the trigger fields whose consumers are unknown
//! (`RE/notes/race-logic.md` §3.2, line 4 tokens 5 and 8..17).
use nfs_assets::GamePaths;
use nfs_formats::scn::{ElementKind, Scene};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.game_data().join("Track");
    const NAMES: [&str; 11] =
        ["+0x8C", "+0xD4 seq", "+0xD6", "+0xD8 geom", "+0xDA", "+0xDC cam", "+0xDE marker", "+0xEC", "+0xE0", "+0xA0", "+0x9C"];
    let mut hist: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new(); NAMES.len()];
    let mut files = 0;
    let mut triggers = 0;
    let mut mismatched_da = 0;
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.extension().and_then(|e| e.to_str()).map(|e| !e.eq_ignore_ascii_case("scn")).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(scene) = Scene::parse(&text) else { continue };
        files += 1;
        for e in &scene.elements {
            if e.kind != ElementKind::Trigger {
                continue;
            }
            let Some(line) = e.params_text.iter().find(|l| l.len() == 17) else { continue };
            if line.len() < 17 {
                continue;
            }
            triggers += 1;
            if line[7] != line[10] {
                mismatched_da += 1;
            }
            if line[15] != "0" {
                println!("  +0xA0 = {} in {} element {} ({:?}) type {}", line[15], p.file_name().unwrap().to_string_lossy(), e.line, e.name, line[0]);
            }
            for (slot, tok) in [4usize, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16].iter().enumerate() {
                *hist[slot].entry(line[*tok].clone()).or_default() += 1;
            }
        }
    }
    println!("{files} scene files, {triggers} triggers; +0xDA differs from +0xD4 on {mismatched_da}");
    for (n, h) in NAMES.iter().zip(&hist) {
        let mut v: Vec<_> = h.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        println!("  {n:<12} {:?}", v.iter().take(8).map(|(s, c)| format!("{s}×{c}")).collect::<Vec<_>>());
    }
}
