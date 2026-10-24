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
    /// the car as `SB` found it (0x4A0D6B: after the blend, before the re-seat), by section; empty when the trace has no `SB` lines
    sb: std::collections::HashMap<String, Vec<u32>>,
    /// the apply took its snap arm (`d > 20`): 0x4A02A0 ran (`NS`) and 0x49AE30 left `SB`'s stored points at the *new* position
    snapped: bool,
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
    let mut sb: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    let mut snapped = false;
    let mut open_store: Option<(i32, Vec<u32>)> = None;
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.first().copied() {
            Some("SB") => {
                sb = susp_sections(l);
            }
            Some("NS") => snapped = true,
            Some("AI") => {
                sb.clear();
                snapped = false;
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
                    calls.push(Call { tick, pre, post, blk, blk_after, extra, post_m0, slice, sb: sb.clone(), snapped });
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

/// The remote car's world as the harness builds it: coastal's collision grid and slice table, catalogue car 111 (the 356 B of the live
/// race, `NFS_DEADRECK_CAR`) with its stock parts and the neutral set-up, its `BndP` body.
struct Env {
    grid: nfs_physics::collision::TriangleGrid,
    specs: nfs_physics::specs::CarSpecs,
    body_pts: Vec<Vec3>,
    body_half: Vec3,
    wheel_radius: f32,
    table: nfs_game::race::SliceTable,
}

fn env() -> Option<Env> {
    let paths = nfs_assets::GamePaths::locate().ok()?;
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
    // the installed-part list `car+0x768` (`SuspDamage` 0x493BC0's wobble of the front wheels reads it)
    specs.parts = nfs_physics::specs::stock_parts(&entry.stock_parts, &tables.parts);
    if let Some(po) = std::env::var("NFS_DEADRECK_PROBE").ok().and_then(|v| v.parse::<f32>().ok()) {
        specs.probe_offset = po;
    }
    eprintln!("car {} {} probe_offset {} mass {}", entry.sim, entry.model, specs.probe_offset, specs.mass);
    let assets = nfs_assets::car::CarAssets::load(&paths, &entry.model).unwrap();
    let (body_pts, body_half) = nfs_game::world::car_body(&assets, false);
    let wheel_radius: f32 = std::env::var("NFS_DEADRECK_WHEEL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    Some(Env { grid, specs, body_pts, body_half, wheel_radius, table })
}

/// The stored world points `car+0x1DC…` of an `SB` / `AI` state as the pair [`Vehicle::net_points`] holds (`origin`, `rows`): the rows are
/// the previous apply's, the origin is what puts the first body point where the trace has it (0x49AE30: `pos + Σ p_k · M[k]`).
fn stored_points(pts: &[u32], body: &[Vec3], rows: Mat3) -> Option<(Vec3, Mat3)> {
    let (p0, w0) = (body.first()?, Vec3::new(fl(*pts.first()?), fl(*pts.get(1)?), fl(*pts.get(2)?)));
    // `rows` is the transpose of the car's `orientation` (rows = body axes): world = pos + Σ p_k · row_k
    let m = rows.transpose();
    Some((w0 - (m.col(0) * p0.x + m.col(1) * p0.y + m.col(2) * p0.z), rows))
}

/// A port-written trace (`NFS_NET_REMOTE_DUMP`) names the spec words `Susp_update` reads and the seat's installed parts (`qspecs`, `qparts`): the harness
/// car then is the port's seat car, not the catalogue's default one.
fn override_specs(specs: &mut nfs_physics::specs::CarSpecs, sb: &std::collections::HashMap<String, Vec<u32>>) {
    if let Some(q) = sb.get("qspecs").filter(|q| q.len() == 8) {
        let o = |i: usize| fl(q[i]);
        (specs.pitch_ret, specs.roll_ret, specs.probe_offset, specs.bump_travel) = (o(0), o(1), o(2), o(3));
        (specs.dive_gain, specs.roll_gain, specs.pitch_clamp, specs.roll_clamp) = (o(4), o(5), o(6), o(7));
    }
    if let Some(q) = sb.get("qparts") {
        specs.parts = q
            .chunks(4)
            .filter(|c| c.len() == 4)
            .map(|c| {
                let mask = std::array::from_fn(|i| c[3] >> i & 1 != 0);
                nfs_physics::specs::InstalledPart { kind: c[0] as u16, mask, scale: fl(c[1]), state: fl(c[2]), carried: fl(c[2]) }
            })
            .collect();
    }
}

/// The remote car's body as **the original** holds it (`car+0x8C…` and the box `car+0x3A8…`, from the first `SB` line of the extended
/// trace), or `None` for a trace without them. The harness's own catalogue car is the default style; the sender's record carries its
/// body style (the live port loads the remote car with it, `race_stages.rs`), whose box differs by millimetres — the "8 mm" of
/// `REFERENCE-2026-10-24-PA-DEADRECK.md` was this.
fn trace_body(text: &str) -> Option<(Vec<Vec3>, Vec3)> {
    let l = text.lines().find(|l| l.starts_with("SB "))?;
    let sec = susp_sections(l);
    let b = sec.get("qbody")?;
    let h = sec.get("q3a8")?;
    let pts = b.chunks(3).map(|c| Vec3::new(fl(c[0]), fl(c[1]), fl(c[2]))).collect();
    Some((pts, Vec3::new(fl(h[0]), fl(h[1]), fl(h[2]))))
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
    let Some(Env { grid, mut specs, mut body_pts, mut body_half, wheel_radius, table }) = env() else {
        eprintln!("skipping: no game data");
        return;
    };
    if let Some((p, h)) = std::fs::read_to_string(std::env::var("NFS_DEADRECK_TRACE").unwrap()).ok().and_then(|t| trace_body(&t)) {
        (body_pts, body_half) = (p, h);
    }
    if let Some(sb) = calls.iter().find(|c| c.sb.contains_key("qspecs")).map(|c| &c.sb) {
        override_specs(&mut specs, sb);
    }
    let skip: usize = std::env::var("NFS_DEADRECK_SKIP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let max_calls: usize = std::env::var("NFS_DEADRECK_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);

    let m = |w: &[u32]| [[fl(w[0]), fl(w[1]), fl(w[2])], [fl(w[3]), fl(w[4]), fl(w[5])], [fl(w[6]), fl(w[7]), fl(w[8])]];
    // the wheels' own trackers live from call to call, as the original's do; the stored world points `car+0x1DC…` are the previous call's
    // post-state (0x4A0DB3 / 0x49B9FC: 0x49AE30 at the end of the tick)
    let wheel_trackers = std::cell::RefCell::new([nfs_physics::collision::PatchTracker::default(); 4]);
    let prev_post: std::cell::RefCell<Option<(i32, Vec3, Mat3)>> = std::cell::RefCell::new(None);
    let no_points = std::env::var_os("NFS_DEADRECK_NO_STORED_POINTS").is_some();
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
        if let Some(pl) = c.extra.get("xpl") {
            // the car's own plane `car+0x3E8` / `car+0x400`
            v.tracker.plane_normal = Vec3::new(fl(pl[0]), fl(pl[1]), fl(pl[2]));
            v.tracker.plane_point = Vec3::new(fl(pl[6]), fl(pl[7]), fl(pl[8]));
        }
        for (w, t) in v.wheels.iter_mut().zip(wheel_trackers.borrow().iter()) {
            w.tracker = *t;
        }
        if let Some((t, p, r)) = *prev_post.borrow()
            && t + 1 == c.tick
            && !no_points
        {
            v.net_points = Some((p, r));
        }
        // no apply on the tick before (the car was stale and driven by its AI): the points are where the last tick's end left the body,
        // one step of its own velocity behind the position the apply finds
        if v.net_points.is_none() {
            v.net_points = Some((v.position - v.velocity * (1.0 / 64.0), v.orientation));
        }
        // with `SB` the original's own stored points (the even tick's `Car_ground` refreshes them: its `Susp_update` calls 0x49AE30 itself)
        if !no_points
            && !c.snapped
            && let Some(q) = c.sb.get("qpts")
            && let Some(np) = stored_points(q, &v.body_points, v.net_points.map_or(v.orientation, |n| n.1))
        {
            v.net_points = Some(np);
        }
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
        if !c.sb.is_empty() {
            // what the car's own ticks left in the wheels (`SB`'s blocks: the roughness state, the rotation, the compression) and the shared random word
            for (i, w) in v.wheels.iter_mut().enumerate() {
                let b = &c.sb["qwh"][49 * i..49 * (i + 1)];
                w.compression = fl(b[26]);
                w.airborne = b[33] != 0;
                w.rough_prev = fl(b[29]);
                w.rough_target = fl(b[30]);
                w.rough_phase = fl(b[31]);
                w.rotation = fl(b[34]);
            }
            nfs_physics::lcg::STATE.store(c.sb["qrng"][0], std::sync::atomic::Ordering::Relaxed);
            if let Some(d) = c.sb.get("qdmg").filter(|d| d.len() == 10) {
                // the damage zones `car+0x4E4…` (`SuspDamage` 0x493BC0's wobble of the front wheels reads them)
                for (z, w) in v.damage.iter_mut().zip(d.iter()) {
                    *z = fl(*w);
                }
            }
        }
        if std::env::var_os("NFS_DEADRECK_BLEND").is_some() && !c.sb.is_empty() {
            // the blend alone (the airborne block skips the re-seat): against the position `SB` found after it
            let mut v2 = v.clone();
            v2.net_dead_reckon(&NetBlock { airborne: true, ..blk }, c.tick, &grid, &specs, centre);
            let d = (v2.position - Vec3::new(fl(c.sb["q330"][0]), fl(c.sb["q330"][1]), fl(c.sb["q330"][2]))).abs().max_element();
            eprintln!("BLEND {} {:e}", c.tick, d);
            if c.snapped {
                // the snap tail's own `Susp_update` (on the stored points of before the snap) left these in the wheels' blocks, which `SB` shows
                for i in 0..4 {
                    let b = &c.sb["qwh"][49 * i..49 * (i + 1)];
                    let w = &v2.wheels[i];
                    eprintln!(
                        "SNAPWHEEL {} {i}: port probe ({}, {}) contact y {} susp_y {} zone {:x} | orig probe ({}, {}) contact y {} susp_y {} zone {:x}",
                        c.tick, w.world_pos.x, w.world_pos.z, w.world_pos.y - v2.ride_height, w.susp_y, w.zone, fl(b[13]), fl(b[15]), fl(b[17]), fl(b[23]), b[35]
                    );
                }
            }
            if d > 0.5 {
                eprintln!("BLENDDETAIL {} port {:?} SB {:?}", c.tick, v2.position, Vec3::new(fl(c.sb["q330"][0]), fl(c.sb["q330"][1]), fl(c.sb["q330"][2])));
            }
        }
        v.net_dead_reckon(&blk, c.tick, &grid, &specs, centre);
        *wheel_trackers.borrow_mut() = std::array::from_fn(|i| v.wheels[i].tracker);
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
        let post_pos = Vec3::new(fl(c.post[0]), fl(c.post[1]), fl(c.post[2]));
        let mr = m(&c.post[7..16]);
        *prev_post.borrow_mut() = Some((c.tick, post_pos, Mat3::from_cols(Vec3::from(mr[0]), Vec3::from(mr[1]), Vec3::from(mr[2])).transpose()));
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
        m: Vec<u32>,
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
            m: sect("M"),
        }
    };
    let (mut pis, mut pos_) = (Vec::new(), Vec::new());
    for l in text.lines() {
        if l.starts_with("PI ") {
            pis.push(parse(l));
        } else if l.starts_with("FP ") {
            // the forced experiment (`drpy4.py`): the call's inputs after the progress word was moved, which replace the `PI` line's
            if let Some(last) = pis.last_mut() {
                *last = parse(l);
            }
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
    // the put-on-the-road calls' result (0x4A0803 / 0x4A0930 → 0x4A02A0) against the original's: worst x / z, y, velocity, forward row
    let world = env();
    let mut worst = [0.0f32; 4];
    let mut placed = 0;
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
                // the numbers the rule works with
                let age = a.tick.wrapping_sub(blk.tick);
                let sb = table.track_slice(blk.position, (a.slice[0] & 0xFFFF) as usize, preferred);
                let along = table.slices[sb].forward.dot(blk.velocity);
                let lap_m = table.lap_len as f64 * 6.0;
                let err = ((table.frac(sb) as f64 + (along as f64 * 0.015625 * age as f64) / lap_m) - fl(a.progress) as f64) * lap_m;
                let own = table.slices[(a.slice[0] & 0xFFFF) as usize].forward.dot(Vec3::new(fl(a.vel[0]), fl(a.vel[1]), fl(a.vel[2])));
                eprintln!("   age {age}, sb {sb}, along {along}, own {own}, err {err}, e {}, dv {}, ctl before {:x?}, after {:x?}", -err, along - own, a.ctl, b.ctl);
            }
        }
        if let (StalePlace::PutOnRoad { forward, along }, "put", Some(Env { grid, specs, body_pts, body_half, wheel_radius, .. })) = (&got, want, world.as_ref()) {
            let pre_pos = Vec3::new(fl(a.pos[0]), fl(a.pos[1]), fl(a.pos[2]));
            let mut v = Vehicle::new(pre_pos, Mat3::IDENTITY, specs, 0.7, *wheel_radius);
            v.set_car_body(body_pts.clone(), *body_half);
            v.position = pre_pos;
            v.velocity = Vec3::new(fl(a.vel[0]), fl(a.vel[1]), fl(a.vel[2]));
            v.orientation = Mat3::from_cols(Vec3::new(fl(a.m[0]), fl(a.m[1]), fl(a.m[2])), Vec3::new(fl(a.m[3]), fl(a.m[4]), fl(a.m[5])), Vec3::new(fl(a.m[6]), fl(a.m[7]), fl(a.m[8]))).transpose();
            v.cos_tilt = 1.0;
            v.net_car = true;
            v.net_points = Some((pre_pos - v.velocity * (1.0 / 64.0), v.orientation));
            v.tracker.seat(grid, pre_pos);
            let st = block_of(&a.blk);
            v.net_stale_place(Vec3::from(st.position), *forward, *along, grid, specs);
            let want_pos = Vec3::new(fl(b.pos[0]), fl(b.pos[1]), fl(b.pos[2]));
            let want_vel = Vec3::new(fl(b.vel[0]), fl(b.vel[1]), fl(b.vel[2]));
            let want_fwd = Vec3::new(fl(b.m[6]), fl(b.m[7]), fl(b.m[8]));
            placed += 1;
            worst[0] = worst[0].max((v.position.x - want_pos.x).abs().max((v.position.z - want_pos.z).abs()));
            worst[1] = worst[1].max((v.position.y - want_pos.y).abs());
            worst[2] = worst[2].max((v.velocity - want_vel).abs().max_element());
            worst[3] = worst[3].max((v.orientation.row(2) - want_fwd).abs().max_element());
        }
    }
    eprintln!("stale arm: {} calls, original left {left} / braked {brake} / put on the road {put}, differing {bad}", pis.len());
    if placed > 0 {
        eprintln!("stale put-on-road: {placed} placements compared, worst |dx,dz| {:e}, |dy| {:e}, |dv| {:e}, |d forward row| {:e}", worst[0], worst[1], worst[2], worst[3]);
    }
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

/// One `SB` / `SU` line pair of the extended trace (`drpy2.py`): the car just before `Susp_update` 0x499A70 is called from the apply
/// (0x4A0D6B) and just after it returns (0x4A0D76), with everything it reads — the stored world points `car+0x1DC…`, the body points,
/// the plane, the four wheels' whole blocks (`car+0x820 + 0xC4·i`) — and the ground height it returned (`st0`).
struct Susp {
    tick: i32,
    pre: std::collections::HashMap<String, Vec<u32>>,
    post: std::collections::HashMap<String, Vec<u32>>,
    ground_h: f32,
    normal: Vec3,
}

fn susp_sections(l: &str) -> std::collections::HashMap<String, Vec<u32>> {
    l.split(" | ")
        .skip(1)
        .map(|s| {
            let mut it = s.split_whitespace();
            let k = it.next().unwrap().to_string();
            (k, it.map(|w| u32::from_str_radix(w, 16).unwrap()).collect())
        })
        .collect()
}

/// `Susp_update` + `y -= BodyHeight` (the apply's re-seat 0x4A0D6B…0x4A0DB2) on the original's own state: `SB` is the car before the call,
/// `SU` after. The port's [`Vehicle::net_reseat`] runs on the car built from `SB` (the wheels' blocks, the stored points = the previous
/// apply's end, the plane, the roughness state) and is compared with `SU`: the ground height, the normal, each wheel's probe and contact
/// point, `BodyHeight`, the position and the spin.
#[test]
fn the_reseat_leaves_the_car_the_original_left() {
    let Ok(path) = std::env::var("NFS_DEADRECK_TRACE") else {
        eprintln!("skipping: no NFS_DEADRECK_TRACE");
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Some(Env { grid, mut specs, mut body_pts, mut body_half, wheel_radius, table: _ }) = env() else {
        eprintln!("skipping: no game data");
        return;
    };
    if let Some((p, h)) = trace_body(&text) {
        (body_pts, body_half) = (p, h);
    }
    let mut calls: Vec<Susp> = Vec::new();
    let mut open: Option<(i32, std::collections::HashMap<String, Vec<u32>>)> = None;
    // the original's own end of every apply (`AO`): the position and rows 0x49AE30 builds the next tick's stored points from
    let mut ao_post: std::collections::HashMap<i32, (Vec3, Mat3)> = std::collections::HashMap::new();
    for l in text.lines() {
        if l.starts_with("AO ") {
            let tick: i32 = l.split_whitespace().nth(1).unwrap().parse().unwrap();
            let body = words(l.split("pos/vel ").nth(1).unwrap().split('|').next().unwrap());
            let mm = words(l.split(" | M ").nth(1).unwrap().split('|').next().unwrap());
            ao_post.insert(
                tick,
                (
                    Vec3::new(fl(body[0]), fl(body[1]), fl(body[2])),
                    Mat3::from_cols(Vec3::new(fl(mm[0]), fl(mm[1]), fl(mm[2])), Vec3::new(fl(mm[3]), fl(mm[4]), fl(mm[5])), Vec3::new(fl(mm[6]), fl(mm[7]), fl(mm[8]))).transpose(),
                ),
            );
        } else if l.starts_with("SB ") {
            open = Some((l.split_whitespace().nth(1).unwrap().parse().unwrap(), susp_sections(l)));
        } else if l.starts_with("SU ") {
            let Some((tick, pre)) = open.take() else { continue };
            let t: Vec<&str> = l.split(" | ").next().unwrap().split_whitespace().collect();
            let st0 = t.iter().find_map(|w| w.strip_prefix("st0=")).unwrap();
            let ground_h = if st0 == "nan" { f32::NAN } else { f32::from_bits(u32::from_str_radix(st0, 16).unwrap().swap_bytes()) };
            let n: Vec<f32> = t.iter().find_map(|w| w.strip_prefix("nout=")).map(|f| f.to_string()).into_iter().chain(t.iter().skip_while(|w| !w.starts_with("nout=")).skip(1).map(|w| w.to_string())).map(|w| fl(u32::from_str_radix(&w, 16).unwrap())).collect();
            calls.push(Susp { tick, pre, post: susp_sections(l), ground_h, normal: Vec3::new(n[0], n[1], n[2]) });
        }
    }
    if calls.is_empty() {
        eprintln!("skipping: the trace has no SB / SU lines");
        return;
    }
    if let Some(c) = calls.iter().find(|c| c.pre.contains_key("qspecs")) {
        override_specs(&mut specs, &c.pre);
    }
    let max_calls: usize = std::env::var("NFS_DEADRECK_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    {
        // the body points the original holds (`car+0x8C…`, after 0x49ABC0 dealt the lowest four into the wheel slots) against the harness car's
        let q = &calls[0].pre["qbody"];
        let n = (q.len() / 3).min(body_pts.len());
        let worst = (0..n).map(|k| (Vec3::new(fl(q[3 * k]), fl(q[3 * k + 1]), fl(q[3 * k + 2])) - body_pts[k]).abs().max_element()).fold(0.0f32, f32::max);
        for k in 0..n {
            eprintln!("  body point {k}: trace ({}, {}, {}) harness {:?}", fl(q[3 * k]), fl(q[3 * k + 1]), fl(q[3 * k + 2]), body_pts[k]);
        }
        eprintln!("body points: trace {} / harness {}, worst difference {worst:e}; half extents trace {:?} harness {body_half:?}", q.len() / 3, body_pts.len(), calls[0].pre["q3a8"].iter().map(|w| fl(*w)).collect::<Vec<_>>());
    }
    if let Some(sp) = calls.iter().find_map(|c| c.pre.get("qspec").filter(|s| !s.is_empty())) {
        // the car's own spec block `[car+0x760]` (+0x114 pitch / +0x118 roll retention, +0x120 / +0x124 probe offset / bump travel, +0x128 / +0x12C dive / roll gain,
        // +0x134 / +0x138 the injection clamps) against the harness car's
        let o = |off: usize| fl(sp[off / 4]);
        eprintln!(
            "specs (original | harness): pitch_ret {} | {}, roll_ret {} | {}, probe_offset {} | {}, bump_travel {} | {}, dive_gain {} | {}, roll_gain {} | {}, clamps {} {} | {} {}",
            o(0x114), specs.pitch_ret, o(0x118), specs.roll_ret, o(0x120), specs.probe_offset, o(0x124), specs.bump_travel, o(0x128), specs.dive_gain, o(0x12C), specs.roll_gain, o(0x134), o(0x138), specs.pitch_clamp, specs.roll_clamp
        );
    }
    let m3 = |w: &[u32]| Mat3::from_cols(Vec3::new(fl(w[0]), fl(w[1]), fl(w[2])), Vec3::new(fl(w[3]), fl(w[4]), fl(w[5])), Vec3::new(fl(w[6]), fl(w[7]), fl(w[8]))).transpose();
    let v3 = |w: &[u32]| Vec3::new(fl(w[0]), fl(w[1]), fl(w[2]));
    let mut trackers = [nfs_physics::collision::PatchTracker::default(); 4];
    // [ground h, normal, bodyheight, xz after, y after (before the re-seat's y -= bh), spin, probe xz, contact y, zone word]
    let mut rows: Vec<[f32; 14]> = Vec::new();
    let mut zone_bad = 0;
    let (mut printed, want_print) = (0, std::env::var("NFS_DEADRECK_PRINT").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0));
    for c in calls.iter().take(max_calls) {
        let pre = &c.pre;
        let mut v = Vehicle::new(v3(&pre["q330"]), Mat3::IDENTITY, &specs, 0.7, wheel_radius);
        v.set_car_body(body_pts.clone(), body_half);
        v.position = v3(&pre["q330"]);
        v.velocity = v3(&pre["q330"][3..]);
        v.orientation = m3(&pre["qM"]);
        v.angular_velocity = v3(&pre["qw"]);
        v.cos_tilt = fl(pre["q39c"][1]);
        v.speed_est = fl(pre["q35c"][0]);
        v.accel_world = v3(&pre["qd40"]);
        v.accel_local = v3(&pre["qd40"][3..]);
        v.dive = fl(pre["qdc4"][0]);
        v.roll_lp = fl(pre["qdc4"][1]);
        v.throttle_byte = (pre["qd7c"][0] >> 24) as u8;
        v.body_height = 0.0;
        v.net_car = true;
        v.game_tick = c.tick as u64;
        // the car's own plane `car+0x3E8` / `car+0x400`
        let pl = &pre["qpl"];
        v.tracker.plane_normal = v3(pl);
        v.tracker.plane_point = v3(&pl[6..]);
        v.tracker.seat(&grid, v.position);
        v.tracker.plane_normal = v3(pl);
        v.tracker.plane_point = v3(&pl[6..]);
        for (i, w) in v.wheels.iter_mut().enumerate() {
            let b = &pre["qwh"][49 * i..49 * (i + 1)];
            w.tracker = trackers[i];
            w.compression = fl(b[26]);
            w.airborne = b[33] != 0;
            w.rough_prev = fl(b[29]);
            w.rough_target = fl(b[30]);
            w.rough_phase = fl(b[31]);
            w.rotation = fl(b[34]);
        }
        // the stored points the original probes from (`qpts`: its own 0x49AE30 outputs — the previous apply's end, or the even tick's `Car_ground`)
        let stored_rows = ao_post.get(&(c.tick - 1)).map_or(v.orientation, |p| p.1);
        v.net_points = stored_points(&pre["qpts"], &v.body_points, stored_rows);
        if let Some(d) = pre.get("qdmg").filter(|d| d.len() == 10) {
            for (z, w) in v.damage.iter_mut().zip(d.iter()) {
                *z = fl(*w);
            }
        }
        // the shared random stream the roughness draws from, as the original's word stood
        nfs_physics::lcg::STATE.store(pre["qrng"][0], std::sync::atomic::Ordering::Relaxed);
        let pre_pos = v.position;
        let bh = v.net_reseat(&grid, &specs);
        trackers = std::array::from_fn(|i| v.wheels[i].tracker);
        // the original's own state after Susp_update: its position then still had `+0x424` to lose, its spin `car+0x388`
        let post = &c.post;
        let want_pos = v3(&post["q330"]);
        let want_bh = fl(post["q424"][0]);
        let y_pre = v.position.y + bh;
        let mut dz = 0.0f32;
        let (mut dpxz, mut dcy, mut dsy, mut dhd, mut dair) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0usize);
        for i in 0..4 {
            let b = &post["qwh"][49 * i..49 * (i + 1)];
            let w = &v.wheels[i];
            dpxz = dpxz.max((w.world_pos.x - fl(b[13])).abs()).max((w.world_pos.z - fl(b[15])).abs());
            dcy = dcy.max((w.world_pos.y - v.ride_height - fl(b[17])).abs());
            dsy = dsy.max((w.susp_y - fl(b[23])).abs());
            dhd = dhd.max((w.height_d - fl(b[25])).abs());
            dair += usize::from(w.airborne != (b[33] != 0));
            if (w.zone & 0xFFFF) != (b[35] & 0xFFFF) {
                zone_bad += 1;
                dz = 1.0;
            }
        }
        let row = [
            (v.ground_h - c.ground_h).abs(),
            (v.ground_normal - c.normal).abs().max_element(),
            (bh - want_bh).abs(),
            (v.position.x - want_pos.x).abs().max((v.position.z - want_pos.z).abs()),
            (y_pre - want_pos.y).abs(),
            (v.angular_velocity - v3(&post["qw"])).abs().max_element(),
            dpxz,
            dcy,
            dz,
            dsy,
            dhd,
            dair as f32,
            (v.roll_lp - fl(post["qdc4"][1])).abs(),
            (v.dive - fl(post["qdc4"][0])).abs(),
        ];
        if printed < want_print && (row[7] > 1e-3 || row[3] > 1e-3) {
            printed += 1;
            eprintln!("RESEAT {} row {row:?} pre y {} port y_pre {y_pre} want {} ground_h port {} orig {} bh port {bh} orig {want_bh}", c.tick, pre_pos.y, want_pos.y, v.ground_h, c.ground_h);
            for i in 0..4 {
                let b = &post["qwh"][49 * i..49 * (i + 1)];
                let a = &pre["qwh"][49 * i..49 * (i + 1)];
                eprintln!(
                    "   wheel {i}: port probe {:?} contact y {} zone {:x} | orig probe ({}, {}) contact y {} zone {:x} (pre zone {:x}, pre comp {})",
                    v.wheels[i].world_pos, v.wheels[i].world_pos.y - v.ride_height, v.wheels[i].zone, fl(b[13]), fl(b[15]), fl(b[17]), b[35], a[35], fl(a[26])
                );
            }
        }
        if std::env::var_os("NFS_DEADRECK_CSV").is_some() {
            eprintln!("RS {} {:?}", c.tick, row);
        }
        rows.push(row);
    }
    let pct = |k: usize, q: f32| {
        let mut x: Vec<f32> = rows.iter().map(|r| r[k]).collect();
        x.sort_by(|a, b| a.total_cmp(b));
        x[((x.len() - 1) as f32 * q) as usize]
    };
    eprintln!("re-seats compared {} ({zone_bad} wheel zone words differ)", rows.len());
    for (k, name) in ["ground height", "ground normal", "BodyHeight", "xz after (two-wheel shift)", "y before y -= bh", "spin", "wheel probe xz", "wheel contact y", "zone differs", "wheel susp y (+0x5C)", "wheel d (+0x64)", "wheel airborne differs", "roll filter (+0xDC8)", "dive filter (+0xDC4)"].iter().enumerate() {
        eprintln!("  {name:<28} median {:e}  p99 {:e}  max {:e}", pct(k, 0.5), pct(k, 0.99), pct(k, 1.0));
    }
}
