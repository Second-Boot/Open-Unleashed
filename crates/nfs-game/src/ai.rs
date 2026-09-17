//! Opponent driver reconstructed from `AIState_Racing` (`RE/notes/ai.md` §3, CONFIRMED unless noted):
//! target speed from the summed heading deviation over a speed-dependent lookahead (`.ais` tables),
//! throttle/brake from the rpm error, steering toward the mean of four racing-line points ahead.
//!
//! The junction branch choice of `race-logic.md` §10.4 is in: once per junction the lookahead reaches,
//! the car draws from the original's single global stream and picks a link, which every road preview
//! then follows. Not yet reproduced: the per-track steer factor 0x4A2950 beyond its default (0.85),
//! skill randomisation (fe.txt has all skills 1.0), damage/wet factors, and the tracker's
//! `preferredSection` hop (the port's `find_slice` still hill-climbs). Avoidance (§3.2) follows the
//! notes for the two cars directly ahead in race order.

use crate::race::{RaceProgress, SliceTable};
use glam::Vec3;
use nfs_formats::ais::{AiCarData, RacingLine};
use nfs_physics::specs::{CarSpecs, GRIP_DRY};
use nfs_physics::vehicle::{Controls, Vehicle};

pub struct AiDriver {
    pub data: AiCarData,
    /// carSpecs[0x13C]/[0x140]/[0x144] (raw `.sim` +0x13C..+0x144, i.e. `unknown_tail[19..22]`)
    pub k_lookahead: f32,
    pub k_steer: f32,
    pub k_angle: f32,
    pub stuck_ticks: u32,
    /// `car+0xD8B = 5` from the facing test: the car is pointing back down the route at speed.
    pub wrong_way: bool,
    pub target_speed: f32,
    /// `.SKILL{SLOW,MEDIUM,FAST,DEFAULT}` means and `.SKILLRANGE*` half-ranges (§3.1); quick race
    /// uses 1.0 / 0 (fe.txt), career events `difficulty × per.mean[k]` / `per.range[k]` (0x4EBDA0).
    pub skill_mean: [f32; 4],
    pub skill_range: [f32; 4],
    /// Current bend class (0 slow, 1 medium, 2 fast) and the personal speed factor drawn for it.
    pub band: Option<u8>,
    /// `SKILL [0x6573F4]` 0..2 — the difficulty the catch-up band is keyed on
    pub skill: u8,
    /// `car+0xE94` — the junction node a branch was last chosen at, so it is chosen only once
    last_node: Option<usize>,
    /// `car+0xE98` — the chosen link inside that node, passed to every road lookahead
    pub link: Option<usize>,
    /// `car+0x0C` — the tracker's preferred segment, the branch's segment index
    pub preferred_segment: Option<usize>,
    /// the human's progress in laps and average pace, for the catch-up band (§5.2 rule 1)
    catchup: Option<(f32, f32)>,
    pub speed_factor: f32,
    /// LCG state (`0x5D1028 *= 0x5D102C`, §3.1 random draws).
    pub rng: u32,
    /// Road-width normaliser `[0x5E6E34]`: 12 by default, 10 on castle, 9 on forest (§10.4).
    pub wf_div: f32,
    /// Track name and direction for the per-track speed zones (`crate::zones`, 0x4A2DC0).
    pub track: String,
    pub backwards: bool,
    /// Debug: slices ahead of the last steering target and its lateral offset from the centreline.
    pub dbg_n: i32,
    pub dbg_offset: f32,
    pub dbg_dot: f32,
}

/// Default steering factor 0x4A2950 (0x5B25E0 = 0.85) and curve factor 0x4A2DC0 (1.0).
const STEER_FACTOR: f32 = 0.85;
const CURVE_FACTOR: f32 = 1.0;

impl AiDriver {
    pub fn new(data: AiCarData, specs: &CarSpecs, sim_tail: &[f32; 22]) -> Self {
        Self { data, k_lookahead: sim_tail[19], k_steer: sim_tail[20] * STEER_FACTOR, k_angle: sim_tail[21], stuck_ticks: 0, wrong_way: false, target_speed: 0.0, skill_mean: [1.0; 4], skill_range: [0.0; 4], band: None, skill: 0, catchup: None, last_node: None, link: None, preferred_segment: None, speed_factor: 1.0, rng: 0x1234_5679, wf_div: 12.0, track: String::new(), backwards: false, dbg_n: 0, dbg_offset: 0.0, dbg_dot: 0.0 }
            .tap(|d| {
                // curve factor applies to the speed curve (§2.1); default 1.0
                for v in d.data.bend_speed.iter_mut() {
                    *v *= CURVE_FACTOR;
                }
                let _ = specs;
            })
    }

    /// Per-track road-width normaliser and speed zones (0x4A2DC0).
    pub fn set_track(&mut self, track: &str, backwards: bool) {
        self.wf_div = crate::zones::width_divisor(track);
        self.track = track.to_ascii_lowercase();
        self.backwards = backwards;
    }

    /// Speed estimate `car+0x35C = max(|vx|,|vz|) + 0.25·min(|vx|,|vz|)` used by the AI.
    fn speed_est(v: &Vehicle) -> f32 {
        let (a, b) = (v.v_local.x.abs(), v.v_local.z.abs());
        a.max(b) + 0.25 * a.min(b)
    }

    /// Career opponents: `SKILL[k] = difficulty × mean[k]`, `RANGE[k] = range[k]` (0x4EBDA0, CONFIRMED).
    pub fn set_skill(&mut self, difficulty: f32, pairs: [[f32; 2]; 4]) {
        for k in 0..4 {
            self.skill_mean[k] = difficulty * pairs[k][0];
            self.skill_range[k] = pairs[k][1];
        }
        self.band = None;
    }

    /// Uniform draw in 0..1 (multiplicative LCG as in the original, low bits dropped).
    fn rand01(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(0x0001_9660).wrapping_add(0x3C6E_F35F);
        ((self.rng >> 8) & 0xFFFF) as f32 * 1.525_878_9e-5
    }

    /// Personal speed factor `car+0xEA0`: redrawn whenever the bend class changes (§3.1).
    /// Catch-up rubber band (`ai.md` §3.1 tail / §5.2 rule 1, CONFIRMED). `player_progress` is the
    /// human's progress in laps (`car+0xAF8`), `pace` the `(progress − start)·640000/tick` term.
    /// At `SKILL` 0 the multiplier is 1.0, so the band does nothing — which is the shipped setting.
    pub fn set_catchup(&mut self, skill: u8, player_progress: f32, pace: f32) {
        self.skill = skill.min(2);
        self.catchup = Some((player_progress, pace));
    }

    /// `0x5CC384` — how close in laps the opponent must be for the band to apply.
    const LAP_BAND: [f32; 3] = [1.0, 0.07, 0.03];
    /// `0x5CC378` — the target-speed multiplier once it does.
    const BAND_MULT: [f32; 3] = [1.0, 0.98, 0.95];

    fn draw_speed_factor(&mut self, band: u8) {
        if self.band == Some(band) {
            return;
        }
        let r01 = self.rand01();
        let u = |d: &mut Self| d.rand01() * 2.0 - 1.0;
        let base = self.skill_mean[3] + self.skill_range[3] * u(self);
        let b = band as usize;
        self.speed_factor = if r01 <= self.skill_mean[b] { self.skill_mean[b] + self.skill_range[b] * u(self) } else { base };
        self.band = Some(band);
    }

    /// `0x5CC390..0x5CC3A4` — the fixed fork thresholds for the two easier difficulties.
    const FORK_T2: [f32; 2] = [0.55, 0.70];
    const FORK_A: [f32; 2] = [0.40, 0.50];
    const FORK_B: [f32; 2] = [0.70, 0.80];

    /// The branch choice of `race-logic.md` §10.4, run once per junction the lookahead reaches.
    ///
    /// On the hardest difficulty the `.jnc` weights *are* the probabilities; on the two easier ones a
    /// fork is re-weighted by fixed thresholds against the weight buckets 0.5 / 0.4 / 0.3, which the
    /// shipped data is authored around (every two-way fork is 0.85/0.15 and every three-way one
    /// 0.6/0.3/0.1). Choosing sets the link every road lookahead then follows and the segment the
    /// tracker prefers.
    fn choose_branch(&mut self, table: &SliceTable, slice: usize, lookahead: i32) {
        let Some(node) = table.junction_within(slice, lookahead) else { return };
        if self.last_node == Some(node) {
            return;
        }
        self.last_node = Some(node);
        // the branch draw comes from the original's single global stream, not the car's own
        let r = crate::rng::rand01();
        let want = table.graph_want();
        let links = &table.junctions.nodes[node].links;
        let mode = self.skill.min(2) as usize;
        let matches = links.iter().filter(|l| l.dir() == want).count();
        let mut cum = 0.0;
        for (i, l) in links.iter().enumerate() {
            if l.dir() != want {
                continue;
            }
            let w = table.junctions.segments[l.segment].weight[l.end];
            let take = if mode >= 2 {
                cum += w;
                r < cum
            } else {
                match matches {
                    2 => (r <= Self::FORK_T2[mode]) == (w >= 0.5),
                    3 => {
                        if r <= Self::FORK_A[mode] {
                            w >= 0.4
                        } else if r <= Self::FORK_B[mode] {
                            (0.3..0.4).contains(&w)
                        } else {
                            w < 0.3
                        }
                    }
                    // one match, or a fork shape the thresholds do not cover: take the first
                    _ => true,
                }
            };
            if take {
                tracing::debug!(node, link = i, segment = l.segment, weight = w, r, matches, mode, "AI takes a branch");
                self.link = Some(i);
                self.preferred_segment = Some(l.segment);
                return;
            }
        }
        // nothing matched: the previous branch stands
    }

    /// Target speed 0x40B650 (m/s).
    pub fn target_speed(&mut self, table: &SliceTable, race: &RaceProgress, v: &Vehicle, specs: &CarSpecs) -> f32 {
        let s0 = race.slice;
        let sl0 = &table.slices[s0];
        let fwd0 = sl0.forward;
        let wf = ((sl0.width_a + sl0.width_b) / self.wf_div).min(1.0);
        let speed = Self::speed_est(v);
        // round-to-nearest (fistp), lookahead in slices, heading loop inclusive (§10.5)
        let idx = ((speed * 0.2 * self.k_lookahead).min(23.0).round() as usize).min(23);
        let lookahead = self.data.lookahead[idx].max(0.0).round() as usize;
        let (mut max_l, mut max_r) = (0f32, 0f32);
        // §10.4: pick a branch once per junction, then let every road preview follow it
        self.choose_branch(table, s0, lookahead as i32);
        let link = self.link;
        for k in 2..=lookahead.max(2) {
            let s = table.add_graph(s0, k as i32, link);
            let sl = &table.slices[s];
            let a = sl.forward.dot(fwd0).clamp(-1.0, 1.0).acos().to_degrees();
            if sl.lateral.dot(fwd0) < 0.0 {
                max_r = max_r.max(a);
            } else {
                max_l = max_l.max(a);
            }
        }
        // per-track zone multiplier on the bend angle (0x4A2DC0 zones, ai.md §2.3)
        let zone = crate::zones::zone_mult(&self.track, self.backwards, s0);
        let ang = ((max_l + max_r) * self.k_angle * zone).min(115.0);
        let c3 = (ang / 3.0) as usize;
        let mut vt = self.data.bend_speed[c3.min(39)];
        // surface factor from the car's surface (`car+0x464`): dry table row × class column, ≤ 1
        // (off the tarmac the target drops to 0.5–0.85 of the curve — §3.1)
        let surface = v.wheels[0].surface & 0xF;
        vt *= GRIP_DRY[surface][specs.class].min(1.0);
        // personal speed factor per bend class (slow > 20, medium 10..20, fast ≤ 10)
        let band = if c3 > 20 { 0 } else if c3 > 10 { 1 } else { 2 };
        self.draw_speed_factor(band);
        let mut vt = vt * self.speed_factor * wf;
        // catch-up: a slow human close by holds the field back a little
        if let Some((player_progress, pace)) = self.catchup {
            let own = race.progress;
            let k = self.skill as usize;
            if (own - player_progress).abs() < Self::LAP_BAND[k] && pace > 0.0 && pace < 35.0 {
                vt *= Self::BAND_MULT[k];
            }
        }
        vt
    }

    /// Steering / throttle / brake 0x40BFE0. `ahead` = up to two cars directly ahead in race order:
    /// (position, slice, v_long, half width, is_human).
    pub fn controls(&mut self, table: &SliceTable, line: Option<&RacingLine>, race: &RaceProgress, v: &Vehicle, specs: &CarSpecs, tick: u64, ahead: &[(Vec3, usize, f32, f32, bool)], half_width: f32) -> Controls {
        let vt = self.target_speed(table, race, v, specs);
        self.target_speed = vt;
        let g = v.gear.max(2).min(specs.gear_slots - 1);
        let ratio = specs.ratio_rpm_per_mps[g];
        let err = ratio * (vt - v.v_long());
        let mut throttle = ((err / specs.redline) * 256.0).clamp(0.0, 254.0) as u8;
        let mut brake = 0u8;
        if err < 0.0 {
            brake = ((2.0 * err.abs() / specs.redline) * 256.0).clamp(0.0, 254.0) as u8;
            if brake > 0 {
                throttle = 0;
            }
        }
        // steering target: mean of four racing-line points N..N+3 slices ahead
        let s0 = race.slice;
        let sl0 = &table.slices[s0];
        let wf = ((sl0.width_a + sl0.width_b) / self.wf_div).min(1.0);
        let mut n = (self.k_steer * v.v_long()).abs() * 4.0 / 21.0;
        n = if wf < 1.0 { (n * wf).max(2.0) } else { n.max(4.0) };
        let speed = Self::speed_est(v);
        if speed > 16.0 && tick > 0x380 {
            n *= speed * 0.0625;
        }
        let n = n as i32;
        let point = |s: usize| -> Vec3 {
            let sl = &table.slices[s];
            match line.and_then(|l| l.points.get(s)) {
                Some(p) => Vec3::new(p[0], sl.centre.y + 1.0, p[1]),
                None => sl.centre + Vec3::Y,
            }
        };
        let mut target = Vec3::ZERO;
        for i in 0..4 {
            target += point(table.add_graph(s0, n + i, self.link));
        }
        target *= 0.25;
        // avoidance (§10.2, CONFIRMED): racing cars stand off `t` metres from the centre of a car
        // beside or shortly ahead; the car two ahead is processed first, the car directly ahead last
        let no_avoid = std::env::var_os("NFS_AI_NOAVOID").is_some();
        for &(opos, _oslice, ovlong, ohalf, _human) in ahead.iter().take(if no_avoid { 0 } else { 2 }).rev() {
            let dir = (target - v.position).normalize_or_zero();
            let d1 = opos - v.position;
            let along = d1.dot(dir);
            if along > -1.0 && (along < 11.0 || (along < 20.0 && v.v_long() - 3.0 > ovlong)) {
                let sep = dir * along - d1;
                let t = (along * 0.05 + 1.15).min(2.0) * (ohalf + half_width);
                if sep.length() < t {
                    target = opos + sep.normalize_or_zero() * t;
                }
            }
        }
        let dir = (target - v.position).normalize_or_zero();
        let right = v.orientation.row(0);
        let steer = (dir.dot(right) * 256.0).round().clamp(-126.0, 126.0) as i32;
        self.dbg_n = n;
        self.dbg_dot = dir.dot(right);
        {
            let ts = table.find_slice(target, s0);
            let sl = &table.slices[ts];
            self.dbg_offset = (target - sl.centre).dot(sl.lateral);
        }
        let steer = if std::env::var_os("NFS_AI_FLIP").is_some() { -steer } else { steer };
        // `AIState_Racing` 0x40CA20's reset triggers (`ai.md` §3.2): five seconds below 2 m/s after
        // the start, or facing the wrong way at more than 10 m/s — `dot(slice forward, car
        // forward) < −0.707`, the sign flipped when the race runs backwards.
        if v.v_long() < 2.0 && v.race_started {
            self.stuck_ticks += 1;
        } else {
            self.stuck_ticks = 0;
        }
        let facing = table.slices[s0].forward.normalize_or_zero().dot(v.forward());
        self.wrong_way = v.race_started && v.speed() > 10.0 && if self.backwards { facing > 0.707 } else { facing < -0.707 };
        Controls { throttle, brake, steer, handbrake: false, gear_request: None }
    }

    pub fn wants_reset(&mut self) -> bool {
        if self.stuck_ticks >= 0xA0 || self.wrong_way {
            self.stuck_ticks = 0;
            self.wrong_way = false;
            true
        } else {
            false
        }
    }
}

trait Tap: Sized {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}
impl<T> Tap for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catchup_band_only_bites_above_skill_0() {
        // the constants of 0x5CC378 / 0x5CC384: no effect at beginner, 2 % at medium, 5 % at hard
        assert_eq!(AiDriver::BAND_MULT[0], 1.0);
        assert_eq!(AiDriver::LAP_BAND[0], 1.0);
        assert!((AiDriver::BAND_MULT[1] - 0.98).abs() < 1e-6);
        assert!((AiDriver::BAND_MULT[2] - 0.95).abs() < 1e-6);
        // and the window tightens as the difficulty rises
        assert!(AiDriver::LAP_BAND[2] < AiDriver::LAP_BAND[1], "the window tightens as the difficulty rises");
    }
}
