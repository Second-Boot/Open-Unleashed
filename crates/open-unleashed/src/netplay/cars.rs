//! The remote cars of a positional race: the stored blocks, the seat arms, the apply and the send.

use super::*;

impl NetPlay {
    /// **0x4A03D0 for the seats whose store asked for it** (`snap_due`): the block's velocity and matrix
    /// go straight into the car (`Vehicle::net_store_snap`). The original runs it inside the receiver
    /// (0x4A0E70), on the network thread, before the next 64 Hz tick reads the car; the port runs it
    /// right after the drain, before the tick's physics.
    pub fn store_snaps(&mut self, mut car: impl FnMut(usize, &nfs_physics::vehicle::NetBlock)) {
        for r in self.remotes.values_mut() {
            if std::mem::take(&mut r.snap_due) {
                car(r.seat, &r.block());
            }
        }
    }

    /// **0x4991E0's three arms for a remote car of a positional race** (0x4991F5…0x499330), by `car+0xDA8`
    /// (`Remote::held`, written by the 64 Hz gate 0x49B9D5 / 0x49B9F1) and `car+0xDAC` (`Remote::got`, the
    /// first block):
    ///
    /// * **`DA8 == 0`, `DAC == 0` — before the first block**: the controller copy reads the **local pad**
    ///   ([`SeatArm::Pad`]): the throttle, brake, steering, handbrake and horn bit of `[0x5E9664…67]`, as the
    ///   local car does (0x4992CF…0x499330). A car a remote player has not yet sent a block for is driven by
    ///   this machine's own controller;
    /// * **`DA8 == 0`, `DAC != 0`** ([`SeatArm::Block`], 0x499285…0x4992CD): the last block — the steering
    ///   `car+0xD90 = (i8) +0x38`, the brake `car+0xD7D = +0x3A`, `car+0xD85` the low bit of `+0x3B` (the
    ///   handbrake bit of the packed buttons), `car+0xD86` its bit 1 (the horn). **The throttle `car+0xD7C` is not
    ///   written by this arm** (0x499285…0x4992CD store `+0xD90`, `+0xD7D`, `+0xD85`, `+0xD86` only): it keeps the
    ///   byte the arm before left — the local pad's before the first block, the AI's after a stale spell — and the
    ///   apply forces the gear neutral (0x4A0D47), so it only revs the engine;
    /// * **`DA8 != 0`** ([`SeatArm::Ai`]): the car is stale (`car+0x70`) — 0x40B650 / 0x40BFE0 drive it, then
    ///   0x4A0680 (`nfs_game::netcar::stale_place`) while `+0x70` is still set; `car+0xD86 = 0`.
    ///
    /// In every arm `car+0xD8B` is then the **local** pad's command (`[0x5E9664] >> 3`, 0x499336), which the
    /// caller sets.
    pub fn seat_arm(&self, seat: usize) -> SeatArm {
        match self.remotes.values().find(|r| r.seat == seat) {
            Some(r) if r.got && r.held => SeatArm::Ai { block: r.block(), stale: r.stale },
            Some(r) if r.got => SeatArm::Block {
                brake: r.state.brake,
                steer: r.state.steer as i32,
                handbrake: r.state.buttons & 1 != 0,
                horn: r.state.buttons & 2 != 0,
            },
            _ => SeatArm::Pad,
        }
    }

    /// **0x49B9AB…0x49B9EC** — the gate in `Car_integrateOrient` 0x49B8E0, then the apply 0x4A0A40
    /// (`Vehicle::net_dead_reckon`): `[0x657408]` (a positional race — the caller), `car+0xDAC` (a block
    /// came) and `car+0x70` clear (else `+0xDA8 = 1` and nothing moves). The class test 0x490140 ≤ 2 is the
    /// caller's (`NetPlay::link_quality`). `centre` is the seat's slice centre (the retrack's walk).
    /// Answers whether the seat is a remote car with a block.
    pub fn apply(
        &mut self,
        seat: usize,
        v: &mut Vehicle,
        specs: &nfs_physics::specs::CarSpecs,
        grid: &nfs_physics::collision::TriangleGrid,
        centre: Option<Vec3>,
        tick: u64,
    ) -> bool {
        let Some(r) = self.remotes.values_mut().find(|r| r.seat == seat) else {
            return false;
        };
        if !r.got {
            return false;
        }
        if r.stale {
            r.held = true;
        } else {
            r.held = false;
            let blk = r.block();
            if v.net_dead_reckon(&blk, tick as i32, grid, specs, centre) {
                // 0x4A0DC3: `|age| > 0x100` — the next ticks leave the car alone until the lag recovers
                r.stale = true;
            }
        }
        true
    }

    /// The send gate of 0x48EE20 — once every eight ticks, the 40-byte `0xBE` packet. `buttons` is the block's
    /// `+0x03`, `car+0xD8A`: the packed controller byte `[0x5E9664]` the local car's controller copy 0x4991E0
    /// took at its last 32 Hz run (0x499211…0x499216) — bit 0 the handbrake, bit 1 the horn, bits 3…7 the
    /// command code (`nfs_game::replay::Packet`).
    pub fn send(&mut self, tick: u64, v: &Vehicle, buttons: u8) {
        if tick & TICK_MASK != TICK_MASK || self.id == u8::MAX || !self.positional {
            return;
        }
        let m = v.orientation.transpose();
        let st = CarState {
            // `car+0xD94` is the ramped steering the port keeps in `Vehicle::steer`, and
            // `car+0xD80` the effective brake, which 0x49E11C reads as a 0..252 byte
            steer: v.steer.clamp(-126, 126) as i8,
            airborne: v.height_marker >= 0.5,
            brake: (v.brake * 252.0).clamp(0.0, 252.0) as u8,
            // `car+0xD8A`, the packed controller buttons `[0x5E9664]` (0x499216) — read back by the receiver's
            // 0x4991E0 remote arm as `car+0xD85` (bit 0, the handbrake) and `car+0xD86` (bit 1, the horn)
            buttons,
            tick: tick as u32,
            // `car+0x38C`, the second component of the angular velocity — the yaw rate
            yaw_rate: v.angular_velocity.y,
            orientation: nfs_net::state::pack_orientation([
                m.x_axis.to_array(),
                m.y_axis.to_array(),
                m.z_axis.to_array(),
            ]),
            velocity: v.velocity.to_array(),
            position: v.position.to_array(),
        };
        tracing::debug!(target: "netplay", "tx tick={tick} pos={:?} steer={} brake={}", st.position, st.steer, st.brake);
        if let Err(e) = self.net.send_game(&st.packet(self.id)) {
            tracing::debug!(error = %e, "car state not sent");
        }
    }
}
