//! One remote player's car as the receiver keeps it, and its seat arm.

use super::*;

/// What [`NetPlay::apply`] reads of the seat's car and the tick besides the vehicle: its `.sim` block, the collision grid, the seat's slice
/// centre (the retrack's walk), its slice and the game tick.
pub struct SeatApply<'a> {
    pub specs: &'a nfs_physics::specs::CarSpecs,
    pub grid: &'a nfs_physics::collision::TriangleGrid,
    pub centre: Option<Vec3>,
    pub slice: usize,
    pub tick: u64,
}

/// **TEST GLUE**: `NFS_NET_LAG_MS`, the artificial delay of every car-state packet.
pub(super) fn lag_glue() -> Option<u64> {
    nfs_game::config::cfg().nfs_net_lag_ms.as_str().and_then(|v| v.parse().ok()).filter(|ms| *ms > 0)
}

/// **TEST GLUE**: `NFS_NET_LOCKSTEP=<D>` (ticks), the deterministic arrival of the remote cars' blocks: each is stored at
/// the receiver's tick *sender tick + D* ([`NetPlay::lockstep_wait`]).
pub(super) fn lockstep_glue() -> Option<u64> {
    nfs_game::config::cfg().nfs_net_lockstep.as_str().and_then(|v| v.parse().ok())
}

/// **TEST GLUE** (`NFS_NET_REMOTE_DUMP=<file>`): the remote cars' dead reckoning in the line format of the
/// original's gdb trace (`RE/notes/REFERENCE-2026-10-24-PA-DEADRECK.md`) — `SI` around the store 0x4A0DF0, `AI`/`AO`
/// around the apply 0x4A0A40, every float as its bit pattern in the car's memory order.
pub(super) fn remote_dump(line: impl FnOnce() -> String) {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    static OUT: OnceLock<Option<Mutex<std::io::BufWriter<std::fs::File>>>> = OnceLock::new();
    let out = OUT.get_or_init(|| {
        let path = nfs_game::config::cfg().nfs_net_remote_dump.as_str()?;
        std::fs::File::create(path).ok().map(|f| Mutex::new(std::io::BufWriter::new(f)))
    });
    if let Some(m) = out
        && let Ok(mut w) = m.lock()
    {
        let _ = writeln!(w, "{}", line());
        let _ = w.flush();
    }
}

/// The seat's car as the original's `AI` line holds it (`drpy2.py`: `carstate`), for the harness of `deadreck_tests.rs` to run on the port's own receiving:
/// position / velocity, the rows, the spin, the block, the tracker's slice, and the state `Susp_update` reads (`+0x35C`, `+0x3A0`, `+0xD40…`,
/// the filters, the throttle byte, the plane). Every float as its bit pattern.
pub(super) fn ai_line(tag: &str, tick: u64, seat: usize, v: &Vehicle, blk: &CarState, slice: usize) -> String {
    let h = |f: &[f32]| f.iter().map(|x| format!("{:08x}", x.to_bits())).collect::<Vec<_>>().join(" ");
    let rows: Vec<f32> = (0..3).flat_map(|r| v.orientation.row(r).to_array()).collect();
    let blk_words = blk.encode().chunks(4).map(|w| format!("{:08x}", u32::from_le_bytes([w[0], w[1], w[2], w[3]]))).collect::<Vec<_>>().join(" ");
    let (pn, pp) = (v.tracker.plane_normal, v.tracker.plane_point);
    format!(
        "{tag} {tick} car=00000000 idx={seat} pos/vel {} 00000000 | M {} | w {} | blk {blk_words} | trk {slice:08x} ffffffff | m {} 00000000 00000000 | x35c {} | x3a0 00000000 {} 00000000 | xd40 {} {} 00000000 00000000 00000000 00000000 00000000 00000000 | xdc4 {} | xd7c 00000000 {:08x} | xpl {} | xwh {}",
        h(&[v.position.x, v.position.y, v.position.z, v.velocity.x, v.velocity.y, v.velocity.z]),
        h(&rows),
        h(&v.angular_velocity.to_array()),
        h(&[v.ground_h]),
        h(&[v.speed_est]),
        h(&[v.cos_tilt]),
        h(&v.accel_world.to_array()),
        h(&v.accel_local.to_array()),
        h(&[v.dive, v.roll_lp]),
        u32::from(v.throttle_byte) << 24,
        h(&[pn.x, pn.y, pn.z, 0.0, 0.0, 0.0, pp.x, pp.y, pp.z]),
        v.wheels.iter().map(|w| format!("{} {:08x}", h(&[w.compression]), u32::from(w.airborne))).collect::<Vec<_>>().join(" "),
    )
}

/// The `SB` line of the same trace — the wheels' blocks (`car+0x7B8 + 0xC4·i`: compression `+0x68`, roughness `+0x74…+0x7C`, airborne `+0x84`,
/// rotation `+0x88`), the shared random word, the damage zones — and the stored world points' origin.
pub(super) fn sb_line(tick: u64, v: &Vehicle, specs: &nfs_physics::specs::CarSpecs) -> String {
    let h = |f: &[f32]| f.iter().map(|x| format!("{:08x}", x.to_bits())).collect::<Vec<_>>().join(" ");
    let mut wh = vec![0u32; 196];
    for (i, w) in v.wheels.iter().enumerate() {
        let b = &mut wh[49 * i..49 * (i + 1)];
        b[26] = w.compression.to_bits();
        b[29] = w.rough_prev.to_bits();
        b[30] = w.rough_target.to_bits();
        b[31] = w.rough_phase.to_bits();
        b[33] = u32::from(w.airborne);
        b[34] = w.rotation.to_bits();
    }
    // the body points `car+0x8C…`, the box `car+0x3A8…`, the stored world points `car+0x1DC…` as 0x49AE30 left them (`net_points`)
    let (origin, rows) = v.net_points.unwrap_or((v.position, v.orientation));
    let pts: Vec<f32> = v.body_points.iter().flat_map(|p| (origin + rows.transpose() * *p).to_array()).collect();
    let body: Vec<f32> = v.body_points.iter().flat_map(|p| p.to_array()).collect();
    // the spec words `Susp_update` reads (`[car+0x760]` +0x114, +0x118, +0x120, +0x124, +0x128, +0x12C, +0x134, +0x138) and the installed parts
    let sp = [specs.pitch_ret, specs.roll_ret, specs.probe_offset, specs.bump_travel, specs.dive_gain, specs.roll_gain, specs.pitch_clamp, specs.roll_clamp];
    let parts: Vec<String> = specs
        .parts
        .iter()
        .map(|p| format!("{:x} {:08x} {:08x} {:x}", p.kind, p.scale.to_bits(), p.state.to_bits(), p.mask.iter().enumerate().fold(0u32, |m, (i, b)| m | (u32::from(*b) << i))))
        .collect();
    format!(
        "SB {tick} car=00000000 | qn {} | qbody {} | q3a8 {} | qpts {} | qspecs {} | qparts {} | q330 {} | qwh {} | qrng {:08x} | qdmg {}",
        v.body_points.len(),
        h(&body),
        h(&[v.half_extents.x, v.half_extents.y, v.half_extents.z, 0.0]),
        h(&pts),
        h(&sp),
        parts.join(" "),
        h(&[v.position.x, v.position.y, v.position.z, v.velocity.x, v.velocity.y, v.velocity.z, 0.0]),
        wh.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" "),
        nfs_physics::lcg::STATE.load(std::sync::atomic::Ordering::Relaxed),
        h(&v.damage),
    )
}

/// One remote player's car as the receiver keeps it — the block `car+0x38…+0x5B` and the lag book
/// `car+0x5C…+0x70`, `+0xDA8`, `+0xDAC` that 0x4A0DF0 maintains (`network.md` §8.4).
#[derive(Debug, Clone)]
pub struct Remote {
    pub seat: usize,
    /// `+0x38…+0x5B`; `state.tick` is `+0x3C` (0 until a block has come)
    pub state: CarState,
    /// `+0x5C`: the smoothed lag, `(7·old + lag) / 8` (0x4A0EB1…0x4A0ED1)
    pub lag_avg: i32,
    /// `+0x60`: consecutive fresh blocks (lag below 0x20)
    pub fresh: i32,
    /// `+0x64`: consecutive late blocks (lag above 0x40)
    pub late: i32,
    /// `+0x68`: the last block's lag, `|now − tick|`, at least 1
    pub lag: i32,
    /// `+0x70`: the car is stale — the apply leaves it alone (`+0xDA8 = 1`)
    pub stale: bool,
    /// `+0xDAC`: a block has come (set by the first, 0x4A0E9F)
    pub got: bool,
    /// `+0xDA8`: the last apply found the car stale
    pub held: bool,
    /// 0x4A03D0 is due: the store found the lag past its limits with `+0x70` clear; the seat's car takes
    /// the block's velocity and matrix at the next [`NetPlay::store_snaps`]
    pub(super) snap_due: bool,
}

impl Remote {
    pub fn new(seat: usize) -> Self {
        Self { seat, state: CarState::default(), lag_avg: 0, fresh: 0, late: 0, lag: 0, stale: false, got: false, held: false, snap_due: false }
    }

    /// **0x4A0DF0(car, block)** — the store. A block whose tick is not newer than `+0x3C` is dropped
    /// (0x4A0E04 `jl`, signed); otherwise it becomes the block, the lag `|now − tick|` (at least 1, 0x4A0E4B…)
    /// goes to `+0x68` and into the smoothed `+0x5C = (7·old + lag) / 8`, and:
    ///
    /// * a lag above 0x40 counts `+0x64` up and, once that passes 4 — or at once above 0x80 — the car is
    ///   stale (`+0x70 = 1`), the snap 0x4A03D0 first if it was not; a lag of 0x40 or less clears `+0x64`;
    /// * a lag below 0x20 counts `+0x60` up, and past 8 the car is fresh again (`+0x70 = 0`); otherwise `+0x60 = 0`;
    /// * the first block (`+0x3C` was 0) sets `+0xDAC`.
    ///
    /// Answers whether the block was taken. `now` is `gGameTick` `[0x606AC4]`.
    pub fn store(&mut self, blk: CarState, now: i32) -> bool {
        let old = self.state.tick as i32;
        if old >= blk.tick as i32 {
            return false;
        }
        self.state = blk;
        let age = now.wrapping_sub(blk.tick as i32);
        self.lag = age.abs().max(1);
        // `(x + (x >> 31 & 7)) >> 3` is the division truncating toward zero
        self.lag_avg = (self.lag_avg * 7 + self.lag) / 8;
        if self.lag < 0x41 {
            self.late = 0;
        } else {
            self.late += 1;
            if self.late > 4 || self.lag > 0x80 {
                if !self.stale {
                    self.snap_due = true;
                }
                self.stale = true;
            }
        }
        if self.lag < 0x20 {
            self.fresh += 1;
            if self.fresh > 8 {
                self.stale = false;
            }
        } else {
            self.fresh = 0;
        }
        if old == 0 {
            self.got = true;
        }
        true
    }

    /// The block as the apply 0x4A0A40 reads it (`+0x39`, `+0x3C`, `+0x40`, `+0x42`, `+0x4A`, `+0x50`).
    pub fn block(&self) -> nfs_physics::vehicle::NetBlock {
        let s = &self.state;
        nfs_physics::vehicle::NetBlock {
            tick: s.tick as i32,
            airborne: s.airborne,
            yaw_rate: s.yaw_rate,
            matrix: nfs_net::state::matrix_of(s.orientation),
            velocity: Vec3::from(s.velocity),
            position: Vec3::from(s.position),
        }
    }
}

/// Which of 0x4991E0's controller arms a remote car of a positional race takes this tick
/// ([`NetPlay::seat_arm`]).
#[derive(Debug, Clone, Copy)]
pub enum SeatArm {
    /// no block yet (`car+0xDA8 == 0`, `car+0xDAC == 0`): the local pad
    Pad,
    /// the last block's controls (`+0x38`, `+0x3A`, `+0x3B`)
    Block { brake: u8, steer: i32, handbrake: bool, horn: bool },
    /// stale (`car+0xDA8 == 1`): the AI arm; `stale` is `car+0x70`, the gate of 0x4A0680
    Ai { block: nfs_physics::vehicle::NetBlock, stale: bool },
}

impl SeatArm {
    /// The seat's `car+0xD86` for the horn's audio: the block's bit 1, the local pad's horn before the first
    /// block, 0 for the AI arm (0x499279: `car+0xD86 = 0`).
    pub fn horn(&self, local_horn: bool) -> bool {
        match *self {
            SeatArm::Pad => local_horn,
            SeatArm::Block { horn, .. } => horn,
            SeatArm::Ai { .. } => false,
        }
    }
}
