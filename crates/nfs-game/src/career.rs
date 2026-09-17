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
use nfs_formats::sim::CarSim;
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
    pub fn club_tournaments(&self) -> Vec<&Tournament> {
        let mut v: Vec<&Tournament> = self.tournaments.iter().filter(|t| matches!(t.kind, TournamentKind::Club(_)) && !t.is_hidden()).collect();
        v.sort_by_key(|t| (t.era(), t.id));
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnedCar {
    pub car_id: u16,
    pub paint_set: u8,
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

impl OwnedCar {
    pub fn new(car_id: u16, paint_set: u8, purchase_price: u32) -> Self {
        Self { car_id, paint_set, parts: Vec::new(), mileage: 0, wins: 0, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, purchase_price, asking_price: None, offer: None }
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
/// (one standard part, scale 1.5). MEDIUM: the getter's formula is from `physics.md` §10, that the
/// state accumulates race damage is the port's reading of what the save keeps.
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

/// Torque added by non-engine type-1 parts is scaled by this constant (`0x5B4A38`, CONFIRMED).
pub const PART_TORQUE_SCALE: f32 = 0.67;

/// `CarSim_applyParts` 0x49CB50 (`RE/notes/loaders.md` §1.3, offsets corrected against the data):
/// mass and top-speed deltas for every part; engine parts replace the torque curve (a zero curve
/// keeps the stock one); other engine-group parts add `0.67 ×` their curve; gearbox parts replace
/// the ratios and final drive. Brake / spring / shock / sway-bar factors (`+0xC0`) are not applied
/// (their consumers in the physics were not traced — UNKNOWN).
pub fn apply_parts(sim: &mut CarSim, parts: &[&Part]) {
    for p in parts {
        sim.mass_kg += p.mass_delta;
        sim.top_speed_mph /= 1.0 + 0.01 * p.top_speed_pct;
        match p.kind {
            1 if p.kind_name.starts_with("Engine") => {
                if p.torque_curve.iter().any(|t| *t != 0.0) {
                    sim.torque_curve = p.torque_curve;
                }
                if p.value_c0 > 0.0 {
                    sim.redline_rpm = p.value_c0;
                }
                match p.induction {
                    1 | 2 => sim.induction = p.induction,
                    _ => {}
                }
            }
            1 => {
                for (t, add) in sim.torque_curve.iter_mut().zip(p.torque_curve.iter()) {
                    *t += add * PART_TORQUE_SCALE;
                }
                if p.kind_name == "Turbo" {
                    sim.induction = 1;
                } else if p.kind_name == "Supercharger" {
                    sim.induction = 2;
                }
            }
            3 if p.kind_name == "Gear Box" => {
                if p.gear_ratios[2..].iter().any(|g| *g != 0.0) {
                    sim.reverse_ratio = p.gear_ratios[0];
                    sim.neutral = p.gear_ratios[1];
                    sim.gear_ratios.copy_from_slice(&p.gear_ratios[2..8]);
                    sim.gear_count = p.forward_gears.max(1) as u32;
                }
                if p.value_c0 > 0.0 {
                    sim.final_drive = p.value_c0;
                }
            }
            _ => {}
        }
    }
}

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
}

fn default_year() -> u16 {
    1950
}

impl Default for Career {
    fn default() -> Self {
        Self { version: SAVE_VERSION, era: Era::Classic, year: 1950, money: STARTING_MONEY, garage: Vec::new(), tournaments: BTreeMap::new(), eras_completed: Vec::new(), finale: None, evolution_complete: false, missions: Vec::new(), promotions_accepted: Vec::new() }
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
    pub fn default_save_path() -> PathBuf {
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
        c.year <= self.year && self.year <= c.year_end.max(c.year) && !matches!(c.dealer_flag_204, 2 | 3) && c.dealer_flag_1f4 == 0
    }

    pub fn dealer_price(&self, c: &CarCatalogEntry) -> u32 {
        c.dealer_price(self.year)
    }

    pub fn buy_car(&mut self, tables: &CareerTables, car_id: u16, paint_set: u8) -> Result<(), CareerError> {
        let car = tables.car(car_id).ok_or(CareerError::UnknownCar { id: car_id })?;
        let price = self.dealer_price(car) as i64;
        if self.money < price {
            return Err(CareerError::NoMoney { need: price, have: self.money });
        }
        self.money -= price;
        self.garage.push(OwnedCar::new(car_id, paint_set, price as u32));
        Ok(())
    }

    /// Current value of garage car `idx` (`0x4F8E50`, HIGH): catalogue price × year curve, mileage
    /// and damage deductions, +1 % per win, installed parts at their aged value, floor 5 %.
    pub fn car_value(&self, tables: &CareerTables, idx: usize) -> Option<i64> {
        let owned = self.garage.get(idx)?;
        let car = tables.car(owned.car_id)?;
        let base = (car.price as f64 * car.value_pct(self.year) as f64 / 100.0).round();
        let pct_m = match owned.mileage {
            0..=5000 => 0.0,
            5001..=10000 => -10.0,
            10001..=15000 => -15.0,
            15001..=20000 => -25.0,
            _ => -30.0,
        };
        let d = owned.damage_pct;
        let pct_d = if d <= 5.0 { -1.0 } else if d <= 10.0 { -6.0 } else if d <= 15.0 { -11.0 } else if d <= 25.0 { -16.0 } else { -26.0 };
        let parts: f64 = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .map(|p| part_value(p, self.year, owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)))
            .sum();
        let value = base + base * pct_m / 100.0 + base * pct_d / 100.0 + base * owned.wins as f64 / 100.0 + parts;
        Some(value.max((0.05 * base).round()) as i64)
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
        owned.damage_pct = zones.iter().sum::<f32>() / 10.0;
        for (id, d) in add {
            match owned.part_state.iter_mut().find(|(pid, _)| *pid == id) {
                Some((_, s)) => *s += d,
                None => owned.part_state.push((id, d)),
            }
        }
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
        owned.damage_pct = owned.damage.iter().sum::<f32>() / 10.0;
        self.money -= cost;
        Ok(cost)
    }

    /// `SELLCAR` / `0x4F91C0(car, ask)`: list garage car `idx` on the used-car market at `ask`
    /// (`car+0x68C`, `+0x691 = 1`); the car stays owned until an offer is accepted. The original
    /// refuses to list a car when fewer than two remain (0x50FCB0).
    pub fn list_for_sale(&mut self, idx: usize, ask: u32) -> Result<(), CareerError> {
        if self.garage.len() < 2 {
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
    pub fn accept_offer(&mut self, idx: usize) -> Result<i64, CareerError> {
        let car = self.garage.get(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        let offer = car.offer.ok_or(CareerError::NoOffer)? as i64;
        self.money = (self.money + offer).clamp(-999_999_999, 999_999_999);
        self.garage.remove(idx);
        Ok(offer)
    }

    /// `0x4F7E90` (run after every career race): score each listed car and, depending on the
    /// bracket, make an offer. `rand` yields the original's 16-bit LCG values.
    pub fn generate_offers(&mut self, tables: &CareerTables, rand: &mut impl FnMut() -> u32) {
        for idx in 0..self.garage.len() {
            let Some(ask) = self.garage[idx].asking_price else { continue };
            if self.garage[idx].offer.is_some() {
                continue;
            }
            let Some(value) = self.car_value(tables, idx) else { continue };
            if value <= 0 {
                continue;
            }
            let car = &self.garage[idx];
            let model_year = tables.car(car.car_id).map(|c| c.year).unwrap_or(self.year);
            let age = self.year.saturating_sub(model_year);
            let mileage_pct = mileage_score(age, car.mileage);
            let score = (((value - ask as i64) as f64 * 100.0 / value as f64).round() as i64) - (car.damage_pct.round() as i64) + 100 + mileage_pct + (car.wins / 5) as i64;
            // bracket table 0x5B7D88
            let (pct, immediate) = match score {
                s if s <= 83 => (20, true),
                s if s <= 89 => (40, true),
                s if s <= 95 => (60, true),
                s if s <= 99 => (80, true),
                s if s <= 104 => (80, false),
                s if s <= 109 => (90, false),
                _ => (100, false),
            };
            let offer = if immediate {
                // asking above the value / damaged: an offer around the value is made at once
                let jitter = ((rand() % 601) as f64 - 300.0) * 1e-4 * value as f64;
                (ask as i64).min(value + jitter.round() as i64).max(0) as u32
            } else if (rand() % 100) < pct {
                ask
            } else {
                continue;
            };
            self.garage[idx].offer = Some(offer);
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

    /// Buy and install `part_id` on garage car `idx`; a part of the same type and type name is
    /// replaced (MEDIUM: the original keeps replaced parts as spares).
    pub fn buy_part(&mut self, tables: &CareerTables, idx: usize, part_id: i16) -> Result<(), CareerError> {
        let part = tables.parts.iter().find(|p| p.id == part_id).ok_or(CareerError::UnknownPart { id: part_id })?;
        let car = self.garage.get(idx).and_then(|c| tables.car(c.car_id)).ok_or(CareerError::UnknownCar { id: 0 })?;
        if part_family(car) != Some(part.family.as_str()) {
            return Err(CareerError::PartMismatch { id: part_id });
        }
        let price = part.price as i64;
        if self.money < price {
            return Err(CareerError::NoMoney { need: price, have: self.money });
        }
        self.money -= price;
        let owned = &mut self.garage[idx];
        owned.parts.retain(|id| tables.parts.iter().find(|p| p.id == *id).map(|p| !(p.kind == part.kind && p.kind_name == part.kind_name)).unwrap_or(false));
        owned.parts.push(part_id);
        Ok(())
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
    /// (0 = did not finish). `garage_idx` = the player's car (mileage / wins).
    pub fn record_event(&mut self, tables: &CareerTables, t: &Tournament, event: usize, order: &[usize], times: &[u64; 8], garage_idx: Option<usize>) -> EventOutcome {
        let ev = &t.events[event];
        let mut points = [0u8; 8];
        let mut earned = [0u32; 8];
        let mut position = 8u8;
        for (pos0, &driver) in order.iter().enumerate() {
            let pos = (pos0 + 1).min(8) as u8;
            if driver < 8 {
                points[driver] = 9 - pos;
                let bonus = match pos {
                    1 => ev.bonus,
                    2 => ev.bonus / 2,
                    3 => ev.bonus / 4,
                    _ => 0,
                };
                earned[driver] = ev.prize[pos as usize - 1] + bonus;
            }
            if driver == 0 {
                position = pos;
            }
        }
        let prize = ev.prize[position as usize - 1];
        let bonus = earned[0] - prize;
        self.money = (self.money + prize as i64 + bonus as i64).clamp(-999_999_999, 999_999_999);
        if let Some(car) = garage_idx.and_then(|i| self.garage.get_mut(i)) {
            car.mileage += 3000;
            if position == 1 {
                car.wins += 1;
            }
        }
        let result = EventResult { position, prize, bonus, points, earned, times: *times };
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
        st.events.clear();
        st.entered = false;
        if first && let Some(next) = YEAR_TABLE.iter().find(|y| **y > self.year) {
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

/// Mileage score of a listed car (`0x4F7E90` tables 0x5B7C88.., CONFIRMED brackets by age).
fn mileage_score(age: u16, km: u32) -> i64 {
    let (good, mid, ok, limit) = match age {
        0..=10 => (500, 2000, 5000, 5000),
        11..=20 => (500, 2000, 5000, 10000),
        21..=30 => (500, 2000, 5000, 15000),
        _ => (500, 2000, 5000, 20000),
    };
    let (a, b, c) = match age {
        0..=10 => (3, 2, 1),
        11..=20 => (4, 3, 2),
        21..=30 => (8, 6, 4),
        _ => (15, 10, 5),
    };
    if km <= good { a } else if km <= mid { b } else if km <= ok { c } else if km > limit { -1 } else { c }
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
pub fn part_aged_price(p: &Part, year: u16) -> i64 {
    let age = (year as i64 - p.year as i64).max(0) as usize;
    let sum: u32 = PART_AGE_PCT.iter().take(age).sum();
    let price = p.price as i64;
    price + price * sum as i64 / 100
}

/// `PartValue` 0x518900 (CONFIRMED): `price + price·sum/100 − (100 − c)·aged/100` with
/// `c = clamp((int)(2·state + 20), 0, 100)`, floored at 0 — so a brand-new part (state 0, age 0) is
/// worth a fifth of its price, and age lifts that slowly.
fn part_value(p: &Part, year: u16, state: f32) -> f64 {
    let aged = part_aged_price(p, year);
    let c = ((2.0 * state + 20.0) as i64).clamp(0, 100);
    let price = p.price as i64;
    let sum: u32 = PART_AGE_PCT.iter().take((year as i64 - p.year as i64).max(0) as usize).sum();
    let v = price * sum as i64 / 100 - (100 - c) * aged / 100 + price;
    v.max(0) as f64
}

/// Standings `0x503660` / sort `0x502880`: points desc, money earned desc, total time asc; time-ranked
/// club events: time asc, money asc. Drivers without a finishing time sort last within a tie.
fn standings_of(events: &[EventResult], time_ranked: bool) -> Vec<(usize, u32, u32, u64)> {
    let mut rows: Vec<(usize, u32, u32, u64)> = (0..8).map(|d| (d, 0u32, 0u32, 0u64)).collect();
    for e in events {
        for d in 0..8 {
            rows[d].1 += e.points[d] as u32;
            rows[d].2 += e.earned[d];
            rows[d].3 += e.times[d];
        }
    }
    let time_key = |t: u64| if t == 0 { u64::MAX } else { t };
    if time_ranked {
        rows.sort_by(|a, b| time_key(a.3).cmp(&time_key(b.3)).then(a.2.cmp(&b.2)));
    } else {
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(time_key(a.3).cmp(&time_key(b.3))));
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

    #[test]
    fn points_prizes_and_standings() {
        let t = tournament(1, 2, false);
        let tables = tables(vec![t.clone(), tournament(31, 1, true)]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        assert_eq!(c.enter(&tables, &t).unwrap(), 0);
        assert_eq!(c.money, STARTING_MONEY - 500);
        let times = [6400, 6500, 6600, 6700, 6800, 6900, 7000, 7100];
        let o = c.record_event(&tables, &t, 0, &[0, 1, 2, 3, 4, 5, 6, 7], &times, Some(0));
        assert_eq!((o.result.position, o.result.prize, o.result.bonus), (1, 1000, 400));
        assert_eq!(o.result.points, [8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(o.result.earned[1], 800 + 200);
        assert_eq!(c.garage[0].mileage, 3000);
        assert_eq!(c.garage[0].wins, 1);
        assert_eq!(c.enter(&tables, &t).unwrap(), 1, "fee not paid again");
        assert_eq!(c.money, STARTING_MONEY - 500 + 1400);
        let o = c.record_event(&tables, &t, 1, &[1, 0, 2, 3, 4, 5, 6, 7], &times, Some(0));
        assert_eq!((o.result.position, o.result.prize, o.result.bonus), (2, 800, 200));
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
        let o = c.record_event(&tables, &t, 0, &[1, 2, 3, 4, 0, 5, 6, 7], &[0; 8], Some(0));
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
        // accepting an offer pays and removes the car
        c.garage[0].offer = Some(12_345);
        let money = c.money;
        assert_eq!(c.accept_offer(0).unwrap(), 12_345);
        assert_eq!(c.money, money + 12_345);
        assert_eq!(c.garage.len(), 1);
        assert!(matches!(c.accept_offer(0), Err(CareerError::NoOffer)));
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
        assert_eq!(mileage_score(35, 18_000), 5);
        assert_eq!(mileage_score(35, 25_000), -1);
    }

    #[test]
    fn save_roundtrip() {
        let mut c = Career::default();
        c.garage.push(OwnedCar { car_id: 76, paint_set: 2, parts: vec![13], mileage: 3000, wins: 1, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, purchase_price: 11_000, asking_price: Some(9_000), offer: None });
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
        c.buy_car(&tables, car.id, 0).unwrap();
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
        assert_eq!(c.garage[0].damage_pct, 60.0);
        let state = c.garage[0].part_state.iter().find(|(id, _)| *id == part.id).map(|(_, s)| *s).unwrap();
        assert!((state - (1.5 - 0.005 * part.condition) * 60.0).abs() < 0.01, "{state}");

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
