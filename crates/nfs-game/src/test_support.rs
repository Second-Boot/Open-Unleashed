//! Shared fixtures of the unit tests: synthetic slice tables (a circular lap, a straight road) and
//! traffic specs, which the modules used to build each their own way (refactor 2026-10-10, D3).
//! Test-only — no game data is read.

use crate::race::{Section, Slice, SliceTable, SLICE_SPACING};
use crate::traffic::TrafficSpec;
use glam::Vec3;

/// The cross-section every slice of a synthetic road shares.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Road {
    pub lane_mask: u32,
    pub lanes_a: u32,
    pub lanes_b: u32,
    pub width_a: f32,
    pub width_b: f32,
    pub lane_width_a: f32,
    pub lane_width_b: f32,
}

impl Road {
    /// One lane a side, `w` metres wide, and as much road.
    pub const fn one_lane(w: f32) -> Self {
        Self { lane_mask: 0, lanes_a: 1, lanes_b: 1, width_a: w, width_b: w, lane_width_a: w, lane_width_b: w }
    }

    /// Two lanes a side, 4 m each, `width` metres of road a side, lane mask unset.
    pub const fn two_lanes(width: f32) -> Self {
        Self { lane_mask: 0, lanes_a: 2, lanes_b: 2, width_a: width, width_b: width, lane_width_a: 4.0, lane_width_b: 4.0 }
    }
}

fn slice(centre: Vec3, forward: Vec3, lateral: Vec3, road: Road) -> Slice {
    Slice {
        centre,
        up: Vec3::Y,
        forward,
        lateral,
        lane_mask: road.lane_mask,
        lanes_a: road.lanes_a,
        lanes_b: road.lanes_b,
        width_a: road.width_a,
        width_b: road.width_b,
        lane_width_a: road.lane_width_a,
        lane_width_b: road.lane_width_b,
    }
}

/// A circular lap of `n` slices, `SLICE_SPACING` apart, one section covering the lap.
pub(crate) fn ring_of(n: usize, road: Road) -> SliceTable {
    let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
    let slices = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            // portable maths (PORT-DEVIATION D1): the ring's bits are the same on every OS / CPU, so the tests that pin
            // a checksum of what drives on it hold on Linux / Windows x86_64 as on macOS arm64
            let (sin, cos) = nfs_math::portable::sin_cosf(a);
            let centre = Vec3::new(cos * r, 0.0, sin * r);
            let forward = Vec3::new(-sin, 0.0, cos);
            slice(centre, forward, Vec3::Y.cross(forward), road)
        })
        .collect();
    SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
}

/// A straight road of `n` slices along +z, one section.
pub(crate) fn straight_of(n: usize, road: Road) -> SliceTable {
    let slices = (0..n).map(|i| slice(Vec3::new(0.0, 0.0, i as f32 * SLICE_SPACING), Vec3::Z, Vec3::X, road)).collect();
    SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
}

/// `n` traffic specs, one model each, the 0.85 x 0.6 x 2.1 m box.
pub(crate) fn traffic_models(n: usize) -> Vec<TrafficSpec> {
    (0..n).map(|i| TrafficSpec { model: i, bus: false, half_extents: Vec3::new(0.85, 0.6, 2.1) }).collect()
}
