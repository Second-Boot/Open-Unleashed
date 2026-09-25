//! Evolution-mode career: eras, tournaments, prize money, garage, persistence.
//!
//! Data tables and rules follow `RE/notes/career.md` (§10 = verified rules, CONFIRMED unless noted):
//!
//! * Starting money `$11 000` (`mov [0x691D90], 11000`).
//! * Event: prize by position, bonus × 1 / ½ / ¼ for positions 1 / 2 / 3, points `9 − position`
//!   (0x503960); standings: points desc, money earned desc, total time asc (0x502880), club events
//!   with the time flag: time asc.
//! * Entry fee at the start of the first event of each attempt (0x4D58FB); tournament prize
//!   `trn.prize[rank]` after the last event for any rank; rank > 3 resets the tournament (0x503AD0).
//! * Era advances when every visible tournament of the era is completed (rank ≤ 3); the chronicle
//!   year steps through the year table on each first completion.
//! * Era finale: one-off event (5 laps) in the loaned prize car `trn+0x10C`; 1st place awards it.
//! * Dealer price by age curve (0x4EE220); car value by year curve, mileage, damage, wins, parts
//!   (0x4F8E50, HIGH); selling pays the value (the original's offer loop is simplified — MEDIUM).
//! * Career races add 3000 km and one win to the owned car.
//!
//! The save file is this port's own JSON (the original `.sav` accessor section is still open).

use nfs_assets::GamePaths;
use nfs_formats::career::{self, Era, Mission, Part, Personality, Tournament, TournamentKind, TrackInfo};
use nfs_formats::fedata::CarCatalogEntry;
use nfs_formats::locale::Strings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STARTING_MONEY: i64 = 11_000;
pub const SAVE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CareerError {
    #[error("{file}: {source}")]
    Table { file: &'static str, #[source] source: nfs_formats::FormatError },
    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    #[error("save file {path}: {source}")]
    Json { path: PathBuf, #[source] source: serde_json::Error },
    #[error("save version {found} is newer than supported {supported}")]
    SaveVersion { found: u32, supported: u32 },
    #[error("tournament {id} is not available in the current era")]
    NotAvailable { id: i16 },
    #[error("not enough money: need ${need}, have ${have}")]
    NoMoney { need: i64, have: i64 },
    #[error("no eligible car in the garage for tournament {id}")]
    NoEligibleCar { id: i16 },
    #[error("unknown car id {id}")]
    UnknownCar { id: u16 },
    #[error("tournament {id} has no remaining events")]
    NoEvents { id: i16 },
    #[error("unknown part id {id}")]
    UnknownPart { id: i16 },
    #[error("part {id} does not fit this car")]
    PartMismatch { id: i16 },
    #[error("the last car cannot be sold")]
    LastCar,
    #[error("no more than {MAX_OWNED_CARS} cars can be owned")]
    GarageFull,
    #[error("no offer has been made for this car")]
    NoOffer,
}

/// All static career tables from `FEData`.
pub struct CareerTables {
    pub tracks: Vec<TrackInfo>,
    pub tournaments: Vec<Tournament>,
    pub parts: Vec<career::Part>,
    pub personalities: Vec<Personality>,
    pub missions: Vec<career::Mission>,
    pub strings: Strings,
    pub catalog: Vec<CarCatalogEntry>,
    /// Raw record images of the tables the original's save game embeds verbatim
    /// (`RE/notes/savegame.md` §4): `nfs5.car`, `nfs5.trn`, `nfs5.fac`, `nfs5.prt`.
    pub raw: RawTables,
}

/// The shipped tables as bytes, kept because the `.sav` stores memory images of them.
#[derive(Debug, Clone, Default)]
pub struct RawTables {
    pub car: Vec<u8>,
    pub trn: Vec<u8>,
    pub fac: Vec<u8>,
    pub prt: Vec<u8>,
}

impl CareerTables {
    pub fn load(paths: &GamePaths) -> Result<Self, CareerError> {
        let dir = paths.fe_data().join("Data");
        let read = |name: &'static str| -> Result<Vec<u8>, CareerError> {
            let path = dir.join(name);
            std::fs::read(&path).map_err(|source| CareerError::Io { path, source })
        };
        let tbl = |file: &'static str| move |source| CareerError::Table { file, source };
        let strings_path = paths.fe_data().join("Locale").join("FEStrings.csv");
        let strings = Strings::parse(&std::fs::read(&strings_path).map_err(|source| CareerError::Io { path: strings_path, source })?, 0);
        let raw = RawTables { car: read("nfs5.car")?, trn: read("nfs5.trn")?, fac: read("nfs5.fac")?, prt: read("nfs5.prt")? };
        Ok(Self {
            tracks: career::parse_track_table(&read("nfs5.trk")?).map_err(tbl("nfs5.trk"))?,
            tournaments: career::parse_tournaments(&raw.trn).map_err(tbl("nfs5.trn"))?,
            parts: career::parse_parts(&raw.prt).map_err(tbl("nfs5.prt"))?,
            personalities: career::parse_personalities(&read("nfs5.per")?).map_err(tbl("nfs5.per"))?,
            missions: career::parse_missions(&raw.fac).map_err(tbl("nfs5.fac"))?,
            catalog: nfs_formats::fedata::parse_car_catalog(&raw.car).map_err(tbl("nfs5.car"))?,
            strings,
            raw,
        })
    }

    pub fn track(&self, id: i16) -> Option<&TrackInfo> {
        self.tracks.iter().find(|t| t.id as i16 == id)
    }
    pub fn tournament(&self, id: i16) -> Option<&Tournament> {
        self.tournaments.iter().find(|t| t.id == id)
    }
    pub fn car(&self, id: u16) -> Option<&CarCatalogEntry> {
        self.catalog.iter().find(|c| c.id == id)
    }
    /// Driver name for an `nfs5.per` id (tournament opponent records use ids 31..37).
    pub fn driver_name(&self, per_id: i16) -> String {
        self.personalities.iter().find(|p| p.id == per_id).map(|p| self.strings.text(p.name_string_id)).unwrap_or_else(|| format!("Driver {per_id}"))
    }
    pub fn tournament_title(&self, t: &Tournament) -> String {
        self.strings.text(t.title_string_id)
    }
    pub fn track_name(&self, id: i16) -> String {
        self.track(id).map(|t| self.strings.text(t.name_string_id)).unwrap_or_else(|| format!("track {id}"))
    }
    /// Era tournaments (Evolution mode) of `era`, visible ones first, hidden finale last.
    pub fn era_tournaments(&self, era: Era) -> Vec<&Tournament> {
        let mut v: Vec<&Tournament> = self.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(era)).collect();
        v.sort_by_key(|t| (t.is_hidden(), t.id));
        v
    }

    /// Club events (Tournament mode), ordered by era then id.
    ///
    /// The original's list accessor `Tournament.Tournament` (0x504D00) filters the 35 `nfs5.trn`
    /// records by `rec+0x04 == 524 + Tournament.EventType` — the club era string ids — and
    /// `rec+0xE4 != 2` (the hidden era finales). **There is no career-progress condition**: the
    /// era the player is up to only reaches the accessor in Evolution mode, where it selects
    /// `[0x691E50]` instead. So every club era is listed from the start. CONFIRMED (H28).
    /// The club tournaments of **one** era — the three lists the jump table at 0x505450 picks
    /// between for `Tournament.EventType` 1, 2 and 3 (`controls.md` §4.105).
    pub fn club_tournaments_of(&self, era: Era) -> Vec<&Tournament> {
        self.club_tournaments().into_iter().filter(|t| t.era() == era).collect()
    }

    pub fn club_tournaments(&self) -> Vec<&Tournament> {
        let mut v: Vec<&Tournament> = self.tournaments.iter().filter(|t| matches!(t.kind, TournamentKind::Club(_)) && !t.is_hidden()).collect();
        v.sort_by_key(|t| (t.era(), t.id));
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnedCar {
    pub car_id: u16,
    /// The exterior: the paint set whose colour 0 is `FECar+0x674` (`OwnedCar.ExtColour`).
    pub paint_set: u8,
    /// The other four appearance dwords the paint shop edits (`savegame.md` §4.7a).
    #[serde(default)]
    pub trim: Trim,
    /// Installed `nfs5.prt` ids (one per part type / type name).
    #[serde(default)]
    pub parts: Vec<i16>,
    /// `car+0x698`: km driven in career races (+3000 per race).
    #[serde(default)]
    pub mileage: u32,
    /// `car+0x69C`: career wins.
    #[serde(default)]
    pub wins: u32,
    /// Body damage in per cent — the mean of `damage`, the figure the value formula and the
    /// garage screens use.
    #[serde(default)]
    pub damage_pct: f32,
    /// The ten body zones as the race left them (`car+0x4E4…`), 0..100 each.
    #[serde(default)]
    pub damage: [f32; 10],
    /// Per installed part, the damage it carries from earlier races — the float the original keeps
    /// at `part+0x164` and writes into the save (`savegame.md` §4.7). A part-damage getter adds it
    /// to the live zone damage (`physics.md` §10).
    #[serde(default)]
    pub part_state: Vec<(i16, f32)>,
    /// The car's setup (`carsetup.lay`), in `CAR_SETUP_NAMES` order — the 17 floats the original
    /// keeps at `FECar+0x6D0` with a backup at `+0x714` (`layouts.md`). Nothing in the game reads
    /// them back, so they change nothing; the port keeps them so the screen and the save match.
    #[serde(default = "default_setup")]
    pub setup: [f32; 17],
    /// `car+0x6A4`: what was paid (0 for prize cars).
    #[serde(default)]
    pub purchase_price: u32,
    /// `car+0x68C` / `+0x691`: asking price while the car is listed on the used-car market.
    #[serde(default)]
    pub asking_price: Option<u32>,
    /// `car+0x694` with `+0x692 = 1`: the standing offer for a listed car.
    #[serde(default)]
    pub offer: Option<u32>,
}

/// A garage car's interior, stripes and package, as list indices into its `nfs5.car` record — the
/// rows `OwnedCar.IntColour` (`rec+0x530 + 16·i`), `.S1Colour` (`rec+0x3F0 + 16·i`), `.S2Colour`
/// (`rec+0x3F4 + 16·i`) offer and the `OwnedCar.PackageNum` index (0 "none" … 7). The original
/// keeps the chosen colours themselves in `FECar+0x678…+0x684`; the port keeps the rows and the
/// save writes the colours (`save::car_image`). Row 0 everywhere is what a bought car starts with
/// (0x501CF0 with no interior chosen: interior 0, paint set 0's stripes, package 0).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Trim {
    pub interior: u8,
    pub stripe1: u8,
    pub stripe2: u8,
    pub package: u8,
}

impl OwnedCar {
    pub fn new(car_id: u16, paint_set: u8, purchase_price: u32) -> Self {
        Self { car_id, paint_set, trim: Trim::default(), parts: Vec::new(), mileage: 0, wins: 0, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, purchase_price, asking_price: None, offer: None }
    }
}

/// `0x4E3160(count, era)` — the knockout series' driver draw (`modes.md` §5.2 / read 2026-09-17,
/// CONFIRMED). The pool is every `nfs5.per` record whose `era` matches the selection (or all of them
/// when it is −1), and each opponent takes one **without replacement**, so no two share a driver.
/// The original draws with the CRT `rand()` seeded from the clock, not the game's LCG, so any source
/// reproduces it equally well.
pub fn pick_personalities(personalities: &[Personality], era: Option<u32>, count: usize, mut rand: impl FnMut(usize) -> usize) -> Vec<i16> {
    let mut pool: Vec<usize> = personalities.iter().enumerate().filter(|(_, p)| era.is_none_or(|e| p.era == e)).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for _ in 0..count {
        if pool.is_empty() {
            break;
        }
        let k = rand(pool.len());
        out.push(personalities[pool[k]].id);
        pool.remove(k);
    }
    out
}

/// Chronicle years (`0x5B8670`): the year steps to the next entry on each first tournament completion.
pub const YEAR_TABLE: [u16; 17] = [1950, 1956, 1960, 1965, 1969, 1970, 1973, 1975, 1978, 1982, 1988, 1989, 1993, 1996, 1997, 1999, 2000];

/// Part family key (`nfs5.prt +8`) for a catalogue car, from its family text and model year.
/// MEDIUM: the original resolves it through the car-info record; the mapping below reproduces the
/// families present in `nfs5.prt` (`356`, `356A`, `356B`, `550A`, `911`, `911 (930)`, `911 (964)`,
/// `911 (993)`, `911 (996)`, `914`, `935`, `944`, `Boxster`, `GT1`).
pub fn part_family(car: &CarCatalogEntry) -> Option<&'static str> {
    let f = car.family.as_str();
    let y = car.year;
    Some(match () {
        _ if f.starts_with("356 B") => "356B",
        _ if f.starts_with("356 A") => "356A",
        _ if f.starts_with("356") => "356",
        _ if f.starts_with("550") => "550A",
        _ if f.starts_with("914") => "914",
        _ if f.starts_with("935") => "935",
        _ if f.starts_with("944") => "944",
        _ if f.starts_with("Boxster") => "Boxster",
        _ if f == "911 GT1" => "GT1",
        _ if f.starts_with("911") && y >= 1999 => "911 (996)",
        _ if f.starts_with("911") && y >= 1994 => "911 (993)",
        _ if f.starts_with("911") && y >= 1989 => "911 (964)",
        _ if f.starts_with("911 Turbo") && y >= 1975 => "911 (930)",
        _ if f.starts_with("911") => "911",
        _ => return None,
    })
}

/// Damage-zone masks from the standard parts of the car's family (`part+0x48[10]`): the first
/// part of each type whose mask is non-zero. Falls back to the defaults per type.
pub fn zone_masks_for(tables: &CareerTables, car: &CarCatalogEntry) -> nfs_physics::specs::ZoneMasks {
    let mut m = nfs_physics::specs::ZoneMasks::default();
    let Some(fam) = part_family(car) else { return m };
    let pick = |kinds: &[u16]| -> Option<[bool; 10]> {
        tables.parts.iter().find(|p| p.family == fam && kinds.contains(&p.kind) && p.zone_mask.iter().any(|b| *b != 0)).map(|p| p.zone_mask.map(|b| b != 0))
    };
    if let Some(z) = pick(&[1]) { m.engine = z; }
    if let Some(z) = pick(&[2]) { m.brakes = z; }
    if let Some(z) = pick(&[3]) { m.gearbox = z; }
    if let Some(z) = pick(&[4, 5, 8]) { m.suspension = z; }
    if let Some(z) = pick(&[6]) { m.aero = z; }
    if let Some(z) = pick(&[9]) { m.tyres = z; }
    m
}

/// The car-setup values in the order the setup screen looks them up (0x4F1097…0x4F1311) and the
/// order they sit in at `FECar+0x6D0`. CONFIRMED.
///
/// The eight `GearN` are **not** eight gears. Race setup 0x4EF272…0x4EF32F pushes them into the
/// named registry parameters, and the names say what they are: `Gear0` is `.REVERSERATIO`,
/// `Gear1 … Gear6` are `.FIRST … .SIXTHRATIO`, and **`Gear7` is `.FINALRATIO`** — the final drive,
/// which is why its range is the full ±0.5 again. The neutral slot has no slider.
/// `RE/notes/physics.md` §3a has the whole path and what each one does to the block.
pub const CAR_SETUP_NAMES: [&str; 17] = [
    "FDownForce",
    "RDownForce",
    "BrakeBias",
    "RideHeight",
    "Suspension",
    "SteeringToe",
    "ShockRate",
    "FTirePressure",
    "RTirePressure",
    "Gear0",
    "Gear1",
    "Gear2",
    "Gear3",
    "Gear4",
    "Gear5",
    "Gear6",
    "Gear7",
];

/// How each of those 17 is **registered** as a front-end accessor, and so how it is shown and
/// stepped: minimum, maximum, step and decimal places, from the seventeen
/// `FEFloatRangeAccessor(name, min, max, step, decimals)` calls at 0x4F29CC…0x4F2D49. CONFIRMED,
/// in the order of [`CAR_SETUP_NAMES`].
///
/// | accessor | min | max | step | dec | registered at |
/// |---|---|---|---|---|---|
/// | `FDownForce` | 0 | 100 | 1 | 0 | 0x4F29CC |
/// | `RDownForce` | 0 | 100 | 1 | 0 | 0x4F2A01 |
/// | `BrakeBias` | 30 | 70 | 1 | 2 | 0x4F2A39 |
/// | `RideHeight` | −20 | 20 | 1 | 2 | 0x4F2A71 |
/// | `Suspension` | −30 | 30 | 1 | 2 | 0x4F2AA9 |
/// | `SteeringToe` | −5 | 5 | 0.1 | 2 | 0x4F2AE1 |
/// | `ShockRate` | 0.7 | 1.5 | 0.01 | 2 | 0x4F2B19 |
/// | `FTirePressure` | 25 | 45 | 1 | 0 | 0x4F2B51 |
/// | `RTirePressure` | 25 | 45 | 1 | 0 | 0x4F2B89 |
/// | `Gear0` | −0.5 | 0.5 | 0.05 | 2 | 0x4F2BC1 |
/// | `Gear1` | −0.5 | 0.5 | 0.05 | 2 | 0x4F2BF9 |
/// | `Gear2` | −0.45 | 0.45 | 0.05 | 2 | 0x4F2C31 |
/// | `Gear3` | −0.4 | 0.405 | 0.05 | 2 | 0x4F2C69 |
/// | `Gear4` | −0.35 | 0.35 | 0.05 | 2 | 0x4F2CA1 |
/// | `Gear5` | −0.3 | 0.3 | 0.05 | 2 | 0x4F2CD9 |
/// | `Gear6` | −0.25 | 0.25 | 0.05 | 2 | 0x4F2D11 |
/// | `Gear7` | −0.5 | 0.5 | 0.05 | 2 | 0x4F2D49 |
///
/// `Gear3`'s maximum really is 0.405 in the image (0x3ECF5C29), where every other gear's pair is
/// symmetric — left as read.
pub const CAR_SETUP_RANGE: [(f32, f32, f32, usize); 17] = [
    (0.0, 100.0, 1.0, 0),
    (0.0, 100.0, 1.0, 0),
    (30.0, 70.0, 1.0, 2),
    (-20.0, 20.0, 1.0, 2),
    (-30.0, 30.0, 1.0, 2),
    (-5.0, 5.0, 0.1, 2),
    (0.7, 1.5, 0.01, 2),
    (25.0, 45.0, 1.0, 0),
    (25.0, 45.0, 1.0, 0),
    (-0.5, 0.5, 0.05, 2),
    (-0.5, 0.5, 0.05, 2),
    (-0.45, 0.45, 0.05, 2),
    (-0.4, 0.405, 0.05, 2),
    (-0.35, 0.35, 0.05, 2),
    (-0.3, 0.3, 0.05, 2),
    (-0.25, 0.25, 0.05, 2),
    (-0.5, 0.5, 0.05, 2),
];

/// What the car-setup screen **shows** for one of the 17, from the stored float.
///
/// The value is not shown as it is stored: it goes through the accessor's index, which quantises
/// it to the registered step.
///
/// * `FEFloatRangeAccessor::setValue` **0x4B9C10** — `index = (int)((v − min) / step)`, with
///   `fistp` rounding to nearest;
/// * the index setter **0x4BC240** clamps it to `0 ..= count − 1`, and the ctor **0x4B9A40** built
///   `count = (int)((max − min) / step + 0.5)` — so the registered maximum is one step out of
///   reach, which is the original's own arithmetic and is left as read;
/// * `FEFloatRangeAccessor::text` **0x4B9AB0** — `v = index · step + min`, then a zero snap
///   (`−1e−4 < v < 1e−7` becomes `0.0`, 0x4B9AE3…0x4B9AFF against [0x5B4FB0] and [0x5B4FB4]),
///   then `sprintf` with `"%1.0f"`, `"%1.1f"`, `"%1.2f"` or `"%1.3f"` by the registered decimals
///   (the jump table at 0x4B9C00).
///
/// So a default car reads `50.00` for its brake bias and `0.00` for every gear, not `50` and `0`.
pub fn car_setup_text(i: usize, v: f32) -> String {
    let (min, _, step, dec) = CAR_SETUP_RANGE[i];
    let (index, _) = car_setup_index(i, v);
    let out = index as f32 * step + min;
    let out = if out > -1e-4 && out < 1e-7 { 0.0 } else { out };
    format!("{out:.dec$}")
}

/// The accessor's index for one of the 17, and how many entries its list has — the pair the
/// slider's knob is placed from. `count` is the ctor's `(int)((max − min) / step + 0.5)`
/// (0x4B9A40) and `index` is 0x4B9C10's rounding, clamped to `0 ..= count − 1` by 0x4BC240.
pub fn car_setup_index(i: usize, v: f32) -> (i32, i32) {
    let (min, max, step, _) = CAR_SETUP_RANGE[i];
    let count = ((max - min) / step + 0.5) as i32;
    let index = ((v - min) / step).round() as i32;
    (index.clamp(0, (count - 1).max(0)), count)
}

/// What `FECar`'s constructor writes into that block (0x4EED2D…0x4EED80). CONFIRMED.
pub const CAR_SETUP_DEFAULT: [f32; 17] =
    [50.0, 50.0, 50.0, 0.0, 0.0, 0.0, 1.0, 30.0, 30.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

fn default_setup() -> [f32; 17] {
    CAR_SETUP_DEFAULT
}

/// The part types each damage getter sums over (`physics.md` §10): 1 engine, 2 brakes,
/// 3 gearbox, 4/5/8 suspension, 6 aero, 9 tyres.
const WEAR_KINDS: [(&str, &[u16]); 6] =
    [("engine", &[1]), ("brakes", &[2]), ("gearbox", &[3]), ("suspension", &[4, 5, 8]), ("aero", &[6]), ("tyres", &[9])];

/// The scale and offset each part-damage getter applies (`PartWear`): over the car's installed
/// parts of the group's types, `scale = Σ (1.5 − 0.005·condition)` and `offset = Σ state` — the
/// damage those parts carry from earlier races. A group with nothing installed keeps the default
/// (one standard part, scale 1.5). The getter's formula is from `physics.md` §10; that `+0x164`
/// is the carried state is CONFIRMED by `Car_raceInit` 0x41085F, a twenty-iteration loop that
/// copies `carInfo+0x78 … +0xC4` into `[car+0x768 + 4i]->+0x164` — the save keeps the same field.
pub fn part_wear_for(tables: &CareerTables, owned: &OwnedCar) -> nfs_physics::specs::PartWearSet {
    let mut out = nfs_physics::specs::PartWearSet::default();
    for (group, kinds) in WEAR_KINDS {
        let parts: Vec<&Part> = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .filter(|p| kinds.contains(&p.kind))
            .collect();
        if parts.is_empty() {
            continue;
        }
        let w = nfs_physics::specs::PartWear {
            scale: parts.iter().map(|p| 1.5 - 0.005 * p.condition).sum(),
            offset: parts.iter().map(|p| owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)).sum(),
            count: parts.len() as u32,
        };
        match group {
            "engine" => out.engine = w,
            "brakes" => out.brakes = w,
            "gearbox" => out.gearbox = w,
            "suspension" => out.suspension = w,
            "aero" => out.aero = w,
            _ => out.tyres = w,
        }
    }
    out
}

/// Where the purchased parts used to be applied. They are applied to the **`carSpecs` block**
/// now, by `CarSpecs::apply_parts` (`RE/notes/loaders.md` §1.3) — which is where 0x49CB50 does it,
/// and which is the only place the `+0xC0` / `+0xC4` factors of the brake, spring, shock, sway-bar,
/// aero and tyre arms can be applied at all: their targets are block fields with no `.sim`
/// counterpart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventResult {
    /// Player's finishing position, 1-based.
    pub position: u8,
    pub prize: u32,
    pub bonus: u32,
    /// Points per driver: index 0 = player, 1..=7 = tournament opponents in table order.
    pub points: [u8; 8],
    /// Prize + bonus per driver (standings tie-break).
    #[serde(default)]
    pub earned: [u32; 8],
    /// Finishing time per driver in 64 Hz ticks (0 = did not finish).
    #[serde(default)]
    pub times: [u64; 8],
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TournamentState {
    /// Fee paid for the current attempt.
    pub entered: bool,
    pub events: Vec<EventResult>,
    /// Completed (rank ≤ 3) at least once.
    pub completed: bool,
    /// How many times, which is what `trn+0xD3C` counts and what
    /// `Tournament.Tournament(TIMESCOMPLETED)` prints (0x506929).
    #[serde(default)]
    pub completions: u32,
    pub final_rank: Option<u8>,
    pub prize: u32,
    /// Attempts that ended with rank > 3 (tournament reset).
    #[serde(default)]
    pub failed_attempts: u32,
}

/// One-off era finale offered after an era completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FinalePending {
    pub era: Era,
    /// Tournament id of the finale record (31 / 32 / 33).
    pub tournament: i16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Career {
    pub version: u32,
    pub era: Era,
    /// Chronicle year (`0x5DB764`).
    #[serde(default = "default_year")]
    pub year: u16,
    pub money: i64,
    pub garage: Vec<OwnedCar>,
    pub tournaments: BTreeMap<i16, TournamentState>,
    /// Eras whose finale was won (prize cars awarded).
    pub eras_completed: Vec<Era>,
    /// Finale waiting to be raced (or declined).
    #[serde(default)]
    pub finale: Option<FinalePending>,
    /// Evolution finished (Modern era completed, `0x691E54`).
    #[serde(default)]
    pub evolution_complete: bool,
    /// Factory Driver: result per mission (`fac+0xE8`, 1 = passed), indexed like `nfs5.fac`.
    #[serde(default)]
    pub missions: Vec<i16>,
    /// Stages whose promotion prize car was accepted (`fac+0xC = −1`).
    #[serde(default)]
    pub promotions_accepted: Vec<i16>,
    /// The used-car market — the vector at `[0x691B88]`, which the original saves as its
    /// `UsedCarMarket` module (`savegame.md` §4.4). The player's own listings stay in the garage
    /// with an `asking_price`; these are the cars the game generates.
    #[serde(default)]
    pub market: Vec<OwnedCar>,
}

fn default_year() -> u16 {
    1950
}

impl Default for Career {
    fn default() -> Self {
        Self { version: SAVE_VERSION, era: Era::Classic, year: 1950, money: STARTING_MONEY, garage: Vec::new(), tournaments: BTreeMap::new(), eras_completed: Vec::new(), finale: None, evolution_complete: false, missions: Vec::new(), promotions_accepted: Vec::new(), market: Vec::new() }
    }
}

/// Availability of one tournament for the current career state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Open,
    /// Era finale record: never listed as a tournament.
    Locked,
    Completed,
}

/// Result of a recorded event, for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct EventOutcome {
    pub result: EventResult,
    /// Set when this event finished the tournament.
    pub final_rank: Option<u8>,
    pub tournament_prize: u32,
    /// Rank > 3: the tournament was reset.
    pub reset: bool,
    pub era_advanced: bool,
}

impl Career {
    /// Where the career lives. `NFS5_SAVE_DIR` moves it, the same way it moves `SaveData`
    /// ([`crate::save::save_dir`]) — without that, a headless sweep or a test run writes into the
    /// player's own career, and since the front end's bottom bar shows the money on every screen,
    /// that silently moves all 97 screenshot goldens (found 2026-09-23 by doing exactly that).
    pub fn default_save_path() -> PathBuf {
        if std::env::var_os("NFS5_SAVE_DIR").is_some() {
            return crate::save::save_dir().join("career.json");
        }
        let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("Library").join("Application Support").join("PorscheUnleashed").join("career.json")
    }

    pub fn load(path: &Path) -> Result<Self, CareerError> {
        let data = std::fs::read(path).map_err(|source| CareerError::Io { path: path.to_path_buf(), source })?;
        let c: Career = serde_json::from_slice(&data).map_err(|source| CareerError::Json { path: path.to_path_buf(), source })?;
        if c.version > SAVE_VERSION {
            return Err(CareerError::SaveVersion { found: c.version, supported: SAVE_VERSION });
        }
        Ok(c)
    }

    pub fn save(&self, path: &Path) -> Result<(), CareerError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| CareerError::Io { path: dir.to_path_buf(), source })?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(|source| CareerError::Json { path: path.to_path_buf(), source })?;
        std::fs::write(path, data).map_err(|source| CareerError::Io { path: path.to_path_buf(), source })
    }

    pub fn state(&self, id: i16) -> TournamentState {
        self.tournaments.get(&id).cloned().unwrap_or_default()
    }

    pub fn availability(&self, _tables: &CareerTables, t: &Tournament) -> Availability {
        if t.is_hidden() {
            return Availability::Locked;
        }
        if self.state(t.id).completed {
            return Availability::Completed;
        }
        Availability::Open
    }

    /// Owned cars eligible for `t` (garage indices).
    pub fn eligible_garage(&self, t: &Tournament) -> Vec<usize> {
        self.garage.iter().enumerate().filter(|(_, c)| t.eligible_cars.contains(&(c.car_id as i16))).map(|(i, _)| i).collect()
    }

    /// Catalogue cars eligible for `t` that the dealer lists this year (0x4F4240).
    pub fn eligible_catalog<'a>(&self, tables: &'a CareerTables, t: &Tournament) -> Vec<&'a CarCatalogEntry> {
        t.eligible_cars.iter().filter_map(|id| tables.car(*id as u16)).filter(|c| !c.sim.is_empty() && self.dealer_lists(c)).collect()
    }

    /// Dealer availability: model years covering the chronicle year and the record flags.
    pub fn dealer_lists(&self, c: &CarCatalogEntry) -> bool {
        c.year <= self.year && self.year <= c.year_end.max(c.year) && !matches!(c.dealer_flag_204, 2 | 3) && c.kind == 0
    }

    pub fn dealer_price(&self, c: &CarCatalogEntry) -> u32 {
        c.dealer_price(self.year)
    }

    /// **0x501CF0**, the dealership's `BUYCAR`: the price (0x4EE220) against the money, the garage
    /// against 80, then the new car — its trim the rows chosen at the dealer — and the garage
    /// sorted (0x4F0EE0).
    pub fn buy_car(&mut self, tables: &CareerTables, car_id: u16, paint_set: u8, trim: Trim) -> Result<(), CareerError> {
        let car = tables.car(car_id).ok_or(CareerError::UnknownCar { id: car_id })?;
        let price = self.dealer_price(car) as i64;
        if self.money < price {
            return Err(CareerError::NoMoney { need: price, have: self.money });
        }
        if self.garage.len() >= MAX_OWNED_CARS {
            return Err(CareerError::GarageFull);
        }
        self.money -= price;
        let mut car = OwnedCar::new(car_id, paint_set, price as u32);
        car.trim = trim;
        self.garage.push(car);
        self.sort_garage(tables);
        Ok(())
    }

    /// **0x4F0EE0** — the garage's order after a purchase (0x501ED3, 0x502161) or a sale
    /// (0x502255): for each `i`, every later `j` that 0x4EF560 puts first swaps with it. 0x4EF560
    /// is 0x4F1D40 on the two FE cars' records (`+4`): era `+0x1B0`, then class `+0x1AC`, then
    /// first year `+0x1F8`, each ascending. A prize car (0x502270) is appended unsorted.
    pub fn sort_garage(&mut self, tables: &CareerTables) {
        let key = |c: &OwnedCar| tables.car(c.car_id).map(|e| (e.era_string_id as i32, e.class_string_id as i32, e.year as i16));
        for i in 0..self.garage.len() {
            for j in i + 1..self.garage.len() {
                if key(&self.garage[j]) < key(&self.garage[i]) {
                    self.garage.swap(i, j);
                }
            }
        }
    }

    /// Current value of garage car `idx` (`0x4F8E50`, CONFIRMED — see [`Career::value_of`]).
    pub fn car_value(&self, tables: &CareerTables, idx: usize) -> Option<i64> {
        self.value_of(tables, self.garage.get(idx)?)
    }

    /// The same figure for any car, garage or market — `0x4F8E50`, read 2026-09-23, **all
    /// integer** after the first step:
    ///
    /// ```text
    /// base  = _ftol(price · curve[year − 1950] · 0.01)                ; 0x4F8E60…0x4F8E73, truncates
    /// m     = (pct_m · base) / −100   pct_m by mileage, 0x5B7C08       ; 0x4F8E9A…0x4F8EB1
    /// d     = (pct_d · base) /  100   pct_d by _ftol(damage %), 0x5B7C48 ; 0x4F8EEE…0x4F8F05
    /// w     = (wins · base) / 100                                      ; 0x4F8F14…0x4F8F3B
    /// value = base + m + d + w + Σ part value(installed) − Σ part value(spares)
    /// value < base · 0.05f → _ftol(base · 0.05f)                       ; 0x4F8FCE…0x4F8FF1
    /// ```
    ///
    /// Every division is C's, truncating toward zero. **The mileage arm divides by −100** — its
    /// magic is `0xAE147AE1`, the negation of the damage arm's `0x51EB851F` — while the table
    /// holds 0, −10, −15, −25, −30, so mileage *raises* the value by 10…30 %. That is the
    /// original's arithmetic and it is kept. Each table matches `lo <= x <= hi` over integer
    /// bands and a value outside every band adds nothing. The spare-parts list `car+0x75C` is
    /// not ported (the port keeps no spares), so nothing is subtracted for it.
    pub fn value_of(&self, tables: &CareerTables, owned: &OwnedCar) -> Option<i64> {
        let car = tables.car(owned.car_id)?;
        let parts: i64 = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .map(|p| part_resale(p, self.year, owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)))
            .sum();
        Some(car_value_from(car.price, car.value_pct(self.year), owned.mileage, owned.damage_pct, owned.wins, parts))
    }

    /// **The used-car market.** `0x4F80C0`, run by `0x4F7DB0(year)` after every career race and by
    /// the refresh cheat, walks the whole catalogue `[0x6919F0]` and offers each model with a
    /// probability that depends on its age:
    ///
    /// ```text
    /// 0x4F80FE  skip when record+0x1F8 (model year) >= year - 1     ; nothing under two years old
    /// 0x4F8114  skip when record+0x204 is 2 or 3
    /// 0x4F812C  roll = rand() % 60
    /// 0x4F813A  skip when roll < USED_CAR_RARITY[year - model year]  ; `jb`
    /// 0x4F8153  otherwise generate a car and append it to the market [0x691B88]
    /// 0x4F8171  but first, once the market holds 25, drop the first entry that is not the
    ///           player's own listing (car+0x6A0 == 0)
    /// ```
    ///
    /// The comparison is `jb`, so the table entry is the chance of **not** being offered, in
    /// sixtieths: a seven-to-ten-year-old Porsche (70, 80, 80, 70) never reaches the market and
    /// anything sixteen years old or more (5) nearly always does.
    pub fn refresh_market(&mut self, tables: &CareerTables, rand: &mut impl FnMut() -> u32) {
        for record in &tables.catalog {
            // `[0x6919F0]` is not the raw table: 0x4F3E40 builds it from the records whose
            // `+0x1F4` is **0** — the playable cars, as against the 1 and 2 that go to
            // `[0x6919FC]` and `[0x6919F8]` — and only those whose model file is on disk
            // (0x4F40E6's `sprintf` and 0x4F40F0's open). Without that filter the market fills
            // with `Eden2` and `EAS7`, the scenery records.
            if record.kind != 0 || record.sim.is_empty() {
                continue;
            }
            if record.year as i64 >= self.year as i64 - 1 || matches!(record.dealer_flag_204, 2 | 3) {
                continue;
            }
            let age = (self.year as i64 - record.year as i64).clamp(0, USED_CAR_RARITY.len() as i64 - 1) as usize;
            if rand() % 60 < USED_CAR_RARITY[age] {
                continue;
            }
            let car = self.generate_used_car(tables, record, rand);
            // 0x4F817A walks for the first entry whose `car+0x6A0` is 0 — a car the game put
            // there rather than one the player listed. This port keeps the player's listings in
            // the garage, so every entry here qualifies and the oldest goes.
            if self.market.len() >= USED_CAR_MARKET_MAX {
                self.market.remove(0);
            }
            self.market.push(car);
        }
    }

    /// `0x4F7E41`: every market car is re-valued before the market is topped up, so a car that
    /// has sat there through a year change is offered at its new price.
    pub fn reprice_market(&mut self, tables: &CareerTables) {
        for i in 0..self.market.len() {
            if let Some(v) = self.value_of(tables, &self.market[i]) {
                self.market[i].asking_price = Some(v.max(0) as u32);
            }
        }
    }

    /// One market car — `0x4F82C0(record)`. A random paint set, a random mileage and win count from
    /// their weighted tables, a share of the parts that fit it, and the asking price its own value
    /// comes to. The original also rolls a decal (a one-in-two chance of `rand() % decalCount`) and
    /// a paint package (`rand() % 100 - 1` about one time in five) and picks an interior colour
    /// separately from the exterior; this port keeps one paint index per car, so those three are
    /// read and not carried.
    ///
    /// Nothing here damages the car: the generator never touches the body zones, and every
    /// `nfs5.prt` record ships `+0x164 = 0`, so a market car's parts are undamaged and its
    /// condition bars read 100. Its price is discounted by mileage alone.
    pub fn generate_used_car(&self, tables: &CareerTables, record: &CarCatalogEntry, rand: &mut impl FnMut() -> u32) -> OwnedCar {
        let mut car = OwnedCar::new(record.id, 0, 0);
        if !record.paint_sets.is_empty() {
            car.paint_set = (rand() as usize % record.paint_sets.len()) as u8;
        }
        car.mileage = weighted_band(&USED_MILEAGE, rand).max(0) as u32;
        car.wins = weighted_band(&USED_WINS, rand).max(0) as u32;
        car.parts = self.generate_used_parts(tables, record, rand);
        car.part_state = car.parts.iter().map(|id| (*id, 0.0)).collect();
        car.asking_price = Some(self.value_of(tables, &car).unwrap_or(0).max(0) as u32);
        car
    }

    /// `0x4F8590(car)` — which parts a market car comes with. It clears the installed list, takes
    /// the parts that fit the car (`0x517D10`, this port's [`Career::parts_for`]), rolls a
    /// **percentage** from `USED_PART_PCT` and installs `pct × available / 100` of them, truncated
    /// (0x4F869E), drawn without replacement: `pick = rand() % remaining`, then
    /// `index[pick] = index[--remaining]` (0x4F89AC), whether the pick was kept or dropped.
    ///
    /// A pick is dropped when it duplicates one already installed — the same type name
    /// (`part+0x28`) **and** the same category (`part+0x26`) — or when both are engines: the four
    /// string compares at 0x4F877E, 0x4F87EA, 0x4F8856 and 0x4F88C2 reject every pairing of
    /// `"Engine"` and `"Engine (Standard)"`, so a car never leaves the market with two.
    fn generate_used_parts(&self, tables: &CareerTables, record: &CarCatalogEntry, rand: &mut impl FnMut() -> u32) -> Vec<i16> {
        let avail = self.parts_for(tables, record);
        if avail.is_empty() {
            return Vec::new();
        }
        let pct = weighted_band(&USED_PART_PCT, rand).max(0);
        let want = (pct as f64 * avail.len() as f64 * 0.01) as usize;
        let mut index: Vec<usize> = (0..avail.len()).collect();
        let mut remaining = avail.len();
        let mut out: Vec<i16> = Vec::new();
        let engine = |n: &str| n == "Engine" || n == "Engine (Standard)";
        for _ in 0..want {
            let pick = if remaining == 0 { 0 } else { rand() as usize % remaining };
            let cand = avail[index[pick]];
            let clash = out.iter().filter_map(|id| avail.iter().find(|p| p.id == *id)).any(|p| {
                (p.kind_name == cand.kind_name && p.kind == cand.kind) || (engine(&p.kind_name) && engine(&cand.kind_name))
            });
            if !clash {
                out.push(cand.id);
            }
            if remaining > 0 {
                remaining -= 1;
                index[pick] = index[remaining];
            }
        }
        out
    }

    /// `0x501F40`: buy market car `idx` at its asking price. The car joins the garage exactly as it
    /// stands, mileage, wins, parts and all.
    pub fn buy_used_car(&mut self, tables: &CareerTables, idx: usize) -> Result<(), CareerError> {
        let car = self.market.get(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        let price = car.asking_price.unwrap_or(0) as i64;
        if self.money < price {
            return Err(CareerError::NoMoney { need: price, have: self.money });
        }
        if self.garage.len() >= MAX_OWNED_CARS {
            return Err(CareerError::GarageFull);
        }
        let mut car = self.market.remove(idx);
        self.money -= price;
        car.purchase_price = price as u32;
        car.asking_price = None;
        car.offer = None;
        self.garage.push(car);
        self.sort_garage(tables);
        Ok(())
    }

    /// Put the damage a race left on the car away with it: the ten zones, their mean as the
    /// car's damage figure, and the share each installed part takes — `(1.5 − 0.005·condition) ×
    /// mean zone damage under its mask`, the term its getter contributes (`physics.md` §10). Parts
    /// keep what they had, so damage accumulates until the car is repaired.
    pub fn record_damage(&mut self, tables: &CareerTables, idx: usize, zones: [f32; 10]) {
        let Some(owned) = self.garage.get(idx) else { return };
        let ids = owned.parts.clone();
        let mut add: Vec<(i16, f32)> = Vec::new();
        for id in &ids {
            let Some(p) = tables.parts.iter().find(|p| p.id == *id) else { continue };
            let mask = p.zone_mask.map(|b| b != 0);
            let n = mask.iter().filter(|b| **b).count();
            if n == 0 {
                continue;
            }
            let mean: f32 = zones.iter().zip(mask.iter()).filter(|(_, m)| **m).map(|(d, _)| *d).sum::<f32>() / n as f32;
            add.push((*id, (1.5 - 0.005 * p.condition) * mean));
        }
        let Some(owned) = self.garage.get_mut(idx) else { return };
        owned.damage = zones;
        for (id, d) in add {
            match owned.part_state.iter_mut().find(|(pid, _)| *pid == id) {
                Some((_, s)) => *s += d,
                None => owned.part_state.push((id, d)),
            }
        }
        owned.damage_pct = car_damage_pct(tables, owned);
    }

    /// `part+0x164` — how damaged one installed part is, as the percentage the two damage lists
    /// print (`"%d %"` at 0x516EF6). A part the car has never raced keeps the table's own `state`.
    pub fn part_damage(&self, idx: usize, part: &career::Part) -> f32 {
        self.garage
            .get(idx)
            .and_then(|o| o.part_state.iter().find(|(id, _)| *id == part.id).map(|(_, s)| *s))
            .unwrap_or(part.state as f32)
    }

    /// What the `QUICKFIX` screen charges to repair one part category (`RepairCost` 0x518880,
    /// CONFIRMED): per installed part of that category, `aged price × (int)(2 × state) / 100`.
    pub fn repair_cost(&self, tables: &CareerTables, idx: usize, kinds: &[u16]) -> i64 {
        let Some(owned) = self.garage.get(idx) else { return 0 };
        owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .filter(|p| kinds.contains(&p.kind))
            .map(|p| {
                let state = owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32);
                part_aged_price(p, self.year) * (2.0 * state) as i64 / 100
            })
            .sum()
    }

    /// Pay for and carry out that repair: the parts of those categories go back to no damage, and
    /// so do the body zones they answer for.
    pub fn repair(&mut self, tables: &CareerTables, idx: usize, kinds: &[u16]) -> Result<i64, CareerError> {
        let cost = self.repair_cost(tables, idx, kinds);
        if cost > self.money {
            return Err(CareerError::NoMoney { need: cost, have: self.money });
        }
        let Some(owned) = self.garage.get(idx) else { return Err(CareerError::UnknownCar { id: 0 }) };
        let ids: Vec<i16> = owned
            .parts
            .iter()
            .filter(|id| tables.parts.iter().any(|p| p.id == **id && kinds.contains(&p.kind)))
            .copied()
            .collect();
        let masks: Vec<[bool; 10]> = ids
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .map(|p| p.zone_mask.map(|b| b != 0))
            .collect();
        let Some(owned) = self.garage.get_mut(idx) else { return Err(CareerError::UnknownCar { id: 0 }) };
        for id in &ids {
            match owned.part_state.iter_mut().find(|(pid, _)| pid == id) {
                Some((_, s)) => *s = 0.0,
                None => owned.part_state.push((*id, 0.0)),
            }
        }
        for m in &masks {
            for (z, on) in owned.damage.iter_mut().zip(m.iter()) {
                if *on {
                    *z = 0.0;
                }
            }
        }
        owned.damage_pct = car_damage_pct(tables, owned);
        self.money -= cost;
        Ok(cost)
    }

    /// `SELLCAR` / `0x4F91C0(car, ask)`: list garage car `idx` on the used-car market at `ask`
    /// (`car+0x68C`, `+0x691 = 1`); the car stays owned until an offer is accepted. The original
    /// refuses to list a car when fewer than two remain (0x50FCB0).
    pub fn list_for_sale(&mut self, idx: usize, ask: u32) -> Result<(), CareerError> {
        // 0x50FCB0: the owned cars (0x501AC0) **minus the ones already listed** (0x4F93C0, the
        // used-car list filtered on `+0x6A0`) must be at least 2 (`cmp esi, 2 / jge`), else
        // one-button `FEStrings` 60 (0x50FCF6 `push 0x3C`; the 0x2BE beside it is the "ok" label). A listed car stays in the garage, so with two cars one can be listed
        // and not the other. CONFIRMED.
        let listed = self.garage.iter().filter(|c| c.asking_price.is_some()).count();
        if self.garage.len().saturating_sub(listed) < 2 {
            return Err(CareerError::LastCar);
        }
        let car = self.garage.get_mut(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        car.asking_price = Some(ask);
        car.offer = None;
        Ok(())
    }

    /// `ID_REMOVEFMBUTTON`: take the car off the market.
    pub fn withdraw_from_sale(&mut self, idx: usize) {
        if let Some(c) = self.garage.get_mut(idx) {
            c.asking_price = None;
            c.offer = None;
        }
    }

    /// `0x502190(car)`: accept the standing offer — the money is paid and the car leaves the garage.
    pub fn accept_offer(&mut self, tables: &CareerTables, idx: usize) -> Result<i64, CareerError> {
        let car = self.garage.get(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        let offer = car.offer.ok_or(CareerError::NoOffer)? as i64;
        self.money = (self.money + offer).clamp(-999_999_999, 999_999_999);
        self.garage.remove(idx);
        self.sort_garage(tables);
        Ok(offer)
    }

    /// **0x4F9720(car)** — the "no" to an offer (807): the offer flag `+0x692` and amount `+0x694`
    /// are cleared; the car stays listed.
    pub fn decline_offer(&mut self, idx: usize) {
        if let Some(c) = self.garage.get_mut(idx) {
            c.offer = None;
        }
    }

    /// The first garage car with a standing offer — what 0x4F9570's list gives the pre-race
    /// screen's tick (0x505FCE), which asks about one at a time.
    pub fn pending_offer(&self) -> Option<usize> {
        self.garage.iter().position(|c| c.offer.is_some())
    }

    /// **0x503E40(id)** — forfeit: the tournament's event records are cleared
    /// (0x503E83…0x503E9B), the same as 0x504BD0 does to a part-way one, so it starts over and
    /// pays its fee again.
    pub fn forfeit(&mut self, id: i16) {
        if let Some(st) = self.tournaments.get_mut(&id) {
            st.events.clear();
            st.entered = false;
        }
    }

    /// `0x4F7E90`, run first by `0x4F7DB0` after every career race: every car on the used-car
    /// list — the player's listings **and** the market's own cars — is scored and may get an
    /// offer. Read 2026-09-23, CONFIRMED:
    ///
    /// ```text
    /// score = _ftol((value − ask)·100.0/value) − _ftol(damage %) + 100
    ///       + mileage_score(age, km) + wins/5                      ; 0x4F7ED8…0x4F7FC1
    /// bracket 0x5B7D88 → (pct, immediate); no bracket → immediate  ; 0x4F7FC3…0x4F7FE5
    /// immediate: offer = clamp(min(ask, value + _ftol((rand%601 − 300)·1e-4f·value)), ≥ 0)
    /// otherwise: rand%100 < pct → the player's car is offered its asking price,
    ///                             a market car is bought by someone else (`+0x690`)
    /// ```
    ///
    /// Nothing is skipped for already holding an offer: an immediate offer is re-drawn after every
    /// race, and a failed roll leaves the old one in place. `0x4F7DB0` then deletes every market
    /// car so bought (0x4F7DE0…0x4F7E3F) before re-pricing the rest. The original draws from the
    /// CRT `rand()` it has just seeded from the clock (0x5A17EB, 0x538110); `rand` stands in.
    pub fn generate_offers(&mut self, tables: &CareerTables, rand: &mut impl FnMut() -> u32) {
        let year = self.year;
        let decide = |this: &Self, car: &OwnedCar, rand: &mut dyn FnMut() -> u32| -> Option<Offer> {
            let ask = car.asking_price? as i64;
            let value = this.value_of(tables, car)?;
            if value <= 0 {
                return None;
            }
            let model_year = tables.car(car.car_id).map(|c| c.year).unwrap_or(year);
            let age = year as i64 - model_year as i64;
            let ratio = ((value - ask) as f64 * 100.0 / value as f64) as i64;
            let score = ratio - car.damage_pct as i64 + 100 + mileage_score(age, car.mileage) + (car.wins / 5) as i64;
            let bracket = OFFER_BRACKETS.iter().rev().find(|(lo, hi, _, _)| (*lo..=*hi).contains(&score));
            let (pct, immediate) = bracket.map_or((0, true), |(_, _, p, i)| (*p, *i));
            Some(if immediate {
                let jitter = (((rand() % 601) as f64 - 300.0) * 1e-4f32 as f64 * value as f64) as i64;
                Offer::Now(ask.min(value + jitter).max(0) as u32)
            } else if ((rand() % 100) as i64) < pct {
                Offer::Taken(ask as u32)
            } else {
                Offer::None
            })
        };
        for idx in 0..self.garage.len() {
            match decide(self, &self.garage[idx], rand) {
                Some(Offer::Now(o)) | Some(Offer::Taken(o)) => self.garage[idx].offer = Some(o),
                _ => {}
            }
        }
        let mut gone = Vec::new();
        for idx in 0..self.market.len() {
            if let Some(Offer::Taken(_)) = decide(self, &self.market[idx], rand) {
                gone.push(idx);
            }
        }
        for idx in gone.into_iter().rev() {
            self.market.remove(idx);
        }
    }

    /// Parts that fit `car`: same family, model year not after the chronicle year, newest year per
    /// part name (the tables carry per-year variants). Sorted by type then price.
    pub fn parts_for<'a>(&self, tables: &'a CareerTables, car: &CarCatalogEntry) -> Vec<&'a Part> {
        let Some(fam) = part_family(car) else { return Vec::new() };
        let mut v: Vec<&Part> = tables.parts.iter().filter(|p| p.family == fam && p.year <= self.year.max(car.year) && (1..=8).contains(&p.kind)).collect();
        v.sort_by(|a, b| a.name_string_id.cmp(&b.name_string_id).then(b.year.cmp(&a.year)));
        v.dedup_by(|a, b| a.name_string_id == b.name_string_id && a.kind == b.kind);
        v.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.kind_name.cmp(&b.kind_name)).then(a.price.cmp(&b.price)));
        v
    }

    /// Installed parts of garage car `idx`.
    pub fn installed_parts<'a>(&self, tables: &'a CareerTables, idx: usize) -> Vec<&'a Part> {
        self.garage.get(idx).map(|c| c.parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect()).unwrap_or_default()
    }

    /// What the parts shop charges for `p` — **`0x5181C0(part, year)`**, the aged price
    /// `price + price·Σ PART_AGE_PCT[0..year − part.year] / 100` in the chronicle year
    /// (0x5032C0). CONFIRMED: the shop list shows it for a part to buy (0x51590A) and the
    /// `Parts.ShoppingList(TOTAL)` accessor 0x516480 subtracts it per part to buy (0x516578) while
    /// adding `0x518220` per part to sell. The port used to charge the bare `part+0x5C`.
    pub fn part_buy_price(&self, p: &Part) -> i64 {
        part_aged_price(p, self.year)
    }

    /// Buy and install `part_id` on garage car `idx`; a part of the same type and type name is
    /// replaced (MEDIUM: the original keeps replaced parts as spares).
    pub fn buy_part(&mut self, tables: &CareerTables, idx: usize, part_id: i16) -> Result<(), CareerError> {
        let part = tables.parts.iter().find(|p| p.id == part_id).ok_or(CareerError::UnknownPart { id: part_id })?;
        let car = self.garage.get(idx).and_then(|c| tables.car(c.car_id)).ok_or(CareerError::UnknownCar { id: 0 })?;
        if part_family(car) != Some(part.family.as_str()) {
            return Err(CareerError::PartMismatch { id: part_id });
        }
        let price = self.part_buy_price(part);
        if self.money < price {
            return Err(CareerError::NoMoney { need: price, have: self.money });
        }
        self.money -= price;
        let owned = &mut self.garage[idx];
        owned.parts.retain(|id| tables.parts.iter().find(|p| p.id == *id).map(|p| !(p.kind == part.kind && p.kind_name == part.kind_name)).unwrap_or(false));
        owned.parts.push(part_id);
        Ok(())
    }

    /// **0x504B60** — is a tournament other than `except` part-way through: some of its events
    /// raced and not all (the count of event records with their done byte clear is `> 0` and
    /// `< events`, 0x504B8B…0x504BA8). The start check 0x4D5AB1 asks it before letting the player
    /// into another one (`FEStrings` 858).
    pub fn partial_elsewhere(&self, tables: &CareerTables, except: i16) -> bool {
        self.tournaments.iter().any(|(&id, st)| {
            id != except
                && tables.tournament(id).is_some_and(|t| !st.events.is_empty() && st.events.len() < t.events.len())
        })
    }

    /// **0x504BD0** — the "yes" to 858: every part-way tournament but `except` has its event
    /// records cleared (0x504C77…0x504C96), so it starts again from the first event and its fee.
    pub fn reset_partial_except(&mut self, tables: &CareerTables, except: i16) {
        for (&id, st) in self.tournaments.iter_mut() {
            if id != except && tables.tournament(id).is_some_and(|t| !st.events.is_empty() && st.events.len() < t.events.len()) {
                st.events.clear();
                st.entered = false;
            }
        }
    }

    /// Index of the next event to race in `t`, or `None` when all are done.
    pub fn next_event(&self, t: &Tournament) -> Option<usize> {
        let done = self.state(t.id).events.len();
        (done < t.events.len()).then_some(done)
    }

    /// Enter `t` and return the event index to race. The fee is paid at the start of the first
    /// event of an attempt (0x4D58FB).
    pub fn enter(&mut self, tables: &CareerTables, t: &Tournament) -> Result<usize, CareerError> {
        // club events are not gated by career progress — the list accessor 0x504D00 filters them by
        // the selected club era alone (H28, CONFIRMED) — while an Evolution tournament belongs to
        // the era the player is up to
        let era_ok = t.is_club() || t.era() == self.era;
        if self.availability(tables, t) != Availability::Open || !era_ok {
            return Err(CareerError::NotAvailable { id: t.id });
        }
        if self.eligible_garage(t).is_empty() {
            return Err(CareerError::NoEligibleCar { id: t.id });
        }
        let event = self.next_event(t).ok_or(CareerError::NoEvents { id: t.id })?;
        if event == 0 {
            let fee = t.entry_fee as i64;
            if self.money < fee {
                return Err(CareerError::NoMoney { need: fee, have: self.money });
            }
            self.money -= fee;
        }
        self.tournaments.entry(t.id).or_default().entered = true;
        Ok(event)
    }

    /// Record event `event` of `t`. `order` = finishing order of driver indices (0 = player,
    /// 1..=7 = tournament opponents in table order); `times[driver]` = finishing time in ticks
    /// (0 = did not finish). `clean[driver]` = the car took no new body damage in the race
    /// ([`clean_race`]) — the event bonus is paid only then. `garage_idx` = the player's car
    /// (mileage / wins).
    #[allow(clippy::too_many_arguments)]
    /// `0x503960` for every racer of one event: points `9 − pos`, prize by position, and the
    /// bonus for a clean top three. Returns the record the event is stored as.
    fn event_result(t: &Tournament, event: usize, order: &[usize], times: &[u64; 8], clean: &[bool; 8]) -> EventResult {
        let ev = &t.events[event];
        let mut points = [0u8; 8];
        let mut earned = [0u32; 8];
        let mut position = 8u8;
        for (pos0, &driver) in order.iter().enumerate() {
            let pos = (pos0 + 1).min(8) as u8;
            if driver < 8 {
                points[driver] = 9 - pos;
                // `0x503960(i, time, pos, ok)`: the bonus arm runs only when `ok`, i.e. the
                // result record's byte `+0x1EC` (`car+0xCC0`) is clear (0x5039D3 `test al, al /
                // je`). The fractions are `_ftol(bonus · 1 / 0.5 / 0.25)` (0x5039E9, 0x503A1E,
                // `[0x5B23F0] = 0.5`, `[0x5B2468] = 0.25`), which truncates like `/ 2` and `/ 4`.
                let bonus = if !clean[driver] {
                    0
                } else {
                    match pos {
                        1 => ev.bonus,
                        2 => ev.bonus / 2,
                        3 => ev.bonus / 4,
                        _ => 0,
                    }
                };
                earned[driver] = ev.prize[pos as usize - 1] + bonus;
            }
            if driver == 0 {
                position = pos;
            }
        }
        let prize = ev.prize[position as usize - 1];
        let bonus = earned[0] - prize;
        EventResult { position, prize, bonus, points, earned, times: *times }
    }

    /// The two race options the front end sets for a tournament event (0x4ECAFD…0x4ECB5B), as
    /// `(TOURNAMENTLASTRACE, TOURNAMENTENDOFERA)` — options 0xB9 `[0x6577D8]` and 0xBA
    /// `[0x6577DC]`. CONFIRMED:
    /// * last race: exactly one event of the tournament is still undone (`0x504300(tid) == 1`);
    /// * end of era: the last race, **and** only one visible tournament of the current era is
    ///   still uncompleted (`0x5047B0(era) == 1`), **and** this one is not a `+0xE2` club event
    ///   (`0x504B00`), **and** it has never been completed (`+0xD3C == 0`, `0x503A70`).
    pub fn tournament_flags(&self, tables: &CareerTables, t: &Tournament) -> (bool, bool) {
        let st = self.state(t.id);
        let last = t.events.len().saturating_sub(st.events.len()) == 1;
        let open_in_era = tables
            .era_tournaments(self.era)
            .iter()
            .filter(|x| !x.is_hidden() && !self.state(x.id).completed)
            .count();
        let end_of_era = last && open_in_era == 1 && !t.knockout && st.completions == 0;
        (last, end_of_era)
    }

    /// The standings as they will stand once this event is recorded — what `HTournamentStandings`
    /// shows at the end of the race, before the front end files the result.
    pub fn standings_after(&self, t: &Tournament, event: usize, order: &[usize], times: &[u64; 8], clean: &[bool; 8]) -> Vec<(usize, u32, u32, u64)> {
        let mut events = self.state(t.id).events.clone();
        events.push(Self::event_result(t, event, order, times, clean));
        standings_of(&events, t.time_ranked())
    }

    pub fn record_event(&mut self, tables: &CareerTables, t: &Tournament, event: usize, order: &[usize], times: &[u64; 8], clean: &[bool; 8], garage_idx: Option<usize>) -> EventOutcome {
        let result = Self::event_result(t, event, order, times, clean);
        let (prize, bonus) = (result.prize, result.bonus);
        self.money = (self.money + prize as i64 + bonus as i64).clamp(-999_999_999, 999_999_999);
        if let Some(car) = garage_idx.and_then(|i| self.garage.get_mut(i)) {
            car.mileage += 3000;
            // **The original counts a win for the player's car whoever won.** 0x4EDFE6's loop runs
            // over every racer's result record and, after `0x503960`, tests *that racer's*
            // position — `cmp [esi], 1` at 0x4EE011, `esi` walking the records by 0x258 — and on a
            // match increments the **player's** car, `[0x690ADC]+0x69C` (0x4EE01B). One racer is
            // always first, so every career race adds one win to the car the player drove. Kept:
            // it feeds the car's value (`w = base · wins / 100`) and the used-car offers.
            if !order.is_empty() {
                car.wins += 1;
            }
        }
        let mut outcome = EventOutcome { result: result.clone(), final_rank: None, tournament_prize: 0, reset: false, era_advanced: false };
        let st = self.tournaments.entry(t.id).or_default();
        st.entered = true;
        st.events.push(result);
        if st.events.len() < t.events.len() {
            return outcome;
        }
        // last event: standings, tournament prize (any rank), completion or reset (0x503AD0)
        let standings = standings_of(&st.events, t.time_ranked());
        let rank = standings.iter().position(|(d, ..)| *d == 0).unwrap_or(7) as u8 + 1;
        let tprize = t.prize[rank as usize - 1];
        st.final_rank = Some(rank);
        st.prize = tprize;
        outcome.final_rank = Some(rank);
        outcome.tournament_prize = tprize;
        self.money = (self.money + tprize as i64).clamp(-999_999_999, 999_999_999);
        if t.is_club() {
            st.completed = true;
            st.completions += 1;
            st.events.clear();
            st.entered = false;
            return outcome;
        }
        if rank > 3 {
            st.events.clear();
            st.entered = false;
            st.failed_attempts += 1;
            outcome.reset = true;
            return outcome;
        }
        let first = !st.completed;
        st.completed = true;
        st.completions += 1;
        st.events.clear();
        st.entered = false;
        // 0x503BBC…0x503BE8: look the current year up **by equality** in the table at 0x5B8670 and,
        // unless it is the last entry (0x503BCD `cmp eax, 0x5B86B0`), step to the next one. A year
        // that is not in the table — only reachable from a save written elsewhere — stays put.
        if first
            && let Some(i) = YEAR_TABLE.iter().position(|y| *y == self.year)
            && let Some(next) = YEAR_TABLE.get(i + 1)
        {
            self.year = *next;
        }
        let era = t.era();
        let era_done = tables.era_tournaments(era).iter().filter(|x| !x.is_hidden()).all(|x| self.state(x.id).completed);
        if era_done && era == self.era {
            if let Some(f) = tables.tournaments.iter().find(|x| x.kind == TournamentKind::Era(era) && x.is_hidden()) {
                self.finale = Some(FinalePending { era, tournament: f.id });
            }
            if era == Era::Modern {
                self.evolution_complete = true;
            }
            if let Some(next) = era.next() {
                self.era = next;
            }
            outcome.era_advanced = true;
        }
        outcome
    }

    /// Finale race over: 1st place awards the loaned prize car (0x4EE17D); the pending finale is
    /// consumed either way.
    pub fn record_finale(&mut self, tables: &CareerTables, order: &[usize]) -> Option<(bool, u16)> {
        let f = self.finale.take()?;
        let t = tables.tournament(f.tournament)?;
        let won = order.first() == Some(&0);
        if won {
            self.garage.push(OwnedCar::new(t.prize_car, 0, 0));
            if !self.eras_completed.contains(&f.era) {
                self.eras_completed.push(f.era);
            }
        }
        Some((won, t.prize_car))
    }

    /// Decline the pending finale (`DECLINE` on the end-of-era screen).
    pub fn decline_finale(&mut self) {
        self.finale = None;
    }

    // ---- Factory Driver (`RE/notes/skill-tests.md` §12) ----

    /// Index of the current mission: the first record without a pass.
    pub fn current_mission(&self, tables: &CareerTables) -> Option<usize> {
        (0..tables.missions.len()).find(|i| self.missions.get(*i).copied().unwrap_or(0) != 1)
    }

    /// Store the result of mission `idx` (a failed mission stays current). Returns the stage whose
    /// promotion is now due, if the pass completed a stage.
    pub fn record_mission(&mut self, tables: &CareerTables, idx: usize, passed: bool) -> Option<i16> {
        if self.missions.len() < tables.missions.len() {
            self.missions.resize(tables.missions.len(), 0);
        }
        self.missions[idx] = passed as i16;
        if !passed {
            return None;
        }
        let stage = tables.missions[idx].stage;
        let stage_done = tables.missions.iter().enumerate().filter(|(_, m)| m.stage == stage).all(|(i, _)| self.missions[i] == 1);
        (stage_done && !self.promotions_accepted.contains(&stage)).then_some(stage)
    }

    /// Promotion due: a completed stage whose prize was not accepted yet.
    pub fn promotion_due<'a>(&self, tables: &'a CareerTables) -> Option<&'a Mission> {
        for stage in 1..=3i16 {
            let ms: Vec<(usize, &Mission)> = tables.missions.iter().enumerate().filter(|(_, m)| m.stage == stage).collect();
            if ms.is_empty() || self.promotions_accepted.contains(&stage) {
                continue;
            }
            if ms.iter().all(|(i, _)| self.missions.get(*i).copied().unwrap_or(0) == 1) {
                return ms.first().map(|(_, m)| *m);
            }
        }
        None
    }

    /// `ACCEPTPRIZECAR`: add the stage's prize car to the garage (purchase price 0).
    pub fn accept_promotion(&mut self, tables: &CareerTables) -> Option<u16> {
        let m = self.promotion_due(tables)?;
        let (stage, car) = (m.stage, m.prize_car as u16);
        self.promotions_accepted.push(stage);
        self.garage.push(OwnedCar::new(car, 0, 0));
        Some(car)
    }

    /// Points standings for `t`: `(driver index, points, money, time)` sorted best first.
    pub fn standings(&self, t: &Tournament) -> Vec<(usize, u32, u32, u64)> {
        standings_of(&self.state(t.id).events, t.time_ranked())
    }
}

/// Mileage score of a listed car — `0x4F7E90`'s four tables, picked by the model's age
/// (≤ 10, ≤ 20, ≤ 30, older: 0x5B7C88, 0x5B7CB8, 0x5B7CE8, 0x5B7D18). Each is `(lo, hi, score)`
/// matched `lo <= km <= hi`; **a mileage in none of the bands scores 0** — the tables jump from
/// 5000 to 10000 / 15000 / 20000 km for the three older ages. CONFIRMED.
fn mileage_score(age: i64, km: u32) -> i64 {
    const TABLES: [[(i64, i64, i64); 4]; 4] = [
        [(0, 500, 3), (501, 2000, 2), (2001, 5000, 1), (5001, 1_000_000, -1)],
        [(0, 500, 4), (501, 2000, 3), (2001, 5000, 2), (10000, 1_000_000, -1)],
        [(0, 500, 8), (501, 2000, 6), (2001, 5000, 4), (15000, 1_000_000, -1)],
        [(0, 500, 15), (501, 2000, 10), (2001, 5000, 5), (20000, 1_000_000, -1)],
    ];
    let t = match age {
        a if a <= 10 => &TABLES[0],
        a if a <= 20 => &TABLES[1],
        a if a <= 30 => &TABLES[2],
        _ => &TABLES[3],
    };
    let km = km as i64;
    t.iter().rev().find(|(lo, hi, _)| (*lo..=*hi).contains(&km)).map_or(0, |(_, _, v)| *v)
}

/// `0x5B7D88`: `(lo, hi, pct, immediate)`. The 90…95 and 95…99 bands overlap at 95 and the
/// loop keeps the **last** match. CONFIRMED.
const OFFER_BRACKETS: [(i64, i64, i64, bool); 7] = [
    (0, 83, 20, true),
    (84, 89, 40, true),
    (90, 95, 60, true),
    (95, 99, 80, true),
    (100, 104, 80, false),
    (105, 109, 90, false),
    (110, 1_000_000, 100, false),
];

/// What `0x4F7E90` decided for one car.
enum Offer {
    None,
    /// an offer made at once (the flag-1 brackets)
    Now(u32),
    /// the roll succeeded: the player's car is offered its asking price, a market car is sold
    Taken(u32),
}

/// Aged value of an installed part (`0x518220`): price × (1 + Σ yearly increments) × condition.
/// The yearly increment table `0x5BA4A4` was not transcribed — 0 % per year is used (MEDIUM).
/// Per-year appreciation of a part, in per cent — the 62-entry table at **0x5BA4A4** in the
/// shipped `nfs5.exe`, summed over the part's age (0x518880 / 0x518900). CONFIRMED (data).
pub const PART_AGE_PCT: [u32; 62] = [
    1, 2, 2, 2, 2, 3, 4, 5, 4, 5, 6, 7, 8, 12, 4, 5, 4, 2, 3, 2, 3, 4, 3, 5, 6, 7, 2, 4, 5, 8, 9, 2, 5, 4, 6, 3, 7, 4, 7, 6, 6, 4, 7,
    3, 6, 8, 3, 4, 5, 2, 4, 3, 4, 2, 5, 2, 1, 3, 7, 5, 4, 3,
];

/// `price + price·Σ PART_AGE_PCT[0..age] / 100` — what a part is worth before its condition is
/// taken off (0x518900, first half). CONFIRMED.
/// How many cars the player may own: both purchases — the dealer's at 0x501D8C and the used-car
/// lot's at 0x501FD0 — test the owned list's byte length against **0x140** (80 pointers) after
/// the money check and refuse with `FEStrings` 61, "you cannot purchase more than 80 cars".
/// CONFIRMED.
pub const MAX_OWNED_CARS: usize = 80;

/// How many cars the used-car market holds before the oldest generated one is dropped —
/// `0x4F8171` compares the list length against **0x19**.
pub const USED_CAR_MARKET_MAX: usize = 25;

/// `0x5B7A10`, indexed by a model's age in years (0…60). See [`Career::refresh_market`]: the entry
/// is the chance in sixtieths of the model **not** being offered.
pub const USED_CAR_RARITY: [u32; 61] = [
    5, 10, 15, 20, 25, 35, 50, 70, 80, 80, 70, 50, 40, 30, 20, 10, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 3, 3, 3, 2, 2,
];

/// One of the used-car generator's weighted draws: `(roll_lo, roll_hi, min, max)`. `rand() % 100`
/// picks the band and the value is `min + rand() % (max - min)` — **`max` exclusive** — or exactly
/// `min` when the two are equal (0x4F866C's `sub`/`jne`). The original's loop does not stop at the
/// first match; the bands do not overlap, so the last one to match is the only one.
pub type Band = (i32, i32, i32, i32);

/// `0x5B7B08`: what share of the parts that fit go on a market car.
pub const USED_PART_PCT: [Band; 4] = [(0, 5, 0, 0), (6, 45, 1, 10), (46, 75, 11, 20), (76, 100, 21, 50)];
/// `0x5B7B48`: the odometer.
pub const USED_MILEAGE: [Band; 4] = [(0, 5, 0, 0), (6, 20, 2000, 15000), (21, 80, 15000, 25000), (81, 100, 25000, 100000)];
/// `0x5B7BC8`: the win count.
pub const USED_WINS: [Band; 4] = [(0, 5, 0, 0), (6, 20, 1, 2), (21, 80, 3, 10), (81, 100, 11, 50)];

pub fn weighted_band(bands: &[Band], rand: &mut impl FnMut() -> u32) -> i32 {
    let roll = (rand() % 100) as i32;
    let mut out = 0;
    for &(lo, hi, min, max) in bands {
        if roll >= lo && roll <= hi {
            out = if max == min { min } else { min + (rand() % (max - min) as u32) as i32 };
        }
    }
    out
}

pub fn part_aged_price(p: &Part, year: u16) -> i64 {
    let age = (year as i64 - p.year as i64).max(0) as usize;
    let sum: u32 = PART_AGE_PCT.iter().take(age).sum();
    let price = p.price as i64;
    price + price * sum as i64 / 100
}

/// What it costs to repair one installed part (`0x518880`, CONFIRMED): the aged price times the
/// part's damage doubled, over a hundred. `0x5188BC` multiplies `part+0x164` by the 2.0 at
/// 0x5BA4A0 and truncates it (`0x5A0F98`), `0x5188E2` multiplies that into the aged price and
/// 0x5188ED divides by a hundred.
///
/// The damage screens sum it over the installed parts of one category — `0x4EFBC0` does category 1
/// — and `OVERALLDAMAGECOST` over all of them (0x4EFF80). Each is printed through 0x4DBF80's
/// thousands commas.
pub fn part_repair_cost(p: &Part, year: u16, state: f32) -> i64 {
    part_aged_price(p, year) * (2.0 * state) as i64 / 100
}

/// What an installed part adds to its car's value — **`0x518220`**, the function `0x4F8E50` calls for
/// every part on the car (0x4F8F6D). CONFIRMED, read 2026-09-23:
///
/// ```text
/// aged  = price + price·Σ PART_AGE_PCT[0..age] / 100            ; 0x51822C…0x51827F, age = year − part+0x46
/// c     = min(_ftol(2.0·state + 20.0), 100), negatives → 0       ; 0x51825C…0x5182A0
/// value = (100 − c)·aged / 100                                   ; 0x5182A2…0x5182BA
/// ```
///
/// `state` is `part+0x164`, the damage the part carries, so an undamaged part is worth **80 %** of
/// its aged price and one at 40 damage nothing. The port used to call this through `0x518900`'s
/// formula (`price·sum/100 − (100 − c)·aged/100 + price`), the complementary share — 20 % for an
/// undamaged part, rising with damage. `0x518900` is a different function, used by the parts shop
/// (0x5171C5, 0x5175F8), which this port does not call.
fn part_resale(p: &Part, year: u16, state: f32) -> i64 {
    part_resale_from(part_aged_price(p, year), state)
}

/// [`part_resale`] given the aged price.
fn part_resale_from(aged: i64, state: f32) -> i64 {
    let c = ((2.0f32 * state + 20.0) as i64).clamp(0, 100);
    (100 - c) * aged / 100
}

/// A car's damage figure — **`0x4EF960`**, read 2026-09-23, CONFIRMED. It is not the body zones'
/// mean: it averages the damage `part+0x164` of the **installed** parts (`car+0x758`) by part type,
///
/// ```text
/// M_k   = mean of part+0x164 over installed parts with type (+0x26) k, 0 when there is none
///         (0x4EF580 for k = 1, 2, 3, 7; the same loop inlined for 4, 5, 6, 8, 9, 10)
/// value = (M1 + M2 + M3 + M4 + M5 + (M6 + M7)·0.5 + M8 + M9 + M10) · (1/9)f   ; 0x4EFB82…0x4EFBB1
/// ```
///
/// Each `M_k` is stored as an `f32`; the sum runs on the x87. A new car's installed list is
/// empty — the FE car constructor 0x4EEBA0 clears `+0x758` (0x4EEC86…) — so a car carrying only
/// its stock parts reads **0 %** however dented its body is; this port's `owned.parts` holds the
/// same list. The value is what `0x4F8E50` and `0x4F7E90` truncate into their damage bands.
/// APPROXIMATION of the last bit only: the result is stored as `f32` here, the original hands the
/// x87 value straight to `_ftol`.
pub fn car_damage_pct(tables: &CareerTables, owned: &OwnedCar) -> f32 {
    let parts: Vec<(u16, f32)> = owned
        .parts
        .iter()
        .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
        .map(|p| (p.kind, owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)))
        .collect();
    damage_pct_from(&parts)
}

/// [`car_damage_pct`] over `(part type, part+0x164)` pairs.
fn damage_pct_from(parts: &[(u16, f32)]) -> f32 {
    let mean = |kind: u16| -> f64 {
        let v: Vec<f64> = parts.iter().filter(|(k, _)| *k == kind).map(|(_, s)| *s as f64).collect();
        let sum: f64 = v.iter().sum();
        (if v.len() > 1 { sum / v.len() as f64 } else { sum }) as f32 as f64
    };
    let total = mean(1) + mean(2) + mean(3) + mean(4) + mean(5) + (mean(6) + mean(7)) * 0.5 + mean(8) + mean(9) + mean(10);
    (total * (1.0f32 / 9.0) as f64) as f32
}

/// `0x4F8E50`'s arithmetic, given the installed parts' total (see [`Career::value_of`]).
pub fn car_value_from(price: u32, curve_pct: f32, mileage: u32, damage_pct: f32, wins: u32, parts: i64) -> i64 {
    const MILEAGE: [(i64, i64, i64); 5] = [(0, 5000, 0), (5001, 10000, -10), (10001, 15000, -15), (15001, 20000, -25), (20001, 1_000_000, -30)];
    const DAMAGE: [(i64, i64, i64); 5] = [(0, 5, -1), (6, 10, -6), (11, 15, -11), (16, 25, -16), (26, 100, -26)];
    let base = (price as f64 * curve_pct as f64 * 0.01) as i64;
    let mileage = mileage as i64;
    let m = MILEAGE.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&mileage)).map_or(0, |(_, _, p)| p * base / -100);
    let dmg = damage_pct as i64;
    let d = DAMAGE.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&dmg)).map_or(0, |(_, _, p)| p * base / 100);
    let w = wins as i64 * base / 100;
    let value = base + m + d + w + parts;
    let floor = base as f64 * 0.05f32 as f64;
    if (value as f64) < floor { floor as i64 } else { value }
}

/// The body-damage total the tournament bonus is judged on: the ten zones `car+0x4E4..` summed
/// into an integer **truncating after every add** — `fild [acc] / fadd [zone] / call _ftol`, at
/// 0x426AEF…0x426B08 for the start and 0x4276E0…0x4276FB for the finish. The x87 runs at MSVC's
/// 53-bit precision, so the add is an `f64` one. CONFIRMED.
pub fn damage_sum(zones: &[f32; 10]) -> i32 {
    zones.iter().fold(0i32, |acc, z| (acc as f64 + *z as f64) as i32)
}

/// Whether a car raced **clean** — no new body damage between the start and the finish.
///
/// `0x426A80(0)`, the HUD's race-start arm, stores [`damage_sum`] of every racer at
/// `[0x60C9C8 + i·4]` and clears each result record's `+0x1EC` byte (0x426AD6, 0x426B0F). In a
/// tournament race (`[0x6573E8] == 4`, 0x426C36) the finish arm calls 0x427660, which sums the
/// finishing zones — `car+0xC98`, which 0x416AC0 copies from `car+0x4E4` — and when that total is
/// **greater** than the start total sets `car+0xCC0 = 1` (0x4276FD `jle` skip, 0x427708). The flag
/// reaches the result record as `+0x1EC`, and `0x503960` pays the event bonus only when it is
/// clear. A car carrying garage damage into the race is judged against that, not against zero.
/// CONFIRMED.
pub fn clean_race(start: i32, finish: &[f32; 10]) -> bool {
    damage_sum(finish) <= start
}

/// Standings `0x503660` / sort `0x502880`: points desc, money earned desc, total time asc; time-ranked
/// club events: time asc, money asc. CONFIRMED — the sort is a bubble pass over 16-byte rows
/// `{points, Σtime, Σmoney, idx}` that swaps only on a strict compare (0x50290F…0x50291E for points,
/// 0x5028C4…0x5028D0 for time), so it is stable, and it has **no special case for a zero time**:
/// every racer arrives with one, because 0x416AC0 projects a time for any car still racing.
fn standings_of(events: &[EventResult], time_ranked: bool) -> Vec<(usize, u32, u32, u64)> {
    let mut rows: Vec<(usize, u32, u32, u64)> = (0..8).map(|d| (d, 0u32, 0u32, 0u64)).collect();
    for e in events {
        for d in 0..8 {
            rows[d].1 += e.points[d] as u32;
            rows[d].2 += e.earned[d];
            rows[d].3 += e.times[d];
        }
    }
    if time_ranked {
        rows.sort_by(|a, b| a.3.cmp(&b.3).then(a.2.cmp(&b.2)));
    } else {
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_formats::career::TournamentEvent;

    fn tournament(id: i16, events: usize, hidden: bool) -> Tournament {
        Tournament {
            id,
            kind: TournamentKind::Era(Era::Classic),
            start_year: 1950,
            unlock_tracks: vec![],
            name_string_id: 0,
            title_string_id: 0,
            description_string_id: 0,
            restriction_string_id: 0,
            eligible_cars: vec![76],
            entry_fee: 500,
            traffic_level: 0,
            knockout: false,
            visibility: if hidden { 2 } else { 0 },
            difficulty_level: 0,
            flags: [0; 4],
            prize: [5000, 4000, 3000, 2000, 1000, 0, 0, 0],
            prize_car: 168,
            unknown_110: 0,
            custom_traffic: None,
            opponents: vec![],
            events: (0..events).map(|_| TournamentEvent { track_id: 8, laps: 1, backwards: false, direction_flag: 0, prize: [1000, 800, 600, 400, 200, 0, 0, 0], bonus: 400, difficulty: 0.5 }).collect(),
        }
    }

    fn tables(ts: Vec<Tournament>) -> CareerTables {
        CareerTables { raw: Default::default(), tracks: vec![], tournaments: ts, parts: vec![], personalities: vec![], missions: vec![], strings: Strings::default(), catalog: vec![] }
    }

    /// 0x4ECAFD…0x4ECB5B: the last race is the one undone event; the end of the era also needs it
    /// to be the era's only open tournament, never completed before.
    #[test]
    fn last_race_and_end_of_era_flags() {
        let a = tournament(1, 2, false);
        let b = tournament(2, 2, false);
        let tables = tables(vec![a.clone(), b.clone()]);
        let mut c = Career::default();
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8] };
        assert_eq!(c.tournament_flags(&tables, &a), (false, false), "two events to go");
        c.tournaments.entry(1).or_default().events.push(r.clone());
        assert_eq!(c.tournament_flags(&tables, &a), (true, false), "b is still open");
        c.tournaments.entry(2).or_default().completed = true;
        assert_eq!(c.tournament_flags(&tables, &a), (true, true), "a is the era's last");
        c.tournaments.entry(1).or_default().completions = 1;
        assert_eq!(c.tournament_flags(&tables, &a), (true, false), "won before: not the era's end");
    }

    /// `HTournamentStandings` shows the table with this event in it, before the front end files
    /// the result — and building it changes nothing.
    #[test]
    fn standings_after_the_race_include_it_without_recording_it() {
        let t = tournament(1, 2, false);
        let c = Career::default();
        let st = c.standings_after(&t, 0, &[2, 0, 1, 3, 4, 5, 6, 7], &[6400; 8], &[true; 8]);
        assert_eq!(st[0].0, 2, "the winner leads");
        assert_eq!((st[1].0, st[1].1, st[1].2), (0, 7, 800 + 200));
        assert!(c.state(1).events.is_empty(), "nothing recorded");
    }

    /// 0x501FD0: an eighty-first car is refused after the money check, and nothing is spent.
    #[test]
    fn no_more_than_eighty_cars() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else {
            eprintln!("skipping: no game data");
            return;
        };
        let Ok(tables) = CareerTables::load(&paths) else { return };
        let mut c = Career::default();
        for _ in 0..MAX_OWNED_CARS {
            c.garage.push(OwnedCar::new(76, 0, 0));
        }
        let mut lot = OwnedCar::new(76, 0, 0);
        lot.asking_price = Some(1_000);
        c.market.push(lot);
        let money = c.money;
        assert!(matches!(c.buy_used_car(&tables, 0), Err(CareerError::GarageFull)));
        assert_eq!((c.money, c.garage.len(), c.market.len()), (money, 80, 1));
        c.money = 0;
        assert!(matches!(c.buy_used_car(&tables, 0), Err(CareerError::NoMoney { .. })), "money is checked first");
    }

    /// `0x4EF960`: a mean per part type, types 6 and 7 sharing one slot, over nine.
    #[test]
    fn a_cars_damage_figure_is_its_parts_by_type() {
        assert_eq!(damage_pct_from(&[]), 0.0, "stock parts only: nothing on the list, 0 %");
        // one type-1 part at 45 → 45 / 9
        assert!((damage_pct_from(&[(1, 45.0)]) - 5.0).abs() < 1e-5);
        // two type-2 parts average; a type-6 part counts half
        assert!((damage_pct_from(&[(2, 10.0), (2, 30.0), (6, 36.0)]) - (20.0 + 18.0) / 9.0).abs() < 1e-5);
        // types outside 1..10 are not counted
        assert_eq!(damage_pct_from(&[(11, 90.0), (0, 90.0)]), 0.0);
    }

    /// `0x518220`: an undamaged part keeps 80 % of its aged price, damage takes it away, and 40
    /// damage leaves nothing.
    #[test]
    fn a_parts_share_of_the_car_value_falls_with_its_damage() {
        assert_eq!(part_resale_from(1000, 0.0), 800);
        assert_eq!(part_resale_from(1000, 10.0), 600);
        assert_eq!(part_resale_from(1000, 40.0), 0);
        assert_eq!(part_resale_from(1000, 90.0), 0);
        // `_ftol` truncates: 2·4.9 + 20 = 29.8 → 29
        assert_eq!(part_resale_from(999, 4.9), 709);
    }

    /// `0x4F8E50`: truncation everywhere, and mileage **adds** value — the mileage arm divides by
    /// −100 (`0xAE147AE1`) against a table of negative percentages.
    #[test]
    fn car_value_is_the_originals_integer_arithmetic() {
        // base = _ftol(11000 · 93.5 · 0.01) = 10285 (not 10285.0 rounded — the same here)
        assert_eq!(car_value_from(11_000, 93.5, 0, 0.0, 0, 0), 10285 - 102);
        // 7000 km: −10 / −100 → +1028, damage 0 → −102 (the 0..5 band still costs 1 %)
        assert_eq!(car_value_from(11_000, 93.5, 7000, 0.0, 0, 0), 10285 + 1028 - 102);
        // damage 5.9 % truncates to 5, the −1 band — not the −6 one
        assert_eq!(car_value_from(11_000, 93.5, 0, 5.9, 0, 0), 10285 - 102);
        assert_eq!(car_value_from(11_000, 93.5, 0, 6.0, 0, 0), 10285 - 617);
        // three wins: +3 %, truncated
        assert_eq!(car_value_from(11_000, 93.5, 0, 0.0, 3, 0), 10285 - 102 + 308);
        // the floor: 5 % of base, truncated, when the sum falls under it
        assert_eq!(car_value_from(11_000, 93.5, 0, 0.0, 0, -20_000), 514);
    }

    /// The tournament bonus goes only to a car that raced clean (0x426A80 / 0x427660 / 0x503960):
    /// a winner that picked up new damage keeps its prize and points but not its bonus, and the
    /// damage it carried in from the garage does not count against it.
    #[test]
    fn a_damaged_car_forfeits_the_event_bonus() {
        // truncation after every add, not once at the end: 0.6 × 10 is 6.0 but sums to 0
        assert_eq!(damage_sum(&[0.6; 10]), 0);
        assert_eq!(damage_sum(&[1.5; 10]), 10);
        let carried = [10.0; 10];
        let start = damage_sum(&carried);
        assert!(clean_race(start, &carried), "garage damage alone is clean");
        let mut dented = carried;
        dented[1] += 1.0;
        assert!(!clean_race(start, &dented));

        let t = tournament(1, 2, false);
        let tables = tables(vec![t.clone()]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        c.enter(&tables, &t).unwrap();
        let before = c.money;
        let mut clean = [true; 8];
        clean[0] = false;
        clean[2] = false;
        let o = c.record_event(&tables, &t, 0, &[0, 1, 2, 3, 4, 5, 6, 7], &[6400; 8], &clean, Some(0));
        assert_eq!((o.result.position, o.result.prize, o.result.bonus), (1, 1000, 0));
        assert_eq!(o.result.points[0], 8, "points do not depend on it");
        assert_eq!(o.result.earned[1], 800 + 200, "second place was clean");
        assert_eq!(o.result.earned[2], 600, "third place was not");
        assert_eq!(c.money, before + 1000);
    }

    /// 0x503E40 clears a forfeited tournament's records; 0x4F9720 drops an offer and keeps the
    /// listing; the pre-race screen asks about the first car holding one.
    #[test]
    fn forfeit_and_offers() {
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8] };
        let mut c = Career::default();
        c.tournaments.entry(4).or_default().events.push(r);
        c.forfeit(4);
        assert!(c.state(4).events.is_empty());
        c.garage.push(OwnedCar { car_id: 76, paint_set: 0, trim: Trim::default(), parts: vec![], mileage: 0, wins: 0, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, purchase_price: 0, asking_price: Some(9_000), offer: Some(8_000) });
        assert_eq!(c.pending_offer(), Some(0));
        c.decline_offer(0);
        assert_eq!(c.pending_offer(), None);
        assert_eq!(c.garage[0].asking_price, Some(9_000));
    }

    /// 0x504B60 / 0x504BD0: a tournament with some but not all events raced is "part-way"; the
    /// selected one never counts, and the reset clears only the others.
    #[test]
    fn part_way_tournaments_and_their_reset() {
        let t = tables(vec![tournament(1, 3, false), tournament(2, 3, false), tournament(3, 2, false)]);
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8] };
        let mut c = Career::default();
        c.tournaments.entry(1).or_default().events.push(r.clone());
        c.tournaments.entry(3).or_default().events = vec![r.clone(), r.clone()]; // finished
        assert!(c.partial_elsewhere(&t, 2));
        assert!(!c.partial_elsewhere(&t, 1));
        c.reset_partial_except(&t, 2);
        assert!(c.state(1).events.is_empty());
        assert_eq!(c.state(3).events.len(), 2);
        assert!(!c.partial_elsewhere(&t, 2));
    }

    #[test]
    fn points_prizes_and_standings() {
        let t = tournament(1, 2, false);
        let tables = tables(vec![t.clone(), tournament(31, 1, true)]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        assert_eq!(c.enter(&tables, &t).unwrap(), 0);
        assert_eq!(c.money, STARTING_MONEY - 500);
        let times = [6400, 6500, 6600, 6700, 6800, 6900, 7000, 7100];
        let o = c.record_event(&tables, &t, 0, &[0, 1, 2, 3, 4, 5, 6, 7], &times, &[true; 8], Some(0));
        assert_eq!((o.result.position, o.result.prize, o.result.bonus), (1, 1000, 400));
        assert_eq!(o.result.points, [8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(o.result.earned[1], 800 + 200);
        assert_eq!(c.garage[0].mileage, 3000);
        assert_eq!(c.garage[0].wins, 1);
        assert_eq!(c.enter(&tables, &t).unwrap(), 1, "fee not paid again");
        assert_eq!(c.money, STARTING_MONEY - 500 + 1400);
        let o = c.record_event(&tables, &t, 1, &[1, 0, 2, 3, 4, 5, 6, 7], &times, &[true; 8], Some(0));
        assert_eq!((o.result.position, o.result.prize, o.result.bonus), (2, 800, 200));
        // second place, and the car still gains a win: 0x4EE011 tests each racer's position and
        // credits the player's car for whoever was first
        assert_eq!((c.garage[0].mileage, c.garage[0].wins), (6000, 2));
        // player 8+7 = 15 pts / $2400, driver 1 = 7+8 = 15 pts / $2400 → tie on money, player's time
        // 6400+6400 < driver 1's 6500+6500 → player ranks first
        assert_eq!(o.final_rank, Some(1));
        assert_eq!(o.tournament_prize, 5000);
        let st = c.state(1);
        assert!(st.completed && st.events.is_empty());
        assert_eq!(c.money, STARTING_MONEY - 500 + 1400 + 1000 + 5000);
        // the only visible Classic tournament is done: era advances, finale pending, year steps
        assert!(o.era_advanced);
        assert_eq!(c.era, Era::Golden);
        assert_eq!(c.year, 1956);
        assert_eq!(c.finale.as_ref().map(|f| f.tournament), Some(31));
        assert!(c.enter(&tables, &t).is_err(), "completed tournaments are closed");
    }

    #[test]
    fn rank_above_three_resets_the_tournament() {
        let t = tournament(1, 1, false);
        let tables = tables(vec![t.clone()]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        c.enter(&tables, &t).unwrap();
        let o = c.record_event(&tables, &t, 0, &[1, 2, 3, 4, 0, 5, 6, 7], &[0; 8], &[true; 8], Some(0));
        assert!(o.reset && o.final_rank == Some(5));
        assert_eq!(o.tournament_prize, 1000, "tournament prize paid for any rank");
        let st = c.state(1);
        assert!(!st.completed && st.events.is_empty() && st.failed_attempts == 1);
        assert_eq!(c.era, Era::Classic);
        // re-entering pays the fee again
        let m = c.money;
        c.enter(&tables, &t).unwrap();
        assert_eq!(c.money, m - 500);
    }

    #[test]
    fn finale_awards_the_prize_car() {
        let tables = tables(vec![tournament(31, 1, true)]);
        let mut c = Career::default();
        c.finale = Some(FinalePending { era: Era::Classic, tournament: 31 });
        assert_eq!(c.record_finale(&tables, &[0, 1, 2]), Some((true, 168)));
        assert_eq!(c.garage.len(), 1);
        assert_eq!(c.garage[0].car_id, 168);
        assert!(c.finale.is_none());
        assert_eq!(c.eras_completed, vec![Era::Classic]);
    }

    #[test]
    fn used_car_market_offers() {
        let tables = tables(vec![]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        // no catalogue in this fixture → no value, so no offers can be made
        let mut rng = || 0u32;
        c.list_for_sale(0, 9_000).unwrap();
        c.generate_offers(&tables, &mut rng);
        assert_eq!(c.garage[0].offer, None, "unknown car has no value");
        // the last car may not be listed
        let mut one = Career::default();
        one.garage.push(OwnedCar::new(76, 0, 11_000));
        assert!(matches!(one.list_for_sale(0, 1), Err(CareerError::LastCar)));
        // with car 0 listed, the second of two cars cannot be listed too (0x50FCB0 subtracts the
        // listings)
        assert!(matches!(c.list_for_sale(1, 9_000), Err(CareerError::LastCar)));
        // accepting an offer pays and removes the car
        c.garage[0].offer = Some(12_345);
        let money = c.money;
        assert_eq!(c.accept_offer(&tables, 0).unwrap(), 12_345);
        assert_eq!(c.money, money + 12_345);
        assert_eq!(c.garage.len(), 1);
        assert!(matches!(c.accept_offer(&tables, 0), Err(CareerError::NoOffer)));
    }

    #[test]
    fn mileage_score_brackets() {
        // age ≤ 10: +3 / +2 / +1, and −1 above 5000 km (0x5B7C88 tables)
        assert_eq!(mileage_score(5, 400), 3);
        assert_eq!(mileage_score(5, 1_500), 2);
        assert_eq!(mileage_score(5, 4_000), 1);
        assert_eq!(mileage_score(5, 9_000), -1);
        // older cars tolerate more mileage and score higher when barely used
        assert_eq!(mileage_score(35, 400), 15);
        // …but the older tables jump from 5000 to 10000 / 15000 / 20000 km, and a mileage in the
        // gap is in no band: 0 (0x5B7CB8, 0x5B7D18)
        assert_eq!(mileage_score(35, 18_000), 0);
        assert_eq!(mileage_score(15, 7_000), 0);
        assert_eq!(mileage_score(15, 10_000), -1);
        assert_eq!(mileage_score(35, 20_000), -1);
        assert_eq!(mileage_score(35, 25_000), -1);
    }

    #[test]
    fn save_roundtrip() {
        let mut c = Career::default();
        c.garage.push(OwnedCar { car_id: 76, paint_set: 2, trim: Trim::default(), parts: vec![13], mileage: 3000, wins: 1, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, purchase_price: 11_000, asking_price: Some(9_000), offer: None });
        c.tournaments.insert(9, TournamentState { entered: true, ..Default::default() });
        let dir = std::env::temp_dir().join(format!("nfs-career-{}", std::process::id()));
        let path = dir.join("career.json");
        c.save(&path).unwrap();
        assert_eq!(Career::load(&path).unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_knockout_draw_is_without_replacement_and_era_filtered() {
        let per = |id: i16, era: u32| Personality { id, name_string_id: 0, era, skill: [[1.0, 0.0]; 4] };
        let table = vec![per(1, 0), per(2, 0), per(3, 1), per(4, 1), per(5, 2)];
        // era 1 has two drivers; asking for three yields both and stops
        let ids = pick_personalities(&table, Some(1), 3, |_| 0);
        assert_eq!(ids, [3, 4]);
        // no repeats whatever the draw says
        let mut n = 0;
        let ids = pick_personalities(&table, None, 5, |len| {
            n += 1;
            (n - 1) % len
        });
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "{ids:?}");
        assert_eq!(ids.len(), 5);
        // era = None takes the whole table
        assert_eq!(pick_personalities(&table, None, 1, |_| 4), [5]);
    }


    /// The damage a race leaves, what it costs to put right, and what the part is worth meanwhile
    /// (`RE/notes/physics.md` §10, `PartValue` 0x518900 / `RepairCost` 0x518880).
    #[test]
    fn damage_wears_the_parts_and_repair_clears_it() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else {
            eprintln!("skipping: no game data");
            return;
        };
        let Ok(tables) = CareerTables::load(&paths) else { return };
        let mut c = Career::default();
        c.money = 200_000;
        let car = tables.catalog.iter().find(|c| c.price > 0).unwrap().clone();
        c.buy_car(&tables, car.id, 0, Trim::default()).unwrap();
        let part = tables.parts.iter().find(|p| p.family == part_family(&car).unwrap_or_default() && p.kind == 1).cloned();
        let Some(part) = part else { return };
        c.buy_part(&tables, 0, part.id).unwrap();
        assert_eq!(c.repair_cost(&tables, 0, &[1]), 0, "a fresh part needs nothing");

        // the car comes back with its rear zones beaten in
        let mut zones = [0.0f32; 10];
        for z in zones.iter_mut() {
            *z = 60.0;
        }
        c.record_damage(&tables, 0, zones);
        let state = c.garage[0].part_state.iter().find(|(id, _)| *id == part.id).map(|(_, s)| *s).unwrap();
        assert!((state - (1.5 - 0.005 * part.condition) * 60.0).abs() < 0.01, "{state}");
        // the car's damage figure is 0x4EF960's — its one installed part, type 1, over nine — not
        // the zones' mean of 60
        assert!((c.garage[0].damage_pct - state / 9.0).abs() < 1e-4, "{}", c.garage[0].damage_pct);

        // and that is the number the two damage lists print (0x516EF6's `"%d %"`), where a part the
        // car never raced falls back to the table's own state
        assert!((c.part_damage(0, &part) - state).abs() < 0.01);
        let fresh = tables.parts.iter().find(|p| p.id != part.id && p.kind == part.kind);
        if let Some(f) = fresh {
            assert_eq!(c.part_damage(0, f), f.state as f32);
        }

        // the getter's scale and offset follow the parts on the car
        let w = part_wear_for(&tables, &c.garage[0]);
        assert!((w.engine.offset - state).abs() < 0.01);
        assert!((w.engine.scale - (1.5 - 0.005 * part.condition)).abs() < 0.01);

        // repairing costs the aged price times twice the damage, and clears both
        let cost = c.repair_cost(&tables, 0, &[1]);
        let expect = part_aged_price(&part, c.year) * (2.0 * state) as i64 / 100;
        assert_eq!(cost, expect);
        assert!(cost > 0);
        let money = c.money;
        assert_eq!(c.repair(&tables, 0, &[1]).unwrap(), cost);
        assert_eq!(c.money, money - cost);
        assert_eq!(c.repair_cost(&tables, 0, &[1]), 0);
        assert_eq!(part_wear_for(&tables, &c.garage[0]).engine.offset, 0.0);
    }
}
