//! Tyre dust and smoke — the emission rule of `Car_wheelEffects` **0x4A2880**.
//!
//! Once per physics step the original walks the four wheels and hands each one's emitter a
//! strength, a kind and the car's motion:
//!
//! ```text
//! for w in 0..4:
//!     (a, b) = w < 2 ? (0.1, 1.1104) : (0.5, 2.0)          ; front / rear
//!     surf = wheel+0x8C
//!     if surf == 9:                       emit(pos, 0.7,  9, dir, speed, 0.0, 1.0)
//!     elif surf == 6 or (wheel+0x80 and surf == 1): emit(pos, 0.7, 6, dir, speed, 0.0, 1.0)
//!     else:                               emit(pos, wheel+0xB8, surf, dir, speed, a, b)
//! ```
//!
//! and the emitter itself (0x478F30) stops when `intensity <= a` and otherwise works with
//! `(intensity − a)·b`, taking `8` particles per burst — `4` for kind 6. So a front tyre starts
//! marking the road at a tenth of slip while a rear one needs half, which is why wheelspin smokes
//! from the back and understeer from the front. The texture is the same for every surface: the
//! 16-entry table at `0x5D00E8` is `'SKDP'` throughout, the 32×32 entry of
//! `GameData/Render/common.fsh`. CONFIRMED.
//!
//! What a particle then *looks* like — its velocity, its life, how it grows — is inside the
//! emitter pool: the motion below is an **APPROXIMATION**. The two addresses this note used to
//! name are not it — `0x478C10` is the emitter **slot allocator** (a 20-entry ring of 20-byte
//! records, at most two claimed at a time) and `0x47A300` a `lower_bound` over 0x50-byte records
//! keyed on `+0x34`, the draw-order lookup. What moves a particle has not been located.

use glam::{Mat4, Vec3};

/// One live particle.
#[derive(Debug, Clone, Copy)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size0: f32,
    size1: f32,
    tint: [f32; 3],
    /// the alpha byte of the surface's `0x5D00A8` entry, before the fade
    alpha: f32,
}

/// `0x5D00A8[surface & 15]` — the particle colour per surface kind, read straight out of the image
/// (0x479200). The alpha byte is the puff's own, and the original scales all four channels by the
/// emitter's intensity (0x479213). `0x5D0128` is the same table for the no-effects path, opaque and
/// with dark grey 0x303030 where this one has black.
pub const SURFACE_ARGB: [[u8; 4]; 16] = [
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [255, 120, 92, 39],
    [255, 0, 0, 0],
    [255, 78, 51, 13],
    [182, 89, 89, 89],
    [255, 0, 0, 0],
    [150, 179, 179, 179],
    [150, 179, 179, 179],
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
];

#[derive(Debug, Default)]
pub struct Particles {
    live: Vec<Particle>,
    rng: u32,
    /// physics ticks since the last burst, per wheel — the port spaces the bursts out where the
    /// original's emitter has its own timer (APPROXIMATION)
    since: [u32; 4],
}

/// The strength an emitter runs at, or `None` when it is below its threshold (0x478F40).
pub fn wheel_emission(front: bool, surface: usize, wet: bool, slip: f32) -> Option<(usize, f32, usize)> {
    let (a, b) = if front { (0.1f32, 1.1104f32) } else { (0.5f32, 2.0f32) };
    let (kind, intensity, thr, scale) = if surface == 9 {
        (9usize, 0.7f32, 0.0f32, 1.0f32)
    } else if surface == 6 || (wet && surface == 1) {
        (6, 0.7, 0.0, 1.0)
    } else {
        (surface, slip, a, b)
    };
    if intensity <= thr {
        return None;
    }
    let count = if kind == 6 { 4 } else { 8 };
    Some((kind, (intensity - thr) * scale, count))
}

impl Particles {
    /// Cap on the live set, so a long slide cannot grow it without bound (port limit).
    const MAX: usize = 96;

    fn rand(&mut self) -> f32 {
        // the shared LCG (`rng.rs` is the game's; this is only for the look of the spray)
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ((self.rng >> 16) & 0x7FFF) as f32 / 32_768.0 - 0.5
    }

    /// One wheel's emitter for one physics step. `ground` is the colour of the surface under the
    /// wheel (`TrackWorld::light_at`), which the dust takes.
    pub fn wheel(&mut self, index: usize, front: bool, surface: usize, wet: bool, slip: f32, pos: Vec3, velocity: Vec3, ground: [f32; 3]) {
        let Some((kind, strength, count)) = wheel_emission(front, surface, wet, slip) else {
            self.since[index & 3] = 0;
            return;
        };
        self.since[index & 3] += 1;
        if self.since[index & 3] < 6 || self.live.len() >= Self::MAX {
            return;
        }
        self.since[index & 3] = 0;
        // the colour is the original's own per-surface table (`SURFACE_ARGB`), not the ground under
        // the wheel; only how fast a puff rises and how long it lives is still an approximation
        let _ = ground;
        let c = SURFACE_ARGB[kind & 15];
        let tint = [c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0];
        let alpha = c[0] as f32 / 255.0;
        let (rise, life) = match kind {
            1 => (0.9, 0.7),
            6 => (1.4, 0.4),
            _ => (1.2, 0.6),
        };
        let n = ((count as f32) * 0.5 * strength.clamp(0.0, 1.0)).round().max(1.0) as usize;
        let room = Self::MAX - self.live.len();
        for _ in 0..n.min(room) {
            let jitter = Vec3::new(self.rand(), 0.0, self.rand()) * 1.5;
            let off = Vec3::new(self.rand(), 0.0, self.rand()) * 0.2;
            let spread = self.rand() * 0.4;
            self.live.push(Particle {
                pos: pos + off,
                vel: -velocity * 0.15 + jitter + Vec3::Y * rise * (0.5 + strength.min(1.0)),
                age: 0.0,
                life: life * (1.0 + spread),
                size0: 0.2,
                size1: 0.7 + strength.min(1.0) * 0.5,
                tint,
                alpha,
            });
        }
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    /// Advance and retire (APPROXIMATION: rise, drag and growth).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        for p in self.live.iter_mut() {
            p.age += dt;
            p.pos += p.vel * dt;
            p.vel *= 1.0 - 2.0 * dt;
            p.vel.y += 0.4 * dt;
        }
        self.live.retain(|p| p.age < p.life);
    }

    /// Camera-facing quads for the live set, with `right`/`up` from the camera basis. The alpha
    /// fades over the particle's life and rides in the vertex colour, so the caller's material only
    /// needs the `SKDP` texture and alpha blending.
    pub fn draw_items(&self, mesh: nfs_renderer::MeshId, material: nfs_renderer::MaterialId, right: Vec3, up: Vec3) -> Vec<nfs_renderer::DrawItem> {
        self.live
            .iter()
            .map(|p| {
                let t = (p.age / p.life).clamp(0.0, 1.0);
                let size = p.size0 + (p.size1 - p.size0) * t;
                let fade = (1.0 - t) * 0.45;
                let m = Mat4::from_cols(
                    (right * size).extend(0.0),
                    (up * size).extend(0.0),
                    right.cross(up).extend(0.0),
                    p.pos.extend(1.0),
                );
                nfs_renderer::DrawItem { light: p.tint, alpha: fade * p.alpha, ..nfs_renderer::DrawItem::new(mesh, material, m) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_per_axle() {
        // a front tyre marks the road from a tenth of slip, a rear one from a half (0x4A2880)
        assert!(wheel_emission(true, 1, false, 0.2).is_some());
        assert!(wheel_emission(false, 1, false, 0.2).is_none());
        assert!(wheel_emission(false, 1, false, 0.6).is_some());
        // …and the strength is what is left over the threshold, times the axle's scale
        let (_, s, n) = wheel_emission(true, 1, false, 0.6).unwrap();
        assert!((s - 0.5 * 1.1104).abs() < 1e-4, "{s}");
        assert_eq!(n, 8);
    }

    #[test]
    fn water_and_surface_nine_always_emit() {
        // wet tarmac and surface 6 are the spray, four particles at a fixed 0.7
        let (kind, s, n) = wheel_emission(false, 1, true, 0.0).unwrap();
        assert_eq!((kind, n), (6, 4));
        assert!((s - 0.7).abs() < 1e-6);
        assert_eq!(wheel_emission(false, 9, false, 0.0).map(|e| e.0), Some(9));
    }

    #[test]
    fn a_burst_lives_and_dies() {
        let mut p = Particles::default();
        for _ in 0..6 {
            p.wheel(0, true, 3, false, 0.9, Vec3::ZERO, Vec3::Z * 20.0, [0.5, 0.4, 0.3]);
        }
        assert!(!p.is_empty());
        for _ in 0..200 {
            p.step(1.0 / 64.0);
        }
        assert!(p.is_empty(), "every particle retires");
    }
}
