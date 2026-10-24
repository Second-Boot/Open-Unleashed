//! **The remote car's store and apply against the running original** (`RE/notes/REFERENCE-2026-10-24-PA-DEADRECK.md`).
//!
//! The trace is a live positional race (port hosts, the original joins) read with gdb on the original: a
//! non-stopping breakpoint at the store 0x4A0DF0 (`SI` with the received block, `SO` with the lag book it
//! leaves) and at the apply 0x4A0A40 (`AI` the car before, `AO` the car after, 0x4A0DDC). Point
//! `NFS_DEADRECK_TRACE` at the file; without it (or without the game data) the tests skip.

use super::*;
use glam::Mat3;
use nfs_net::state::{CarState, matrix_of};
use nfs_physics::vehicle::NetBlock;

fn words(s: &str) -> Vec<u32> {
    s.split_whitespace().map(|w| u32::from_str_radix(w, 16).unwrap()).collect()
}

fn fl(w: u32) -> f32 {
    f32::from_bits(w)
}

/// The nine words of `car+0x38…+0x5B` as the 36-byte block.
fn block_of(w: &[u32]) -> CarState {
    let bytes: Vec<u8> = w.iter().take(9).flat_map(|x| x.to_le_bytes()).collect();
    CarState::decode(&bytes).unwrap()
}

struct Call {
    tick: i32,
    pre: Vec<u32>,
    post: Vec<u32>,
    blk: Vec<u32>,
    /// `car+0x38…` as `AO` finds it: a store from the network thread between the `AI` print and the apply's own read
    /// (it holds the block's critical section) shows as a different block here
    blk_after: Vec<u32>,
    /// the car's own state `Susp_update` reads (`x…` sections of the second trace format); empty in the first
    extra: std::collections::HashMap<&'static str, Vec<u32>>,
    post_m0: u32,
    /// the word `car+8`: the tracker's road slice
    slice: usize,
}

/// `AI`/`AO` pairs (the early exit — no block yet — has `+0x3C == 0` and is dropped) and the `SI`/`SO` pairs.
fn load() -> Option<(Vec<Call>, Vec<(i32, Vec<u32>, Vec<u32>)>)> {
    let path = std::env::var("NFS_DEADRECK_TRACE").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let sect = |l: &str, key: &str| -> Option<Vec<u32>> {
        let rest = l.split(&format!("{key} ")).nth(1)?;
        Some(words(rest.split('|').next()?))
    };
    let mut calls = Vec::new();
    let mut stores = Vec::new();
    let mut open: Option<(i32, Vec<u32>, Vec<u32>, std::collections::HashMap<&'static str, Vec<u32>>)> = None;
    let mut open_store: Option<(i32, Vec<u32>)> = None;
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("AI") => {
                let tick: i32 = t[1].parse().unwrap();
                let body = l.split("pos/vel ").nth(1).unwrap();
                let mut pre = words(body.split('|').next().unwrap());
                pre.extend(sect(l, "M").unwrap());
                pre.extend(sect(l, "w").unwrap());
                let mut extra = std::collections::HashMap::new();
                for k in ["x35c", "x3a0", "xd40", "xd64", "xdc4", "xd7c", "xpl", "xw", "xwh"] {
                    if let Some(w) = sect(l, k) {
                        extra.insert(k, w);
                    }
                }
                extra.insert("trk", sect(l, "trk").unwrap());
                open = Some((tick, pre, sect(l, "blk").unwrap(), extra));
            }
            Some("AO") => {
                let (tick, pre, blk, extra) = open.take().unwrap();
                let slice = (extra["trk"][0] & 0xFFFF) as usize;
                let body = l.split("pos/vel ").nth(1).unwrap();
                let post_m0 = sect(l, "m").unwrap()[0];
                let mut post = words(body.split('|').next().unwrap());
                post.extend(sect(l, "M").unwrap());
                post.extend(sect(l, "w").unwrap());
                if blk[1] != 0 {
                    let blk_after = sect(l, "blk").unwrap();
                    calls.push(Call { tick, pre, post, blk, blk_after, extra, post_m0, slice });
                }
            }
            Some("SI") => {
                let tick: i32 = t[1].parse().unwrap();
                open_store = Some((tick, words(l.split("blk ").nth(1).unwrap())));
            }
            Some("SN") => {
                // `[0x606AC4]` as 0x4A0E75 read it (the network thread races the tick thread: `SI`'s and `SO`'s own
                // reads can be a tick apart)
                if let Some(o) = open_store.as_mut() {
                    o.0 = t[3].trim_start_matches("now=").parse().unwrap();
                }
            }
            Some("SO") => {
                let (tick, blk) = open_store.take().unwrap();
                let mut rest: Vec<u32> = t[3..].iter().filter(|w| **w != "lag" && **w != "da").map(|w| u32::from_str_radix(w, 16).unwrap()).collect();
                // `[+0x3C] [+0x5C..+0x70] [+0xDA8, +0xDAC]`
                let tick_after = rest.remove(0);
                let _ = tick_after;
                stores.push((tick, blk, rest));
            }
            _ => {}
        }
    }
    Some((calls, stores))
}

/// 0x4A0DF0 on the original's own (block, arrival tick) stream: the port's [`Remote::store`] must leave the lag book
/// the original left — `+0x5C` smoothed lag, `+0x60` fresh, `+0x64` late, `+0x68` lag, `+0x70` stale, `+0xDAC`.
#[test]
fn the_store_leaves_the_lag_book_the_original_left() {
    let Some((_, stores)) = load() else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let mut r = Remote::new(0);
    let (mut n, mut bad) = (0, 0);
    for (tick, blk, book) in &stores {
        if n == 0 {
            // the trace is attached mid-race: the first store's own result seeds the lag book
            r.state = block_of(blk);
            r.lag_avg = book[0] as i32;
            r.fresh = book[1] as i32;
            r.late = book[2] as i32;
            r.lag = book[3] as i32;
            r.stale = book[5] != 0;
            r.got = book[7] != 0;
            n += 1;
            continue;
        }
        let taken = r.store(block_of(blk), *tick);
        n += 1;
        // book: [+0x5C, +0x60, +0x64, +0x68, +0x6C, +0x70, +0xDA8, +0xDAC]
        let want = (book[0] as i32, book[1] as i32, book[2] as i32, book[3] as i32, book[5] != 0, book[7] != 0);
        let got = (r.lag_avg, r.fresh, r.late, r.lag, r.stale, r.got);
        if taken && got != want {
            bad += 1;
            if bad < 6 {
                eprintln!("store {n} at tick {tick}: original {want:?}, port {got:?}");
            }
        }
    }
    eprintln!("stores compared {n}, differing {bad}");
    assert_eq!(bad, 0);
}

/// 0x4A0A40 on the original's own pre-state: for each live call the port's [`Vehicle::net_dead_reckon`] is run on a car
/// holding the original's position, velocity, matrix and spin before the call, and compared with the original's after.
#[test]
fn the_apply_leaves_the_car_where_the_original_left_it() {
    let Some((calls, _)) = load() else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let Ok(paths) = nfs_assets::GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let track = std::env::var("NFS_DEADRECK_TRACK").unwrap_or_else(|_| "coastal".into());
    let t = nfs_assets::track::TrackAssets::load(&paths, &track).unwrap();
    let g = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
    // the remote car as the original built it from the sender's car record: the catalogue car `NFS_DEADRECK_CAR`
    // (111 = the 356 B 1600 Super Coupe of the live race) with its stock parts and the neutral set-up folded in
    let car_id: u16 = std::env::var("NFS_DEADRECK_CAR").ok().and_then(|v| v.parse().ok()).unwrap_or(111);
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    let entry = tables.car(car_id).expect("catalogue car");
    let sim = nfs_assets::car::CarAssets::load_sim(&paths, &entry.sim).unwrap();
    let mut specs = nfs_physics::specs::CarSpecs::from_sim(&sim, entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935"));
    let stock: Vec<&nfs_formats::career::Part> = entry.stock_parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
    specs.apply_parts(&stock, &entry.sim_family);
    specs.apply_setup(&nfs_game::career::CAR_SETUP_DEFAULT, true);
    if let Some(po) = std::env::var("NFS_DEADRECK_PROBE").ok().and_then(|v| v.parse::<f32>().ok()) {
        specs.probe_offset = po;
    }
    eprintln!("car {} {} probe_offset {} mass {}", entry.sim, entry.model, specs.probe_offset, specs.mass);
    let assets = nfs_assets::car::CarAssets::load(&paths, &entry.model).unwrap();
    let (body_pts, body_half) = nfs_game::world::car_body(&assets, false);
    let wheel_radius: f32 = std::env::var("NFS_DEADRECK_WHEEL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let skip: usize = std::env::var("NFS_DEADRECK_SKIP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let max_calls: usize = std::env::var("NFS_DEADRECK_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);

    let m = |w: &[u32]| [[fl(w[0]), fl(w[1]), fl(w[2])], [fl(w[3]), fl(w[4]), fl(w[5])], [fl(w[6]), fl(w[7]), fl(w[8])]];
    // one call: the port's apply on the original's pre-state with block `blk`; the five distances to the original's post-state
    let run = |c: &Call, blk_words: &[u32]| -> [f32; 5] {
        let mut v = Vehicle::new(Vec3::new(fl(c.pre[0]), fl(c.pre[1]), fl(c.pre[2])), Mat3::IDENTITY, &specs, 0.7, wheel_radius);
        v.set_car_body(body_pts.clone(), body_half);
        v.position = Vec3::new(fl(c.pre[0]), fl(c.pre[1]), fl(c.pre[2]));
        v.velocity = Vec3::new(fl(c.pre[3]), fl(c.pre[4]), fl(c.pre[5]));
        let rows = m(&c.pre[7..16]);
        v.orientation = Mat3::from_cols(Vec3::from(rows[0]), Vec3::from(rows[1]), Vec3::from(rows[2])).transpose();
        v.angular_velocity = Vec3::new(fl(c.pre[16]), fl(c.pre[17]), fl(c.pre[18]));
        v.tracker.seat(&grid, v.position);
        // `car+0x424` before the call: the trace prints 0 for a remote car (0x4A0D8B zeroes it on every call)
        v.body_height = 0.0;
        if let (Some(a), Some(b), Some(c3), Some(d), Some(e), Some(sp)) =
            (c.extra.get("xd40"), c.extra.get("xdc4"), c.extra.get("xd7c"), c.extra.get("x3a0"), c.extra.get("xwh"), c.extra.get("x35c"))
        {
            // `car+0xD4C` (the local acceleration), the dive / roll filters, the throttle byte, `cos tilt`, `car+0x35C`,
            // the wheels' compression `+0x68` and airborne `+0x84` — what the car's own tick left for `Susp_update`
            v.accel_local = Vec3::new(fl(a[3]), fl(a[4]), fl(a[5]));
            v.accel_world = Vec3::new(fl(a[0]), fl(a[1]), fl(a[2]));
            v.dive = fl(b[0]);
            v.roll_lp = fl(b[1]);
            v.throttle_byte = (c3[0] >> 24) as u8;
            v.cos_tilt = fl(d[1]);
            v.speed_est = fl(sp[0]);
            for i in 0..4 {
                v.wheels[i].compression = fl(e[2 * i]);
                v.wheels[i].airborne = e[2 * i + 1] != 0;
            }
        }
        let st = block_of(blk_words);
        let blk = NetBlock {
            tick: st.tick as i32,
            airborne: st.airborne,
            yaw_rate: st.yaw_rate,
            matrix: matrix_of(st.orientation),
            velocity: Vec3::from(st.velocity),
            position: Vec3::from(st.position),
        };
        let centre = table.slices.get(c.slice).map(|sl| sl.centre);
        v.net_dead_reckon(&blk, c.tick, &grid, &specs, centre);
        let want_pos = Vec3::new(fl(c.post[0]), fl(c.post[1]), fl(c.post[2]));
        let want_vel = Vec3::new(fl(c.post[3]), fl(c.post[4]), fl(c.post[5]));
        let want_m = m(&c.post[7..16]);
        let want_w = Vec3::new(fl(c.post[16]), fl(c.post[17]), fl(c.post[18]));
        let dp = (v.position.x - want_pos.x).abs().max((v.position.z - want_pos.z).abs());
        let dy = (v.position.y - want_pos.y).abs();
        let dv = (v.velocity - want_vel).abs().max_element();
        let got_m = [v.orientation.row(0).to_array(), v.orientation.row(1).to_array(), v.orientation.row(2).to_array()];
        let dm = (0..3).flat_map(|i| (0..3).map(move |j| (i, j))).map(|(i, j)| (got_m[i][j] - want_m[i][j]).abs()).fold(0.0, f32::max);
        let dw = (v.angular_velocity - want_w).abs().max_element();
        if std::env::var_os("NFS_DEADRECK_GH").is_some() {
            eprintln!("GH {} port ground_h {} orig {} port y {} orig y {}", c.tick, v.ground_h, fl(c.post_m0), v.position.y, want_pos.y);
        }
        if std::env::var_os("NFS_DEADRECK_POST").is_some() {
            eprintln!("POST {} {:e} {:e} {:e}", c.tick, v.position.x, v.position.y, v.position.z);
        }
        [dp, dy, dv, dm, dw]
    };
    let mut rows: Vec<[f32; 5]> = Vec::new();
    let mut raced = 0;
    for c in calls.iter().skip(skip).take(max_calls) {
        let mut best = run(c, &c.blk);
        if c.blk_after != c.blk {
            // a store raced the print: the apply read one of the two blocks
            raced += 1;
            let alt = run(c, &c.blk_after);
            if alt[0] + alt[3] < best[0] + best[3] {
                best = alt;
            }
        }
        if std::env::var_os("NFS_DEADRECK_CSV").is_some() {
            eprintln!("CSV {} {:e} {:e} {:e} {:e} {:e}", c.tick, best[0], best[1], best[2], best[3], best[4]);
        }
        rows.push(best);
    }
    let pct = |k: usize, q: f32| {
        let mut v: Vec<f32> = rows.iter().map(|r| r[k]).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f32 * q) as usize]
    };
    eprintln!("applies compared {} ({raced} with a store racing the print)", rows.len());
    for (k, name) in ["xz position", "y position", "velocity", "matrix", "spin"].iter().enumerate() {
        eprintln!("  {name:<12} median {:e}  p99 {:e}  max {:e}", pct(k, 0.5), pct(k, 0.99), pct(k, 1.0));
    }
    assert!(rows.len() > 10);
    // the dead reckoning proper — the predicted position, the velocity (untouched by the blend), the blended matrix — is the original's to the bit
    // (`xz`: to the last bit of an `f32` in all but the calls a car off the road reaches the two-wheel shift of `Susp_update`'s re-seat, whose
    // inputs — the wheels' own patch trackers — the trace does not hold: a few centimetres at most, in about one call in sixty)
    assert!(pct(0, 0.9) == 0.0 && pct(0, 1.0) < 0.1 && pct(3, 1.0) < 1e-5 && pct(2, 1.0) == 0.0, "the apply departs from the original's");
    // the re-seat (`Susp_update` + `BodyHeight`) reads the car's own wheel and body state, which the trace holds only in part: a constant
    // 8 mm in `y` and ~5e-4 turns/s in the spin remain (REFERENCE-2026-10-24-PA-DEADRECK.md, "Residue")
    assert!(pct(1, 1.0) < 0.1 && pct(4, 1.0) < 0.05);
}

/// 0x4A0680 on the original's own stale spell: `PI` is the car at the call (0x4A0680, from 0x49924D), `PO` at the return site
/// 0x499252. The decision is read off the difference — the controls `car+0xD9C` / `+0xD9D` (the brake rule), the position (put
/// on the road) or nothing — and compared with [`nfs_game::netcar::stale_place`] on the same car, block and tick.
#[test]
fn the_stale_arm_decides_as_the_original_did() {
    use nfs_game::netcar::{StalePlace, stale_place};
    let Ok(path) = std::env::var("NFS_DEADRECK_TRACE") else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Ok(paths) = nfs_assets::GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    struct S {
        tick: i32,
        pos: [u32; 3],
        vel: [u32; 3],
        blk: Vec<u32>,
        ctl: Vec<u32>,
        slice: Vec<u32>,
        progress: u32,
    }
    let parse = |l: &str| -> S {
        let t: Vec<&str> = l.split_whitespace().collect();
        let body: Vec<u32> = words(l.split("pos/vel ").nth(1).unwrap().split('|').next().unwrap());
        let sect = |k: &str| words(l.split(&format!("{k} ")).nth(1).unwrap().split('|').next().unwrap());
        S {
            tick: t[1].parse().unwrap(),
            pos: [body[0], body[1], body[2]],
            vel: [body[3], body[4], body[5]],
            blk: sect("blk"),
            ctl: sect("xctl"),
            slice: sect("xsl"),
            progress: sect("xprog")[0],
        }
    };
    let (mut pis, mut pos_) = (Vec::new(), Vec::new());
    for l in text.lines() {
        if l.starts_with("PI ") {
            pis.push(parse(l));
        } else if l.starts_with("PO ") {
            pos_.push(parse(l));
        }
    }
    if pis.is_empty() {
        eprintln!("skipping: the trace has no 0x4A0680 calls");
        return;
    }
    let track = std::env::var("NFS_DEADRECK_TRACK").unwrap_or_else(|_| "coastal".into());
    let t = nfs_assets::track::TrackAssets::load(&paths, &track).unwrap();
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let (mut left, mut brake, mut put, mut bad) = (0, 0, 0, 0);
    for (a, b) in pis.iter().zip(pos_.iter()) {
        let want = if a.pos != b.pos {
            put += 1;
            "put"
        } else if a.ctl != b.ctl {
            brake += 1;
            "brake"
        } else {
            left += 1;
            "left"
        };
        let st = block_of(&a.blk);
        let blk = NetBlock {
            tick: st.tick as i32,
            airborne: st.airborne,
            yaw_rate: st.yaw_rate,
            matrix: matrix_of(st.orientation),
            velocity: Vec3::from(st.velocity),
            position: Vec3::from(st.position),
        };
        let preferred = (a.slice[1] != u32::MAX).then_some(a.slice[1] as usize);
        let got = stale_place(&table, &blk, a.tick, (a.slice[0] & 0xFFFF) as usize, preferred, Vec3::new(fl(a.vel[0]), fl(a.vel[1]), fl(a.vel[2])), fl(a.progress));
        let ok = match (&got, want) {
            (StalePlace::Left, "left") | (StalePlace::PutOnRoad { .. }, "put") => true,
            // the brake rule leaves `throttle -= min(throttle, byte)` and `brake = byte`: compare the bytes the original left
            (StalePlace::Brake(byte), "brake") => b.ctl[0] & 0xFF00 == (u32::from(*byte) << 8) || (b.ctl[0] >> 8) & 0xFF == u32::from(*byte),
            _ => false,
        };
        if !ok {
            bad += 1;
            if bad < 6 {
                eprintln!("0x4A0680 at tick {}: original {want}, port {got:?} (car slice {:x}, progress {})", a.tick, a.slice[0], fl(a.progress));
            }
        }
    }
    eprintln!("stale arm: {} calls, original left {left} / braked {brake} / put on the road {put}, differing {bad}", pis.len());
    assert_eq!(bad, 0);
}

/// 0x4A0C0B…0x4A0D23, the retrack walk: `W0` is `P` and the copy's zone word after the first 0x474060 (0x4A0C29), `WK` every step of the walk
/// (0x4A0CF0), `WE` the `P` the blend gets (0x4A0D26). The port's [`Vehicle::net_retrack`] on the same `P`, slice centre and a tracker seated on
/// the car must stop where the original did.
#[test]
fn the_retrack_walk_stops_where_the_original_stopped() {
    let Ok(path) = std::env::var("NFS_DEADRECK_TRACE") else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Ok(paths) = nfs_assets::GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let track = std::env::var("NFS_DEADRECK_TRACK").unwrap_or_else(|_| "coastal".into());
    let t = nfs_assets::track::TrackAssets::load(&paths, &track).unwrap();
    let g = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let specs = nfs_physics::specs::CarSpecs::from_sim(&nfs_assets::car::CarAssets::load_sim(&paths, "356BcoupeS16").unwrap(), false);
    let v3 = |w: &[&str]| Vec3::new(fl(u32::from_str_radix(w[0], 16).unwrap()), fl(u32::from_str_radix(w[1], 16).unwrap()), fl(u32::from_str_radix(w[2], 16).unwrap()));
    let (mut n, mut walked, mut bad, mut worst) = (0, 0, 0, 0.0f32);
    let (mut car_pos, mut slice) = (Vec3::ZERO, 0usize);
    let mut p0: Option<Vec3> = None;
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("AI") => {
                let body = words(l.split("pos/vel ").nth(1).unwrap().split('|').next().unwrap());
                car_pos = Vec3::new(fl(body[0]), fl(body[1]), fl(body[2]));
                slice = (words(l.split("trk ").nth(1).unwrap().split('|').next().unwrap())[0] & 0xFFFF) as usize;
            }
            Some("W0") => p0 = Some(v3(&t[3..6])),
            Some("WE") => {
                let (Some(p), Some(centre)) = (p0.take(), table.slices.get(slice).map(|s| s.centre)) else { continue };
                let want = v3(&t[3..6]);
                let mut v = Vehicle::new(car_pos, Mat3::IDENTITY, &specs, 0.7, 0.3);
                v.position = car_pos;
                v.tracker.seat(&grid, car_pos);
                let got = v.net_retrack(&grid, p, centre);
                n += 1;
                walked += usize::from(got != p);
                let d = (got - want).abs().max_element();
                worst = worst.max(d);
                if d > 1e-3 {
                    bad += 1;
                    if bad < 6 {
                        eprintln!("walk at tick {}: P {p:?} -> original {want:?}, port {got:?}", t[1]);
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!("retrack: {n} applies, {walked} walked off the road, {bad} ends differ, worst {worst:e}");
    if n == 0 {
        eprintln!("skipping: the trace has no W0/WE lines");
        return;
    }
    assert_eq!(bad, 0);
}

/// 0x4A03D0, the store's snap (a lag past its limits with `+0x70` clear): `NI` the car as the snap is entered (the block just stored in
/// `+0x38…`), `NO` after it returns (0x4A0EF7). The port's [`Vehicle::net_store_snap`] must leave the velocity and the matrix the original did.
#[test]
fn the_stores_snap_leaves_the_car_the_original_left() {
    let Ok(path) = std::env::var("NFS_DEADRECK_TRACE") else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Ok(paths) = nfs_assets::GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let track = std::env::var("NFS_DEADRECK_TRACK").unwrap_or_else(|_| "coastal".into());
    let t = nfs_assets::track::TrackAssets::load(&paths, &track).unwrap();
    let g = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
    let specs = nfs_physics::specs::CarSpecs::from_sim(&nfs_assets::car::CarAssets::load_sim(&paths, "356BcoupeS16").unwrap(), false);
    let parse = |l: &str| -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let body = words(l.split("pos/vel ").nth(1).unwrap().split('|').next().unwrap());
        let sect = |k: &str| words(l.split(&format!("{k} ")).nth(1).unwrap().split('|').next().unwrap());
        (body, sect("M"), sect("blk"))
    };
    let mut open: Option<(Vec<u32>, Vec<u32>, Vec<u32>)> = None;
    let (mut n, mut worst_v, mut worst_m) = (0, 0.0f32, 0.0f32);
    for l in text.lines() {
        if l.starts_with("NI ") {
            open = Some(parse(l));
        } else if l.starts_with("NO ") {
            let Some((pre, pre_m, blk)) = open.take() else { continue };
            let (post, post_m, _) = parse(l);
            let st = block_of(&blk);
            let mut v = Vehicle::new(Vec3::new(fl(pre[0]), fl(pre[1]), fl(pre[2])), Mat3::IDENTITY, &specs, 0.7, 0.3);
            v.position = Vec3::new(fl(pre[0]), fl(pre[1]), fl(pre[2]));
            let rows = [[fl(pre_m[0]), fl(pre_m[1]), fl(pre_m[2])], [fl(pre_m[3]), fl(pre_m[4]), fl(pre_m[5])], [fl(pre_m[6]), fl(pre_m[7]), fl(pre_m[8])]];
            v.orientation = Mat3::from_cols(Vec3::from(rows[0]), Vec3::from(rows[1]), Vec3::from(rows[2])).transpose();
            v.tracker.seat(&grid, v.position);
            v.net_store_snap(Vec3::from(st.velocity), &matrix_of(st.orientation), &grid);
            let want_v = Vec3::new(fl(post[3]), fl(post[4]), fl(post[5]));
            let dv = (v.velocity - want_v).abs().max_element();
            let dm = (0..3)
                .flat_map(|i| (0..3).map(move |j| (i, j)))
                .map(|(i, j)| (v.orientation.row(i)[j] - fl(post_m[3 * i + j])).abs())
                .fold(0.0, f32::max);
            worst_v = worst_v.max(dv);
            worst_m = worst_m.max(dm);
            n += 1;
            eprintln!("snap at {}: |dv| {dv:e} |dM| {dm:e} (port v {:?}, original {want_v:?})", l.split_whitespace().nth(1).unwrap(), v.velocity);
        }
    }
    if n == 0 {
        eprintln!("skipping: the trace has no 0x4A03D0 calls");
        return;
    }
    eprintln!("store snaps compared {n}: worst |dv| {worst_v:e}, |dM| {worst_m:e}");
    assert!(worst_v < 1e-5 && worst_m < 1e-5);
}
