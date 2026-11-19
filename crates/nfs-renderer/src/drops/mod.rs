//! ENHANCEMENT E40 — raindrops on the windscreen of the cockpit (driver) view. **Not in the original, render-only.**
//!
//! The original's drops (`SPLT`, 0x4577A0 …) are two faint additive sprites on the glass plane and do not move; they
//! stay as they are. This adds refractive drops over the float scene (E7): the app hands a [`Drops`] (the glass quad
//! in the world, the rain's wetness, the car's speed, the render clock) with the scene of the driver view while it
//! rains and the car is not under cover; the renderer keeps a small CPU simulation of the water on the glass
//! ([`Sim`]), turns it into capsule instances (a drop is a disc, a running drop a teardrop, a trail a thin faded
//! capsule) binned in a grid, and one full-screen pass over the resolved float image looks each glass pixel up in
//! that grid and, inside a drop, samples the scene through the drop's lens (an inverted, wide-angle view of the
//! point behind it, a little blur, a dark rim, a small highlight).
//!
//! * **The windscreen region** is the original's own: the `Wndo` corners the splat plane is built from
//!   (`weather::SplatPlane`), projected with the frame's matrices. The screen position of a pixel on that plane (and
//!   the glass's distance there) comes from a homography of the quad's corners, so a drop sits at a fixed place on the
//!   glass whatever the camera does. A pixel counts only when the scene's depth there is at least the glass's
//!   distance (minus 3 cm): the dashboard, the pillars, the mirror, the wipers' rest pose stay clean.
//! * **The water** (all in metres on the glass): beads are born at a rate that grows with the wetness and the speed
//!   (a car drives into the rain); two beads that touch merge by volume; a drop that is heavy enough runs, down the
//!   glass by its weight (the in-plane component of gravity, so a rolled car runs sideways), UP it by the airflow
//!   (an aerodynamic force growing with the speed squared, the faster the more the small ones go), a little
//!   outward from the glass's middle; it leaves a trail that fades, loses water as it runs and merges with what it
//!   meets. Everything below the contact-line force sticks. **Wipers:** the original's are only in the model (not
//!   animated), so a soft periodic wipe clears the glass: two blades that sweep once over 0.9 s, every 3.5-9 s (the
//!   heavier the rain the oftener), at once under cover, and soon after the rain stops.
//! * **Deterministic from the render clock, never the sim RNG:** the state is stepped by the scene's clock in fixed
//!   1/60 s steps from a private xorshift generator with a constant seed; the first frame (or a jump of the clock)
//!   starts from a pre-roll of 2.8 wipe periods at the current rain and speed so the glass is never empty at the first look.

use crate::gfx::{self, DepthBinding, DepthSrc, Fullscreen, PostCtx, UniformBuf, Variants};
use crate::wgsl;
use crate::Camera;
use crate::gpu::HDR_FORMAT;
use glam::{Mat3, Vec2, Vec3};

/// What the app hands the renderer for the driver view of a rainy track (read only with E40 and E7 on; `None` = none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drops {
    /// The windscreen's four corners in the world, in any order (the splat plane's `Wndo` points through the car's pose).
    pub corners: [Vec3; 4],
    /// The rain's wetness at the car, 0..1 (the sky `.bin`'s track B field 5), already faded by the cover.
    pub wet: f32,
    /// How far inside a covered stretch the car is, 0..1 (E24's held, ramped value).
    pub cover: f32,
    /// The car's speed, m/s.
    pub speed: f32,
    /// The sim clock, seconds (a paused game's stands, a replay's is the replay's).
    pub time: f64,
    /// The world's up.
    pub up: Vec3,
}

/// Most drops / trail pieces the water keeps.
const MAX_DROPS: usize = 560;
/// The wobble phase range of a new drop (one turn, as tuned; kept as written so the drop pattern stays the same).
#[allow(clippy::approx_constant)]
const WOBBLE_TURN: f32 = 6.283;
const MAX_TRAILS: usize = 640;
/// The instance texture: two texels per instance.
const INST_W: u32 = 512;
const INST_H: u32 = 8;
pub(crate) const MAX_INSTANCES: usize = (INST_W * INST_H / 2) as usize;
/// The grid over the glass: cells and the instance slots of a cell.
const GRID_COLS: usize = 48;
const GRID_ROWS: usize = 24;
const CELL_SLOTS: usize = 16;
const NO_INSTANCE: u32 = 0xFFFF_FFFF;
/// The data textures' texel sizes: `Rgba32Float`, `R32Uint`.
const INST_TEXEL_BYTES: u32 = 16;
const GRID_TEXEL_BYTES: u32 = 4;
/// The refraction's reach: the sample point lies this many radii on the other side of the drop's centre.
const REFRACT: f32 = 2.4;
/// The step of the simulation, seconds.
const STEP: f32 = 1.0 / 60.0;
/// The step of the pre-roll of a fresh glass (coarser than the live step).
const PRE_STEP: f32 = 1.0 / 20.0;

mod glass;
mod pass;
mod sim;

pub(crate) use glass::{Glass, glass};
pub(crate) use pass::{DropsPass, Look};
#[cfg(test)]
pub(crate) use pass::source;
#[cfg(test)]
use sim::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> [Vec3; 4] {
        // an upright 1.2 x 0.6 m pane two metres ahead of a camera at the origin looking down -z (right-handed look-at)
        [Vec3::new(-0.6, 0.3, -2.0), Vec3::new(0.6, 0.3, -2.0), Vec3::new(0.6, -0.3, -2.0), Vec3::new(-0.6, -0.3, -2.0)]
    }

    fn cam() -> Camera {
        Camera { position: Vec3::ZERO, target: Vec3::new(0.0, 0.0, -1.0), up: Vec3::Y, fov_y_radians: 1.0, near: 0.1, far: 700.0 }
    }

    fn drops(corners: [Vec3; 4]) -> Drops {
        Drops { corners, wet: 1.0, cover: 0.0, speed: 0.0, time: 0.0, up: Vec3::Y }
    }

    #[test]
    fn the_glass_maps_its_corners_to_the_unit_square_and_knows_its_distance() {
        let c = cam();
        let g = glass(&drops(quad()), &c, 1.6).expect("glass");
        let vp = c.view_projection(1.6);
        let uv_of = |p: Vec3| {
            let q = vp * p.extend(1.0);
            let ndc = Vec2::new(q.x, q.y) / q.w;
            let h = g.to_uv * Vec3::new(ndc.x, ndc.y, 1.0);
            (Vec2::new(h.x, h.y) / h.z, 1.0 / (g.inv_dist.dot(Vec3::new(ndc.x, ndc.y, 1.0))))
        };
        let mut uvs: Vec<Vec2> = quad().iter().map(|&p| uv_of(p).0).collect();
        uvs.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
        for (want, got) in [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0)].iter().zip(&uvs) {
            assert!((*want - *got).length() < 1e-3, "{want:?} vs {got:?}");
        }
        // the centre of the pane is the middle of the glass and two metres away
        let (mid, dist) = uv_of(Vec3::new(0.0, 0.0, -2.0));
        assert!((mid - Vec2::splat(0.5)).length() < 1e-3, "{mid:?}");
        assert!((dist - 2.0).abs() < 1e-3, "{dist}");
        assert!((g.size[0] - 1.2).abs() < 1e-4 && (g.size[1] - 0.6).abs() < 1e-4);
        // an upright pane: gravity runs straight down it
        assert!(g.gravity[0].abs() < 1e-4 && (g.gravity[1] + 1.0).abs() < 1e-4, "{:?}", g.gravity);
    }

    #[test]
    fn a_tilted_pane_has_a_gravity_component_and_a_pane_behind_the_camera_is_none() {
        let c = cam();
        // the top leaned back 35 degrees
        let t = 35f32.to_radians();
        let mut q = quad();
        for p in q.iter_mut() {
            let top = p.y > 0.0;
            p.z = -2.0 + if top { -0.3 * t.sin() } else { 0.3 * t.sin() };
            p.y = if top { 0.3 * t.cos() } else { -0.3 * t.cos() };
        }
        let g = glass(&drops(q), &c, 1.6).expect("glass");
        assert!((g.gravity[1] + t.cos()).abs() < 0.02, "{:?}", g.gravity);
        let behind = quad().map(|p| Vec3::new(p.x, p.y, 2.0));
        assert!(glass(&drops(behind), &c, 1.6).is_none());
    }

    fn params(wet: f32, speed: f32) -> Params {
        Params { wet, cover: 0.0, speed, gravity: [0.0, -0.57], size: [1.13, 0.65] }
    }

    fn bead(x: f32, y: f32, r: f32) -> Drop {
        Drop { pos: Vec2::new(x, y), r, dir: Vec2::NEG_Y, run: false, mark: Vec2::ZERO, wob: 0.0 }
    }

    #[test]
    fn the_water_is_a_function_of_the_clock_alone() {
        let p = params(1.0, 12.0);
        let (mut a, mut b) = (Sim::new(5.0, &p), Sim::new(5.0, &p));
        for k in 1..=200 {
            a.advance(5.0 + k as f64 / 30.0, &p);
            b.advance(5.0 + k as f64 / 30.0, &p);
        }
        let (mut ia, mut ga, mut ib, mut gb) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        a.build(p.size, &mut ia, &mut ga);
        b.build(p.size, &mut ib, &mut gb);
        assert!(!ia.is_empty());
        assert_eq!((ia, ga), (ib, gb));
    }

    #[test]
    fn big_drops_run_down_at_rest_and_small_ones_are_blown_up_at_speed() {
        let mut s = Sim::new(0.0, &params(0.0, 0.0));
        s.drops.clear();
        s.trails.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.4, 0.0045));
        s.drops.push(bead(0.9, 0.3, 0.0010));
        for _ in 0..30 {
            s.step(STEP, &params(0.0, 0.0));
        }
        assert!(s.drops.iter().any(|d| d.r > 0.003 && d.pos.y < 0.38), "a fat drop runs down: {:?}", s.drops);
        assert!(s.drops.iter().any(|d| d.r < 0.0015 && (d.pos - Vec2::new(0.9, 0.3)).length() < 1e-4), "a bead sticks");
        // at 40 m/s the airflow wins over the weight, even for a small drop
        let mut s = Sim::new(0.0, &params(0.0, 40.0));
        s.drops.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.2, 0.0020));
        for _ in 0..30 {
            s.step(STEP, &params(0.0, 40.0));
        }
        assert!(s.drops.iter().any(|d| d.pos.y > 0.22), "blown up the glass: {:?}", s.drops);
    }

    #[test]
    fn touching_drops_merge_by_volume() {
        let mut s = Sim::new(0.0, &params(0.0, 0.0));
        s.drops.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.3, 0.0020));
        s.drops.push(bead(0.5033, 0.3, 0.0020));
        s.step(STEP, &params(0.0, 0.0));
        assert_eq!(s.drops.len(), 1);
        assert!((s.drops[0].r - 0.0020 * 2f32.cbrt()).abs() < 1e-5);
    }

    #[test]
    fn the_wipe_clears_the_glass_and_a_dry_glass_goes_idle() {
        let mut s = Sim::new(0.0, &params(1.0, 10.0));
        assert!(!s.drops.is_empty());
        // the rain stops: within a few seconds the wipe has cleared everything and the pass has nothing to do
        let dry = params(0.0, 10.0);
        for _ in 0..(5.0 / STEP) as usize {
            s.step(STEP, &dry);
        }
        assert!(s.drops.is_empty() && s.trails.is_empty(), "{} drops left", s.drops.len());
        assert!(!s.busy(&dry));
        assert!(s.busy(&params(0.5, 10.0)));
        // under cover the glass is cleared at once and nothing is born
        let mut s = Sim::new(0.0, &params(1.0, 10.0));
        let cover = Params { cover: 1.0, ..params(1.0, 10.0) };
        for _ in 0..(2.0 / STEP) as usize {
            s.step(STEP, &cover);
        }
        assert!(s.drops.is_empty());
    }

    #[test]
    fn a_jump_of_the_clock_starts_a_fresh_glass_and_the_instances_fit_the_budget() {
        let p = params(1.0, 25.0);
        let mut s = Sim::new(0.0, &p);
        assert!(!s.advance(0.5, &p));
        assert!(s.advance(100.0, &p), "a jump over 3 s starts afresh");
        assert!(s.advance(50.0, &p), "a clock that went back starts afresh");
        let (mut inst, mut grid) = (Vec::new(), Vec::new());
        let n = s.build(p.size, &mut inst, &mut grid);
        assert!(n > 0 && n <= MAX_INSTANCES && inst.len() == 2 * n);
        assert_eq!(grid.len(), GRID_COLS * GRID_ROWS * CELL_SLOTS);
        assert!(grid.iter().all(|&i| i == NO_INSTANCE || (i as usize) < n));
        assert!(MAX_DROPS + MAX_TRAILS <= MAX_INSTANCES);
    }
}
