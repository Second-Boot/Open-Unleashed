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

fn row(out: &mut impl Write, tick: u64, car: usize, v: &Vehicle) {
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
        row(&mut *f, self.tick, 0, v);
        for (k, o) in self.road.opponents.iter().enumerate() {
            row(&mut *f, self.tick, k + 1, &o.vehicle);
        }
        let _ = f.flush();
    }
}

/// **TEST GLUE, experiment only** — `NFS_PA_TICK0=1`: the first call of [`App::simulate`] processes
/// tick 0 (the original's first `GameTick` runs with `[0x606AC4] = 0` and increments at its end,
/// 0x415101; the port increments first, so its first processed tick is 1). `NFS_PA_RNG_SKIP=<n>`:
/// `n` draws of the shared stream before that first tick (the original's stream stands 8 draws past
/// the seed at tick 0, measured). Answers whether to skip this call's increment.
pub(crate) fn experiment_tick0(tick: u64) -> bool {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if tick != 0 || DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let skip: u32 = std::env::var("NFS_PA_RNG_SKIP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for _ in 0..skip {
        nfs_game::rng::rand16();
    }
    std::env::var_os("NFS_PA_TICK0").is_some()
}
