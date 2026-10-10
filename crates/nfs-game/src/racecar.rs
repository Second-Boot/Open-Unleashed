//! Multi-car race state: every racer has a vehicle, race progress and a controller. Positions
//! follow the original's ranking table (sorted by progress, ties by the earlier slice-change tick
//! `car+0xB14` — `Race_updateProgress` 0x416310, [`crate::race::ranking`]). AI controllers are added from `RE/notes/ai.md`.

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
    pub wall_hits: u32,
    pub resets: u32,
    /// Knocked out (lap knockout / series): parked, ranked below the runners.
    pub eliminated: bool,
    /// diagnostic only: was the car off every drivable patch last tick (`NFS_OFFROAD_LOG`)
    pub off_surface: bool,
    /// `car+0xD87`, the lamp byte: bit 0 | 1 the headlights, 2 the high beams, 3 (8) / 4 (0x10) the
    /// indicators. `Car_init` 0x410C74 starts it as the night flag; only the commands of a human car's
    /// controller sample change it (`lamp_command`, 0x49933E's switch in 0x4991E0) — the racers'
    /// draw reads it (0x439627 for the headlights, 0x439408 for the indicators).
    pub lamp_bits: u8,
    /// the car's own `.TINYCAR` (`carinfo+0x518`, per-car record id 0x24F): the Gulliver cheat's bit on the machine
    /// the car belongs to — a network player's car carries what its own machine sent (0x4EF467…0x4EF4A0)
    pub tiny: bool,
}

impl RaceCar {
    pub fn controls_for(&mut self, player: &Controls, table: &SliceTable, line: Option<&nfs_formats::ais::RacingLine>, tick: u64, ahead: &[crate::ai::AheadCar]) -> Controls {
        match &mut self.controller {
            Controller::Player => *player,
            // finish state 2 (`ai.md` §3.2): throttle 0, full brake
            Controller::Parked if self.progress.finished => Controls { brake: 255, ..Default::default() },
            Controller::Parked => Controls { brake: 254, ..Default::default() },
            // A finished (or knocked-out) opponent still runs the whole of 0x40B650 and 0x40BFE0 —
            // the draws, the aim point, the avoidance and the steering — and 0x40C5EE only replaces
            // the pedals (and, knocked out, the aim point) by `car+0xB50`, which the caller keeps in
            // `Vehicle::race_state` (`AiDriver::controls`). (history: `game-history.md` §8.1)
            // The own half width
            // is `car+0x3A8` (0x40C577), the model box of 0x445090 — not the 0.9 the port passed.
            Controller::Ai(ai) => ai.controls(table, line, &self.progress, &self.vehicle, &self.specs, tick, ahead, self.vehicle.half_extents.x),
        }
    }
}

/// Race positions: index in the ranking table (0 = leader), [`crate::race::ranking`].
pub fn positions(cars: &[RaceCar]) -> Vec<usize> {
    let keys: Vec<(f32, u64)> = cars.iter().map(|c| (c.progress.progress, c.progress.tick)).collect();
    let order = crate::race::ranking(&keys);
    let mut pos = vec![0; cars.len()];
    for (rank, &i) in order.iter().enumerate() {
        pos[i] = rank;
    }
    pos
}

/// Update every car's progress (32 Hz).
pub fn update_progress(cars: &mut [RaceCar], table: &SliceTable, tick: u64, num_laps: u32) {
    for c in cars.iter_mut() {
        let (pos, fwd) = (c.vehicle.position, c.vehicle.forward());
        // an AI-driven car (`car+0xDA8`) keeps its wrong-way counter clear (0x4A76EE)
        c.progress.wrong_way_gate = crate::race::WrongWayGate::Reset;
        c.progress.update(table, pos, fwd, tick, num_laps);
    }
}

/// Grid placement for `n` cars, car `g` (0 = pole) — see `race::grid_pose`.
pub fn grid_position(table: &SliceTable, start: Option<&nfs_formats::scn::Scene>, g: u32, n: u32, rule: &crate::race::GridRule) -> (Vec3, glam::Mat3, usize) {
    crate::race::grid_pose(table, start, g, n, rule)
}
