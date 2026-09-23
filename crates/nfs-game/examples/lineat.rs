//! Diagnostic: the slices nearest a point, with the `.lsp` racing line and the drivable patch
//! there — for reading why a car leaves the road at a place.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = nfs_assets::GamePaths::locate().unwrap();
    let t = nfs_assets::track::TrackAssets::load(&paths, &a[0]).unwrap();
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let (x, z): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let lsp = paths.root.join(format!("GameData/Track/racer/work/{}0.lsp", a[0]));
    let line = std::fs::read(&lsp).ok().and_then(|d| nfs_formats::ais::RacingLine::parse(&d).ok());
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let mut near: Vec<(f32, usize)> = table
        .slices
        .iter()
        .enumerate()
        .map(|(i, s)| (((s.centre.x - x).powi(2) + (s.centre.z - z).powi(2)).sqrt(), i))
        .collect();
    near.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    for (d, i) in near.iter().take(6) {
        let s = &table.slices[*i];
        let lp = line.as_ref().and_then(|l| l.points.get(*i)).copied();
        let probe = |px: f32, pz: f32, py: f32| grid.find_patch(glam::Vec3::new(px, py, pz)) & 0xF;
        print!("slice {i:5} d={d:6.1} centre=({:8.1},{:7.1},{:8.1}) widths={:.1}/{:.1} patch={}", s.centre.x, s.centre.y, s.centre.z, s.lane_width_a, s.lane_width_b, probe(s.centre.x, s.centre.z, s.centre.y));
        if let Some(p) = lp {
            let off = (glam::Vec3::new(p[0], s.centre.y, p[1]) - s.centre).dot(s.lateral);
            println!("  line=({:8.1},{:8.1}) offset={off:6.1} patch={}", p[0], p[1], probe(p[0], p[1], s.centre.y));
        } else {
            println!("  line=none");
        }
    }
}
