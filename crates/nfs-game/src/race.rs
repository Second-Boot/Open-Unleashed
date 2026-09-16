//! Track slices, progress, laps, wrong-way and start grid — from `RE/notes/race-logic.md`
//! (CONFIRMED unless noted). Slices are 80-byte records in the track CRP `SimD` level entry,
//! 6.0 m apart along the road; `SimT` gives the slice count per section (main lap first).

use glam::{Mat3, Vec3};
use nfs_formats::crp::{Crp, CrpEntry};
use nfs_formats::scn::{ElementKind, Scene};

pub const SLICE_SPACING: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice {
    pub centre: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    /// up × forward — the car's right-hand side ("A" side)
    pub lateral: Vec3,
    pub lane_mask: u32,
    pub lanes_a: u32,
    pub lanes_b: u32,
    pub width_a: f32,
    pub width_b: f32,
    pub lane_width_a: f32,
    pub lane_width_b: f32,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub first: usize,
    pub last: usize,
    /// entry / exit lap fractions
    pub f0: f32,
    pub f1: f32,
}

#[derive(Debug, Clone)]
pub struct SliceTable {
    pub slices: Vec<Slice>,
    pub sections: Vec<Section>,
    /// slices of the main lap (section 0)
    pub lap_len: usize,
}

impl SliceTable {
    pub fn from_crp(crp: &Crp, data: &[u8]) -> Result<Self, String> {
        let simd = crp.level.iter().find(|e| e.is_level(b"SimD")).ok_or("Could not find sim data in data group (no SimD)")?;
        let bytes = crp.entry_data(data, simd).map_err(|e| e.to_string())?;
        let n = match simd {
            CrpEntry::Level { b, .. } => *b as usize,
            _ => 0,
        };
        if bytes.len() < n * 80 {
            return Err(format!("SimD: {} bytes for {n} slices", bytes.len()));
        }
        let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let u = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let v3 = |o: usize| Vec3::new(f(o), f(o + 4), f(o + 8));
        let mut slices = Vec::with_capacity(n);
        for i in 0..n {
            let b = i * 80;
            slices.push(Slice {
                centre: v3(b + 4),
                up: v3(b + 0x10),
                forward: v3(b + 0x1C),
                lateral: v3(b + 0x28),
                lane_mask: u(b + 0x34),
                lanes_a: u(b + 0x38),
                lanes_b: u(b + 0x3C),
                width_a: f(b + 0x40),
                width_b: f(b + 0x44),
                lane_width_a: f(b + 0x48),
                lane_width_b: f(b + 0x4C),
            });
        }
        let counts: Vec<usize> = match crp.level.iter().find(|e| e.is_level(b"SimT")) {
            Some(e) => {
                let d = crp.entry_data(data, e).map_err(|x| x.to_string())?;
                d.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize).collect()
            }
            None => vec![n],
        };
        let mut sections = Vec::new();
        let mut first = 0;
        for c in counts {
            if c == 0 {
                continue;
            }
            // entry/exit fractions: main lap spans 0..1; branch fractions come from the junction records
            // (not built yet — MEDIUM): approximate a branch by its main-lap projection later.
            sections.push(Section { first, last: first + c - 1, f0: 0.0, f1: 1.0 });
            first += c;
        }
        let lap_len = sections.first().map(|s| s.last - s.first + 1).unwrap_or(n);
        Ok(Self { slices, sections, lap_len })
    }

    fn d2(&self, i: usize, p: Vec3) -> f32 {
        let c = self.slices[i].centre;
        let dx = c.x - p.x;
        let dz = c.z - p.z;
        dx * dx + dz * dz
    }

    /// `SliceTable::findSlice` 0x481400: hill-climb in the xz plane from `prev`, with a coarse global
    /// scan over every 8th slice when farther than 128 m. Restricted to the main lap (junction branch
    /// picking not implemented — MEDIUM).
    pub fn find_slice(&self, pos: Vec3, prev: usize) -> usize {
        let n = self.lap_len.max(1);
        let mut s = prev.min(n - 1);
        if self.d2(s, pos) > 16384.0 {
            let mut best = (f32::MAX, s);
            let mut i = 0;
            while i < n {
                let d = self.d2(i, pos);
                if d < best.0 {
                    best = (d, i);
                }
                i += 8;
            }
            s = best.1;
        }
        for _ in 0..1000 {
            let d = self.d2(s, pos);
            let next = (s + 1) % n;
            let prev_i = (s + n - 1) % n;
            let dn = self.d2(next, pos);
            let dp = self.d2(prev_i, pos);
            if dn < d && dn <= dp {
                s = next;
            } else if dp < d {
                s = prev_i;
            } else {
                break;
            }
        }
        s
    }

    /// Lap fraction of a slice (0x416290).
    pub fn frac(&self, slice: usize) -> f32 {
        let sec = self.sections.iter().find(|s| slice >= s.first && slice <= s.last).unwrap_or(&self.sections[0]);
        let len = (sec.last - sec.first + 1) as f32;
        (slice - sec.first) as f32 / len * (sec.f1 - sec.f0) + sec.f0
    }

    /// Slice arithmetic inside the main lap (wrapping). Junction following (0x4817D0) not implemented.
    pub fn add(&self, slice: usize, delta: i32) -> usize {
        let n = self.lap_len as i32;
        ((slice as i32 + delta).rem_euclid(n)) as usize
    }
}

/// Per-car race bookkeeping (`car+0x530..`).
#[derive(Debug, Clone, Default)]
pub struct RaceProgress {
    pub slice: usize,
    pub laps: u32,
    pub unlaps: u32,
    pub progress: f32,
    pub prev_progress: f32,
    pub lap_start_tick: u64,
    pub lap_times: Vec<u64>,
    pub wrong_way_ticks: u32,
    pub finished: bool,
}

impl RaceProgress {
    pub fn new(start_slice: usize, table: &SliceTable) -> Self {
        let p = table.frac(start_slice);
        Self { slice: start_slice, progress: p, prev_progress: p, ..Default::default() }
    }

    /// Update at 32 Hz (even ticks): tracker, `Lap_check` 0x417440, `Race_updateProgress` 0x416310,
    /// wrong-way 0x4A7660. Returns true when a lap was completed this call.
    pub fn update(&mut self, table: &SliceTable, pos: Vec3, forward: Vec3, tick: u64, num_laps: u32) -> bool {
        self.slice = table.find_slice(pos, self.slice);
        let frac = table.frac(self.slice);
        let new = self.laps as f32 + frac - self.unlaps as f32;
        let fn_ = frac;
        let fp = self.prev_progress.rem_euclid(1.0);
        let mut lap_done = false;
        if fn_ > 0.5 && fp < 0.1 && self.prev_progress != new {
            self.unlaps += 1;
        } else if fn_ < 0.1 && fp > 0.5 {
            if self.unlaps > 0 {
                self.unlaps -= 1;
            } else {
                self.laps += 1;
                self.lap_times.push(tick - self.lap_start_tick);
                self.lap_start_tick = tick;
                lap_done = true;
                if self.laps >= num_laps {
                    self.finished = true;
                }
            }
        }
        self.prev_progress = new;
        self.progress = self.laps as f32 + frac - self.unlaps as f32;
        if forward.dot(table.slices[self.slice].forward) < 0.0 {
            self.wrong_way_ticks += 1;
        } else {
            self.wrong_way_ticks = 0;
        }
        lap_done
    }
}

/// Start pose from the grid rule (`Grid_slot` 0x410580 + `Car_placeAtSlice` 0x49B090).
/// `grid_pos` 0 = pole, `n` racers. Returns (position, orientation rows x/up/forward, slice).
pub fn grid_pose(table: &SliceTable, start_scene: Option<&Scene>, grid_pos: u32, n: u32) -> (Vec3, Mat3, usize) {
    // start slice = slice nearest the type-2 trigger, else 1
    let start_slice = start_scene
        .and_then(|s| {
            s.of_kind(&ElementKind::Trigger).find(|e| e.params.first().and_then(|p| p.first()).map(|t| *t as i32) == Some(2)).map(|e| table.find_slice(Vec3::from(e.position), 0))
        })
        .unwrap_or(1);
    let delta = if n > 2 { 3 * (n as i32 - grid_pos as i32) + 1 } else { 1 };
    let slice = table.add(start_slice, delta);
    let s = &table.slices[slice];
    let lateral = if grid_pos & 1 == 1 { 0.5 * s.lane_width_b } else { -0.5 * s.lane_width_a };
    let pos = s.centre + s.lateral * lateral + s.up * 0.5;
    let fwd = s.forward.normalize();
    let up = s.up.normalize();
    let right = up.cross(fwd).normalize();
    let up = fwd.cross(right).normalize();
    (pos, Mat3::from_cols(right, up, fwd).transpose(), slice)
}
