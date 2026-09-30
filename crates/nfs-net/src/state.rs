//! The per-tick car-state broadcast — message `0xBE` (`network.md` §5.2).
//!
//! `NfsNet_tick` 0x48EDF0 runs straight from the world tick and sends **once every eight ticks**,
//! i.e. 8 Hz against the 64 Hz clock, a 40-byte datagram: the four-byte message head and then the
//! 36-byte block `0x4A0110` fills from the local player's car. Nothing here is interpolated or
//! predicted on the sender's side — the position goes out at full precision and everything else is
//! quantised.

/// `(gGameTick & 7) == 7` — the send gate at 0x48EE20.
pub const TICK_MASK: u64 = 7;
/// The whole datagram: the head plus [`CarState::SIZE`].
pub const PACKET_SIZE: usize = 0x28;

/// The car state as it crosses the wire.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CarState {
    /// +0x00 — `car+0xD94`, the **ramped steering**, signed, ±126. 0x49DF8C…0x49E000 walk it
    /// toward the target `car+0xD90` by at most 12 a tick (18 under `RALLY`), and 0x49E151
    /// normalises it by 126 for the display. The packet carries its low byte.
    pub steer: i8,
    /// +0x01 — `car+0x424 >= 0.5`, the body is clear of the ground
    pub airborne: bool,
    /// +0x02 — `car+0xD80`, the **effective brake**, 0..252: 0x49E11C divides it by 252 into
    /// `[0x628FAC]`, and 0x49DD71 forces it to 0xFF before the start, which is the "brake full"
    /// half of the race-start gate. Its sibling `car+0xD7F` is the effective throttle, and the two
    /// raw controller bytes behind them are `car+0xD7C` / `car+0xD7D` (`0x5E9666`/`0x5E9667` × 2).
    pub brake: u8,
    /// +0x03 — `car+0xD8A`, the **packed controller buttons** `[0x5E9664]`, copied into the car
    /// each frame at 0x499216 and unpacked at 0x499316: bit 0 → `car+0xD85`, bit 1 → `car+0xD86`,
    /// and `>> 3` → `car+0xD8B`, the command byte whose 7…15 range 0x49934A switches on (5 is the
    /// AI's reset request). Bit 0 is the handbrake and bit 1 the horn (`replay.md` §2, the packer 0x412E70;
    /// `network.md` §11.21): the receiver's remote arm (0x4992B3…0x4992BF) takes exactly those two into
    /// `car+0xD85` / `car+0xD86`; its command bits are not read from the block (the remote car's `car+0xD8B` is the
    /// receiver's own pad's command, 0x499336).
    pub buttons: u8,
    /// +0x04 — the sender's `gGameTick`
    pub tick: u32,
    /// +0x08 — `car+0x38C`, quantised ×1000. That is the angular velocity's **second** component:
    /// 0x49B8E0 takes `car+0x388 / +0x38C / +0x390` as X, Y, Z (its Ry gets `+0x38C`) and the
    /// receiver's snap 0x4A04A0 writes `(0, ω, 0)` there — the **yaw** rate in turns/s. (`network.md`
    /// §5.2 called it `ω.x`, and the port sent its `angular_velocity.x` until NM 2026-09-30.)
    pub yaw_rate: f32,
    /// +0x0A — the orientation, as the four 16.16 quaternion components `0x49FE60` packs
    pub orientation: [i16; 4],
    /// +0x12/+0x14/+0x16 — `car+0x33C/+0x340/+0x344`, the velocity, quantised ×100
    pub velocity: [f32; 3],
    /// +0x18 — `car+0x330…+0x338`, the position, at full precision
    pub position: [f32; 3],
}

/// `[0x5B3060]` — 0x3A83126F, the yaw rate's scale (0x4A0ACA).
pub const F_MILLI: f32 = f32::from_bits(0x3A83_126F);
/// `[0x5B3CA0]` — 0x3C23D70A, the velocity's scale (0x4A0A7A).
pub const F_CENTI: f32 = f32::from_bits(0x3C23_D70A);

/// Round half **away from zero**, which is what the original's `fadd ±0.5` then `ftol` does
/// (0x4A01C0…0x4A01E0 and the three velocity sites).
fn round_away(v: f32) -> i32 {
    let half = if v < 0.0 { -0.5 } else { 0.5 };
    (v + half) as i32
}

impl CarState {
    /// The block `0x4A0110` writes — 0x24 bytes, the datagram's +4 onward.
    pub const SIZE: usize = 0x24;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.steer as u8;
        b[1] = self.airborne as u8;
        b[2] = self.brake;
        b[3] = self.buttons;
        b[4..8].copy_from_slice(&self.tick.to_le_bytes());
        b[8..10].copy_from_slice(&(round_away(self.yaw_rate * 1000.0) as i16).to_le_bytes());
        for (i, q) in self.orientation.iter().enumerate() {
            b[0x0A + i * 2..0x0C + i * 2].copy_from_slice(&q.to_le_bytes());
        }
        for (i, v) in self.velocity.iter().enumerate() {
            b[0x12 + i * 2..0x14 + i * 2].copy_from_slice(&(round_away(v * 100.0) as i16).to_le_bytes());
        }
        for (i, p) in self.position.iter().enumerate() {
            b[0x18 + i * 4..0x1C + i * 4].copy_from_slice(&p.to_le_bytes());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < Self::SIZE {
            return None;
        }
        let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        let f32_at = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(Self {
            steer: b[0] as i8,
            airborne: b[1] != 0,
            brake: b[2],
            buttons: b[3],
            tick: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            // 0x4A0ACA: `fild` x `[0x5B3060]` = 0.001f - the float constant, not a division
            yaw_rate: i16_at(8) as f32 * F_MILLI,
            orientation: [i16_at(0x0A), i16_at(0x0C), i16_at(0x0E), i16_at(0x10)],
            // 0x4A0A7A...: `fild` x `[0x5B3CA0]` = 0.01f
            velocity: [i16_at(0x12) as f32 * F_CENTI, i16_at(0x14) as f32 * F_CENTI, i16_at(0x16) as f32 * F_CENTI],
            position: [f32_at(0x18), f32_at(0x1C), f32_at(0x20)],
        })
    }

    /// The whole 40-byte datagram: `0xBE`, the sender's player id, then the block.
    ///
    /// 0x48EE28 writes the type and 0x48EE51 the id — `net+0xDC` when connected, otherwise `0xFE`
    /// — and `0x4A0110` fills from +4, so bytes 2 and 3 of the head are never written.
    pub fn packet(&self, player_id: u8) -> [u8; PACKET_SIZE] {
        let mut p = [0u8; PACKET_SIZE];
        p[0] = crate::msg::kind::CAR_STATE;
        p[1] = player_id;
        p[4..].copy_from_slice(&self.encode());
        p
    }

    /// `0x48F5E8` — the sender's id is byte 1, and a packet carrying our own id is dropped.
    pub fn from_packet(p: &[u8], my_id: u8) -> Option<(u8, Self)> {
        if p.len() < PACKET_SIZE || p[0] != crate::msg::kind::CAR_STATE || p[1] == my_id {
            return None;
        }
        Self::decode(&p[4..]).map(|s| (p[1], s))
    }
}

/// The orientation the wire carries: a quaternion from the car's 3×3 frame, in 16.16 and then
/// `>> 2` into four `i16` (`0x49FE60`, packed at 0x49FFFF…0x4A000F; `0x4A0020` reads it back by
/// doubling each component).
///
/// The original works in fixed point throughout and takes its square roots with the integer routine
/// 0x5624B0; this does the same algebra in floating point, so a component can differ in its last
/// bit. The branch structure — trace first, otherwise the largest diagonal element — is the
/// original's.
pub fn pack_orientation(m: [[f32; 3]; 3]) -> [i16; 4] {
    // the matrix reaches 0x49FE60 already multiplied by 65536 (0x4A0185…0x4A0196)
    let q = matrix_to_quat(m);
    // 16.16, then `sar 2` (0x49FFF6) — which is the 2¹⁴ of `QUAT_SCALE`
    let s = |v: f32| ((v * 65536.0) as i32 >> 2).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    [s(q[0]), s(q[1]), s(q[2]), s(q[3])]
}

/// The scale the wire carries: `q × 2¹⁴`. The decoder 0x4A0020 forms the products `2·qᵢ·qⱼ` by
/// doubling one factor and multiplying, and builds the matrix against `0x10000000 = 2²⁸` as its
/// unit — which is `(q·2¹⁴)²·2`, so a stored component is the quaternion times 16384.
pub const QUAT_SCALE: f32 = 16384.0;

/// **0x4A0020(q, out)** — the block's quaternion as the nine integers of a 3×3 matrix scaled by
/// 2¹⁶, in `car+0x364`'s row-major order (`out[3·row + col]`), exactly the integer arithmetic of
/// the original (`q = (x, y, z, w)` are the four `i16`s; `0x10000000` is the unit; the products
/// are 32-bit and shifted `>> 12`). The receiver multiplies each by `[0x5B253C]` = 2⁻¹⁶
/// (0x4A0AE8) — [`matrix_of`] — and does **not** normalise: the quantised quaternion's error stays.
pub fn decode_matrix(q: [i16; 4]) -> [i32; 9] {
    let (x, y, z, w) = (q[0] as i32, q[1] as i32, q[2] as i32, q[3] as i32);
    let z2 = z * 2;
    let w_x = w * x * 2;
    let w_y = w * y * 2;
    let xx = x * x * 2;
    let yy = y * y * 2;
    let xy = x * y * 2;
    let base = 0x1000_0000 - z * z2;
    [
        (base - yy) >> 12,
        (xy + w * z2) >> 12,
        (x * z2 - w_y) >> 12,
        (xy - w * z2) >> 12,
        (base - xx) >> 12,
        (y * z2 + w_x) >> 12,
        (x * z2 + w_y) >> 12,
        (y * z2 - w_x) >> 12,
        ((0x1000_0000 - yy) - xx) >> 12,
    ]
}

/// [`decode_matrix`] as floats (`x 2^-16`, `[0x5B253C]`), rows in `car+0x364`'s order — the rows of
/// the car's orientation (body x, up, forward).
pub fn matrix_of(q: [i16; 4]) -> [[f32; 3]; 3] {
    let m = decode_matrix(q).map(|v| v as f32 * f32::from_bits(0x3780_0000));
    [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]
}

/// The inverse of [`pack_orientation`].
pub fn unpack_orientation(q: [i16; 4]) -> [f32; 4] {
    q.map(|v| v as f32 / QUAT_SCALE)
}

fn matrix_to_quat(m: [[f32; 3]; 3]) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    if trace > 0.0 {
        // 0x49FEA3: `trace + 1.0` (0x10000 in 16.16), then the reciprocal of its root
        let r = (trace + 1.0).sqrt();
        let s = 0.5 / r;
        [(m[1][2] - m[2][1]) * s, (m[2][0] - m[0][2]) * s, (m[0][1] - m[1][0]) * s, r * 0.5]
    } else {
        // 0x49FF18: the largest diagonal element picks the axis
        let i = if m[1][1] > m[0][0] { if m[2][2] > m[1][1] { 2 } else { 1 } } else if m[2][2] > m[0][0] { 2 } else { 0 };
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        let r = (m[i][i] - m[j][j] - m[k][k] + 1.0).sqrt();
        let s = 0.5 / r;
        let mut q = [0.0f32; 4];
        q[i] = r * 0.5;
        q[j] = (m[i][j] + m[j][i]) * s;
        q[k] = (m[i][k] + m[k][i]) * s;
        q[3] = (m[j][k] - m[k][j]) * s;
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CarState {
        CarState {
            steer: 0x11,
            airborne: true,
            brake: 0x22,
            buttons: 0x33,
            tick: 0x0001_2345,
            yaw_rate: -1.234,
            orientation: [100, -200, 300, 16383],
            velocity: [12.34, -5.0, 40.0],
            position: [123.5, -4.25, 6789.0],
        }
    }

    #[test]
    fn the_block_is_thirty_six_bytes_and_survives_a_round_trip() {
        let s = sample();
        let b = s.encode();
        assert_eq!(b.len(), 0x24);
        let back = CarState::decode(&b).unwrap();
        // the position is exact; the rest is quantised
        assert_eq!(back.position, s.position);
        assert_eq!(back.orientation, s.orientation);
        assert_eq!((back.steer, back.brake, back.buttons, back.airborne, back.tick), (0x11, 0x22, 0x33, true, 0x0001_2345));
        assert!((back.yaw_rate - s.yaw_rate).abs() <= 0.001);
        for i in 0..3 {
            assert!((back.velocity[i] - s.velocity[i]).abs() <= 0.01, "{i}");
        }
    }

    #[test]
    fn the_quantisation_is_the_originals() {
        // ×1000 for the spin and ×100 for the velocity, rounded away from zero
        let mut s = sample();
        s.yaw_rate = 0.0015;
        s.velocity = [0.005, -0.005, 0.0];
        let b = s.encode();
        assert_eq!(i16::from_le_bytes([b[8], b[9]]), 2, "0.0015 x1000 rounds to 2");
        assert_eq!(i16::from_le_bytes([b[0x12], b[0x13]]), 1);
        assert_eq!(i16::from_le_bytes([b[0x14], b[0x15]]), -1, "away from zero, not toward it");
    }

    #[test]
    fn a_packet_is_forty_bytes_and_drops_our_own_id() {
        let s = sample();
        let p = s.packet(3);
        assert_eq!(p.len(), 0x28);
        assert_eq!((p[0], p[1], p[2], p[3]), (0xBE, 3, 0, 0));
        assert!(CarState::from_packet(&p, 3).is_none(), "our own broadcast is ignored");
        let (id, back) = CarState::from_packet(&p, 9).unwrap();
        assert_eq!(id, 3);
        assert_eq!(back.position, s.position);
    }

    /// 0x4A0020: the identity quaternion (w = 2^14) gives the identity matrix; a quarter turn about Y
    /// gives the rows `pack_orientation` was given, to the 14-bit quantisation.
    #[test]
    fn the_decoder_is_the_originals_integer_matrix() {
        let id = matrix_of([0, 0, 0, 16384]);
        assert_eq!(id, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let m = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        let back = matrix_of(pack_orientation(m));
        for i in 0..3 {
            for j in 0..3 {
                assert!((back[i][j] - m[i][j]).abs() < 2e-3, "{i}{j}: {back:?}");
            }
        }
        // the integers, not a normalised quaternion: 2^16 is the unit
        assert_eq!(decode_matrix([0, 0, 0, 16384])[0], 0x10000);
    }

    #[test]
    fn the_orientation_round_trips_through_the_quaternion() {
        // a quarter turn about Y
        let m = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        let q = pack_orientation(m);
        let f = unpack_orientation(q);
        let n = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2] + f[3] * f[3]).sqrt();
        assert!((n - 1.0).abs() < 0.01, "unit quaternion, got {n}");
        // identity is the w component alone
        let qi = pack_orientation([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let fi = unpack_orientation(qi);
        assert!(fi[0].abs() < 0.01 && fi[1].abs() < 0.01 && fi[2].abs() < 0.01);
        assert!((fi[3] - 1.0).abs() < 0.01, "{fi:?}");
    }

    #[test]
    fn the_send_gate_is_every_eighth_tick() {
        let fires: Vec<u64> = (0..24).filter(|t| t & TICK_MASK == 7).collect();
        assert_eq!(fires, vec![7, 15, 23]);
    }
}
