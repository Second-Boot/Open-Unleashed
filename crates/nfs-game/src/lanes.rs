//! Lane choice for the `AIState_Normal` cars — traffic and cops (`RE/notes/traffic.md` §12 and §23).
//!
//! Every AI tick the car scores three candidates — the lane to its left, the lane it is in, and the lane
//! to its right — and moves to the best one. The scores come from eight small routines that only ever
//! add penalties, except for one bonus when the road is about to narrow and one when a fast human is
//! sitting behind a traffic car. `0x4012B0` then takes the argmax with ties going to the current lane,
//! converts it to a lateral target and caps the target speed for the change.
//!
//! Lane numbering follows the original: lanes 6 and 7 straddle the centre line, lanes below 7 are the
//! negative ("A") side and lanes from 7 up the positive ("B") side; `6 − lanesA` and `lanesB + 7` are the
//! two shoulders and anything outside that range is off the road.
//!
//! Ported: §23.0 helpers, §12.4 blockers and speed caps, §23.1–§23.6 scorers, §12.5 the pick, §12.6 the
//! lateral target and §12.7 the speed cap, plus §12.3's script reactions (`traffic.rs` pushes them into
//! the same scores). Not ported: `0x402F00`'s post-finish pull-over (humans only), and the two abort tests of
//! §12.6's `0x402790`, which §28.5 shows is dead for traffic (every writer of its `car+0x107C` bias is
//! an opponent path). `0x402920`, the other early return, **is** ported — see `squeeze_past`.

use crate::race::{Slice, SliceTable};

/// `0x40EA50(lane)` — the bit of one lane in the slice's lane-exist mask.
pub fn lane_bit(lane: i32) -> u32 {
    1u32 << (31 - (lane + 9).clamp(0, 31))
}

/// `0x40EA90(slice, mask)` — does that lane exist on that slice.
pub fn lane_exists(sl: &Slice, lane: i32) -> bool {
    sl.lane_mask & lane_bit(lane) != 0
}

/// `0x40EB20(slice, offset)` — the lane a lateral offset falls in, clamped to 2..11.
pub fn lane_of(sl: &Slice, offset: f32) -> i32 {
    let (base, w) = if offset < 0.0 { (6, sl.lane_width_a) } else { (7, sl.lane_width_b) };
    if w <= 0.0 {
        return base;
    }
    (base + (offset / w) as i32).clamp(2, 11)
}

/// `0x40EBB0(car)` — the mask of every lane the car's body spans, from `lateral ± (halfWidth − 0.5)`.
pub fn lane_span(sl: &Slice, lateral: f32, half_width: f32) -> u32 {
    let (lo, hi) = (lane_of(sl, lateral - half_width + 0.5), lane_of(sl, lateral + half_width - 0.5));
    let mut m = 0;
    for l in lo.min(hi)..=lo.max(hi) {
        m |= lane_bit(l);
    }
    m
}

/// `0x40E6E0(a, b)` / `0x40E860(a, b)` — the signed gap in metres along the route, positive when `a` is
/// ahead of `b`. (The original refines it by projecting on the car's forward vector inside ±12 m; the
/// port keeps the slice distance.)
pub fn gap_between(table: &SliceTable, a_slice: usize, b_slice: usize) -> f32 {
    let n = table.lap_len as i32;
    let (pa, pb) = (table.route_pos.get(a_slice).copied().unwrap_or(0) as i32, table.route_pos.get(b_slice).copied().unwrap_or(0) as i32);
    let mut d = pa - pb;
    if d > n / 2 {
        d -= n;
    } else if d < -n / 2 {
        d += n;
    }
    d as f32 * 6.0
}

/// One of the five reachable `prsonal.bin` rows (§23.9). A traffic car's `PERSONALITY` comes from the
/// racer slot array and is taken modulo 5, so rows 5..8 — including the one labelled "Traffic" — are
/// unreachable; the port gives traffic row 0, as §23.10 recommends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Personality {
    /// `+0x08` — how far ahead a blocker is looked for, metres
    pub lookahead: f32,
    /// `+0x0C..+0x18` — the four lane offsets a car may take inside its lane
    pub lane_offsets: [f32; 4],
    /// `+0x1C` — the lane-offset re-roll divisor; the offset is redrawn with probability 1/N
    pub reroll: u32,
    /// `+0x20` — braking factor
    pub braking: f32,
}

/// Rows 0..4 of the shipped file, expanded (§23.9).
pub const PERSONALITIES: [Personality; 5] = [
    Personality { lookahead: 210.0, lane_offsets: [0.0, 0.0, 0.0, 0.0], reroll: 400, braking: 1.2 },
    Personality { lookahead: 210.0, lane_offsets: [0.0, 0.0, 0.0, 0.0], reroll: 400, braking: 1.2 },
    Personality { lookahead: 210.0, lane_offsets: [-1.0, -0.5, 0.0, 1.0], reroll: 100, braking: 1.0 },
    Personality { lookahead: 90.0, lane_offsets: [-1.0, -0.5, 0.0, 1.0], reroll: 100, braking: 1.0 },
    Personality { lookahead: 90.0, lane_offsets: [-2.0, 0.0, 1.0, 2.0], reroll: 40, braking: 1.0 },
];

/// What the scorers need to know about one other car on the road.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneCar {
    pub slice: usize,
    /// `car+0x1020` — the lane its centre is in
    pub lane: i32,
    /// `car+0xEA8` — every lane its body spans
    pub lane_mask: u32,
    /// `car+0xEC0` — signed speed
    pub speed: f32,
    /// `car+0xEC4` — the octagonal speed magnitude
    pub speed_est: f32,
    /// `car+0x3B0` — half the body length
    pub half_len: f32,
    /// `car+0xED0` — the car's own lateral offset from the centreline
    pub lateral: f32,
    /// `car+0x538` — half the body's extent projected on the slice's lateral axis
    pub half_span: f32,
    /// `car+0x52C & 1`
    pub is_human: bool,
}

/// The per-car scratch block `0x5E6658 … 0x5E66AC`: slot 0 = lane−1, 1 = current, 2 = lane+1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LaneScratch {
    pub score: [f32; 3],
    /// index into the caller's car list
    pub blocker: [Option<usize>; 3],
    pub gap: [f32; 3],
    pub hard: [f32; 3],
    pub soft: [f32; 3],
}

/// The car doing the scoring.
#[derive(Debug, Clone, Copy)]
pub struct LaneSelf {
    pub slice: usize,
    pub lane: i32,
    pub lateral: f32,
    pub dir: f32,
    pub speed: f32,
    pub target_speed: f32,
    pub half_width: f32,
    pub half_len: f32,
    pub is_traffic: bool,
    /// metres travelled (`0x40E9E0`)
    pub travelled: f32,
}

impl LaneScratch {
    /// `0x401F20(car)` (§12.4) — who is in each lane and how fast this car may go there. Must run before
    /// the scorers: `0x402200` and `0x401C60` read what it writes.
    pub fn find_blockers(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar], pers: &Personality) {
        self.hard = [me.target_speed; 3];
        self.soft = [me.target_speed; 3];
        self.blocker = [None; 3];
        self.gap = [0.0; 3];
        let masks = [lane_bit(me.lane - 1), lane_bit(me.lane), lane_bit(me.lane + 1)];
        let lanes = [me.lane - 1, me.lane, me.lane + 1];
        // the original walks the race order outward in the travel direction and stops once three lanes
        // are filled; nearest-first over the whole list reaches the same three cars
        let mut order: Vec<usize> = (0..cars.len()).collect();
        order.sort_by(|&a, &b| {
            let ga = (me.dir * gap_between(table, cars[a].slice, me.slice)).abs();
            let gb = (me.dir * gap_between(table, cars[b].slice, me.slice)).abs();
            ga.partial_cmp(&gb).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut used = 0u32;
        let mut found = 0;
        for i in order {
            if found >= 3 {
                break;
            }
            let o = &cars[i];
            let g = gap_between(table, o.slice, me.slice);
            let gd = me.dir * g;
            // the closing-rate weights: gentler when almost stopped, much gentler inside 15 m
            let (mut a, mut b) = if me.speed.abs() < 2.222 { (0.8333, 0.4) } else { (1.25, 0.5556) };
            if gd < 15.0 {
                a = 0.16667;
                b = 0.1;
            }
            if me.half_len >= gd || gd >= pers.lookahead {
                continue;
            }
            let t = (g.abs() - 2.0).max(0.0);
            let (hard, soft) = (o.speed + a * t * me.dir, o.speed + b * t * me.dir);
            for k in 0..3 {
                if o.lane_mask & masks[k] != 0 && used & masks[k] == 0 && (0..14).contains(&lanes[k]) {
                    self.hard[k] = hard;
                    self.soft[k] = soft;
                    self.blocker[k] = Some(i);
                    self.gap[k] = gd;
                    used |= masks[k];
                    found += 1;
                }
            }
        }
    }

    /// `0x401A00` (§23.1) — a car travelling `+1` belongs on lanes ≥ 7, one travelling `−1` on lanes < 7.
    pub fn score_own_side(&mut self, me: &LaneSelf, one_way: bool, any_traffic_or_cops: bool) {
        if one_way || !any_traffic_or_cops {
            return;
        }
        let p = if me.is_traffic { -12.0 } else { -4.0 };
        let l = me.lane;
        if me.dir == 1.0 {
            if l - 1 < 7 {
                self.score[0] += p;
            }
            if l < 7 {
                self.score[1] += p;
            }
        } else {
            if l >= 7 {
                self.score[1] += p;
            }
            if l + 1 >= 7 {
                self.score[2] += p;
            }
        }
    }

    /// `0x402200` (§23.2) — the scorer that actually produces lane changes: how much slower than its
    /// target each lane's blocker forces the car to go.
    pub fn score_blockers(&mut self, me: &LaneSelf) {
        let late = me.travelled >= 600.0;
        for k in 0..3 {
            if self.blocker[k].is_some() && self.gap[k] < 30.0 {
                self.score[k] -= 14.0;
            }
        }
        for k in 0..3 {
            if self.blocker[k].is_none() {
                continue;
            }
            let (h, s) = (self.hard[k], self.soft[k]);
            let mut add = |e: f32, w: f32| {
                if e >= 0.0 {
                    self.score[k] += e * w;
                }
            };
            add(me.dir * (me.speed - h), -1.3);
            // MEDIUM: the `late` gate is only in the original's dir == 1 branch — a copy-paste slip that
            // lets oncoming traffic score the target-speed terms from the first metre
            if late || me.dir < 0.0 {
                add(me.dir * (me.target_speed - h), -0.5);
            }
            add(me.dir * (me.speed - s), -0.5);
            if late || me.dir < 0.0 {
                add(me.dir * (me.target_speed - s), -0.1);
            }
        }
    }

    /// `0x4026B0` (§23.3) — do not move into a lane with a car abreast. Traffic is exempt.
    pub fn score_abreast(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar]) {
        if me.is_traffic {
            return;
        }
        let t = me.half_len * 2.5;
        for o in cars {
            let d = gap_between(table, o.slice, me.slice).abs();
            if d > 10.0 || d >= t {
                continue;
            }
            if o.lane == me.lane - 1 {
                self.score[0] -= 6.0;
            } else if o.lane == me.lane + 1 {
                self.score[2] -= 6.0;
            }
        }
    }

    /// `0x401760` (§23.4) — −10 for a lane that is not on the road here, and a one-off −40 for one that
    /// disappears within the next `speed · 4` metres. `scan` is the original's `car+0x101C` window.
    pub fn score_lane_exists(&mut self, table: &SliceTable, me: &LaneSelf, scan: &mut i32, ai_ticks: i32) {
        let lanes = [me.lane - 1, me.lane, me.lane + 1];
        let n = (me.speed.abs() * 4.0 / 6.0) as i32;
        let cur = &table.slices[me.slice];
        for k in 0..3 {
            if !lane_exists(cur, lanes[k]) {
                self.score[k] -= 10.0;
            }
        }
        let d = me.dir as i32;
        let sa = table.add(me.slice, n * d);
        let sb = table.add(me.slice, (n + 1) * d);
        if cur.lane_mask != table.slices[sa].lane_mask || cur.lane_mask != table.slices[sb].lane_mask {
            *scan = 0x49;
        }
        if *scan > 0 {
            let step = (((n as f32) * 0.04) as i32).max(1);
            *scan -= ai_ticks;
            let mut still = [true; 3];
            let (mut k, mut off) = (0, 0);
            while k < n {
                let s = table.add(me.slice, off);
                for j in 0..3 {
                    if still[j] && !lane_exists(&table.slices[s], lanes[j]) {
                        self.score[j] -= 40.0;
                        still[j] = false;
                    }
                }
                k += step;
                off += step * d;
            }
        }
    }

    /// `0x401B70` (§23.5) — the only side-lane bonus: when the road narrows ahead, step toward the
    /// centre line.
    pub fn score_narrowing(&mut self, table: &SliceTable, me: &LaneSelf) {
        let n = ((me.speed.abs() * 0.41667) as i32).max(5);
        let s = table.add(me.slice, n * me.dir as i32);
        let interior = |sl: &Slice| (7 - sl.lanes_a as i32)..=(sl.lanes_b as i32 + 6);
        if interior(&table.slices[s]).contains(&me.lane) {
            return;
        }
        if !interior(&table.slices[me.slice]).contains(&me.lane) {
            return;
        }
        if me.lane < 7 {
            self.score[2] += 40.0;
        } else {
            self.score[0] += 40.0;
        }
    }

    /// `0x401C60` (§23.6) — the shoulders, and the −1000 that keeps a car on the road. The one bonus in
    /// the system: a traffic car pulls onto the shoulder for a human doing more than 8.889 m/s behind it.
    pub fn score_shoulders(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar]) {
        let mut bonus = -4.9;
        if me.is_traffic {
            bonus = -38.0;
            if let Some(i) = self.blocker[1]
                && cars[i].is_human
                && cars[i].speed_est > 8.889
            {
                bonus = 5.0;
            }
        }
        let sl = &table.slices[me.slice];
        let (lo_out, hi_out) = (6 - sl.lanes_a as i32, sl.lanes_b as i32 + 7);
        let narrow_a = sl.width_a - sl.lane_width_a * (sl.lanes_a as f32) < me.half_width;
        let narrow_b = sl.width_b - sl.lane_width_b * (sl.lanes_b as f32) < me.half_width;
        let l = me.lane;
        if l - 1 == lo_out || l - 1 == hi_out {
            self.score[0] += bonus;
            if narrow_a {
                self.score[0] -= 100.0;
            }
        }
        if l + 1 == lo_out || l + 1 == hi_out {
            self.score[2] += bonus;
            if narrow_b {
                self.score[2] -= 100.0;
            }
        }
        if l == lo_out || l == hi_out {
            self.score[1] += bonus;
            if if l < 7 { narrow_a } else { narrow_b } {
                self.score[1] -= 100.0;
            }
        }
        for (k, cand) in [l - 1, l, l + 1].into_iter().enumerate() {
            if cand < lo_out || cand > hi_out {
                self.score[k] -= 1000.0;
            }
        }
    }

    /// `0x402920` (§28.3) — **not** an abort: when the chosen lane is blocked by a car within 20 m,
    /// this re-aims the lateral target at the middle of the free strip beside it, and the caller then
    /// skips its own lane-centre formula *and its road clamps*. `None` means no strip is wide enough,
    /// and the caller falls through to the lane centre. Three quirks of the original are kept: the lane
    /// widths come from the **other** car's slice, the margin uses the car's body-frame half width
    /// while the span uses the road-projected one, and the result is not clamped to the road.
    pub fn squeeze_past(table: &SliceTable, me: &LaneSelf, other: &LaneCar, lane: i32) -> Option<f32> {
        let sl = &table.slices[other.slice.min(table.slices.len() - 1)];
        let w = if lane >= 7 { sl.lane_width_b } else { sl.lane_width_a };
        let (lo, hi) = ((lane - 7) as f32 * w, (lane - 6) as f32 * w);
        let (o_lo, o_hi) = (other.lateral - other.half_span, other.lateral + other.half_span);
        let (u, v) = (hi - o_hi, o_lo - lo);
        let m = 1.5 * me.half_width;
        if u <= m && v <= m {
            return None;
        }
        // a tie takes the low strip, as the original's `jne` does
        Some(if u > v { o_hi + 0.5 * u } else { o_lo - 0.5 * v })
    }

    /// `0x4012B0` (§12.5) — the argmax, ties to the current lane. Returns `(slot, lane)`.
    pub fn pick(&self, me: &LaneSelf) -> (usize, i32) {
        let (mut slot, mut lane, mut best) = (1usize, me.lane, self.score[1]);
        if self.score[0] > self.score[1] {
            slot = 0;
            lane = me.lane - 1;
            best = self.score[0];
        }
        if self.score[2] > best {
            slot = 2;
            lane = me.lane + 1;
        }
        (slot, lane.clamp(0, 13))
    }

    /// `0x402A40` (§12.6, §28.1) — the chosen lane's lateral target. A blocker within 20 m in that
    /// lane first gets the squeeze-past of §28.3, which bypasses the lane centre and the road clamps;
    /// otherwise the car aims at the lane's centre, or hugs the edge in an outermost lane, clamped to
    /// the road. (`0x402790`, the other early return, is dead for traffic: every writer of its
    /// `car+0x107C` bias is an opponent path — §28.5.)
    pub fn lateral_target(&self, table: &SliceTable, me: &LaneSelf, lane: i32, slot: usize, cars: &[LaneCar]) -> f32 {
        if let Some(i) = self.blocker[slot]
            && let Some(o) = cars.get(i)
            && gap_between(table, o.slice, me.slice).abs() < 20.0
            && let Some(t) = Self::squeeze_past(table, me, o, lane)
        {
            return t;
        }
        self.lane_centre(table, me, lane)
    }

    /// The lane-centre half of `0x402A40`, with its road clamps.
    pub fn lane_centre(&self, table: &SliceTable, me: &LaneSelf, lane: i32) -> f32 {
        let sl = &table.slices[me.slice];
        let (lo_out, hi_out) = (6 - sl.lanes_a as i32, sl.lanes_b as i32 + 7);
        let l = lane as f32;
        let mut lat = if lane != lo_out && lane != hi_out {
            // an interior lane: sit in its centre
            if lane < 7 { sl.lane_width_a * (l - 6.5) } else { sl.lane_width_b * (l - 6.5) }
        } else if lane < 7 {
            // the outermost lane: hug the edge
            sl.lane_width_a * (l - 6.0) - me.half_width
        } else {
            sl.lane_width_b * (l - 7.0) + me.half_width
        };
        lat = lat.min(sl.width_b - me.half_width);
        lat.max(me.half_width - sl.width_a)
    }

    /// `0x402BE0` (§12.7) — cap the target speed while changing lane, and hurry the change past a fast
    /// blocker. Returns the new target speed (signed). `lateral_rate` is the original's `0x40EF30`,
    /// which §28.6 shows is `dot(slice lateral axis, velocity)` — for a kinematic traffic car that is
    /// numerically its own `car+0xEF0`.
    pub fn capped_target(&self, me: &LaneSelf, lane: i32, lateral_target: f32, lateral_rate: f32, cars: &[LaneCar]) -> f32 {
        let mut vmax = self.hard[1];
        let blk = self.blocker[1];
        if lane != me.lane && let Some(i) = blk {
            let gap = self.gap[1].abs();
            let dv = (me.speed - cars[i].speed).abs();
            let dlat = (lateral_target - me.lateral).abs();
            if !(dlat * dv * 1.2 > lateral_rate.abs() * gap) {
                vmax = if lane > me.lane { self.hard[2] } else { self.hard[0] };
            }
        }
        let mut v = if me.dir == 1.0 { me.target_speed.min(vmax).max(-1.3) } else { me.target_speed.max(vmax).min(1.3) };
        if lane != me.lane
            && let Some(i) = blk
            && cars[i].speed.abs() > 10.0
        {
            v = if me.dir == 1.0 { v.max(6.6667) } else { v.min(-6.6667) };
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::{Section, SLICE_SPACING};
    use glam::Vec3;

    fn ring(n: usize, lanes_a: u32, lanes_b: u32) -> SliceTable {
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                let mut mask = 0;
                for l in (6 - lanes_a as i32)..=(lanes_b as i32 + 7) {
                    mask |= lane_bit(l);
                }
                Slice {
                    centre: Vec3::new(a.cos() * r, 0.0, a.sin() * r),
                    up: Vec3::Y,
                    forward,
                    lateral: Vec3::Y.cross(forward),
                    lane_mask: mask,
                    lanes_a,
                    lanes_b,
                    // a real road has a shoulder beyond the marked lanes
                    width_a: lanes_a as f32 * 4.0 + 3.0,
                    width_b: lanes_b as f32 * 4.0 + 3.0,
                    lane_width_a: 4.0,
                    lane_width_b: 4.0,
                }
            })
            .collect();
        SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
    }

    fn me(lane: i32, lateral: f32) -> LaneSelf {
        LaneSelf { slice: 10, lane, lateral, dir: 1.0, speed: 16.5, target_speed: 16.5, half_width: 0.9, half_len: 2.1, is_traffic: true, travelled: 1000.0 }
    }

    #[test]
    fn lanes_and_masks_agree_with_each_other() {
        let t = ring(200, 2, 2);
        let sl = &t.slices[0];
        // lane 7 starts at the centre line on the positive side, lane 6 on the negative side
        assert_eq!(lane_of(sl, 0.5), 7);
        assert_eq!(lane_of(sl, 4.5), 8);
        assert_eq!(lane_of(sl, -0.5), 6);
        assert_eq!(lane_of(sl, -4.5), 5);
        // the shipped mask covers the whole road and nothing outside it
        assert!(lane_exists(sl, 4) && lane_exists(sl, 9));
        assert!(!lane_exists(sl, 3) && !lane_exists(sl, 10));
        // a car spans the lanes its body covers
        let m = lane_span(sl, 2.0, 0.9);
        assert_eq!(m, lane_bit(7), "a car inside one lane spans only that lane");
        assert_eq!(lane_span(sl, 4.0, 0.9), lane_bit(7) | lane_bit(8), "straddling the line spans both");
    }

    #[test]
    fn a_slow_blocker_makes_the_car_change_lane() {
        let t = ring(400, 2, 2);
        let m = me(8, 6.0);
        // a slow car two slices (12 m) ahead in lane 8, nothing in the lanes either side. Farther out
        // the cap is not restrictive at all: the permitted speed grows by 1.25 m/s per metre of gap.
        let cars = [LaneCar { slice: 12, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: false }];
        let mut s = LaneScratch::default();
        s.find_blockers(&t, &m, &cars, &PERSONALITIES[0]);
        // lane 8 is the car's own lane, so the blocker lands in slot 1 and caps that lane's speed
        assert_eq!(s.blocker[1], Some(0), "{:?}", s.blocker);
        assert!(s.hard[1] < m.target_speed, "the blocked lane is capped below the target: {:?}", s.hard);
        assert_eq!(s.hard[0], m.target_speed, "the clear lanes keep the target");
        s.score_blockers(&m);
        assert!(s.score[1] < s.score[0] && s.score[1] < s.score[2], "the blocked lane scores worst: {:?}", s.score);
        // so the car moves out of it
        let (slot, lane) = s.pick(&m);
        assert!(slot != 1 && lane != m.lane, "slot {slot} lane {lane}");
    }

    #[test]
    fn the_shoulder_costs_a_traffic_car_more_than_an_opponent() {
        let t = ring(200, 2, 2);
        let mut s = LaneScratch::default();
        let m = me(5, -6.0); // lane 5 is not on a 2+2 road (lo_out = 4), lane 4 is the shoulder
        s.score_shoulders(&t, &m, &[]);
        assert!(s.score[0] <= -38.0, "the shoulder is expensive for traffic: {:?}", s.score);
        let mut s2 = LaneScratch::default();
        let mut o = m;
        o.is_traffic = false;
        s2.score_shoulders(&t, &o, &[]);
        assert!(s2.score[0] > s.score[0], "and cheaper for an opponent");
        // anything off the road at all costs 1000
        let mut s3 = LaneScratch::default();
        let edge = me(4, -10.0);
        s3.score_shoulders(&t, &edge, &[]);
        assert!(s3.score[0] <= -1000.0, "lane 3 is off a 2+2 road: {:?}", s3.score);
    }

    #[test]
    fn traffic_pulls_over_for_a_fast_human() {
        let t = ring(200, 2, 2);
        let m = me(8, 6.0);
        let cars = [LaneCar { slice: 12, lane: 8, lane_mask: lane_bit(8), speed: 40.0, speed_est: 40.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: true }];
        let mut s = LaneScratch::default();
        s.blocker[1] = Some(0);
        s.score_shoulders(&t, &m, &cars);
        // lane 9 is the B-side shoulder of a 2+2 road, and the bonus flips it positive
        assert!(s.score[2] > 0.0, "the shoulder becomes attractive: {:?}", s.score);
    }

    #[test]
    fn a_blocker_in_the_target_lane_is_squeezed_past() {
        let t = ring(200, 2, 2);
        let m = me(8, 6.0);
        // lane 8 spans 4.0..8.0; a car sitting at 5.0 with a 0.9 half-span leaves 2.1 m above it and
        // 0.1 m below, and the margin is 1.5 x the car's own 0.9 half width
        let o = LaneCar { slice: 10, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 5.0, half_span: 0.9, is_human: false };
        let target = LaneScratch::squeeze_past(&t, &m, &o, 8).expect("the upper strip is wide enough");
        assert!((target - (5.9 + 0.5 * 2.1)).abs() < 1e-5, "aim at the middle of the free strip: {target}");
        // a car parked in the middle of the lane leaves nothing on either side
        let centred = LaneCar { lateral: 6.0, half_span: 1.4, ..o };
        assert_eq!(LaneScratch::squeeze_past(&t, &m, &centred, 8), None);
        // and then the caller falls back to the lane centre
        let mut s = LaneScratch::default();
        s.blocker[1] = Some(0);
        assert_eq!(s.lateral_target(&t, &m, 8, 1, &[centred]), s.lane_centre(&t, &m, 8));
        // a blocker further than 20 m away is ignored entirely
        let far = LaneCar { slice: 14, ..o };
        assert_eq!(s.lateral_target(&t, &m, 8, 1, &[far]), s.lane_centre(&t, &m, 8));
    }

    #[test]
    fn the_wrong_side_of_the_road_is_penalised() {
        let mut s = LaneScratch::default();
        let m = me(7, 2.0);
        s.score_own_side(&m, false, true);
        // travelling +1 from lane 7: stepping down to lane 6 crosses the centre line
        assert_eq!(s.score[0], -12.0);
        assert_eq!(s.score[1], 0.0);
        assert_eq!(s.score[2], 0.0);
        // a one-way road switches the whole rule off
        let mut s2 = LaneScratch::default();
        s2.score_own_side(&m, true, true);
        assert_eq!(s2.score, [0.0; 3]);
    }
}
