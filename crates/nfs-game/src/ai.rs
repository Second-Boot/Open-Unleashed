//! Opponent driver reconstructed from `AIState_Racing` (`RE/notes/ai.md` §3, CONFIRMED unless noted):
//! target speed from the summed heading deviation over a speed-dependent lookahead (`.ais` tables),
//! throttle/brake from the rpm error, steering toward the mean of four racing-line points ahead.
//!
//! The junction branch choice of `race-logic.md` §10.4 is in: once per junction the lookahead reaches,
//! the car draws from the original's single global stream and picks a link, which every road preview
//! then follows. Not yet reproduced:
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
    /// The `.ais` bend-speed curve as it was parsed, before `0x4A2DC0`'s per-track × per-tier
    /// factor is applied to it — kept so `set_track` can re-scale from the original rather than
    /// compound onto an already-scaled curve.
    bend_speed_base: [f32; 40],
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
    /// `sim` tail 20 before the per-track steering factor is applied
    k_steer_base: f32,
    pub track: String,
    pub backwards: bool,
    /// Debug: slices ahead of the last steering target and its lateral offset from the centreline.
    pub dbg_n: i32,
    pub dbg_offset: f32,
    pub dbg_dot: f32,
}

/// The steering factor `carSpecs[0x140]` is multiplied by (0x49CB16 → 0x4A2950): a table over the
/// **track** and the car's family key (`nfs5.car` +0x115, compared case-insensitively). Everything
/// the table does not name keeps 1.0 — which is most of the grid on most of the tracks; the 0.85
/// the port used to apply everywhere is only one entry of it. CONFIRMED.
///
/// | track | family | factor |
/// |---|---|---|
/// | `autobahn` | `935` `GT1` `GT2` `GT3` | 0.85 |
/// | `castle` | `935` `GT1` `GT2` `GT3` | 0.80 |
/// | `castle` | `993T` `996T` | 0.85 |
/// | `castle` | `993` `996` `boxster` | 0.90 |
/// | `monaco1` **backwards only** | as `autobahn` | 0.85 |
/// | `monaco2` | `935` `GT1` `GT2` `GT3` | 0.85 |
/// | `monaco2` | `928` | 0.925 |
/// | `monaco3` **backwards only** | `930T` `965T` | 1.10 |
pub fn steer_factor(track: &str, family: &str, backwards: bool) -> f32 {
    let is = |a: &str| family.eq_ignore_ascii_case(a);
    let race = is("935") || is("GT1") || is("GT2") || is("GT3");
    let track = track.to_ascii_lowercase();
    match track.as_str() {
        "autobahn" => {
            if race {
                0.85
            } else {
                1.0
            }
        }
        // monaco1 only has a table when it is driven backwards, and then it is the autobahn's
        "monaco1" if backwards => {
            if race {
                0.85
            } else {
                1.0
            }
        }
        "castle" => {
            if race {
                0.80
            } else if is("993T") || is("996T") {
                0.85
            } else if is("993") || is("996") || is("boxster") {
                0.90
            } else {
                1.0
            }
        }
        "monaco2" => {
            if race {
                0.85
            } else if is("928") {
                0.925
            } else {
                1.0
            }
        }
        "monaco3" if backwards => {
            if is("930T") || is("965T") {
                1.10
            } else {
                1.0
            }
        }
        _ => 1.0,
    }
}

/// The car's **class tier**, 1…5 — `0x4A2BC0(family)`, compared against `carInfo+0x2A8`, the
/// `nfs5.car +0x115` family key. Anything the chain does not name falls through to the `edi = 5`
/// the function seeds at 0x4A2BCC.
///
/// The two `alps` rows are the original's: 935 and GT2 drop to tier 1 **on that track only**
/// (0x4A2C1F and 0x4A2C4B each guard their family's tier-1 exit with a second `strcmp`).
fn class_tier(family: &str, track: &str) -> u32 {
    let f = family.to_ascii_uppercase();
    let alps = track.eq_ignore_ascii_case("alps");
    match f.as_str() {
        "356" | "356A" | "356B" => 1,
        "935" if alps => 1,
        "GT2" if alps => 1,
        "550A" | "928" | "930T" | "964" => 2,
        "914" | "944" | "959" | "965T" => 3,
        "993" | "993T" | "996" | "996T" | "GT2" | "GT3" | "BOXSTER" => 4,
        _ => 5,
    }
}

/// `0x4A2DC0`'s return value: the factor the **whole `.ais` bend-speed curve** is multiplied by,
/// per track and per class tier (applied at 0x49CAF5, inside the 40-entry loop).
///
/// Rows verified instruction by instruction against the binary: **monaco1** (0x4A2F7C…0x4A2FB8),
/// **alps** (0x4A37F5…0x4A384E), **autobahn** (0x4A3A9F…0x4A3AC6) and **industrial**
/// (0x4A45AC…0x4A45BB). The rest come from a single reading pass over the same ladders and are
/// marked HIGH rather than CONFIRMED in `ai.md`. The ladders mix `jne` and `jl`, which is how a
/// tier can share its neighbour's value — alps gives tiers 3 and 4 both 1.2 through the `jl` at
/// 0x4A3839.
///
/// An unknown track returns **1.0** (0x4A45C5 hands back the argument slot 0x4A2E0E seeded with
/// 1.0), which is also what the port did for every track before this was read.
fn curve_factor(track: &str, family: &str) -> f32 {
    let tier = class_tier(family, track) as usize;
    // tiers 1…5
    let row: [f32; 5] = match track.to_ascii_lowercase().as_str() {
        "monaco1" => [1.175, 1.22, 1.275, 1.325, 1.5],
        "monaco2" => [1.175, 1.2, 1.275, 1.325, 1.6],
        "monaco3" => [1.2, 1.225, 1.275, 1.325, 1.6],
        "monaco4" => [1.2, 1.2, 1.275, 1.325, 1.375],
        "monaco5" => [1.2, 1.2, 1.275, 1.325, 1.5],
        "alps" => [1.0, 1.15, 1.2, 1.2, 1.3],
        "autobahn" => [1.0, 1.1, 1.1, 1.2, 1.3],
        "canyon" => [1.1, 1.1, 1.15, 1.15, 1.2],
        "castle" => [1.0, 1.0, 1.05, 1.1, 1.1],
        "coastal" => [1.1, 1.1, 1.2, 1.2, 1.3],
        "farmland" => [1.0, 1.0, 1.05, 1.1, 1.2],
        "foothills" => [1.2, 1.2, 1.25, 1.3, 1.4],
        "forest" => [1.0, 1.0, 1.05, 1.15, 1.25],
        "industrial" => [1.0, 1.0, 1.0, 1.0, 1.1],
        _ => [1.0; 5],
    };
    row[tier - 1]
}

/// `monaco3` is the one track whose factor depends on the direction — its two ladders are at
/// 0x4A311A (backwards) and 0x4A3258 (forwards).
fn curve_factor_dir(track: &str, family: &str, backwards: bool) -> f32 {
    if backwards && track.eq_ignore_ascii_case("monaco3") {
        let tier = class_tier(family, track) as usize;
        return [1.15, 1.15, 1.2, 1.325, 1.6][tier - 1];
    }
    curve_factor(track, family)
}

impl AiDriver {
    pub fn new(data: AiCarData, specs: &CarSpecs, sim_tail: &[f32; 22]) -> Self {
        Self { data, bend_speed_base: [0.0; 40], k_lookahead: sim_tail[19], k_steer: sim_tail[20], k_steer_base: sim_tail[20], k_angle: sim_tail[21], stuck_ticks: 0, wrong_way: false, target_speed: 0.0, skill_mean: [1.0; 4], skill_range: [0.0; 4], band: None, skill: 0, catchup: None, last_node: None, link: None, preferred_segment: None, speed_factor: 1.0, rng: 0x1234_5679, wf_div: 12.0, track: String::new(), backwards: false, dbg_n: 0, dbg_offset: 0.0, dbg_dot: 0.0 }
            .tap(|d| {
                // The curve factor is per track *and* per car class, so it cannot be applied here
                // — `set_track` does it, over a pristine copy of the curve so that re-entering a
                // track does not compound it.
                d.bend_speed_base = d.data.bend_speed;
                let _ = specs;
            })
    }

    /// Per-track road-width normaliser and speed zones (0x4A2DC0).
    pub fn set_track(&mut self, track: &str, backwards: bool, family: &str) {
        self.wf_div = crate::zones::width_divisor(track);
        self.track = track.to_ascii_lowercase();
        self.backwards = backwards;
        // `carSpecs[0x140] *= 0x4A2950(family)` (0x49CB16) — the steering factor is per track and
        // per car family, and 1.0 unless the table names the pair
        self.k_steer = self.k_steer_base * steer_factor(track, family, backwards);
        // …and `0x4A2DC0`'s return value scales the whole bend-speed curve (0x49CAF5)
        let f = curve_factor_dir(track, family, backwards);
        for (v, base) in self.data.bend_speed.iter_mut().zip(self.bend_speed_base.iter()) {
            *v = base * f;
        }
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
mod steer_factor_tests {
    use super::steer_factor;

    /// The table at 0x4A2950. Everything it does not name is 1.0 — including every car on every
    /// track it has no entry for, which is what the port used to get wrong by applying 0.85
    /// everywhere.
    #[test]
    fn the_table_is_only_a_handful_of_pairs() {
        // the common case
        assert_eq!(steer_factor("alps", "993", false), 1.0);
        assert_eq!(steer_factor("autobahn", "993", false), 1.0);
        assert_eq!(steer_factor("castle", "356", false), 1.0);

        // the race cars, per track
        assert_eq!(steer_factor("autobahn", "GT2", false), 0.85);
        assert_eq!(steer_factor("castle", "GT2", false), 0.80);
        assert_eq!(steer_factor("monaco2", "935", false), 0.85);

        // castle's three tiers of road car
        assert_eq!(steer_factor("castle", "996T", false), 0.85);
        assert_eq!(steer_factor("castle", "996", false), 0.90);
        // the family key is compared case-insensitively, so the catalogue's `boxster` matches
        assert_eq!(steer_factor("castle", "boxster", false), 0.90);
        assert_eq!(steer_factor("CASTLE", "BOXSTER", false), 0.90);

        assert_eq!(steer_factor("monaco2", "928", false), 0.925);

        // two entries only exist when the track is driven backwards
        assert_eq!(steer_factor("monaco1", "GT1", false), 1.0);
        assert_eq!(steer_factor("monaco1", "GT1", true), 0.85);
        assert_eq!(steer_factor("monaco3", "930T", false), 1.0);
        assert_eq!(steer_factor("monaco3", "930T", true), 1.10);
        assert_eq!(steer_factor("monaco3", "993", true), 1.0);
    }
}

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

#[cfg(test)]
mod curve_factor_tests {
    use super::{class_tier, curve_factor, curve_factor_dir};

    /// `0x4A2BC0`'s five tiers, including the two families that drop a tier on alps only.
    #[test]
    fn the_class_tiers_are_the_five_the_chain_assigns() {
        for f in ["356", "356A", "356B"] {
            assert_eq!(class_tier(f, "canyon"), 1, "{f}");
        }
        for f in ["550A", "928", "930T", "964"] {
            assert_eq!(class_tier(f, "canyon"), 2, "{f}");
        }
        for f in ["914", "944", "959", "965T"] {
            assert_eq!(class_tier(f, "canyon"), 3, "{f}");
        }
        for f in ["993", "993T", "996", "996T", "GT2", "GT3", "BOXSTER"] {
            assert_eq!(class_tier(f, "canyon"), 4, "{f}");
        }
        // unnamed families take the seeded 5 — the GT1 and the 935 among them
        assert_eq!(class_tier("GT1", "canyon"), 5);
        assert_eq!(class_tier("935", "canyon"), 5);
        // …but 935 and GT2 are tier 1 on alps, and nowhere else (0x4A2C1F / 0x4A2C4B)
        assert_eq!(class_tier("935", "alps"), 1);
        assert_eq!(class_tier("GT2", "alps"), 1);
        assert_eq!(class_tier("GT2", "monaco1"), 4);
    }

    /// The four rows read instruction by instruction, and the unknown-track default.
    #[test]
    fn the_verified_rows_match_the_ladders() {
        // monaco1, 0x4A2F7C…0x4A2FB8
        assert_eq!(curve_factor("monaco1", "356"), 1.175);
        assert_eq!(curve_factor("monaco1", "964"), 1.22);
        assert_eq!(curve_factor("monaco1", "944"), 1.275);
        assert_eq!(curve_factor("monaco1", "993"), 1.325);
        assert_eq!(curve_factor("monaco1", "GT1"), 1.5);
        // alps, 0x4A37F5…0x4A384E — the `jl` at 0x4A3839 gives tiers 3 and 4 the same value
        assert_eq!(curve_factor("alps", "356"), 1.0);
        assert_eq!(curve_factor("alps", "964"), 1.15);
        assert_eq!(curve_factor("alps", "944"), 1.2);
        assert_eq!(curve_factor("alps", "993"), 1.2);
        assert_eq!(curve_factor("alps", "GT1"), 1.3);
        // and on alps a 935 is tier 1, so it gets the slowest row
        assert_eq!(curve_factor("alps", "935"), 1.0);
        // autobahn, 0x4A3A9F…0x4A3AC6 — the `jl` puts tiers 2 and 3 together
        assert_eq!(curve_factor("autobahn", "964"), 1.1);
        assert_eq!(curve_factor("autobahn", "944"), 1.1);
        assert_eq!(curve_factor("autobahn", "993"), 1.2);
        // industrial, 0x4A45AC — only tier 5 is not 1.0
        assert_eq!(curve_factor("industrial", "993"), 1.0);
        assert_eq!(curve_factor("industrial", "GT1"), 1.1);
        // an unknown track is 1.0 for everyone (0x4A45C5)
        assert_eq!(curve_factor("skidpad", "GT1"), 1.0);
    }

    /// Only monaco3 changes with the direction (0x4A311A backwards, 0x4A3258 forwards).
    #[test]
    fn only_monaco3_is_direction_dependent() {
        for t in ["monaco1", "alps", "canyon", "industrial"] {
            for f in ["356", "993", "GT1"] {
                assert_eq!(curve_factor_dir(t, f, false), curve_factor_dir(t, f, true), "{t}/{f}");
            }
        }
        assert_eq!(curve_factor_dir("monaco3", "356", false), 1.2);
        assert_eq!(curve_factor_dir("monaco3", "356", true), 1.15);
        assert_eq!(curve_factor_dir("monaco3", "944", true), 1.2);
    }
}
