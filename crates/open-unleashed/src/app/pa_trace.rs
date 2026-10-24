//! **TEST GLUE** (not in the original): the per-tick vehicle trace against the running original
//! (`RE/notes/REFERENCE-2026-10-22-PA-TRACE.md`).
//!
//! * `NFS_PA_INPUT=<file>` — a scripted controller: lines `tick throttle brake steer [handbrake]` in the
//!   units of the original's 4-byte packet `0x5E9664..0x5E9667` (`replay.md` §2: throttle / brake
//!   0..0x7E, steering −63..63, the handbrake bit 0). Each line holds from its `gGameTick` on. The
//!   car reads the packet doubled (`Car_applyInput` 0x4992CF), as [`Controls`] carries it. The same
//!   file drives the original: a gdb breakpoint at 0x499211 (after the replay hook, before
//!   `car+0xD8A = [0x5E9664]`) writes the bytes for the human car.
//! * `NFS_PA_TRACE=<file>` — one row per tick at the start of [`App::simulate`] (the state the
//!   previous tick left, as the original's `GameTick` 0x414FF0 entry sees it): the human car and
//!   every opponent, in the columns the gdb script writes from the car record (`RE/STRUCTURES.md`).

use crate::*;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

fn script() -> &'static [(u64, u8, u8, i8, bool)] {
    static S: OnceLock<Vec<(u64, u8, u8, i8, bool)>> = OnceLock::new();
    S.get_or_init(|| {
        let Some(p) = nfs_game::config::cfg().nfs_pa_input.as_str() else { return Vec::new() };
        let text = std::fs::read_to_string(p).unwrap_or_default();
        let mut v: Vec<(u64, u8, u8, i8, bool)> = text
            .lines()
            .filter_map(|l| {
                let l = l.split('#').next().unwrap_or("");
                let f: Vec<i64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                (f.len() >= 4).then(|| (f[0] as u64, f[1].clamp(0, 0x7E) as u8, f[2].clamp(0, 0x7E) as u8, f[3].clamp(-63, 63) as i8, f.get(4).is_some_and(|&h| h != 0)))
            })
            .collect();
        v.sort_by_key(|e| e.0);
        v
    })
}

/// The scripted controller's sample for `tick`, or `controls` as it stands without a script.
pub(crate) fn input(tick: u64, controls: Controls) -> Controls {
    let s = script();
    match s.iter().rev().find(|e| e.0 <= tick) {
        Some(&(_, thr, brk, steer, hb)) => Controls { throttle: thr * 2, brake: brk * 2, steer: steer as i32 * 2, handbrake: hb, command: 0 },
        None if !s.is_empty() => Controls { command: 0, ..Controls::default() },
        None => controls,
    }
}

/// TEST GLUE: `NFS_PA_LOOPS=<file>` — the player's loop entries the odd tick's drain hands 0x4A93C0
/// (`CollisionSfx::drain_player`), last-first as it walks them: `tick chan sample v·65535`, the
/// columns the PA-AUDIOMIX gdb script logs at the original's 0x4A93C0 entry (there `idA idB`
/// instead of the slot, and the pan / distance words).
pub(crate) fn loops(tick: u64, entries: &[nfs_audio::sfx::QueuedSfx], listener: Vec3, car: Vec3, look: Vec3, car_fwd: Vec3) {
    static W: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    let w = W.get_or_init(|| {
        let p = nfs_game::config::cfg().nfs_pa_loops.as_str()?;
        Some(Mutex::new(std::io::BufWriter::new(std::fs::File::create(p).ok()?)))
    });
    let Some(w) = w else { return };
    let mut w = w.lock().unwrap();
    let _ = writeln!(w, "# {tick} listener {:.4} {:.4} {:.4} car {:.4} {:.4} {:.4} d {:.3} look {:.4} {:.4} {:.4} carfwd {:.4} {:.4} {:.4}", listener.x, listener.y, listener.z, car.x, car.y, car.z, (listener - car).length(), look.x, look.y, look.z, car_fwd.x, car_fwd.y, car_fwd.z);
    for e in entries.iter().rev() {
        match *e {
            nfs_audio::sfx::QueuedSfx::Loop { chan, sample, v } => {
                let _ = writeln!(w, "{tick} {chan} {sample} {}", (v as f64 * 65535.0) as i64);
            }
            // a one-shot (the original's `chan` −1): its kind and `car+0x430`
            ref other => {
                let _ = writeln!(w, "# {tick} oneshot {other:?}");
            }
        }
    }
    let _ = w.flush();
}

fn writer() -> Option<&'static Mutex<std::io::BufWriter<std::fs::File>>> {
    static W: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    W.get_or_init(|| {
        let p = nfs_game::config::cfg().nfs_pa_trace.as_str()?;
        let mut f = std::io::BufWriter::new(std::fs::File::create(p).ok()?);
        let _ = writeln!(f, "{}", HEADER);
        Some(Mutex::new(f))
    })
    .as_ref()
}

/// The columns, shared with the gdb script (`pa_trace_gdb.py`).
pub(crate) const HEADER: &str = "tick,car,px,py,pz,vx,vy,vz,wx,wy,wz,fx,fy,fz,ux,uy,uz,rpm,gear,steer,thr,brk,bh,spd,\
lat0,lon0,slip0,cmp0,lim0,lat1,lon1,slip1,cmp1,lim1,lat2,lon2,slip2,cmp2,lim2,lat3,lon3,slip3,cmp3,lim3,alive,shtm,rng,\
su0,sv0,dm0,su1,sv1,dm1,su2,sv2,dm2,su3,sv3,dm3,z0,z1,z2,z3,z4,z5,z6,z7,z8,z9";

fn row(out: &mut impl Write, tick: i64, car: usize, v: &Vehicle) {
    let (p, vel, w, f, u) = (v.position, v.velocity, v.angular_velocity, v.orientation.row(2), v.orientation.row(1));
    let _ = write!(
        out,
        "{tick},{car},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.3},{},{},{},{},{:.6},{:.6}",
        p.x,
        p.y,
        p.z,
        vel.x,
        vel.y,
        vel.z,
        w.x,
        w.y,
        w.z,
        f.x,
        f.y,
        f.z,
        u.x,
        u.y,
        u.z,
        v.rpm,
        v.gear,
        v.steer,
        v.throttle_raw,
        v.brake_raw,
        v.body_height,
        v.speed_est
    );
    for wh in &v.wheels {
        let _ = write!(out, ",{:.6},{:.6},{:.6},{:.6},{:.6}", wh.lateral, wh.longitudinal, wh.slip, wh.compression, wh.limit);
    }
    let _ = write!(out, ",{},{},{}", v.engine_alive, v.shift_timer, nfs_game::rng::state());
    for wh in &v.wheels {
        let _ = write!(out, ",{:.6},{:.6},{:.6}", wh.slip_vel[0], wh.slip_vel[2], wh.demand);
    }
    for z in v.damage {
        let _ = write!(out, ",{z:.4}");
    }
    let _ = writeln!(out);
}

impl App {
    /// One trace row per car for the state the last tick left (see the module).
    pub(crate) fn pa_trace_tick(&self) {
        let Some(w) = writer() else { return };
        let Ok(mut f) = w.lock() else { return };
        let Some(v) = self.vehicle.as_ref() else { return };
        if std::env::var_os("NFS_PA_RNGLOG").is_some() {
            eprintln!("ROW {}", self.tick as i64 - 1);
        }
        if self.tick == 0
            && let Some(p) = nfs_game::config::cfg().nfs_pa_trace.as_str()
        {
            let mut d = format!("{:#?}\n", self.specs);
            if let (Some(t), Some(r)) = (self.slices.as_ref(), self.race.as_ref()) {
                for k in r.slice.saturating_sub(2)..(r.slice + 3).min(t.slices.len()) {
                    let sl = &t.slices[k];
                    d += &format!("slice {k} centre {:?} up {:?} fwd {:?} lat {:?}\n", sl.centre, sl.up, sl.forward, sl.lateral);
                }
                d += &format!("car rows {:?} {:?} {:?}\n", v.orientation.row(0), v.orientation.row(1), v.orientation.row(2));
            }
            let _ = std::fs::write(format!("{p}.specs"), d);
        }
        // (numbered as the gdb script numbers the original's: `[0x606AC4] − 1`, the tick that left it)
        let t = self.tick as i64 - 1;
        row(&mut *f, t, 0, v);
        for (k, o) in self.road.opponents.iter().enumerate() {
            row(&mut *f, t, k + 1, &o.vehicle);
        }
        // the cops, registered after the racers (`traffic.md` §8), in the traffic rows' layout below
        if let Some(pol) = &self.road.police {
            let base = 1 + self.road.opponents.len();
            for (j, c) in pol.cars.iter().enumerate() {
                let (p, v, w, fw, u) = (c.position, c.velocity, c.angular_velocity, c.orientation.row(2), c.orientation.row(1));
                let _ = write!(
                    &mut *f,
                    "{t},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},0,0,0,0,0,0,{:.6},{},{},{:.6}",
                    base + j, p.x, p.y, p.z, v.x, v.y, v.z, w.x, w.y, w.z, fw.x, fw.y, fw.z, u.x, u.y, u.z, c.speed, c.slice, c.active as u8, c.target_speed
                );
                let _ = write!(&mut *f, "{}", ",0".repeat(17));
                let _ = write!(&mut *f, ",0,0,{}", nfs_game::rng::state());
                let _ = writeln!(&mut *f, "{}", ",0".repeat(22));
            }
        }
        // the traffic cars, registered after the racers and the cops (`traffic.md` §8): position,
        // velocity, `car+0x388` (ω), the orientation rows, `car+0xEC0` in the speed column and `car+0x08` / `+0x7F` /
        // `+0xEB8` in the wheel columns' place; the shared word
        if let Some(tr) = &self.road.traffic {
            let base = 1 + self.road.opponents.len() + self.road.police.as_ref().map_or(0, |p| p.cars.len());
            for (j, c) in tr.cars.iter().enumerate() {
                let (p, v, w, fw, u) = (c.position, c.velocity, c.angular_velocity, c.orientation.row(2), c.orientation.row(1));
                let _ = write!(
                    &mut *f,
                    "{t},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},0,0,0,0,0,0,{:.6},{},{},{:.6}",
                    base + j, p.x, p.y, p.z, v.x, v.y, v.z, w.x, w.y, w.z, fw.x, fw.y, fw.z, u.x, u.y, u.z, c.speed, c.slice, c.active as u8, c.target_speed
                );
                let _ = write!(&mut *f, "{}", ",0".repeat(17));
                let _ = write!(&mut *f, ",0,0,{}", nfs_game::rng::state());
                let _ = writeln!(&mut *f, "{}", ",0".repeat(22));
            }
        }
        let _ = f.flush();
    }
}

/// The seventeen car-setup sliders of `fe.txt`'s `car[k].*` keys (16.16 fixed), in the order of
/// [`nfs_game::career::CAR_SETUP_DEFAULT`], each key missing keeping its neutral value.
pub(crate) fn fe0_setup(root: &std::path::Path, k: usize) -> [f32; 17] {
    let reg = nfs_formats::fetxt::Registry::load(root);
    let keys = [
        "frontdownforce", "reardownforce", "brakebias", "rideheight", "suspensiontravel", "steeringtoe", "shockrate",
        "fronttirepressure", "reartirepressure", "REVERSERATIO", "FIRSTRATIO", "SECONDRATIO", "THIRDRATIO",
        "FOURTHRATIO", "FIFTHRATIO", "SIXTHRATIO", "FINALRATIO",
    ];
    let mut setup = nfs_game::career::CAR_SETUP_DEFAULT;
    for (i, name) in keys.iter().enumerate() {
        setup[i] = reg.u32(&format!("car[{k}].{name}"), (setup[i] * 65536.0) as u32) as i32 as f32 / 65536.0;
    }
    setup
}

/// **TEST GLUE** `NFS_PA_FE0` with opponents: the cars an `fe = 0` race of the original builds for
/// slots `1..=n` from `fe.txt`'s `car[k].*` (`ai.md` §7: `.FILENAME`, `.SPECNAME` → the `.sim`,
/// `.AISPECNAME` → the `.ais` and the family, `.PERSONALITY`, the `.SKILL*` words and ranges as
/// floats' bits) — no parts, no `FECar` (so no stock fit); the model's first catalogue record gives
/// only the body, paint and sound. The pairs go in the key order slow / medium / fast / default.
pub(crate) fn fe0_opponents(root: &std::path::Path, catalog: &[nfs_formats::fedata::CarCatalogEntry], n: usize) -> Vec<CareerOpponent> {
    let reg = nfs_formats::fetxt::Registry::load(root);
    let text = |k: usize, key: &str, def: &str| reg.get(&format!("car[{k}].{key}")).map_or(def.to_string(), |s| s.trim().to_string());
    let float = |k: usize, key: &str, def: f32| f32::from_bits(reg.u32(&format!("car[{k}].{key}"), def.to_bits()));
    (1..=n)
        .filter_map(|k| {
            let model = text(k, "filename", "993");
            let mut car = catalog.iter().find(|c| c.model.eq_ignore_ascii_case(&model))?.clone();
            car.sim = text(k, "specname", "default");
            car.sim_family = text(k, "aispecname", "default");
            car.stock_parts = Vec::new();
            let pair = |s: &str| [float(k, &format!("skill{s}"), 1.0), float(k, &format!("skillrange{s}"), 0.0)];
            Some(CareerOpponent {
                // `fe.txt`'s `car[k].colorconfig = c, c, c, c'` (signed ARGB words): the first as the body colour
                // (TEST GLUE, APPROXIMATION: the original's reader of the key was not traced; the shipped
                // values give the original's red / red / blue / red at slots 1..3 on the screen)
                paint: reg
                    .get(&format!("car[{k}].colorconfig"))
                    .and_then(|v| v.split(',').next()?.trim().parse::<i32>().ok())
                    .map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
                    .unwrap_or_else(|| car.paint_sets.first().map_or([0x20, 0x40, 0xC0], |s| s.colours[0])),
                name: text(k, "drivername", &car.display_name),
                difficulty: 1.0,
                skill: [pair("slow"), pair("medium"), pair("fast"), pair("default")],
                // `fe.txt`'s `car[k].startingpos` (shipped = k, 1 = pole), the port's 0-based `STARTINGPOS - 1`
                grid: Some(reg.u32(&format!("car[{k}].startingpos"), 0).wrapping_sub(1)),
                personality: reg.u32(&format!("car[{k}].personality"), k as u32) as i16,
                parts: Vec::new(),
                decal: -1,
                livery: None,
                car,
            })
        })
        .collect()
}

/// **TEST GLUE** `NFS_PA_FE0`: the racer slot array's `.PERSONALITY` words (`0x65808C + 0x580·k + 0x14C`,
/// nine slots) as an `fe = 0` race reads them from `fe.txt`'s `car[k].personality`, a missing key = 0 (`.bss`).
/// A traffic car's row is `slot[its registration index] % 5` (`TrafficSet::slot_personalities`).
pub(crate) fn fe0_slot_personalities(root: &std::path::Path) -> Vec<i32> {
    let reg = nfs_formats::fetxt::Registry::load(root);
    (0..9).map(|k| reg.u32(&format!("car[{k}].personality"), 0) as i32).collect()
}

/// **TEST GLUE** `NFS_PA_FE0`: the racer slot array's `.STARTINGPOS` word (`0x658200 + 0x580·k`) as an
/// `fe = 0` race reads it from `fe.txt`'s `car[k].startingpos`, a missing key = 0 (`.bss`).
pub(crate) fn fe0_startingpos(root: &std::path::Path, k: usize) -> i32 {
    nfs_formats::fetxt::Registry::load(root).u32(&format!("car[{k}].startingpos"), 0) as i32
}

/// `fe.txt`'s `car[k].transmission` as `carinfo+0xC`: 1 automatic, anything else (or no key) manual.
pub(crate) fn fe0_automatic(root: &std::path::Path, k: usize) -> bool {
    nfs_formats::fetxt::Registry::load(root).u32(&format!("car[{k}].transmission"), 0) == 1
}

/// **TEST GLUE** `NFS_PA_FE0`: the racer slots `1..=8` whose `car[k].class` is `cop` (symbol 8, `Cop`), in slot order,
/// with their `car[k].filename` (the model name).
pub(crate) fn fe0_cops(root: &std::path::Path) -> Vec<(usize, String)> {
    let reg = nfs_formats::fetxt::Registry::load(root);
    (1..=8)
        .filter(|&k| reg.u32(&format!("car[{k}].class"), 0) == 8)
        .map(|k| (k, reg.get(&format!("car[{k}].filename")).map_or(String::new(), |s| s.trim().to_string())))
        .collect()
}
