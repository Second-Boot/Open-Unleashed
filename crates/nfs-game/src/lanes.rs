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
//! the same scores). Not ported: `0x402F00`'s post-finish pull-over (it asks for a racer, and only cops and
//! traffic reach it, so it never fires — `traffic.md` §12.10a). §12.6's `0x402790` is dead for traffic
//! (no traffic path writes its `car+0x107C` bias) but live for a chasing cop, whose `AIState_Chase`
//! writes it every tick (`traffic.md` §28.3, corrected 2026-09-26) — `override_holds`. `0x402920`, the
//! other early return, **is** ported — see `squeeze_past`.

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
///
/// **It does not divide by the lane width.** It rounds the width to a whole number and looks the
/// reciprocal up in a 20-entry table:
///
/// ```text
/// 0x40EB6F  fld  width ; fistp n            ; round to nearest, not truncate
/// 0x40EB78  fld  [n*4 + 0x5E7550]           ; = 1/n
/// 0x40EB7F  fmul offset
/// 0x40EB85  fistp                           ; round to nearest again
/// 0x40EB8E  clamp 2 … 11
/// ```
///
/// The table is built by 0x4068A0: `[0x5E7550] = 0.0` and `[0x5E7550 + 4i] = 1/i` for
/// **i = 1…19** (the loop stores while `eax < 0x5E75A0`). So a 3.7 m lane is treated as 4 m, a
/// width under 0.5 rounds to 0 and reads the table's leading **0.0**, which puts everything in
/// the base lane — which is what the old `w <= 0.0` guard was approximating.
///
/// Both conversions are bare `fistp`, i.e. **round to nearest**; the port truncated with `as i32`
/// and divided by the raw width. Corrected 2026-09-20.
pub fn lane_of(sl: &Slice, offset: f32) -> i32 {
    let (base, w) = if offset < 0.0 { (6, sl.lane_width_a) } else { (7, sl.lane_width_b) };
    let n = round_half_even(w);
    if !(1..=19).contains(&n) {
        // n = 0 reads the table's 0.0; anything past 19 is off the end of it and never happens
        // on shipped data, where a lane is 2…6 m
        return base;
    }
    (base + round_half_even(offset * (1.0 / n as f32))).clamp(2, 11)
}

/// `fistp` with the x87's default rounding mode: **to nearest, ties to even**. Rust's `f32::round`
/// is ties-away-from-zero, and `as i32` truncates, so neither is it.
fn round_half_even(v: f32) -> i32 {
    let r = v.round();
    // `round` only differs from the x87 on an exact .5, where it goes away from zero
    if (v - v.trunc()).abs() == 0.5 && (r as i32) % 2 != 0 { r as i32 - v.signum() as i32 } else { r as i32 }
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

/// `0x40E860(a, b)` in full (0x40E860…0x40E91B, read 2026-09-26): the coarse slice gap of
/// [`gap_between`], and when that is within ±12 m (`[0x5B240C]` / `[0x5B24C8]`, neither `> 12` nor
/// `< −12`) instead `dot2D(a+0x330 − b+0x330, a+0x3D0)` — the x–z projection of the offset on `a`'s
/// forward axis. `+0x3D0` is the slice record's own forward `+0x1C`, copied as it stands by the
/// tracker refresh 0x499520 whenever the car's patch changes (0x4996A3…0x49971E: `+0x3B8` ← slice
/// `+0x28`, `+0x3C4` ← `+0x10`, `+0x3D0` ← `+0x1C` of `slices[car+8]`, read 2026-09-26); only
/// `Car_placeAtSlice` orthogonalises it, and the 0x49AF70 it ends with refreshes it again. So the
/// dot is with the raw `x` / `z` of the slice's forward, **not** renormalised in x–z (0x40E8E9…
/// 0x40E913: `f.x·dx + f.z·dz`). *(Until 2026-09-26 the port normalised it in x–z, an
/// APPROXIMATION that read `+0x3D0` as orthogonalised.)* Positions `None`: the coarse gap alone.
pub fn gap_fine(table: &SliceTable, a_slice: usize, b_slice: usize, pos: Option<(glam::Vec3, glam::Vec3)>) -> f32 {
    let d = gap_between(table, a_slice, b_slice);
    match pos {
        Some((pa, pb)) if (-12.0..=12.0).contains(&d) => {
            let f = table.slices[a_slice].forward;
            // 0x532330(1, a+0x330, b+0x330): the offset `a − b`; then `f.x·dx + f.z·dz`
            f.x * (pa.x - pb.x) + f.z * (pa.z - pb.z)
        }
        _ => d,
    }
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
    /// `car+0x52C & 4` — the **human**. `Car_register` sets this bit only on the human path
    /// (0x40F270).
    pub is_human: bool,
    /// `car+0x52C & 1` — **any racer**, the human or an opponent. `Car_register` loads `ecx = 1`
    /// once at 0x40F23F and ORs it in on both paths, 0x40F291 for the human and 0x40F32E for an
    /// opponent, so bit 0 is membership of the racer list `[0x5E8E2C]` and bit 2 / bit 3 say
    /// which kind. Several notes called bit 0 "human"; corrected 2026-09-20.
    pub is_racer: bool,
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
    ///
    /// `cars` is every other car in the world, **in registration order** (0x40F189: the human, the
    /// opponents, the cops, the traffic) — the order ties keep in the race-order array. The walk is
    /// 0x401FA2…0x401FD3's: `0x5E8B88[(car+0xEB0·i + n + car+0x528) mod n]` for `i = 0, 1, …` while
    /// fewer than three lanes are filled (0x4021E9), i.e. outward from the car's own place in the
    /// array, in its travel direction, wrapping round. `0x5E8B88` is sorted by 0x417710 on
    /// `+0xAF8 − +0xB20`, stable; for a traffic car or a cop `+0xAF8` is the bare lap fraction of
    /// its slice (0x4169B0's non-racer branch, 0x416A21…0x416AA1) and `+0xB20` is 0, for a racer it
    /// is `frac − unlaps` — so the key here is the slice's place on the lap. In the race the walk
    /// takes the real array: [`Self::find_blockers_placed`] with every car's `car+0x528` from
    /// [`crate::race_order::RaceOrder`], sorted at the end of the previous even tick with the
    /// original's stable bubble sort. Without places (this entry point, the unit tests) it sorts the
    /// cars itself on the slices they have now — APPROXIMATIONS there: a racer that is unlapping or
    /// has finished (`+0xB20` ahead of the lap, `traffic.md` §17.5c) is keyed on its slice all the
    /// same, and ties keep the registration order where the original keeps the previous sort's.
    pub fn find_blockers(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar], pers: &Personality) {
        self.find_blockers_at(table, me, cars, pers, None)
    }

    /// [`Self::find_blockers`] with the world positions `0x40E860`'s fine step needs: `positions` is
    /// `(this car's, one per entry of cars)`. Within ±12 m the gap is the x–z projection of the offset
    /// on the other car's forward axis ([`gap_fine`]), so a car on the same slice or the next one —
    /// which the slice gap puts at 0 or ±6 m — is seen at its real distance (audit 2026-09-26,
    /// `traffic.md` §34.6).
    pub fn find_blockers_at(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar], pers: &Personality, positions: Option<(glam::Vec3, &[glam::Vec3])>) {
        self.find_blockers_placed(table, me, cars, pers, positions, None)
    }

    /// [`Self::find_blockers_at`] walking the real `0x5E8B88[]`: `places` is `(this car's
    /// car+0x528, one per entry of cars)` — the places [`crate::race_order::RaceOrder`] gave every
    /// car at the end of the previous even tick (0x417800 → 0x417710), which is what 0x401FA2's
    /// `0x5E8B88[(car+0xEB0·i + n + car+0x528) mod n]` reads. Without them the walk falls back to
    /// a fresh sort on the slices of this tick (the APPROXIMATION described above).
    pub fn find_blockers_placed(&mut self, table: &SliceTable, me: &LaneSelf, cars: &[LaneCar], pers: &Personality, positions: Option<(glam::Vec3, &[glam::Vec3])>, places: Option<(usize, &[usize])>) {
        self.hard = [me.target_speed; 3];
        self.soft = [me.target_speed; 3];
        self.blocker = [None; 3];
        self.gap = [0.0; 3];
        let masks = [lane_bit(me.lane - 1), lane_bit(me.lane), lane_bit(me.lane + 1)];
        let lanes = [me.lane - 1, me.lane, me.lane + 1];
        // Read 2026-09-27 (`traffic.md` §34.11): **the real
        // walk already carries the original's within-slice history and this branch is not it.**
        // `places` — [`crate::race_order::RaceOrder`], the previous even tick's `0x5E8B88` as
        // `0x417710`'s stable bubble sort (equal keys keep the order the last change of lead left
        // them in) — is what `0x401FA2` reads, and the `Some(places)` arm below sorts on it alone;
        // `positions` never enters that arm's order (only `gap_fine`'s distance does). So whenever
        // `places` is supplied — every tick once the race has run its first even-tick resort, both
        // `traffic.rs` and `police.rs`'s real callers — the walk already is `0x5E8B88`'s history,
        // swaps and all; a unit test pins it
        // (`with_places_the_within_slice_order_is_the_race_orders_history_not_the_current_position`).
        //
        // What follows (the race-order array *without* this car, sorted stably by the lap place of
        // the slice; this car sits before the others on its own slice, which the slice gap alone
        // skips: gap 0) is the `None`-`places` **fallback** only — used before that first resort
        // and by the unit tests below, not by a running race. There, with positions, the cars of
        // one slice are ordered by how far along it they are: their keys are equal, and the stable
        // sort keeps the order they came in — a car further along its slice entered it earlier,
        // travelling either way, and a car entering from below bubbles up only to the bottom of the
        // equal run, one from above down only to its top. This fallback does not carry that
        // history: two cars that pass each other inside a slice swap its own order here, where the
        // original's `0x5E8B88` would not — a real, narrow gap, but not one the running race meets.
        let along = |slice: usize, p: glam::Vec3| {
            let sl = &table.slices[slice];
            (p - sl.centre).dot(sl.forward.normalize_or_zero())
        };
        let key = |slice: usize, p: Option<glam::Vec3>| (table.route_pos.get(slice).copied().unwrap_or(0), p.map_or(f32::NEG_INFINITY, |p| along(slice, p)));
        let pos_of = |i: usize| positions.and_then(|(_, ps)| ps.get(i).copied());
        let mut sorted: Vec<usize> = (0..cars.len()).collect();
        let own = match places.filter(|(_, p)| p.len() == cars.len()) {
            // the array itself: the others by their `car+0x528`, and this car's own place among them
            Some((mine, p)) => {
                sorted.sort_by_key(|&i| p[i]);
                sorted.partition_point(|&i| p[i] < mine)
            }
            None => {
                sorted.sort_by(|&a, &b| key(cars[a].slice, pos_of(a)).partial_cmp(&key(cars[b].slice, pos_of(b))).unwrap_or(std::cmp::Ordering::Equal));
                let mine = key(me.slice, positions.map(|(p, _)| p));
                sorted.partition_point(|&i| key(cars[i].slice, pos_of(i)) < mine)
            }
        };
        let m = sorted.len();
        let walk = (0..m).map(|k| if me.dir > 0.0 { sorted[(own + k) % m] } else { sorted[(own + m - 1 - k) % m] });
        let mut used = 0u32;
        let mut found = 0;
        for i in walk {
            if found == 3 {
                break;
            }
            let o = &cars[i];
            // `g = 0x40E860(o, car)` (0x401FED…0x401FEF: `push esi` the car, `push edi` the other)
            // and `gd = car+0xEB0 · g`
            let pos = positions.and_then(|(mine, ps)| ps.get(i).map(|&p| (p, mine)));
            let g = gap_fine(table, o.slice, me.slice, pos);
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
            // 0x402095…0x4020EB: the gap `fistp`-rounded, less the 2 m margin, towards zero never
            // past it; a cop or an opponent (`52C & 0x28`) keeps at least 1 m going forwards and
            // takes −1 going backwards once the sum passes 1 — neither can happen once the gap has
            // cleared `car+0x3B0`, as the checks above require
            let gi = g.round_ties_even() as i32;
            let t = if me.dir == 1.0 {
                let mut t = gi - 2;
                if !me.is_traffic && t < 1 {
                    t = 1;
                }
                t.max(0)
            } else {
                let mut t = gi + 2;
                if !me.is_traffic && t > 1 {
                    t = -1;
                }
                t.min(0)
            };
            let (hard, soft) = (o.speed + a * t as f32, o.speed + b * t as f32);
            // the own lane first (0x4020F3…0x402145), then lane−1 (…0x40218F), then lane+1 (…0x4021D9);
            // the own lane has no range test, its neighbours need `0 <= lane < 14`
            for k in [1, 0, 2] {
                if o.lane_mask & masks[k] != 0 && used & masks[k] == 0 && (k == 1 || (0..14).contains(&lanes[k])) {
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
            // the scores are ints: every term is `fld e; fmul w; fiadd score; call _ftol`
            // (0x402302…0x402313 and its seven siblings), i.e. added and **truncated towards zero**
            // at each step (audit 2026-09-26, `traffic.md` §34.12) — the port kept the fractions
            let mut add = |e: f32, w: f32| {
                if e >= 0.0 {
                    self.score[k] = (self.score[k] + e * w).trunc();
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
            // **0x401C8D tests `& 1`**, so an opponent bearing down on a traffic car makes it
            // pull over exactly as the player does. This used to ask for the human.
            if let Some(i) = self.blocker[1]
                && cars[i].is_racer
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
    /// the road. (`0x402790`, the other early return, is dead for traffic — no traffic path writes its
    /// `car+0x107C` bias — and the cop's lane pass runs it first, [`LaneScratch::override_holds`].)
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

    /// `0x402790(car)` — read to the instruction 2026-09-17 (§28.3) and again 2026-09-26: does the
    /// held offset `car+0x1078` (`offset`) still win the lane vote with its bias `car+0x107C`
    /// (`bias`) added? For each edge of the body widened by 0.25 m — `offset ∓ (car+0x3A8 + 0.25)` —
    /// the lane it falls in (`0x40EB20`) maps to a slot (the current lane → 1, above → 2, below →
    /// 0), a fresh copy of the three scores gets the bias on that slot, and the slot must be the
    /// argmax (first strict `>` wins, ties to the lower slot). Both edges must win; the caller then
    /// takes `car+0xEB4 = car+0x1078` unclamped. Only a chasing cop ever has a positive bias
    /// (`traffic.md` §28.3, corrected 2026-09-26).
    pub fn override_holds(&self, table: &SliceTable, me: &LaneSelf, offset: f32, bias: f32) -> bool {
        let sl = &table.slices[me.slice.min(table.slices.len() - 1)];
        let slot_of = |off: f32| {
            let l = lane_of(sl, off);
            if l == me.lane {
                1
            } else if l >= me.lane {
                2
            } else {
                0
            }
        };
        let wins = |slot: usize| {
            let mut s = self.score;
            s[slot] += bias;
            let mut b = 0;
            if s[1] > s[0] {
                b = 1;
            }
            if s[2] > s[b] {
                b = 2;
            }
            b == slot
        };
        wins(slot_of(offset - me.half_width - 0.25)) && wins(slot_of(me.half_width + offset + 0.25))
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

    /// The lane index is `base + round(offset × table[round(width)])` where the table is `1/i`
    /// (0x40EB6F…0x40EB88). Both roundings are bare `fistp`, i.e. to nearest — and the width one
    /// matters, because a third of the shipped slice sides are not whole metres.
    #[test]
    fn a_lane_width_is_rounded_before_its_reciprocal_is_taken() {
        let at = |w: f32, off: f32| {
            let mut t = ring(200, 2, 2);
            for sl in &mut t.slices {
                sl.lane_width_a = w;
                sl.lane_width_b = w;
            }
            lane_of(&t.slices[0], off)
        };
        // 7.5 rounds to 8 (ties to even), so the 7→8 step is at 4 m, not 3.75
        assert_eq!(at(7.5, 3.9), 7);
        assert_eq!(at(7.5, 4.1), 8);
        // 4.5 rounds **down** to 4, again ties-to-even — the step is at 2 m
        assert_eq!(at(4.5, 1.9), 7);
        assert_eq!(at(4.5, 2.1), 8);
        // 3.75 rounds to 4, so it behaves exactly as 4.5 does
        assert_eq!(at(3.75, 1.9), 7);
        assert_eq!(at(3.75, 2.1), 8);
        // 5.5 rounds up to 6 — the step is at 3 m
        assert_eq!(at(5.5, 2.9), 7);
        assert_eq!(at(5.5, 3.1), 8);
        // a width under half a metre rounds to 0 and reads the table's leading 0.0: everything
        // lands in the base lane
        assert_eq!(at(0.4, 50.0), 7);
        assert_eq!(at(0.4, -50.0), 6);
        // and the clamp still holds at both ends
        assert_eq!(at(1.0, 100.0), 11);
        assert_eq!(at(1.0, -100.0), 2);
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
        // A car spans the lanes its body covers — and the boundaries are where **rounding** puts
        // them, not truncation: `lane = base + round(offset/w)`, so with a 4 m lane the step from
        // 7 to 8 is at 2 m, not 4 m. Lanes 6 and 7 are the two halves of the centre lane and
        // everything outside them is a full width. A car at 2.0 with 0.9 of half-width has edges
        // at 1.6 and 2.4, which round to 0 and 1 — it straddles.
        assert_eq!(lane_span(sl, 1.0, 0.9), lane_bit(7), "a car well inside one lane spans only that lane");
        assert_eq!(lane_span(sl, 2.0, 0.9), lane_bit(7) | lane_bit(8), "straddling the 7/8 line spans both");
        assert_eq!(lane_span(sl, 4.0, 0.9), lane_bit(8), "a car well inside lane 8");
    }

    #[test]
    fn the_blockers_are_found_by_walking_the_race_order_in_the_travel_direction() {
        // 0x401F20 walks `0x5E8B88` from the car's own place in its travel direction; two cars on
        // the same slice keep their registration order there, so going forwards the earlier one is
        // met first and going backwards the later one
        let t = ring(400, 2, 2);
        let car = |slice: usize, speed: f32| LaneCar { slice, lane: 8, lane_mask: lane_bit(8), speed, speed_est: speed, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: false, is_racer: false };
        let mut s = LaneScratch::default();
        let fwd = me(8, 6.0);
        s.find_blockers(&t, &fwd, &[car(15, 4.0), car(15, 5.0)], &PERSONALITIES[0]);
        assert_eq!(s.blocker[1], Some(0));
        let back = LaneSelf { dir: -1.0, speed: -16.5, target_speed: -16.5, ..fwd };
        s.find_blockers(&t, &back, &[car(5, -4.0), car(5, -5.0)], &PERSONALITIES[0]);
        assert_eq!(s.blocker[1], Some(1));
        // across the lap line: at slice 398 going forwards, a car at slice 2 is 24 m ahead and
        // one at slice 390 is behind; the walk wraps from the top of the array to its bottom
        let late = LaneSelf { slice: 398, ..fwd };
        s.find_blockers(&t, &late, &[car(390, 4.0), car(2, 5.0), car(6, 6.0)], &PERSONALITIES[0]);
        assert_eq!(s.blocker[1], Some(1));
        assert_eq!(s.gap[1], 24.0);
        // the permitted speed is the blocker's plus `a · (round(gap) − 2)`
        assert!((s.hard[1] - (5.0 + 1.25 * 22.0)).abs() < 1e-4, "{:?}", s.hard);
    }

    #[test]
    fn within_twelve_metres_the_gap_is_the_projection_on_the_other_cars_axis() {
        // 0x40E860's fine step: a car on the same slice 5 m ahead is a blocker 5 m away, where the
        // slice gap (0 m) skipped it as overlapping
        let t = ring(400, 2, 2);
        let fwd = me(8, 6.0);
        let sl = t.slices[10];
        let f = sl.forward.normalize();
        let car = LaneCar { slice: 10, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: false, is_racer: false };
        let mut s = LaneScratch::default();
        s.find_blockers(&t, &fwd, &[car], &PERSONALITIES[0]);
        assert_eq!(s.blocker[1], None, "the slice gap alone: 0 m, overlapping");
        let (mine, other) = (sl.centre, sl.centre + f * 5.0);
        s.find_blockers_at(&t, &fwd, &[car], &PERSONALITIES[0], Some((mine, &[other])));
        assert_eq!(s.blocker[1], Some(0));
        assert!((s.gap[1] - 5.0).abs() < 1e-3, "{}", s.gap[1]);
        // travelling −1 the car ahead on its own slice sits below it in the race order and is met
        // first, before a car further down the road in the same lane
        let back = LaneSelf { dir: -1.0, speed: -16.5, target_speed: -16.5, lane: 5, lateral: -6.0, ..fwd };
        let near = LaneCar { lane: 5, lane_mask: lane_bit(5), lateral: -6.0, speed: -4.0, ..car };
        let far = LaneCar { slice: 8, speed: -2.0, ..near };
        let ps = [t.slices[8].centre, sl.centre - f * 5.0];
        s.find_blockers_at(&t, &back, &[far, near], &PERSONALITIES[0], Some((sl.centre, &ps)));
        assert_eq!(s.blocker[1], Some(1), "the nearer car, met first");
        assert!((s.gap[1] - 5.0).abs() < 1e-3, "{}", s.gap[1]);
        // beyond ±12 m the slice gap stands, whatever the positions say
        assert_eq!(gap_fine(&t, 13, 10, Some((other, mine))), 18.0);
        // slice 12's axis is turned 1.8° from slice 10's on this ring
        assert!((gap_fine(&t, 12, 10, Some((sl.centre + f * 11.0, mine))) - 11.0).abs() < 1e-2);
    }

    /// The real walk (0x401FA2) reads `car+0x528`, the previous even tick's `0x417710` sort —
    /// [`crate::race_order::RaceOrder`], which keeps each car's history (a stable sort: equal keys
    /// keep the order the last change of lead left them in, `race_order.rs`). Two cars sharing a
    /// slice are met in **that** order, not by their position along the slice right now: the
    /// `along`-projection tie-break at `find_blockers_placed`'s top (the doc above this function)
    /// is the `None`-`places` fallback only — used before the first even-tick resort (or the unit
    /// tests), never once `places` is supplied. So a pass inside a slice does not swap the walk
    /// order when the real array is given, matching the original; `traffic.md` §34.11's remaining
    /// APPROXIMATION note is for that fallback alone.
    #[test]
    fn with_places_the_within_slice_order_is_the_race_orders_history_not_the_current_position() {
        let t = ring(400, 2, 2);
        let fwd = me(8, 6.0);
        let sl = t.slices[15];
        let f = sl.forward.normalize();
        let a = LaneCar { slice: 15, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: false, is_racer: false };
        let b = LaneCar { slice: 15, ..a };
        // by position alone `a` sits closer to the slice's start than `b`: walking forward, the
        // sorted-by-`along` fallback (no places) meets `a` first
        let ps = [sl.centre + f * 1.0, sl.centre + f * 4.0];
        let mut s = LaneScratch::default();
        s.find_blockers_at(&t, &fwd, &[a, b], &PERSONALITIES[0], Some((t.slices[8].centre, &ps)));
        assert_eq!(s.blocker[1], Some(0), "without places: position order, a (closer along) met first");
        // with places giving `b` the earlier place — the race order's history — it is met first
        // instead, although `a` is still the one closer along by position this tick
        s.find_blockers_placed(&t, &fwd, &[a, b], &PERSONALITIES[0], Some((t.slices[8].centre, &ps)), Some((0, &[2, 1])));
        assert_eq!(s.blocker[1], Some(1), "with places: the race order's history wins over position");
    }

    #[test]
    fn the_blocker_terms_are_truncated_to_ints_at_each_step() {
        // 0x402302…0x402313: `score = _ftol(score + e·w)`. A blocker 1.9 m/s under the car's speed
        // and target: −1.3·1.9 = −2.47 → −2, then −0.5·1.9 → −2.95 → −2 ... (each truncated)
        let mut s = LaneScratch { blocker: [None, Some(0), None], gap: [0.0, 50.0, 0.0], hard: [0.0, 14.6, 0.0], soft: [0.0, 14.6, 0.0], ..Default::default() };
        let m = LaneSelf { speed: 16.5, target_speed: 16.5, ..me(8, 6.0) };
        s.score_blockers(&m);
        // e = 1.9 for all four terms: −2.47 → −2; −2 − 0.95 → −2; −2 − 0.95 → −2; −2 − 0.19 → −2
        assert_eq!(s.score[1], -2.0, "{:?}", s.score);
    }

    #[test]
    fn a_slow_blocker_makes_the_car_change_lane() {
        let t = ring(400, 2, 2);
        let m = me(8, 6.0);
        // a slow car two slices (12 m) ahead in lane 8, nothing in the lanes either side. Farther out
        // the cap is not restrictive at all: the permitted speed grows by 1.25 m/s per metre of gap.
        let cars = [LaneCar { slice: 12, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: false, is_racer: false }];
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
        let cars = [LaneCar { slice: 12, lane: 8, lane_mask: lane_bit(8), speed: 40.0, speed_est: 40.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: true, is_racer: true }];
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
        let o = LaneCar { slice: 10, lane: 8, lane_mask: lane_bit(8), speed: 4.0, speed_est: 4.0, half_len: 2.1, lateral: 5.0, half_span: 0.9, is_human: false, is_racer: false };
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
