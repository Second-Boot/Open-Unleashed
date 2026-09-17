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
//! the `nfs5.car` record). It does **not** write the `accessor` block: which concrete accessor
//! class backs each of the ~220 names is UNKNOWN (§9), and an unknown payload cannot be sized, so
//! the block is omitted — §9.6 says the original then falls back to its default options. Because
//! of that, and because a few port-side bookkeeping fields have no home in the original's format
//! (see [`career_from_sav`]), the port's own JSON career remains the live save and the `.sav` is
//! written beside it.

use crate::career::{Career, CareerTables, EventResult, OwnedCar, TournamentState};
use nfs_formats::career::{Era, MISSION_RECORD_SIZE, PART_RECORD_SIZE, TOURNAMENT_RECORD_SIZE};
use nfs_formats::fedata::CAR_RECORD_SIZE;
use nfs_formats::sav::{PlayerInfo, SaveFile, SavedCar, SavedPart, TournamentModule, FE_CAR_IMAGE};
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
    // Only the part id and the float at part+0x164 survive a save/load (§4.7); a freshly installed
    // part carries the value from its `nfs5.prt` record, so that is what is written.
    car.installed = c
        .parts
        .iter()
        .map(|id| {
            let state = tables
                .parts
                .iter()
                .position(|p| p.id == *id)
                .and_then(|i| tables.raw.prt.get(i * PART_RECORD_SIZE + 0x164..i * PART_RECORD_SIZE + 0x168))
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .unwrap_or(0.0);
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
        damage_pct: 0.0,
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
        put_i16(&mut t, base + trn::COMPLETED, i16::from(st.completed));
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
        let completed = get_i16(table, base + trn::COMPLETED) != 0;
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
            out.insert(tour.id, TournamentState { entered: !events.is_empty(), events, completed, final_rank: None, prize: 0, failed_attempts: 0 });
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

/// Build the original's save game from the port's career state.
pub fn career_to_sav(c: &Career, tables: &CareerTables, profile: &Profile) -> SaveFile {
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
    SaveFile {
        missions: Some(mission_table(tables, c)),
        parts_market: None,
        tournament: Some(tournament),
        player: Some(player),
        track_records: true,
        used_cars: None,
        personalities: None,
        knockout: None,
        cd_player: None,
        accessors: None,
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
