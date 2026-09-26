//! Career / progression tables from `FEData/Data/nfs5.{trk,trn,prt,per,fac}`.
//!
//! Layouts follow `RE/notes/career.md` (loaders 0x4FB750, 0x502FF0, 0x49C3A0, 0x4E2F30, 0x51ABA0).
//! All five files are flat arrays of fixed-size records; the record count is `size / RECORD_SIZE`
//! exactly as the original loaders compute it. Fields whose meaning was not established keep
//! `unknown_*` names with their file offset.

use crate::{f32_le, fixed_str, u16_le, u32_le, FormatError};

fn i16_le(data: &[u8], off: usize, what: &'static str) -> Result<i16, FormatError> {
    Ok(u16_le(data, off, what)? as i16)
}

fn records<'a>(data: &'a [u8], size: usize, what: &'static str) -> Result<impl Iterator<Item = &'a [u8]>, FormatError> {
    if !data.len().is_multiple_of(size) {
        return Err(FormatError::SizeMismatch { what, declared: data.len() / size * size, actual: data.len() });
    }
    Ok(data.chunks_exact(size))
}

// ---------------------------------------------------------------------------------------------
// nfs5.trk
// ---------------------------------------------------------------------------------------------

pub const TRACK_RECORD_SIZE: usize = 0x4A8;

/// One default track record holder (`trk + 0x1B0 + i*0x4C`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecord {
    pub driver: String,
    pub car: String,
    /// Time in 64 Hz ticks (`RE/notes/career.md` §1, CONFIRMED: 0x492A00 formats the same value as
    /// `t/3840` min, `(t/64) % 60` s, `(t & 63)·100/64` hundredths).
    pub time: u32,
}

/// One `nfs5.trk` record.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub id: u32,
    pub name_string_id: u32,
    /// CRP base name (`coastal`).
    pub file: String,
    /// Backwards-direction art suffix (`bk`), empty for circuits.
    pub backwards_suffix: String,
    pub display_name: String,
    pub art: String,
    pub art_small: String,
    /// 0 = always available, 3 = locked until a tournament whose era has ended lists it
    /// (0x4FBC10, `nfs_game::track_list`).
    pub lock_mode: u16,
    pub circuit: bool,
    pub unknown_70: f32,
    /// `+0x78`: the track's **difficulty**, 0/1/2. `Track1.Track(DIFFICULTY)` reads it as a word at
    /// 0x4FB730 and looks up `FEStrings[0x1C2 + it]` — 450…452, the same three names
    /// `Game.Difficutly` uses. CONFIRMED; this used to be an UNKNOWN with no reader located.
    pub difficulty: u32,
    pub traffic_cars: [u16; 7],
    pub police_cars: [u16; 5],
    pub unknown_94: u32,
    pub records: Vec<TrackRecord>,
}

pub fn parse_track_table(data: &[u8]) -> Result<Vec<TrackInfo>, FormatError> {
    let what = "nfs5.trk";
    let mut out = Vec::new();
    for r in records(data, TRACK_RECORD_SIZE, what)? {
        let n_records = u16_le(r, 0x1AE, what)? as usize;
        if n_records > 8 {
            return Err(FormatError::UnexpectedValue { what, offset: 0x1AE, expected: "<= 8 record holders".into(), found: n_records.to_string() });
        }
        let mut recs = Vec::with_capacity(n_records);
        for i in 0..n_records {
            let b = 0x1B0 + i * 0x4C;
            recs.push(TrackRecord { driver: fixed_str(r, b, 20, what)?, car: fixed_str(r, b + 0x14, 52, what)?, time: u32_le(r, b + 0x48, what)? });
        }
        let mut traffic_cars = [0u16; 7];
        for (i, t) in traffic_cars.iter_mut().enumerate() {
            *t = u16_le(r, 0x7C + i * 2, what)?;
        }
        let mut police_cars = [0u16; 5];
        for (i, t) in police_cars.iter_mut().enumerate() {
            *t = u16_le(r, 0x8A + i * 2, what)?;
        }
        out.push(TrackInfo {
            id: u32_le(r, 0, what)?,
            name_string_id: u32_le(r, 4, what)?,
            file: fixed_str(r, 8, 20, what)?,
            backwards_suffix: fixed_str(r, 0x1C, 20, what)?,
            display_name: fixed_str(r, 0x30, 20, what)?,
            art: fixed_str(r, 0x44, 20, what)?,
            art_small: fixed_str(r, 0x58, 20, what)?,
            lock_mode: u16_le(r, 0x6C, what)?,
            circuit: u16_le(r, 0x6E, what)? == 1,
            unknown_70: f32_le(r, 0x70, what)?,
            difficulty: u32_le(r, 0x78, what)?,
            traffic_cars,
            police_cars,
            unknown_94: u32_le(r, 0x94, what)?,
            records: recs,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// nfs5.trn
// ---------------------------------------------------------------------------------------------

pub const TOURNAMENT_RECORD_SIZE: usize = 0xD40;
const OPPONENT_SIZE: usize = 0x58;
const EVENT_SIZE: usize = 0xA4;

/// Era / mode of a tournament (`trn + 4`, a string id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TournamentKind {
    /// 521 / 522 / 523: Evolution-mode era tournaments.
    Era(Era),
    /// 524 / 525 / 526: Tournament-mode club events of an era.
    Club(Era),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum Era {
    Classic,
    Golden,
    Modern,
}

impl Era {
    pub fn string_id(self) -> u32 {
        match self {
            Era::Classic => 521,
            Era::Golden => 522,
            Era::Modern => 523,
        }
    }
    pub fn next(self) -> Option<Era> {
        match self {
            Era::Classic => Some(Era::Golden),
            Era::Golden => Some(Era::Modern),
            Era::Modern => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TournamentOpponent {
    /// 31..37; driver name id (HYPOTHESIS: `IDS_DRIVER1 + n` or an `nfs5.per` id).
    pub driver: i16,
    /// `nfs5.car` id.
    pub car_id: i16,
    /// Installed `nfs5.prt` ids (0-terminated list, up to 22).
    pub parts: Vec<i16>,
    pub unknown_30: u32,
    /// 8 × RGBA paint / decal colours.
    pub colours: [[u8; 4]; 8],
    pub unknown_50: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TournamentEvent {
    /// `nfs5.trk` id.
    pub track_id: i16,
    /// Laps (`+0x02`; 0 in the file means 1 — point-to-point stages).
    pub laps: u16,
    /// `+0x04`: drive the track backwards.
    pub backwards: bool,
    /// `+0x05`: second direction flag (0 in every shipped event).
    pub direction_flag: u8,
    /// Prize money by finishing position 1..8 (`[0]` = 1st).
    pub prize: [u32; 8],
    /// Bonus; paid ×1 / ×0.5 / ×0.25 for positions 1 / 2 / 3 (0x503960).
    pub bonus: u32,
    /// 0.45…1.0, HYPOTHESIS: AI difficulty.
    pub difficulty: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tournament {
    pub id: i16,
    pub kind: TournamentKind,
    pub start_year: u16,
    /// Track ids unlocked once the chronicle year reaches the year this tournament's era ends
    /// (0x502990: 1970 / 1989 / 2000; 0x4FBC10) — not while the era is active.
    pub unlock_tracks: Vec<i16>,
    pub name_string_id: u32,
    pub title_string_id: u32,
    pub description_string_id: u32,
    pub restriction_string_id: u32,
    /// Eligible `nfs5.car` ids.
    pub eligible_cars: Vec<i16>,
    pub entry_fee: u32,
    /// `+0xE0`: traffic level, index into `{0, 2, 4, 8}` traffic cars (0x4FC425).
    pub traffic_level: u16,
    /// HYPOTHESIS: knockout format when 1.
    pub knockout: bool,
    /// 2 = hidden era finale (ids 31, 32, 33), skipped by the per-era lists.
    pub visibility: u16,
    /// `+0xE6`: `Game.Difficulty` level 0..2 passed to the race (catch-up rubber band).
    pub difficulty_level: u16,
    /// `+0xE8`: club events with a non-zero byte are ranked by total time instead of points.
    pub flags: [u8; 4],
    /// Tournament prize money for the final standings 1..8.
    pub prize: [u32; 8],
    /// `+0x10C`: prize car id of the era (168 '56 550 A, 169 '78 935/78, 170 '98 GT1) — on the era
    /// finale records; the loaned finale car.
    pub prize_car: u16,
    pub unknown_110: u32,
    /// `+0x380` / `+0x382`: explicit traffic car ids when the flag is set.
    pub custom_traffic: Option<Vec<i16>>,
    pub opponents: Vec<TournamentOpponent>,
    pub events: Vec<TournamentEvent>,
}

impl Tournament {
    pub fn era(&self) -> Era {
        match self.kind {
            TournamentKind::Era(e) | TournamentKind::Club(e) => e,
        }
    }
    pub fn is_club(&self) -> bool {
        matches!(self.kind, TournamentKind::Club(_))
    }
    /// Hidden era-finale record (`visibility == 2`).
    pub fn is_hidden(&self) -> bool {
        self.visibility == 2
    }
    /// Standings ranked by total time (club events with `flags[0] != 0`, 0x502880).
    pub fn time_ranked(&self) -> bool {
        self.flags[0] != 0
    }
}

pub fn parse_tournaments(data: &[u8]) -> Result<Vec<Tournament>, FormatError> {
    let what = "nfs5.trn";
    let mut out = Vec::new();
    for (ri, r) in records(data, TOURNAMENT_RECORD_SIZE, what)?.enumerate() {
        let base = ri * TOURNAMENT_RECORD_SIZE;
        let kind = match u32_le(r, 4, what)? {
            521 => TournamentKind::Era(Era::Classic),
            522 => TournamentKind::Era(Era::Golden),
            523 => TournamentKind::Era(Era::Modern),
            524 => TournamentKind::Club(Era::Classic),
            525 => TournamentKind::Club(Era::Golden),
            526 => TournamentKind::Club(Era::Modern),
            other => return Err(FormatError::UnknownRecordType { what, offset: base + 4, code: other }),
        };
        let n_unlock = i16_le(r, 0x0A, what)?.clamp(0, 18) as usize;
        let unlock_tracks = (0..n_unlock).map(|i| i16_le(r, 0x0C + i * 2, what)).collect::<Result<_, _>>()?;
        let n_cars = i16_le(r, 0xDA, what)?;
        if !(0..=84).contains(&n_cars) {
            return Err(FormatError::UnexpectedValue { what, offset: base + 0xDA, expected: "0..=84 eligible cars".into(), found: n_cars.to_string() });
        }
        let eligible_cars = (0..n_cars as usize).map(|i| i16_le(r, 0x30 + i * 2, what)).collect::<Result<_, _>>()?;
        let mut prize = [0u32; 8];
        for (i, p) in prize.iter_mut().enumerate() {
            *p = u32_le(r, 0xEC + i * 4, what)?;
        }
        let n_opp = u32_le(r, 0x114, what)? as usize;
        if n_opp > 7 {
            return Err(FormatError::UnexpectedValue { what, offset: base + 0x114, expected: "<= 7 opponents".into(), found: n_opp.to_string() });
        }
        let mut opponents = Vec::with_capacity(n_opp);
        for i in 0..n_opp {
            let b = 0x118 + i * OPPONENT_SIZE;
            let mut parts = Vec::new();
            for k in 0..22 {
                let p = i16_le(r, b + 4 + k * 2, what)?;
                if p == 0 {
                    break;
                }
                parts.push(p);
            }
            let mut colours = [[0u8; 4]; 8];
            for (k, c) in colours.iter_mut().enumerate() {
                let o = b + 0x34 + k * 4;
                c.copy_from_slice(&r[o..o + 4]);
            }
            opponents.push(TournamentOpponent { driver: i16_le(r, b, what)?, car_id: i16_le(r, b + 2, what)?, parts, unknown_30: u32_le(r, b + 0x30, what)?, colours, unknown_50: i16_le(r, b + 0x50, what)? });
        }
        // the original clamps the event count to 10 (0x502BA0); the file allows 15 slots
        let n_events = i16_le(r, 0x39C, what)?;
        if !(0..=15).contains(&n_events) {
            return Err(FormatError::UnexpectedValue { what, offset: base + 0x39C, expected: "0..=15 events".into(), found: n_events.to_string() });
        }
        let mut events = Vec::with_capacity(n_events as usize);
        for i in 0..n_events.min(10) as usize {
            let b = 0x3A0 + i * EVENT_SIZE;
            let mut prize = [0u32; 8];
            for (k, p) in prize.iter_mut().enumerate() {
                *p = u32_le(r, b + 4 + (k + 1) * 4, what)?;
            }
            events.push(TournamentEvent {
                track_id: i16_le(r, b, what)?,
                laps: u16_le(r, b + 2, what)?.max(1),
                backwards: r[b + 4] != 0,
                direction_flag: r[b + 5],
                prize,
                bonus: u32_le(r, b + 0x28, what)?,
                difficulty: f32_le(r, b + 0x2C, what)?,
            });
        }
        let mut flags = [0u8; 4];
        flags.copy_from_slice(&r[0xE8..0xEC]);
        let custom_traffic = if u16_le(r, 0x380, what)? != 0 {
            Some((0..7).map(|i| i16_le(r, 0x382 + i * 2, what)).collect::<Result<Vec<_>, _>>()?.into_iter().filter(|id| *id != 0).collect())
        } else {
            None
        };
        out.push(Tournament {
            id: i16_le(r, 0, what)?,
            kind,
            start_year: u16_le(r, 8, what)?,
            unlock_tracks,
            name_string_id: u32_le(r, 0x20, what)?,
            title_string_id: u32_le(r, 0x24, what)?,
            description_string_id: u32_le(r, 0x28, what)?,
            restriction_string_id: u32_le(r, 0x2C, what)?,
            eligible_cars,
            entry_fee: u32_le(r, 0xDC, what)?,
            traffic_level: u16_le(r, 0xE0, what)?,
            knockout: u16_le(r, 0xE2, what)? == 1,
            visibility: u16_le(r, 0xE4, what)?,
            difficulty_level: u16_le(r, 0xE6, what)?,
            flags,
            prize,
            prize_car: u16_le(r, 0x10C, what)?,
            unknown_110: u32_le(r, 0x110, what)?,
            custom_traffic,
            opponents,
            events,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// nfs5.prt
// ---------------------------------------------------------------------------------------------

pub const PART_RECORD_SIZE: usize = 0x168;

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub id: i16,
    pub name_string_id: u32,
    /// Car family the part fits (`356`, `911 (993)`, `GT1` …).
    pub family: String,
    pub unknown_24: u16,
    /// The part's category, and the heading the parts lists group it under: 1 ENGINE, 2 BRAKES,
    /// 3 TRANSMISSION, 4 SUSPENSION, 5 SHOCKS, 6 and 7 both WEIGHT REDUCTION, 8 SWAY BARS,
    /// 9 TIRES, 10 BODYWORK — `partCategoryString` 0x518D60 maps `+0x26` to string 2554…2562, and
    /// 6 and 7 share one (CONFIRMED; the earlier reading of 6, 7, 9 and 10 was a guess).
    pub kind: u16,
    pub kind_name: String,
    pub year: u16,
    /// `+0x48` u8[10]: damage zones (0..7 ring, 8 roof, 9) whose mean feeds this part's damage
    /// (`0x4938A0` family, `RE/notes/physics.md`); zero for the default part.
    pub zone_mask: [u8; 10],
    /// Added to `sim + 0x40` (mass, kg).
    pub mass_delta: f32,
    /// `sim + 0x90 /= 1 + 0.01 * v`.
    pub top_speed_pct: f32,
    pub price: u32,
    pub condition: f32,
    /// `{group, slot, geometry id}` visual swaps.
    pub geometry_swaps: Vec<[i16; 3]>,
    /// `+0xC0`, the arm's first tuning value, and what it means is the `kind`'s business
    /// (`nfs-physics`'s `CarSpecs::apply_parts`): a gearbox part's final drive, a brake / spring /
    /// shock / sway-bar factor whose identity is **1**, an aero part's front percentage — and on an
    /// engine part a redline in rpm that **nothing reads**.
    pub value_c0: f32,
    /// `+0xC4`, the second: a spring's ride height in millimetres, an aero part's rear percentage,
    /// a tyre's rear grip figure. 0 on every other kind.
    pub boost: f32,
    /// `+0xC8`. **Not an induction type** — the induction comes from an engine part's *name*
    /// (0x49CDC5 / 0x49CE09). On a tyre part (`kind == 9`) this is the **grip-table column**, 0, 1
    /// or 2, which 0x49D1FA writes straight into `carSpecs+0x1C0`. It is 0 or 1 on engine parts and
    /// 0 everywhere else, and nothing reads it there.
    pub grip_class: u32,
    pub forward_gears: i16,
    pub gearbox_param: i16,
    /// Reverse, neutral, 1st … (gearbox parts).
    pub gear_ratios: [f32; 8],
    /// 21 points, 500 rpm spacing (engine parts).
    pub torque_curve: [f32; 21],
    pub state: u32,
}

/// The `FEStrings` id of the heading a part's category is listed under (`partCategoryString`
/// 0x518D60). CONFIRMED.
pub fn part_category_string(kind: u16) -> Option<u32> {
    Some(match kind {
        1..=5 => 2553 + kind as u32,
        6 | 7 => 2559,
        8 => 2560,
        9 => 2561,
        10 => 2562,
        _ => return None,
    })
}

pub fn parse_parts(data: &[u8]) -> Result<Vec<Part>, FormatError> {
    let what = "nfs5.prt";
    let mut out = Vec::new();
    for (ri, r) in records(data, PART_RECORD_SIZE, what)?.enumerate() {
        let n_swaps = i16_le(r, 0x8C, what)?;
        if !(0..=16).contains(&n_swaps) {
            return Err(FormatError::UnexpectedValue { what, offset: ri * PART_RECORD_SIZE + 0x8C, expected: "0..=16 geometry swaps".into(), found: n_swaps.to_string() });
        }
        let geometry_swaps = (0..n_swaps as usize)
            .map(|i| Ok([i16_le(r, 0x8E + i * 6, what)?, i16_le(r, 0x90 + i * 6, what)?, i16_le(r, 0x92 + i * 6, what)?]))
            .collect::<Result<_, FormatError>>()?;
        let mut zone_mask = [0u8; 10];
        zone_mask.copy_from_slice(&r[0x48..0x52]);
        let mut gear_ratios = [0f32; 8];
        for (i, g) in gear_ratios.iter_mut().enumerate() {
            *g = f32_le(r, 0xF0 + i * 4, what)?;
        }
        let mut torque_curve = [0f32; 21];
        for (i, t) in torque_curve.iter_mut().enumerate() {
            *t = f32_le(r, 0x110 + i * 4, what)?;
        }
        out.push(Part {
            id: i16_le(r, 0, what)?,
            name_string_id: u32_le(r, 4, what)?,
            family: fixed_str(r, 8, 28, what)?,
            unknown_24: u16_le(r, 0x24, what)?,
            kind: u16_le(r, 0x26, what)?,
            kind_name: fixed_str(r, 0x28, 28, what)?,
            year: u16_le(r, 0x46, what)?,
            zone_mask,
            mass_delta: f32_le(r, 0x54, what)?,
            top_speed_pct: f32_le(r, 0x58, what)?,
            price: u32_le(r, 0x5C, what)?,
            condition: f32_le(r, 0x60, what)?,
            geometry_swaps,
            value_c0: f32_le(r, 0xC0, what)?,
            boost: f32_le(r, 0xC4, what)?,
            grip_class: u32_le(r, 0xC8, what)?,
            forward_gears: i16_le(r, 0xEA, what)?,
            gearbox_param: i16_le(r, 0xEC, what)?,
            gear_ratios,
            torque_curve,
            state: u32_le(r, 0x164, what)?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// nfs5.per
// ---------------------------------------------------------------------------------------------

pub const PERSONALITY_RECORD_SIZE: usize = 0x2C;

#[derive(Debug, Clone, PartialEq)]
pub struct Personality {
    pub id: i16,
    /// 18000+n (`IDS_DRIVERn`) or 2500+n (factory driver names, ids 24–30).
    pub name_string_id: u32,
    /// 0 / 1 / 2.
    pub era: u32,
    /// Four (mean, range) pairs = SKILL / SKILLRANGE for the bend classes slow, medium, fast and
    /// the default (0x4EBDA0: `SKILL[k] = difficulty × mean[k]`, `RANGE[k] = range[k]`). CONFIRMED.
    pub skill: [[f32; 2]; 4],
}

pub fn parse_personalities(data: &[u8]) -> Result<Vec<Personality>, FormatError> {
    let what = "nfs5.per";
    let mut out = Vec::new();
    for r in records(data, PERSONALITY_RECORD_SIZE, what)? {
        let mut skill = [[0f32; 2]; 4];
        for (i, s) in skill.iter_mut().enumerate() {
            s[0] = f32_le(r, 0x0C + i * 8, what)?;
            s[1] = f32_le(r, 0x10 + i * 8, what)?;
        }
        out.push(Personality { id: i16_le(r, 0, what)?, name_string_id: u32_le(r, 4, what)?, era: u32_le(r, 8, what)?, skill });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// nfs5.fac
// ---------------------------------------------------------------------------------------------

pub const MISSION_RECORD_SIZE: usize = 0xF0;

/// Cone rule of a mission (`+0xC8..+0xCE`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConeRule {
    /// Fail above this many knocked cones (`+0xCC`; unused by the shipped data).
    Limit(u16),
    /// Seconds added per knocked cone (`+0xCE`).
    PenaltySeconds(u16),
}

/// One Factory Driver mission (`RE/notes/skill-tests.md` §2, CONFIRMED unless noted).
#[derive(Debug, Clone, PartialEq)]
pub struct Mission {
    /// Briefing text id; the title is `intro + 1` (except `2m10` → 3331 is a result line, `3m09` → 3501).
    pub intro_string_id: u32,
    /// 1 / 2 / 3 (Junior … Chief Test Driver tiers).
    pub stage: i16,
    /// Promotion text id at the end of the stage (3191 / 3342 / 3495).
    pub promotion_string_id: u32,
    /// Prize car id at the end of the stage (222 '73 RS 2.7, 221 '97 Boxster, 223 '00 996 Turbo).
    pub prize_car: i16,
    /// Mission number inside the stage (HIGH).
    pub number: u16,
    /// Narrator: 0 Billy, 1 Stephanie, 2 Dieter, 3 Frank, 4 Klaus, 5 Rolf. The name is
    /// `FEStrings[2500 + n]` — `Factory.Data(NARRNAME)` reads this field at 0x519DCA and adds
    /// `0x9C4` at 0x519DCE. (`FEStrings.csv` carries the same six names again at 18176, which an
    /// earlier note pointed at; the address-backed constant is 2500.)
    pub narrator: i16,
    /// `nfs5.trk` id.
    pub track_id: i16,
    pub laps: u16,
    /// Scene number `N` → `<track>_ST<N>.scn` (0 = none: circuit lap race / plain race).
    pub scene: i16,
    pub police: bool,
    /// `+0x28 == 2`: track driven backwards.
    pub backwards: bool,
    /// `+0x2A`: the value the mission puts into the player slot's `.CABRIODOWN` accessor (id 0x24C,
    /// `0x4EEF48`, the only writer of that accessor in the game). Set on four coupé missions and clear
    /// on every open-top one, so its effect on the car model is not explained by the data
    /// (`RE/notes/skill-tests.md` §2).
    pub cabrio_down: bool,
    /// `0M01` … `3m11`.
    pub code: String,
    /// Traffic level 0..3 → `{0, 2, 4, 8}` cars.
    pub traffic_level: i16,
    /// AI skill / difficulty 0..2.
    pub skill: i16,
    pub success_string_ids: [u32; 3],
    pub failure_string_ids: [u32; 3],
    pub car_id: i16,
    pub parts: Vec<i16>,
    pub colours: [[u8; 4]; 8],
    pub colour_index_bc: i16,
    pub colour_index_be: i16,
    /// `+0xC0`: 1 only on `0M01` and `3m07`, the two missions driven in an open car; no reader exists
    /// anywhere in the executable, so nothing is done with it here either.
    pub unknown_c0: u8,
    /// Time limit in seconds (`+0xC1` flag, `+0xC2`).
    pub time_limit_s: Option<u16>,
    /// Damage limit in per cent (`+0xC4` flag, `+0xC6`).
    pub damage_limit_pct: Option<u16>,
    pub cone_rule: Option<ConeRule>,
    /// Minimum finishing position (`+0xD0` flag, `+0xD2`).
    pub min_rank: Option<u16>,
    /// Opponents `(nfs5.per id, nfs5.car id)`; any entry turns the mission into a plain race.
    pub opponents: Vec<(i16, i16)>,
    /// Runtime result of the last attempt in the shipped file (0).
    pub result: i16,
}

impl Mission {
    pub fn title_string_id(&self) -> u32 {
        match self.intro_string_id {
            3330 => 3330,
            3500 => 3500,
            id => id + 1,
        }
    }
    pub fn is_race(&self) -> bool {
        !self.opponents.is_empty()
    }
    pub fn scene_file(&self, track_file: &str) -> Option<String> {
        (self.scene > 0 && !self.is_race()).then(|| format!("{track_file}_st{}.scn", self.scene))
    }
}

pub fn parse_missions(data: &[u8]) -> Result<Vec<Mission>, FormatError> {
    let what = "nfs5.fac";
    let mut out = Vec::new();
    for (ri, r) in records(data, MISSION_RECORD_SIZE, what)?.enumerate() {
        let n_parts = i16_le(r, 0x72, what)?;
        if !(0..=22).contains(&n_parts) {
            return Err(FormatError::UnexpectedValue { what, offset: ri * MISSION_RECORD_SIZE + 0x72, expected: "0..=22 parts".into(), found: n_parts.to_string() });
        }
        let parts = (0..n_parts as usize).map(|i| i16_le(r, 0x74 + i * 2, what)).collect::<Result<_, _>>()?;
        let mut colours = [[0u8; 4]; 8];
        for (k, c) in colours.iter_mut().enumerate() {
            c.copy_from_slice(&r[0xA0 + k * 4..0xA4 + k * 4]);
        }
        let ids = |base: usize| -> Result<[u32; 3], FormatError> { Ok([u32_le(r, base, what)?, u32_le(r, base + 4, what)?, u32_le(r, base + 8, what)?]) };
        let mut opponents = Vec::new();
        for k in 0..5 {
            let per = i16_le(r, 0xD4 + k * 4, what)?;
            let car = i16_le(r, 0xD6 + k * 4, what)?;
            if per != 0 || car != 0 {
                opponents.push((per, car));
            }
        }
        // 0x4EC102…0x4EC151: `CONEIMIT` takes `+0xCC` only for kind 0 and `CONEPENALTY` `+0xCE`
        // only for kind 1 (`cmp word [+0xCA], 0 / 1`); any other kind leaves both −1
        let cone_rule = if r[0xC8] != 0 {
            match u16_le(r, 0xCA, what)? {
                0 => Some(ConeRule::Limit(u16_le(r, 0xCC, what)?)),
                1 => Some(ConeRule::PenaltySeconds(u16_le(r, 0xCE, what)?)),
                _ => None,
            }
        } else {
            None
        };
        out.push(Mission {
            intro_string_id: u32_le(r, 0, what)?,
            stage: i16_le(r, 8, what)?,
            promotion_string_id: u32_le(r, 0x0C, what)?,
            prize_car: i16_le(r, 0x10, what)?,
            number: u16_le(r, 0x1C, what)?,
            narrator: i16_le(r, 0x1E, what)?,
            track_id: i16_le(r, 0x20, what)?,
            laps: u16_le(r, 0x22, what)?.max(1),
            scene: i16_le(r, 0x24, what)?,
            police: r[0x26] != 0,
            backwards: i16_le(r, 0x28, what)? == 2,
            cabrio_down: r[0x2A] != 0,
            code: fixed_str(r, 0x3F, 8, what)?,
            traffic_level: i16_le(r, 0x54, what)?,
            skill: i16_le(r, 0x56, what)?,
            success_string_ids: ids(0x58)?,
            failure_string_ids: ids(0x64)?,
            car_id: i16_le(r, 0x70, what)?,
            parts,
            colours,
            colour_index_bc: i16_le(r, 0xBC, what)?,
            colour_index_be: i16_le(r, 0xBE, what)?,
            unknown_c0: r[0xC0],
            time_limit_s: (r[0xC1] != 0).then(|| u16_le(r, 0xC2, what)).transpose()?,
            damage_limit_pct: (r[0xC4] != 0).then(|| u16_le(r, 0xC6, what)).transpose()?,
            cone_rule,
            min_rank: (r[0xD0] != 0).then(|| u16_le(r, 0xD2, what)).transpose()?.filter(|r| *r != 0),
            opponents,
            result: i16_le(r, 0xE8, what)?,
        });
    }
    Ok(out)
}

/// `SaveData\nfs5.trk` — the persisted track-record tables (`RE/notes/savegame.md` §7, CONFIRMED).
///
/// The `TrackRecords` save module (0x4FAD80) writes *the whole in-memory `nfs5.trk` array* —
/// `[0x691C94] × 0x4A8` bytes — to its own file rather than into the `.sav`, which is why records
/// are global across profiles. On start-up 0x4FAE20 seeds the file from `FEData\data\nfs5.trk` when
/// it is missing, reads it back, derives the record count as `size / 0x4A8`, and runs the
/// empty-slot fixup: **unconditionally for all 8 slots of every track, ignoring `trk+0x1AE`**, any
/// entry whose time is 0 becomes `"n/a"` with time 359 999.
///
/// This type holds the array as raw bytes so a port can round-trip a file the original wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecordFile {
    pub raw: Vec<u8>,
}

/// The empty-slot time written by the 0x4FAE20 fixup.
pub const NO_RECORD_TIME: u32 = 359_999;
/// Record holders per track (`trk+0x1B0`, stride 0x4C, fastest first).
pub const RECORD_SLOTS: usize = 8;
const SLOT_STRIDE: usize = 0x4C;
const SLOTS_AT: usize = 0x1B0;

impl TrackRecordFile {
    /// Wrap a `nfs5.trk` image and apply the 0x4FAE20 empty-slot fixup.
    pub fn load(data: &[u8]) -> Result<Self, FormatError> {
        if !data.len().is_multiple_of(TRACK_RECORD_SIZE) {
            return Err(FormatError::SizeMismatch { what: "nfs5.trk", declared: TRACK_RECORD_SIZE, actual: data.len() % TRACK_RECORD_SIZE });
        }
        let mut f = Self { raw: data.to_vec() };
        f.fixup();
        Ok(f)
    }

    pub fn count(&self) -> usize {
        self.raw.len() / TRACK_RECORD_SIZE
    }

    /// `nfs5.trk` id of record `i` (`*(u16*)trk`, the key 0x4FAFC0 searches by).
    pub fn id_at(&self, i: usize) -> u16 {
        let b = i * TRACK_RECORD_SIZE;
        u16::from_le_bytes([self.raw[b], self.raw[b + 1]])
    }

    pub fn index_of(&self, id: u16) -> Option<usize> {
        (0..self.count()).find(|i| self.id_at(*i) == id)
    }

    fn slot(&self, track: usize, k: usize) -> usize {
        track * TRACK_RECORD_SIZE + SLOTS_AT + k * SLOT_STRIDE
    }

    /// 0x4FAE20's tail: every zero-time slot of every track becomes `"n/a"` / 359 999.
    fn fixup(&mut self) {
        for t in 0..self.count() {
            for k in 0..RECORD_SLOTS {
                let e = self.slot(t, k);
                if u32::from_le_bytes([self.raw[e + 0x48], self.raw[e + 0x49], self.raw[e + 0x4A], self.raw[e + 0x4B]]) == 0 {
                    // `strcpy(e, "n/a")` (0x4FAF41…0x4FAF67): four bytes, the rest of the
                    // 20-byte name left as it was
                    self.raw[e..e + 4].copy_from_slice(b"n/a\0");
                    self.raw[e + 0x48..e + 0x4C].copy_from_slice(&NO_RECORD_TIME.to_le_bytes());
                }
            }
        }
    }

    pub fn entries(&self, track: usize) -> Vec<TrackRecord> {
        (0..RECORD_SLOTS)
            .map(|k| {
                let e = self.slot(track, k);
                TrackRecord {
                    driver: fixed_str(&self.raw, e, 20, "nfs5.trk").unwrap_or_default(),
                    car: fixed_str(&self.raw, e + 0x14, 52, "nfs5.trk").unwrap_or_default(),
                    time: u32::from_le_bytes([self.raw[e + 0x48], self.raw[e + 0x49], self.raw[e + 0x4A], self.raw[e + 0x4B]]),
                }
            })
            .collect()
    }

    /// `Records_insert` 0x4FAFC0: find the record for `id`, insert at the first slot whose time is
    /// **strictly** greater than `time`, shift the rest down, drop the eighth. Returns the 0-based
    /// rank, or `None` when the time does not make the table.
    ///
    /// The driver name is a plain `strcpy` into the 20-byte field (0x4FB0BF); the car name is
    /// `strncpy(slot+0x14, car, 0x11)` followed by `slot[0x25] = 0` (0x4FB0AB/0x4FB0E8), so only the
    /// first **17** characters of a car name are ever stored even though the field is 52 bytes.
    ///
    /// Byte for byte (2026-09-26): the compare is `jl`, signed (0x4FB006); the shift moves each
    /// row with the time dword and two `strcpy`s, driver then car (0x4FB03F…0x4FB09D), and the new
    /// row gets its time, `strcpy` of the driver and the `strncpy` of the car over whatever the
    /// slot held — no field is cleared, so bytes past a terminator stay as they were. *(Until this
    /// date the port copied whole rows and zeroed the new one, and clipped the driver to 19.)* A
    /// driver name that runs past the 0x48 bytes before the time is cut there (APPROXIMATION: the
    /// original's `strcpy` would run on into the time it had just stored).
    ///
    /// An id no record carries is **not** refused: the search loop 0x4FAFDD…0x4FAFEB falls out
    /// with `edi` still the table base `[0x691CE8]` and 0x4FAFED adds 0x1B0 to that, so the time
    /// goes into **record 0**'s table. *(Until 2026-09-26 the port returned `None`.)* With no
    /// record at all the original would write past its empty buffer; the port refuses that.
    pub fn insert(&mut self, id: u16, driver: &str, car: &str, time: u32) -> Option<usize> {
        if self.count() == 0 {
            return None;
        }
        let t = self.index_of(id).unwrap_or(0);
        let times: Vec<u32> = self.entries(t).iter().map(|e| e.time).collect();
        let k = times.iter().position(|e| (time as i32) < (*e as i32))?;
        // `strcpy(raw[dst..], bytes)` kept inside the row's 0x48 bytes of text
        fn strcpy(raw: &mut [u8], row: usize, at: usize, bytes: &[u8]) {
            let end = row + 0x48;
            for (i, b) in bytes.iter().copied().chain(std::iter::once(0)).enumerate() {
                if at + i >= end {
                    break;
                }
                raw[at + i] = b;
            }
        }
        let cstr = |raw: &[u8], at: usize, end: usize| -> Vec<u8> { raw[at..end].iter().copied().take_while(|b| *b != 0).collect() };
        for j in (k + 1..RECORD_SLOTS).rev() {
            let (dst, src) = (self.slot(t, j), self.slot(t, j - 1));
            let tm = self.raw[src + 0x48..src + 0x4C].to_vec();
            self.raw[dst + 0x48..dst + 0x4C].copy_from_slice(&tm);
            let d = cstr(&self.raw, src, src + 0x48);
            strcpy(&mut self.raw, dst, dst, &d);
            let c = cstr(&self.raw, src + 0x14, src + 0x48);
            strcpy(&mut self.raw, dst, dst + 0x14, &c);
        }
        let e = self.slot(t, k);
        self.raw[e + 0x48..e + 0x4C].copy_from_slice(&time.to_le_bytes());
        let d: Vec<u8> = driver.chars().map(|c| c as u32 as u8).collect();
        strcpy(&mut self.raw, e, e, &d);
        // strncpy(slot+0x14, car, 0x11): up to 17 bytes, zero-padded to 17 when shorter
        let c: Vec<u8> = car.chars().map(|c| c as u32 as u8).take(0x11).collect();
        for i in 0..0x11 {
            self.raw[e + 0x14 + i] = c.get(i).copied().unwrap_or(0);
        }
        self.raw[e + 0x25] = 0;
        Some(k)
    }
}

#[cfg(test)]
mod track_record_tests {
    use super::*;

    /// 0x4FAE20's fixup is a `strcpy` of `"n/a"`: four bytes, the rest of the name untouched.
    #[test]
    fn the_empty_slot_fixup_writes_only_the_string() {
        let mut d = vec![0u8; TRACK_RECORD_SIZE];
        d[SLOTS_AT..SLOTS_AT + 8].copy_from_slice(b"Longname");
        let f = TrackRecordFile::load(&d).unwrap();
        assert_eq!(&f.raw[SLOTS_AT..SLOTS_AT + 8], b"n/a\0name");
        assert_eq!(f.entries(0)[0].time, NO_RECORD_TIME);
    }

    /// 0x4FAFC0 moves rows with `strcpy`s and writes the new one over the old bytes: whatever
    /// follows a terminator stays.
    #[test]
    fn an_insert_leaves_the_bytes_past_each_terminator() {
        let mut d = vec![0u8; TRACK_RECORD_SIZE];
        d[0..2].copy_from_slice(&8u16.to_le_bytes());
        for k in 0..RECORD_SLOTS {
            let e = SLOTS_AT + k * SLOT_STRIDE;
            d[e..e + 8].copy_from_slice(b"Speedkng");
            d[e + 0x14..e + 0x14 + 9].copy_from_slice(b"356 B Car");
            d[e + 0x48..e + 0x4C].copy_from_slice(&(1000 * (k as u32 + 1)).to_le_bytes());
        }
        let mut f = TrackRecordFile::load(&d).unwrap();
        assert_eq!(f.insert(8, "Hans", "911", 500), Some(0));
        let e = SLOTS_AT;
        assert_eq!(&f.raw[e..e + 9], b"Hans\0kng\0", "strcpy over the old name");
        assert_eq!(&f.raw[e + 0x14..e + 0x14 + 0x12], b"911\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", "strncpy pads to 17, then +0x25 = 0");
        assert_eq!(f.entries(0)[1].driver, "Speedkng");
        assert_eq!(f.entries(0)[7].time, 7000, "the slowest dropped off");
        assert_eq!(f.insert(8, "Hans", "911", 0x8000_0000), Some(0), "the compare is signed (jl): past i32::MAX a time is negative");
    }

    /// 0x4FAFED: an id no record has falls through to record 0.
    #[test]
    fn an_unknown_track_id_goes_into_record_0() {
        let mut d = vec![0u8; 2 * TRACK_RECORD_SIZE];
        d[0..2].copy_from_slice(&8u16.to_le_bytes());
        d[TRACK_RECORD_SIZE..TRACK_RECORD_SIZE + 2].copy_from_slice(&9u16.to_le_bytes());
        let mut f = TrackRecordFile::load(&d).unwrap();
        assert_eq!(f.insert(77, "Hans", "911", 500), Some(0));
        assert_eq!(f.entries(0)[0].time, 500, "record 0 took it");
        assert_eq!(f.entries(1)[0].time, NO_RECORD_TIME, "record 1 untouched");
        assert_eq!(TrackRecordFile::load(&[]).map(|mut e| e.insert(8, "Hans", "911", 1)).ok().flatten(), None);
    }
}
