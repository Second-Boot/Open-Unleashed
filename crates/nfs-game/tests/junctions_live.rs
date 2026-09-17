//! The junction graph against the shipped data. The strongest check available: the number of nodes the
//! geometric passes derive must equal the number of records in `<track>.jnc`, which is that file's size
//! divided by 36 — the original rejects the file outright when they disagree.

use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::junctions::Junctions;
use nfs_game::race::SliceTable;

const TRACKS: [&str; 15] = [
    "alps", "autobahn", "canyon", "castle", "coastal", "farmland", "foothills", "forest", "industrial", "monaco1", "monaco2", "monaco3", "monaco4", "monaco5", "skidpad",
];

#[test]
fn the_derived_node_count_matches_every_jnc_file() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut checked = 0;
    for t in TRACKS {
        let Ok(a) = TrackAssets::load(&paths, t) else { continue };
        let Ok(table) = SliceTable::from_crp(&a.crp, &a.crp_data) else { continue };
        let Some(path) = GamePaths::find_file(&a.dir, &format!("{t}.jnc")) else { continue };
        let Ok(jnc) = std::fs::read(&path) else { continue };
        let centres: Vec<_> = table.slices.iter().map(|s| s.centre).collect();
        let sections: Vec<(usize, usize)> = table.sections.iter().map(|s| (s.first, s.last)).collect();
        let mut j = Junctions::build(&centres, &sections);
        assert_eq!(j.nodes.len(), jnc.len() / 36, "{t}: derived {} nodes, {} has {}", j.nodes.len(), path.display(), jnc.len() / 36);
        assert!(j.apply_jnc(&jnc), "{t}: the weights should load once the counts agree");
        // every record's leading word is its own index, which is what makes the file positional
        for (i, rec) in jnc.chunks_exact(36).enumerate() {
            assert_eq!(u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]) as usize, i, "{t}: record {i}");
        }
        // and the lap is a sane fraction of the track
        assert!(j.lap_slices > 0 && j.lap_slices <= table.slices.len(), "{t}: lap {} of {}", j.lap_slices, table.slices.len());
        checked += 1;
    }
    assert_eq!(checked, 15, "every shipped track should have been checked");
}
