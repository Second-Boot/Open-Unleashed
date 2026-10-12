//! One remote player's car as the receiver keeps it, and its seat arm.

use super::*;

/// **TEST GLUE**: `NFS_NET_LAG_MS`, the artificial delay of every car-state packet.
pub(super) fn lag_glue() -> Option<u64> {
    std::env::var("NFS_NET_LAG_MS").ok().and_then(|v| v.parse().ok()).filter(|ms| *ms > 0)
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
