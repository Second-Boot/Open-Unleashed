//! The original save-game layout: profiles, `player.lst` and the `.sav` container
//! (`RE/notes/savegame.md`, CONFIRMED unless noted).
//!
//! **Where the files go.** The original keeps everything in `<gamedir>\SaveData\`, created at
//! start-up by 0x4B6AA0, and uses no other location. This port treats the installation as
//! read-only reference material, so the same directory tree lives under
//! `~/Library/Application Support/PorscheUnleashed/SaveData/` (override with `NFS5_SAVE_DIR`).
//! Everything *inside* it — file names, `player.lst`, `nfs5.trk`, the `.sav` byte layout — is the
//! original's.
//!
//! **A profile is a file** (§2): the stem is `<picture-digit><Name>`, the digit being the driver
//! picture 0..9 (`[0x5DB748]`, the same value that is stored inside `PlayerInfo`, *not* a format
//! version). `XXDefXX.sav` is the new-career template and is hidden from the list; the list is
//! sorted with the original's comparator `strcmp(a+1, b+1)`, i.e. by name, ignoring the digit, and
//! a name may not be reused with a different digit.
//!
//! **What the port writes.** [`career_to_sav`] fills `PlayerInfo`, `Tournament` and `Missions` —
//! the three modules that carry the career (§9.4) — from the port's own career state, embedding
//! the same record images the original does (`nfs5.trn`, `nfs5.fac`, the FE car object built on
//! the `nfs5.car` record), plus an `accessor` block holding the values the port actually has
//! (§6.2a gives the class, and so the payload shape, of all 221 registered names). A subset is
//! legal: the original looks every name up in its own registry and anything absent keeps its
//! default (§9.6). Because a few port-side bookkeeping fields still have no home in the format
//! (see [`career_from_sav`]), the port's own JSON career remains the live save and the `.sav` is
//! written beside it.

use crate::career::{Career, CareerTables, EventResult, OwnedCar, TournamentState};
use nfs_formats::career::{Era, MISSION_RECORD_SIZE, PART_RECORD_SIZE, TOURNAMENT_RECORD_SIZE};
use nfs_formats::fedata::CAR_RECORD_SIZE;
use nfs_formats::sav::{AccessorBlock, AccessorValue, KnockoutState, PlayerInfo, SaveFile, SavedCar, SavedPart, TournamentModule, FE_CAR_IMAGE};
use std::path::{Path, PathBuf};

/// `[0x5D8F2C]` / `[0x5DB748]` default driver picture.
pub const DEFAULT_PICTURE: u8 = 8;
/// The template profile the original writes at start-up and clones for a new career (§2.3).
pub const TEMPLATE_STEM: &str = "XXDefXX";

/// `SaveData\`, relocated out of the read-only installation (see the module docs).
pub fn save_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NFS5_SAVE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Library").join("Application Support").join("PorscheUnleashed").join("SaveData")
}

/// One profile: the file `SaveData\<picture><name>.sav`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Driver picture 0..9 — the leading digit of the file name.
    pub picture: u8,
    pub name: String,
}

impl Profile {
    pub fn new(name: impl Into<String>, picture: u8) -> Self {
        Self { picture: picture.min(9), name: name.into() }
    }
    /// `sprintf("%d%s", pictureIdx, name)` (0x4DEDA0).
    pub fn stem(&self) -> String {
        format!("{}{}", self.picture, self.name)
    }
    pub fn path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.sav", self.stem()))
    }
    /// The reverse of the `%d%s`: `LoadInfo` takes the name as `stem + 1` (0x4DEC26).
    pub fn from_stem(stem: &str) -> Option<Self> {
        let mut c = stem.chars();
        let d = c.next()?.to_digit(10)? as u8;
        Some(Self { picture: d, name: c.as_str().to_string() })
    }
}

/// `RebuildUserList` 0x4DEF60: every `*.sav` except the template, sorted by `strcmp(a+1, b+1)`.
pub fn list_profiles(dir: &Path) -> Vec<Profile> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.split('.').next() else { continue };
        if !name.to_lowercase().ends_with(".sav") || stem.len() >= 0x64 || stem == TEMPLATE_STEM {
            continue;
        }
        if let Some(p) = Profile::from_stem(stem) {
            out.push(p);
        }
    }
    // 0x4DF370 compares the stems from their second character, so the digit never orders the list.
    out.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    out
}

/// `IsProfileNameTaken` 0x4DE510: a name is taken if any of the ten digits already uses it, and
/// the empty name is rejected.
pub fn name_taken(dir: &Path, name: &str) -> bool {
    name.is_empty() || (0..=9).any(|d| dir.join(format!("{d}{name}.sav")).exists())
}

/// `player.lst` (0x4DEC7B / 0x4DEF70): the raw stem of the profile to auto-select, no terminator,
/// ignored when longer than 100 bytes.
pub fn read_player_lst(dir: &Path) -> Option<String> {
    let d = std::fs::read(dir.join("player.lst")).ok()?;
    (d.len() <= 0x64).then(|| d.iter().map(|&b| b as char).collect())
}

pub fn write_player_lst(dir: &Path, stem: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("player.lst"), stem.as_bytes())
}

/// `DeleteProfile` 0x4DE8A0: the `.sav` plus the two per-profile HUD copies, which carry the name
/// **without** the digit (`inc esi` at 0x4DE8F8).
pub fn delete_profile(dir: &Path, p: &Profile) -> std::io::Result<()> {
    let _ = std::fs::remove_file(dir.join(format!("{}1.hud", p.name)));
    let _ = std::fs::remove_file(dir.join(format!("{}2.hud", p.name)));
    std::fs::remove_file(p.path(dir))
}

// ---------------------------------------------------------------------------------------------
// FE car object
// ---------------------------------------------------------------------------------------------

/// Offsets inside the FE car object (`FECar::init` 0x4EEBA0 / 0x4EEC55, CONFIRMED).
///
/// `savegame.md` §4.7 says the image "begins with the 1648-byte `nfs5.car` record"; reading the
/// constructor shows it is off by four — `car+0x00` is a per-object instance number (0x4EEBFE
/// picks the lowest value no other car uses) and the `rep movsd` of 0x19C dwords lands the record
/// at `car+0x04`.
pub mod car {
    /// `car+0x04`, 0x670 bytes: the `nfs5.car` record.
    pub const RECORD: usize = 0x04;
    /// The six values `FECar::init` takes after the record. Which front-end accessor owns which
    /// slot is HIGH, not CONFIRMED: the six `OwnedCar.*` accessors are constructed in the order
    /// ExtColour, IntColour, S1Colour, S2Colour, PackageNum, DecalNum (0x4F1EF7…0x4F2004) into
    /// consecutive object slots, and the six getters sit in the matching consecutive vtable slots
    /// +0x74…+0x88 of `FEOwnedCarAccessor` (vtable 0x5B7868) reading +0x674…+0x688 in order.
    pub const EXT_COLOUR: usize = 0x674;
    pub const INT_COLOUR: usize = 0x678;
    pub const S1_COLOUR: usize = 0x67C;
    pub const S2_COLOUR: usize = 0x680;
    pub const PACKAGE_NUM: usize = 0x684;
    pub const DECAL_NUM: usize = 0x688;
    /// Career fields (`career.md` §8/§10, CONFIRMED).
    pub const ASKING_PRICE: usize = 0x68C;
    pub const GONE: usize = 0x690;
    pub const LISTED: usize = 0x691;
    pub const OFFER_PENDING: usize = 0x692;
    pub const OFFER: usize = 0x694;
    pub const MILEAGE: usize = 0x698;
    pub const WINS: usize = 0x69C;
    /// 1 = the player's car, 0 = a used-car-market car.
    pub const OWNED: usize = 0x6A0;
    pub const PURCHASE_PRICE: usize = 0x6A4;
    /// The 17 setup floats and the backup the setup screen restores from (`career.md` §12); the two
    /// blocks end exactly at the 0x758-byte image.
    pub const SETUP: usize = 0x6D0;
    pub const SETUP_BACKUP: usize = 0x714;
}

/// Build the FE car image the save stores for one garage car.
fn car_image(tables: &CareerTables, c: &OwnedCar, instance: u32) -> SavedCar {
    let mut image = vec![0u8; FE_CAR_IMAGE];
    image[0..4].copy_from_slice(&instance.to_le_bytes());
    if let Some(i) = tables.catalog.iter().position(|e| e.id == c.car_id) {
        let base = i * CAR_RECORD_SIZE;
        if let Some(rec) = tables.raw.car.get(base..base + CAR_RECORD_SIZE) {
            image[car::RECORD..car::RECORD + CAR_RECORD_SIZE].copy_from_slice(rec);
        }
    }
    let mut car = SavedCar::new(image);
    if let Some(p) = tables.car(c.car_id).and_then(|e| e.paint_sets.get(c.paint_set as usize)) {
        car.set_field_u32(car::EXT_COLOUR, p.packed[0]);
        car.set_field_u32(car::S1_COLOUR, p.packed[1]);
        car.set_field_u32(car::S2_COLOUR, p.packed[2]);
        car.set_field_u32(car::PACKAGE_NUM, p.package);
    }
    car.set_field_u32(car::ASKING_PRICE, c.asking_price.unwrap_or(0));
    car.set_field_u8(car::LISTED, u8::from(c.asking_price.is_some()));
    car.set_field_u8(car::OFFER_PENDING, u8::from(c.offer.is_some()));
    car.set_field_u32(car::OFFER, c.offer.unwrap_or(0));
    car.set_field_u32(car::MILEAGE, c.mileage);
    car.set_field_u32(car::WINS, c.wins);
    car.set_field_u8(car::OWNED, 1);
    car.set_field_u32(car::PURCHASE_PRICE, c.purchase_price);
    // the setup and the copy the screen's "cancel" restores from (0x4EF4C0/0x4EF4E0)
    for (i, v) in c.setup.iter().enumerate() {
        car.set_field_u32(car::SETUP + i * 4, v.to_bits());
        car.set_field_u32(car::SETUP_BACKUP + i * 4, v.to_bits());
    }
    // Only the part id and the float at part+0x164 survive a save/load (§4.7). That float is the
    // damage the part carries, so the port writes what the car has accumulated and falls back to
    // the `nfs5.prt` record for a part that has never been raced.
    car.installed = c
        .parts
        .iter()
        .map(|id| {
            let state = c.part_state.iter().find(|(pid, _)| pid == id).map(|(_, s)| *s).unwrap_or_else(|| {
                tables
                    .parts
                    .iter()
                    .position(|p| p.id == *id)
                    .and_then(|i| tables.raw.prt.get(i * PART_RECORD_SIZE + 0x164..i * PART_RECORD_SIZE + 0x168))
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .unwrap_or(0.0)
            });
            SavedPart { id: *id as i32, state }
        })
        .collect();
    car
}

fn owned_from_image(tables: &CareerTables, car: &SavedCar) -> OwnedCar {
    let car_id = u16::from_le_bytes([car.image.get(car::RECORD).copied().unwrap_or(0), car.image.get(car::RECORD + 1).copied().unwrap_or(0)]);
    let ext = car.field_u32(car::EXT_COLOUR);
    let package = car.field_u32(car::PACKAGE_NUM);
    let paint_set = tables
        .car(car_id)
        .and_then(|e| e.paint_sets.iter().position(|p| p.packed[0] == ext && p.package == package))
        .unwrap_or(0) as u8;
    OwnedCar {
        car_id,
        paint_set,
        parts: car.installed.iter().map(|p| p.id as i16).collect(),
        mileage: car.field_u32(car::MILEAGE),
        wins: car.field_u32(car::WINS),
        // the container carries the damage a car has taken as the state of each of its parts
        damage_pct: 0.0,
        damage: [0.0; 10],
        part_state: car.installed.iter().map(|p| (p.id as i16, p.state)).collect(),
        setup: std::array::from_fn(|i| f32::from_bits(car.field_u32(car::SETUP + i * 4))),
        purchase_price: car.field_u32(car::PURCHASE_PRICE),
        asking_price: (car.field_u8(car::LISTED) != 0).then(|| car.field_u32(car::ASKING_PRICE)),
        offer: (car.field_u8(car::OFFER_PENDING) != 0).then(|| car.field_u32(car::OFFER)),
    }
}

// ---------------------------------------------------------------------------------------------
// Tournament / Missions images
// ---------------------------------------------------------------------------------------------

/// Event-record offsets inside an `nfs5.trn` tournament (`career.md` §2, CONFIRMED).
mod trn {
    pub const EVENTS: usize = 0x3A0;
    pub const EVENT: usize = 0xA4;
    pub const DONE: usize = 0x30;
    pub const POINTS: usize = 0x32;
    pub const TIMES: usize = 0x44;
    pub const BONUS: usize = 0x64;
    pub const PRIZE: usize = 0x84;
    pub const COMPLETED: usize = 0xD3C;
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i16(buf: &mut [u8], at: usize, v: i16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn get_u32(buf: &[u8], at: usize) -> u32 {
    buf.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}
fn get_i16(buf: &[u8], at: usize) -> i16 {
    buf.get(at..at + 2).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
}

/// The `nfs5.trn` image with the port's tournament state patched into the runtime fields.
fn tournament_table(tables: &CareerTables, c: &Career) -> Vec<u8> {
    let mut t = tables.raw.trn.clone();
    for (i, tour) in tables.tournaments.iter().enumerate() {
        let base = i * TOURNAMENT_RECORD_SIZE;
        if base + TOURNAMENT_RECORD_SIZE > t.len() {
            break;
        }
        let Some(st) = c.tournaments.get(&tour.id) else { continue };
        put_i16(&mut t, base + trn::COMPLETED, st.completions.min(i16::MAX as u32) as i16);
        for (e, r) in st.events.iter().enumerate() {
            let ev = base + trn::EVENTS + e * trn::EVENT;
            if ev + trn::EVENT > t.len() {
                break;
            }
            t[ev + trn::DONE] = 1;
            for d in 0..8 {
                put_i16(&mut t, ev + trn::POINTS + d * 2, r.points[d] as i16);
                put_u32(&mut t, ev + trn::TIMES + d * 4, r.times[d] as u32);
                // The original keeps bonus and prize apart; the port tracks their sum per driver
                // and both halves only for the player, so the player's row is exact and the
                // opponents' whole earnings are recorded as prize money.
                let (bonus, prize) = if d == 0 { (r.bonus, r.prize) } else { (0, r.earned[d]) };
                put_u32(&mut t, ev + trn::BONUS + d * 4, bonus);
                put_u32(&mut t, ev + trn::PRIZE + d * 4, prize);
            }
        }
    }
    t
}

fn tournaments_from_table(tables: &CareerTables, table: &[u8]) -> std::collections::BTreeMap<i16, TournamentState> {
    let mut out = std::collections::BTreeMap::new();
    for (i, tour) in tables.tournaments.iter().enumerate() {
        let base = i * TOURNAMENT_RECORD_SIZE;
        if base + TOURNAMENT_RECORD_SIZE > table.len() {
            break;
        }
        let completions = get_i16(table, base + trn::COMPLETED).max(0) as u32;
        let completed = completions != 0;
        let mut events = Vec::new();
        for e in 0..tour.events.len() {
            let ev = base + trn::EVENTS + e * trn::EVENT;
            if ev + trn::EVENT > table.len() || table[ev + trn::DONE] == 0 {
                break;
            }
            let mut r = EventResult { position: 1, prize: get_u32(table, ev + trn::PRIZE), bonus: get_u32(table, ev + trn::BONUS), points: [0; 8], earned: [0; 8], times: [0; 8] };
            for d in 0..8 {
                r.points[d] = get_i16(table, ev + trn::POINTS + d * 2).clamp(0, 9) as u8;
                r.times[d] = get_u32(table, ev + trn::TIMES + d * 4) as u64;
                r.earned[d] = get_u32(table, ev + trn::PRIZE + d * 4) + get_u32(table, ev + trn::BONUS + d * 4);
            }
            // Position is not stored; it is `9 − points` for the player (0x503960).
            r.position = (9u8).saturating_sub(r.points[0]).max(1);
            events.push(r);
        }
        if completed || !events.is_empty() {
            out.insert(tour.id, TournamentState { entered: !events.is_empty(), events, completed, completions, final_rank: None, prize: 0, failed_attempts: 0 });
        }
    }
    out
}

/// The `nfs5.fac` image with the per-mission result marker at `fac+0xE8` patched in.
fn mission_table(tables: &CareerTables, c: &Career) -> Vec<u8> {
    let mut f = tables.raw.fac.clone();
    for (i, r) in c.missions.iter().enumerate() {
        let at = i * MISSION_RECORD_SIZE + 0xE8;
        if at + 2 <= f.len() {
            put_i16(&mut f, at, *r);
        }
    }
    f
}

// ---------------------------------------------------------------------------------------------
// Career <-> .sav
// ---------------------------------------------------------------------------------------------

/// The money clamp of 0x5012E0.
pub const MONEY_LIMIT: i64 = 999_999_999;

/// What the port knows about a knockout series in progress, for the `Knockout` module (§4.6 /
/// `modes.md` §5.2). `None` writes the module with the series switched off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KnockoutProgress {
    pub active: bool,
    pub race_index: u32,
    pub opponents: u32,
    /// index 0 is the player
    pub alive: Vec<bool>,
    /// 1-based grid places for the next race, index 0 the player
    pub start_pos: Vec<u32>,
    /// the personality ids drawn for opponents 1..n
    pub personalities: Vec<i16>,
}

impl KnockoutProgress {
    fn to_state(&self) -> KnockoutState {
        let mut k = KnockoutState { active: self.active, race_index: self.race_index as i32, opponents: self.opponents as i32, ..Default::default() };
        for (i, a) in self.alive.iter().take(16).enumerate() {
            k.alive[i] = u8::from(*a);
        }
        for (i, p) in self.personalities.iter().take(8).enumerate() {
            k.personalities[i] = *p as i32;
        }
        k.start_pos = [-1; 9];
        for (i, p) in self.start_pos.iter().take(9).enumerate() {
            k.start_pos[i] = *p as i32;
        }
        k.last_pos = [-1; 9];
        k
    }
}

/// Build the original's save game from the port's career state.
pub fn career_to_sav(c: &Career, tables: &CareerTables, profile: &Profile) -> SaveFile {
    career_to_sav_with(c, tables, profile, None, &[], &SavedSettings::default())
}

/// The same with a knockout series, the personality ids in play and the CD player's track list.
/// The front-end settings the port keeps, for the accessor block (`savegame.md` §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SavedSettings {
    /// `Audio.MusicLevel` / `SfxLevel` / `EngineLevel` / `SpeechLevel` / `AmbientLevel`, 0..127
    pub music: u8,
    pub sfx: u8,
    pub engine: u8,
    pub speech: u8,
    pub ambient: u8,
    /// `Audio.Quality`
    pub audio_quality: u8,
    /// `Graphics.ViewDistance` as the original's index into {0.5, 0.67, 0.83, 1.0}
    pub view_distance: u8,
    /// `Track1.Laps` / `Game.NumOpponents` / `Track1.Traffic`
    pub laps: u32,
    pub opponents: u32,
    pub traffic: u32,
    /// `Game.NumKOOpponents` — a **separate** accessor from `Game.NumOpponents`, registered at
    /// 0x4E556C with the same default of 4 but a minimum of 1 (`RE/notes/layouts.md`)
    pub ko_opponents: u32,
    /// the sixteen `FEControllerDeadZoneAccessor`s for **pad 0**, `[min, max]` per axis in the
    /// order they are registered at 0x51E4C0 — `XAxis`, `YAxis`, `ZAxis`, `XRot`, `YRot`, `ZRot`,
    /// `SliderA`, `SliderB` (`RE/notes/controls.md` §4.17)
    pub deadzone: [[u32; 2]; 8],
}

/// The eight axes in the order 0x51E4C0 registers their two accessors, which is `DIJOYSTATE`'s own
/// field order and therefore the order of the axis bitmask's bits.
pub const DEADZONE_AXES: [&str; 8] = ["XAxis", "YAxis", "ZAxis", "XRot", "YRot", "ZRot", "SliderA", "SliderB"];

impl Default for SavedSettings {
    fn default() -> Self {
        // `Game.NumOpponents` and `Game.NumKOOpponents` are both registered with a default of
        // **4** (0x4E5539, 0x4E556C), and every dead-zone accessor with 0 for a `.Min` and 0x80
        // for a `.Max` (0x51E546, 0x51E550)
        Self {
            music: 127,
            sfx: 127,
            engine: 127,
            speech: 127,
            ambient: 127,
            audio_quality: 1,
            view_distance: 3,
            laps: 2,
            opponents: 4,
            traffic: 0,
            ko_opponents: 4,
            deadzone: [[0, 0x80]; 8],
        }
    }
}

pub fn career_to_sav_with(c: &Career, tables: &CareerTables, profile: &Profile, knockout: Option<&KnockoutProgress>, playlist: &[u32], settings: &SavedSettings) -> SaveFile {
    let player = PlayerInfo {
        cars: c.garage.iter().enumerate().map(|(i, car)| car_image(tables, car, i as u32)).collect(),
        money: c.money.clamp(-MONEY_LIMIT, MONEY_LIMIT) as i32,
        picture: profile.picture as u32,
        player_name: profile.name.clone(),
        driver_name: profile.name.clone(),
        // The port has no first-time message gates; the original clears all three on a new career.
        seen_evolution_intro: 0,
        seen_factory_welcome: 0,
        seen_first_mission: 0,
        colours: [0; 8],
    };
    let tournament = TournamentModule {
        year: c.year as u32,
        remaining_tracks: 0,
        selected: 0,
        era_type: c.era.string_id(),
        current_event: -1,
        current_tournament: -1,
        club_mode: u32::from(c.evolution_complete),
        table: tournament_table(tables, c),
    };
    // The accessors the port has real values for, each in the shape its class implies (§6.1/§6.2a).
    let mut entries = vec![
        ("Chronicle.Year".into(), AccessorValue::Dword(c.year as u32)),
        ("Game.GameType".into(), AccessorValue::Dword(u32::from(c.evolution_complete))),
        ("OwnedCars.Car".into(), AccessorValue::OwnedCarSel { cars: c.garage.len() as u32, selection: 0 }),
        ("UserList.NameInput".into(), AccessorValue::Text(profile.name.clone())),
        ("Tournament.Data".into(), AccessorValue::Empty),
        ("Factory.Data".into(), AccessorValue::Empty),
        // the settings screens: every one of these is an `FEIntAccessor` or an
        // `FELocalizedTextAccessor`, so the payload is the plain dword of §6.1
        ("Audio.MusicLevel".into(), AccessorValue::Dword(settings.music as u32)),
        ("Audio.SfxLevel".into(), AccessorValue::Dword(settings.sfx as u32)),
        ("Audio.EngineLevel".into(), AccessorValue::Dword(settings.engine as u32)),
        ("Audio.SpeechLevel".into(), AccessorValue::Dword(settings.speech as u32)),
        ("Audio.AmbientLevel".into(), AccessorValue::Dword(settings.ambient as u32)),
        ("Audio.Quality".into(), AccessorValue::Dword(settings.audio_quality as u32)),
        ("Graphics.ViewDistance".into(), AccessorValue::Dword(settings.view_distance as u32)),
        ("Track1.Laps".into(), AccessorValue::Dword(settings.laps)),
        ("Game.NumOpponents".into(), AccessorValue::Dword(settings.opponents)),
        ("Track1.Traffic".into(), AccessorValue::Dword(settings.traffic)),
        ("Game.NumKOOpponents".into(), AccessorValue::Dword(settings.ko_opponents)),
    ];
    // The sixteen dead-zone accessors. Each holds **one value per pad** — `u32[16]` at
    // `this+0x28`, written with the selection in front (0x51BEC0 / 0x51BEF0, `savegame.md` §6.1) —
    // so this port's edited pad goes in slot 0 and the rest keep the factory's own default, which
    // is what an untouched pad has.
    for (n, axis) in DEADZONE_AXES.iter().enumerate() {
        for (k, suffix) in ["Min", "Max"].iter().enumerate() {
            let mut values = [if k == 0 { 0 } else { 0x80 }; 16];
            values[0] = settings.deadzone[n][k];
            entries.push((format!("{axis}.{suffix}"), AccessorValue::DeadZone { selection: 0, values }));
        }
    }
    // the selected car's setup, which the screen mirrors into `FEFloatAccessor`s — a float rides in
    // the same dword (`career.md` §12, `layouts.md`)
    if let Some(car) = c.garage.first() {
        for (name, v) in crate::career::CAR_SETUP_NAMES.iter().zip(car.setup.iter()) {
            entries.push(((*name).into(), AccessorValue::Dword(v.to_bits())));
        }
    }
    let accessors = AccessorBlock { entries };
    SaveFile {
        missions: Some(mission_table(tables, c)),
        parts_market: None,
        tournament: Some(tournament),
        player: Some(player),
        track_records: true,
        used_cars: None,
        // §4.6: the raw `[0x690410]` block, and §4.5's list of personality ids in play
        knockout: Some(knockout.map(|k| k.to_state()).unwrap_or_default().write()),
        personalities: Some(knockout.map(|k| k.personalities.iter().map(|p| *p as u16).collect()).unwrap_or_default()),
        // §4.8: the selected music tracks
        cd_player: Some(playlist.to_vec()),
        accessors: Some(accessors),
        order: Vec::new(),
    }
}

/// Read back what the original's format can hold. Port-side bookkeeping the format has no field
/// for is lost and left at its default: the per-car damage percentage, a tournament's
/// `final_rank` / `prize` / `failed_attempts`, `eras_completed`, a pending finale and
/// `promotions_accepted`. That is why the port's JSON career, not this file, is the live save.
pub fn career_from_sav(s: &SaveFile, tables: &CareerTables) -> Career {
    let mut c = Career::default();
    if let Some(p) = &s.player {
        c.money = p.money as i64;
        c.garage = p.cars.iter().map(|car| owned_from_image(tables, car)).collect();
    }
    if let Some(t) = &s.tournament {
        c.year = t.year as u16;
        c.era = match t.era_type {
            522 => Era::Golden,
            523 => Era::Modern,
            _ => Era::Classic,
        };
        c.evolution_complete = t.club_mode != 0;
        c.tournaments = tournaments_from_table(tables, &t.table);
    }
    if let Some(a) = &s.accessors {
        for (name, value) in &a.entries {
            if let ("Chronicle.Year", AccessorValue::Dword(y)) = (name.as_str(), value) {
                c.year = *y as u16;
            }
        }
    }
    if let Some(m) = &s.missions {
        c.missions = (0..m.len() / MISSION_RECORD_SIZE).map(|i| get_i16(m, i * MISSION_RECORD_SIZE + 0xE8)).collect();
    }
    c
}

/// Write `<dir>/<stem>.sav` and `player.lst`, as `SaveInfo` + `LoadInfo`'s tail do.
pub fn write_profile(dir: &Path, profile: &Profile, save: &SaveFile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(profile.path(dir), save.write())?;
    write_player_lst(dir, &profile.stem())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_stems_follow_the_original_naming() {
        let p = Profile::new("Hans", 3);
        assert_eq!(p.stem(), "3Hans");
        assert_eq!(p.path(Path::new("/s")), Path::new("/s/3Hans.sav"));
        assert_eq!(Profile::from_stem("3Hans"), Some(p));
        assert_eq!(Profile::from_stem("XXDefXX"), None, "the template has no digit");
    }

    #[test]
    fn the_list_is_sorted_by_name_not_by_digit() {
        let dir = std::env::temp_dir().join(format!("nfs5-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for stem in ["9Anna", "0Zoe", "4Mike", "XXDefXX"] {
            std::fs::write(dir.join(format!("{stem}.sav")), []).unwrap();
        }
        let names: Vec<String> = list_profiles(&dir).iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Anna", "Mike", "Zoe"], "the template is hidden and the digit does not order");
        assert!(name_taken(&dir, "Anna"), "the name is taken whatever the digit");
        assert!(!name_taken(&dir, "Otto"));
        assert!(name_taken(&dir, ""), "the empty name is rejected");
        write_player_lst(&dir, "4Mike").unwrap();
        assert_eq!(read_player_lst(&dir).as_deref(), Some("4Mike"));
        delete_profile(&dir, &Profile::new("Mike", 4)).unwrap();
        assert_eq!(list_profiles(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
