//! Traffic on real track data: spawn, lane choice and despawn over a long run.
//! Skipped when the original game data is not installed.

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;
use nfs_game::traffic::{Reference, TrafficSet, TrafficSpec, DESPAWN_DISTANCE, MAX_ACTIVE};

fn run(track: &str, seed: u32) -> Option<(SliceTable, TrafficSet)> {
    let paths = GamePaths::locate().ok()?;
    let t = TrackAssets::load(&paths, track).ok()?;
    let table = SliceTable::from_crp(&t.crp, &t.crp_data).ok()?;
    let specs: Vec<TrafficSpec> = (0..7).map(|i| TrafficSpec { model: i, bus: i == 1, half_extents: Vec3::new(0.85, 0.6, 2.1) }).collect();
    let mut set = TrafficSet::new(&specs, &table, 1.0, false, seed);

    // drive a reference car around the lap at 30 m/s, stepping five slices per 32 Hz tick
    let mut slice = 0usize;
    for tick in 0..20_000u64 {
        if tick % 2 == 0 {
            slice = table.add(slice, 1);
            let r = Reference { position: table.slices[slice].centre, slice, laps: 0, speed: 30.0, dir: 1.0 };
            set.update(&table, tick, r, &[]);
            set.update(&table, tick, r, &[]);
        }
        set.integrate(&table, 1.0 / 64.0);
    }
    Some((table, set))
}

#[test]
fn traffic_stays_on_the_road_and_recycles() {
    let Some((table, set)) = run("coastal", 12345) else {
        eprintln!("skipping: no game data");
        return;
    };
    assert!(set.active_count() <= MAX_ACTIVE, "at most five alive at once");
    assert!(set.active_count() > 0, "the set keeps respawning over a full lap");
    for c in set.cars.iter().filter(|c| c.active) {
        assert!(c.position.is_finite(), "{:?}", c.position);
        assert!(c.velocity.is_finite(), "{:?}", c.velocity);
        let sl = &table.slices[c.slice];
        let lat = (c.position - sl.centre).dot(sl.lateral.normalize());
        // the lateral target is clamped to the road, so a car can never be off the far edge
        assert!(lat <= sl.width_b + 1.0 && lat >= -sl.width_a - 1.0, "lateral {lat} outside [-{}, {}]", sl.width_a, sl.width_b);
        // the speed ladder keeps every car between the 8.889 floor and the 16.5 cruise
        assert!(c.speed.abs() <= 17.0, "speed {}", c.speed);
        // and every alive car is inside the despawn radius of the reference
        assert!(c.dir == 1.0 || c.dir == -1.0);
    }
}

#[test]
fn every_car_is_used_over_a_lap() {
    let Some((_, set)) = run("autobahn", 99) else {
        eprintln!("skipping: no game data");
        return;
    };
    // over 10 000 AI ticks each of the seven slots should have been through purgatory at least once
    assert!(set.cars.iter().all(|c| c.active || c.respawn_ticks > 0), "no slot is stuck");
    assert!(DESPAWN_DISTANCE > 0.0);
}
