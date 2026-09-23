//! Multi-car race state: every racer has a vehicle, race progress and a controller. Positions
//! follow the original's ranking table (sorted by progress, ties by the earlier progress tick —
//! `Race_updateProgress` 0x416310). AI controllers are added from `RE/notes/ai.md`.

use crate::race::{RaceProgress, SliceTable};
use glam::Vec3;
use nfs_physics::{specs::CarSpecs, vehicle::{Controls, Vehicle}};

pub enum Controller {
    Player,
    /// Holds the brake (used when no `.ais` data could be loaded).
    Parked,
    /// Opponent driver (`crate::ai`).
    Ai(Box<crate::ai::AiDriver>),
}

pub struct RaceCar {
    pub name: String,
    pub vehicle: Vehicle,
    pub specs: CarSpecs,
    pub progress: RaceProgress,
    pub controller: Controller,
    pub grid_pos: u32,
    /// index into the app's uploaded car models
    pub model: usize,
    pub last_progress_tick: u64,
    pub wall_hits: u32,
    pub resets: u32,
    /// Knocked out (lap knockout / series): parked, ranked below the runners.
    pub eliminated: bool,
    /// diagnostic only: was the car off every drivable patch last tick (`NFS_OFFROAD_LOG`)
    pub off_surface: bool,
}

impl RaceCar {
    pub fn controls_for(&mut self, player: &Controls, table: &SliceTable, line: Option<&nfs_formats::ais::RacingLine>, tick: u64, ahead: &[(Vec3, usize, f32, f32, bool)]) -> Controls {
        // finish state 2 (`ai.md` §3.2): throttle 0, full brake
        if self.progress.finished && !matches!(self.controller, Controller::Player) {
            return Controls { brake: 255, ..Default::default() };
        }
        match &mut self.controller {
            Controller::Player => *player,
            Controller::Parked => Controls { brake: 254, ..Default::default() },
            Controller::Ai(ai) => ai.controls(table, line, &self.progress, &self.vehicle, &self.specs, tick, ahead, 0.9),
        }
    }
}

/// Race positions: index in the ranking table (0 = leader).
pub fn positions(cars: &[RaceCar]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cars.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (cars[a].progress.progress, cars[b].progress.progress);
        pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal).then(cars[a].last_progress_tick.cmp(&cars[b].last_progress_tick))
    });
    let mut pos = vec![0; cars.len()];
    for (rank, &i) in order.iter().enumerate() {
        pos[i] = rank;
    }
    pos
}

/// Update every car's progress (32 Hz) and remember when it last advanced.
pub fn update_progress(cars: &mut [RaceCar], table: &SliceTable, tick: u64, num_laps: u32) {
    for c in cars.iter_mut() {
        let before = c.progress.progress;
        let (pos, fwd) = (c.vehicle.position, c.vehicle.forward());
        c.progress.update(table, pos, fwd, tick, num_laps);
        if c.progress.progress > before {
            c.last_progress_tick = tick;
        }
    }
}

/// Grid placement for `n` cars, car `g` (0 = pole) — see `race::grid_pose`.
pub fn grid_position(table: &SliceTable, start: Option<&nfs_formats::scn::Scene>, g: u32, n: u32) -> (Vec3, glam::Mat3, usize) {
    crate::race::grid_pose(table, start, g, n)
}
