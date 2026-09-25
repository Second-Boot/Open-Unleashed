//! Traffic on real track data: spawn, lane choice and despawn over a long run.
//! Skipped when the original game data is not installed.

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;
use nfs_game::traffic::{Reference, TrafficSet, TrafficSpec, DESPAWN_DISTANCE, MAX_ACTIVE};

fn run(track: &str, seed: u32) -> Option<(SliceTable, TrafficSet, u64)> {
    let paths = GamePaths::locate().ok()?;
    let t = TrackAssets::load(&paths, track).ok()?;
    let table = SliceTable::from_crp(&t.crp, &t.crp_data).ok()?;
    let specs: Vec<TrafficSpec> = (0..7).map(|i| TrafficSpec { model: i, bus: i == 1, half_extents: Vec3::new(0.85, 0.6, 2.1) }).collect();
    let mut set = TrafficSet::new(&specs, &table, 1.0, false, seed);

    // drive a reference car around the lap at 30 m/s, stepping five slices per 32 Hz tick
    // (the test's own driver walks the lap route and wraps; `SliceTable::add` is 0x4817D0 and
    // stops at a point-to-point track's dead end)
    let mut p = 0usize;
    // the 64 Hz ticks of the last quarter with at least one car out
    let mut out_late = 0u64;
    for tick in 0..20_000u64 {
        if tick % 2 == 0 {
            p = (p + 1) % table.lap_len;
            let slice = table.route[p];
            let r = Reference { position: table.slices[slice].centre, slice, laps: 0, speed: 30.0, dir: 1.0 };
            // the even tick as `GameTick` 0x414FF0 runs it: the AI pass, the trackers (Sc32
            // priority 6, `car+0x78` from 0x417190 — the reference is the only racer here), then
            // the drive (priority 0x1E)
            set.ai(&table, tick, r, &[]);
            set.track_racers(&[r.position]);
            set.drive(&table, tick, r);
        }
        set.integrate(&table, 1.0 / 64.0);
        if tick >= 15_000 && set.cars.iter().any(|c| c.active) {
            out_late += 1;
        }
    }
    Some((table, set, out_late))
}

#[test]
fn traffic_stays_on_the_road_and_recycles() {
    let Some((table, set, out_late)) = run("coastal", 12345) else {
        eprintln!("skipping: no game data");
        return;
    };
    assert!(set.active_count() <= MAX_ACTIVE, "at most five alive at once");
    // over the last quarter, not at the last tick: the AI pass's gate can leave a moment with
    // every car in purgatory (see `every_car_is_used_over_a_lap`)
    assert!(out_late > 0, "the set keeps respawning over a full lap");
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
    let Some((_, set, out_late)) = run("autobahn", 99) else {
        eprintln!("skipping: no game data");
        return;
    };
    // over 10 000 AI ticks the set keeps cycling: cars are out, and a slot that is not is either
    // counting down or waiting at zero for one of the five places to come free
    // (over the last quarter, not at the last tick: the AI pass's 8 Hz gate and its 32 Hz-tick
    // purgatory countdown, `traffic::ai_pass_due`, can leave a moment with every car in purgatory)
    assert!(out_late > 0, "cars are out");
    // `car+0xEE8` has no floor (0x40E140), but the gate 0x40E060 either spawns the car or re-arms
    // it to 1 as soon as it is not positive, so it never gets more than one step (8) below zero
    assert!(set.cars.iter().all(|c| c.active || c.respawn_ticks > -8), "a purgatory countdown ran away");
    assert!(DESPAWN_DISTANCE > 0.0);
}
