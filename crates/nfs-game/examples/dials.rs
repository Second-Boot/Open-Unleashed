//! Diagnostic: the dial definitions of every car, with the needle angle the HDial geometry gives at
//! both ends of the sweep (`RE/notes/hud.md` §3.3, H32).
use nfs_assets::GamePaths;

fn oclock(cw: f32) -> String {
    let m = ((cw.rem_euclid(360.0)) / 30.0) as i32;
    let h = if m == 0 { 12 } else { m };
    format!("{h}:{:02}", ((cw.rem_euclid(360.0) / 30.0 - m as f32) * 60.0) as i32)
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let cars = nfs_formats::fedata::parse_car_catalog(&std::fs::read(paths.fe_data().join("Data").join("nfs5.car")).unwrap()).unwrap();
    let mut seen: Vec<(i16, i16, i16, i16)> = Vec::new();
    for c in &cars {
        for (what, d) in [("km/h", &c.dial_kmh), ("mph", &c.dial_mph), ("tach", &c.dial_tach)] {
            let key = (d.style, d.total_units, d.start_deg, d.deg_per_unit_x1000);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let a0 = d.screen_cw_deg(0);
            let a1 = d.screen_cw_deg(d.total_units as i32 - 1);
            // the table is filled backwards, so clockwise-from-12 = 90 − (start + (total−1−v)·step)
            println!(
                "{:<22} {what:<7} style {:>2} needle {} units {:>4} start {:>4}° step {:>5}/1000 → screen {a0:>7.1}..{a1:>7.1}°, {} → {} ({})",
                c.display_name.chars().take(22).collect::<String>(),
                d.style,
                d.needle_style,
                d.total_units,
                d.start_deg,
                d.deg_per_unit_x1000,
                oclock(a0),
                oclock(a1),
                if a1 > a0 { "clockwise" } else { "counter-clockwise" }
            );
        }
    }
}
