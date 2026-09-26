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
//! (§6.2a gives the class, and so the payload shape, of all 220 statically registered names). A subset is
//! legal: the original looks every name up in its own registry and anything absent keeps its
//! default (§9.6). Because a few port-side bookkeeping fields still have no home in the format
//! (see [`career_from_sav`]), the port's own JSON career remains the live save and the `.sav` is
//! written beside it.

use crate::career::{Career, CareerTables, EventResult, OwnedCar, TournamentState};
use nfs_formats::career::{Era, MISSION_RECORD_SIZE, PART_RECORD_SIZE, TOURNAMENT_RECORD_SIZE};
use nfs_formats::fedata::CAR_RECORD_SIZE;
use nfs_formats::sav::{AccessorBlock, AccessorValue, KnockoutState, PlayerInfo, SaveFile, SavedCar, SavedPart, TournamentModule, UsedCarMarket, FE_CAR_IMAGE};
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
///
/// The length test (0x4DF08B…0x4DF093, `strlen >= 0x64` skips) is on the **whole file name**:
/// the enumerator 0x59E270 builds `"%s%s"` of the root and the `"%s*.sav"` pattern, and 0x59FEB0
/// puts each `FindFirstFileA` / `FindNextFileA` result's `cFileName` (the find data's `+0x2C`,
/// `[esp+0x58]` there) into the set 0x59FFE0 walks — name and `.sav` together, no directory. So
/// a stem of 96 characters is already left out. *(Until 2026-09-26 the port tested the stem.)*
pub fn list_profiles(dir: &Path) -> Vec<Profile> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.split('.').next() else { continue };
        if !name.to_lowercase().ends_with(".sav") || name.len() >= 0x64 || stem == TEMPLATE_STEM {
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
/// constructor shows it is off by four — `car+0x00` is a per-object instance dword and the
/// `rep movsd` of 0x19C dwords lands the record at `car+0x04`.
///
/// *(Corrected 2026-09-26.)* The instance dword is not "the lowest value no other car uses": the
/// loop at 0x4EEBFE…0x4EEC1F walks the count of `[0x69188C]` but compares the **new car's own**
/// `[ebp+0]` (`mov esi, [ebp]` at 0x4EEC0E, `ebp` = this) with the candidate, so it comes out 1
/// when other cars exist and the fresh object's first dword is 0, and 0 otherwise — the heap's
/// contents decide. The loader copies the saved dword back verbatim. The port writes the car's
/// index (garage first, then the market): APPROXIMATION; the port reads nothing from it, and
/// what the original reads it for is not traced.
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

/// Build the FE car image the save stores for one garage car (`players = true`) or one car the
/// used-car market generated (`false`).
fn car_image(tables: &CareerTables, c: &OwnedCar, instance: u32, players: bool) -> SavedCar {
    let mut image = vec![0u8; FE_CAR_IMAGE];
    image[0..4].copy_from_slice(&instance.to_le_bytes());
    if let Some(i) = tables.catalog.iter().position(|e| e.id == c.car_id) {
        let base = i * CAR_RECORD_SIZE;
        if let Some(rec) = tables.raw.car.get(base..base + CAR_RECORD_SIZE) {
            image[car::RECORD..car::RECORD + CAR_RECORD_SIZE].copy_from_slice(rec);
        }
    }
    // The six appearance dwords (`savegame.md` §4.7a): the colours the paint shop's rows hold —
    // interior `rec+0x530 + 16·i`, stripes `rec+0x3F0` / `rec+0x3F4 + 16·i`, each packed
    // big-endian as the getters read them — the package index and the race number. A bought
    // car's rows are what the dealership had (0x501CF0: the interior chosen there, paint set 0's
    // stripes, package 0x508E70's 0).
    // *(Until 2026-09-25 the port wrote the paint set's first dword as the package — 0 or 205
    // over `nfs5.car`, not a package — the chosen set's stripes, no interior and decal 0.)*
    let be = |o: usize| image.get(car::RECORD + o..car::RECORD + o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let t = c.trim;
    let (s1, s2, int) = (be(0x3F0 + 16 * t.stripe1 as usize), be(0x3F4 + 16 * t.stripe2 as usize), be(0x530 + 16 * t.interior as usize));
    let mut car = SavedCar::new(image);
    // a colour that is not a row — a saved custom colour chosen in the paint shop — is written as
    // the dword itself, which is all the original ever keeps there (§4.7a, `OwnedCar::custom`)
    let [cx, ci, c1, c2] = c.custom;
    if let Some(p) = tables.car(c.car_id).and_then(|e| e.paint_sets.get(c.paint_set as usize)) {
        car.set_field_u32(car::EXT_COLOUR, cx.unwrap_or(p.packed[0]));
        car.set_field_u32(car::INT_COLOUR, ci.unwrap_or(int));
        car.set_field_u32(car::S1_COLOUR, c1.unwrap_or(s1));
        car.set_field_u32(car::S2_COLOUR, c2.unwrap_or(s2));
        car.set_field_u32(car::PACKAGE_NUM, t.package as u32);
        car.set_field_u32(car::DECAL_NUM, t.decal as i32 as u32);
    }
    car.set_field_u32(car::ASKING_PRICE, c.asking_price.unwrap_or(0));
    // a market car carries its price at `+0x68C` (0x4F8577) but is not "listed": that byte is the
    // player's own for-sale flag, and the constructor leaves it 0 (0x4EEC55…)
    car.set_field_u8(car::LISTED, u8::from(players && c.asking_price.is_some()));
    car.set_field_u8(car::OFFER_PENDING, u8::from(c.offer.is_some()));
    car.set_field_u32(car::OFFER, c.offer.unwrap_or(0));
    car.set_field_u32(car::MILEAGE, c.mileage);
    car.set_field_u32(car::WINS, c.wins);
    // 0x4F853E: a car the market generated has `+0x6A0 = 0`; the market's writer 0x4F7A10 stores
    // only those
    car.set_field_u8(car::OWNED, u8::from(players));
    car.set_field_u32(car::PURCHASE_PRICE, c.purchase_price);
    // the setup and the copy the screen's "cancel" restores from (0x4EF4C0/0x4EF4E0)
    for (i, v) in c.setup.iter().enumerate() {
        car.set_field_u32(car::SETUP + i * 4, v.to_bits());
        car.set_field_u32(car::SETUP_BACKUP + i * 4, c.setup_backup[i].to_bits());
    }
    // Only the part id and the float at part+0x164 survive a save/load (§4.7). That float is the
    // damage the part carries, so the port writes what the car has accumulated and falls back to
    // the `nfs5.prt` record for a part that has never been raced. The stock list `car+0x75C` goes
    // first (0x4EE8D0), each copy with its own `+0x164`.
    car.removed = c.stock.iter().map(|(id, state)| SavedPart { id: *id as i32, state: *state }).collect();
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
    // the rows back from the colours they hold; a colour no row holds — a custom one — is kept as
    // the colour with row 0 under it (`OwnedCar::custom`), a zero dword as row 0
    let rec = car.image.get(car::RECORD..car::RECORD + CAR_RECORD_SIZE).unwrap_or(&[]);
    let mut custom = [None; 4];
    let mut row = |slot: usize, base: usize, count_at: usize, v: u32| -> u8 {
        let n = rec.get(count_at..count_at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize).min(40);
        let at = (0..n).position(|i| rec.get(base + 16 * i..base + 16 * i + 4).is_some_and(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) == v));
        if at.is_none() && v != 0 {
            custom[slot] = Some(v);
        }
        at.unwrap_or(0) as u8
    };
    let paint_set = row(0, 0x3EC, 0x3E6, car.field_u32(car::EXT_COLOUR));
    let trim = crate::career::Trim {
        interior: row(1, 0x530, 0x528, car.field_u32(car::INT_COLOUR)),
        stripe1: row(2, 0x3F0, 0x3E6, car.field_u32(car::S1_COLOUR)),
        stripe2: row(3, 0x3F4, 0x3E6, car.field_u32(car::S2_COLOUR)),
        // the list has eight rows; anything else is an older port save's paint-set dword (0 / 205)
        package: Some(car.field_u32(car::PACKAGE_NUM)).filter(|p| *p < 8).unwrap_or(0) as u8,
        decal: (car.field_u32(car::DECAL_NUM) as i32).clamp(-1, 99) as i8,
    };
    // 0x4EE6A0 clears both lists and pushes the first n1 saved parts onto `+0x75C`, the rest onto
    // `+0x758` (0x4EE7F2 / 0x4EE817), each a fresh copy of the table record (0x49C470) with the
    // saved `+0x164`; an id the table lacks fails the load (0x4EE7BA), here it is dropped
    let known = |p: &&SavedPart| tables.parts.iter().any(|t| t.id as i32 == p.id);
    let mut owned = OwnedCar {
        car_id,
        paint_set,
        trim,
        custom,
        parts: car.installed.iter().filter(known).map(|p| p.id as i16).collect(),
        stock: car.removed.iter().filter(known).map(|p| (p.id as i16, p.state)).collect(),
        mileage: car.field_u32(car::MILEAGE),
        wins: car.field_u32(car::WINS),
        // the container carries the damage a car has taken as the state of each of its parts
        damage_pct: 0.0,
        damage: [0.0; 10],
        part_state: car.installed.iter().filter(known).map(|p| (p.id as i16, p.state)).collect(),
        setup: std::array::from_fn(|i| f32::from_bits(car.field_u32(car::SETUP + i * 4))),
        setup_backup: std::array::from_fn(|i| f32::from_bits(car.field_u32(car::SETUP_BACKUP + i * 4))),
        purchase_price: car.field_u32(car::PURCHASE_PRICE),
        asking_price: (car.field_u8(car::LISTED) != 0).then(|| car.field_u32(car::ASKING_PRICE)),
        offer: (car.field_u8(car::OFFER_PENDING) != 0).then(|| car.field_u32(car::OFFER)),
    };
    // 0x4EE853…0x4EE8AE: the loader ends by exchange-sorting the installed list `+0x758` — for i,
    // for j > i, swap when 0x49C350(v[j], v[i]) — whatever order the file had it in
    let rec = |id: i16| tables.parts.iter().find(|p| p.id == id);
    for i in 0..owned.parts.len() {
        for j in i + 1..owned.parts.len() {
            if let (Some(a), Some(b)) = (rec(owned.parts[j]), rec(owned.parts[i])) {
                if crate::career::part_less(a, b) {
                    owned.parts.swap(i, j);
                }
            }
        }
    }
    // a `.sav` the port wrote before 2026-09-25 has no stock list (`n1 = 0`) and only the bought
    // parts installed — port-only, see `OwnedCar::adopt_implicit_stock`
    owned.adopt_implicit_stock(tables);
    owned.damage_pct = crate::career::car_damage_pct(tables, &owned);
    owned
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
        for (e, r) in st.done_events().into_iter().zip(&st.events) {
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
            if ev + trn::EVENT > table.len() {
                break;
            }
            // every record whose done byte is set, wherever it is: the events are raced in any
            // order (0x504F00), so an undone one may sit before a done one
            if table[ev + trn::DONE] == 0 {
                continue;
            }
            let mut r = EventResult { position: 1, prize: get_u32(table, ev + trn::PRIZE), bonus: get_u32(table, ev + trn::BONUS), points: [0; 8], earned: [0; 8], times: [0; 8], event: Some(e as u8) };
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
    /// `+0x60`, where each car finished the last race — what a loaded block held; empty writes the
    /// −1s of a series that has run no race
    pub last_pos: Vec<i32>,
}

impl KnockoutProgress {
    /// The block as `Knockout`'s loader 0x4DDD60 leaves it: a `rep movsd` of 0x21 dwords into
    /// `[0x690410]`, nothing checked — every one of the sixteen `alive` bytes, the eight
    /// personality dwords and both nine-dword position rows, whatever the counts say.
    pub fn from_state(k: &KnockoutState) -> Self {
        Self {
            active: k.active,
            race_index: k.race_index as u32,
            opponents: k.opponents as u32,
            alive: k.alive.iter().map(|a| *a != 0).collect(),
            start_pos: k.start_pos.iter().map(|p| *p as u32).collect(),
            personalities: k.personalities.iter().map(|p| *p as i16).collect(),
            last_pos: k.last_pos.to_vec(),
        }
    }

    /// The block back, as the module writes it (the inverse of [`Self::from_state`]).
    pub fn to_state(&self) -> KnockoutState {
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
        for (i, p) in self.last_pos.iter().take(9).enumerate() {
            k.last_pos[i] = *p;
        }
        k
    }
}

/// One of `OpponentCar.Car`'s seven embedded FE cars (`FEOpponentCarAccessor + 0x24 + i·0x760`,
/// `modes.md` §1.1a): the record 0x4F4FA0 drew and the six values it handed `FECar::init`
/// 0x4EEBA0 — one paint set `k` for the exterior and both stripes, an interior row, a package and
/// a decal. The accessor's Save 0x4F56C0 writes its selection (0x4BC460) and then each car through
/// 0x4EE8D0; its Load 0x4F56F0 reads them back through 0x4EE6A0, and nothing redraws them until
/// the next 0x4F4FA0 (its refresh slot 0x4BC4F0 → 0x4F57A0 only answers row text) — so a knockout
/// series loaded half-way races the cars it was started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpponentCarSave {
    pub car_id: u16,
    pub paint_set: usize,
    pub interior: usize,
    pub package: u32,
    pub decal: i32,
}

/// The FE car image of one [`OpponentCarSave`]: the `nfs5.car` record at `+4` and the six
/// appearance dwords as 0x4F500C…0x4F51B6 fill them — each colour `0xFFFF0000` unless the record
/// has paint sets (`rec+0x3E6`) / interiors (`rec+0x528`), then `rec+0x3EC` / `+0x3F0` / `+0x3F4
/// + 16k` and `rec+0x530 + 16i`. The instance dword and the career fields stay 0: the seven are
/// built by the plain constructor 0x4EE5C0, not `FECar::init`'s instance search (APPROXIMATION —
/// what 0x4EE5C0 leaves in them was not read).
pub fn opponent_car_image(tables: &CareerTables, o: &OpponentCarSave) -> SavedCar {
    let mut image = vec![0u8; FE_CAR_IMAGE];
    if let Some(i) = tables.catalog.iter().position(|e| e.id == o.car_id)
        && let Some(rec) = tables.raw.car.get(i * CAR_RECORD_SIZE..(i + 1) * CAR_RECORD_SIZE)
    {
        image[car::RECORD..car::RECORD + CAR_RECORD_SIZE].copy_from_slice(rec);
    }
    let rec = |o: usize| image.get(car::RECORD + o..car::RECORD + o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let count = |o: usize| image.get(car::RECORD + o..car::RECORD + o + 2).map_or(0, |b| i16::from_le_bytes([b[0], b[1]]));
    let none = 0xFFFF_0000u32;
    let (mut ext, mut s1, mut s2, mut int) = (none, none, none, none);
    if count(0x3E6) > 0 {
        let k = 16 * o.paint_set;
        (ext, s1, s2) = (rec(0x3EC + k), rec(0x3F0 + k), rec(0x3F4 + k));
    }
    if count(0x528) > 0 {
        int = rec(0x530 + 16 * o.interior);
    }
    let mut c = SavedCar::new(image);
    c.set_field_u32(car::EXT_COLOUR, ext);
    c.set_field_u32(car::INT_COLOUR, int);
    c.set_field_u32(car::S1_COLOUR, s1);
    c.set_field_u32(car::S2_COLOUR, s2);
    c.set_field_u32(car::PACKAGE_NUM, o.package);
    c.set_field_u32(car::DECAL_NUM, o.decal as u32);
    c
}

/// [`opponent_car_image`] read back: the record id, the paint set and interior rows whose colours
/// the image holds (row 0 when none matches), the package and the decal.
pub fn opponent_car_of(car: &SavedCar) -> OpponentCarSave {
    let rec = car.image.get(car::RECORD..car::RECORD + CAR_RECORD_SIZE).unwrap_or(&[]);
    let row = |base: usize, count_at: usize, v: u32| -> usize {
        let n = rec.get(count_at..count_at + 2).map_or(0, |b| i16::from_le_bytes([b[0], b[1]]).max(0) as usize).min(40);
        (0..n).position(|i| rec.get(base + 16 * i..base + 16 * i + 4).is_some_and(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) == v)).unwrap_or(0)
    };
    OpponentCarSave {
        car_id: car.car_id(),
        paint_set: row(0x3EC, 0x3E6, car.field_u32(car::EXT_COLOUR)),
        interior: row(0x530, 0x528, car.field_u32(car::INT_COLOUR)),
        package: car.field_u32(car::PACKAGE_NUM),
        decal: car.field_u32(car::DECAL_NUM) as i32,
    }
}

/// Build the original's save game from the port's career state.
pub fn career_to_sav(c: &Career, tables: &CareerTables, profile: &Profile) -> SaveFile {
    career_to_sav_with(c, tables, profile, &SaveSide::default())
}

/// What a `.sav` carries beside the career: the modules and accessors that belong to the front
/// end rather than to `PlayerInfo` / `Tournament` / `Missions`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveSide {
    /// `Knockout`, the 0x84-byte block `[0x690410]` (§4.6). `None` writes it switched off.
    pub knockout: Option<KnockoutProgress>,
    /// `Personalities`, the vector `[0x6907D8]` (§4.5). It is the result of the **last**
    /// `0x4E3160` draw — the draw clears it (0x4E3176) and pushes the ids it picks (0x4E32B5) —
    /// and nothing but the draw's two callers (0x4DE011 knockout, 0x4EBC63 quick race) and this
    /// module read it. `None` writes the running knockout series' ids, as the port did before.
    pub personalities: Option<Vec<u16>>,
    /// `CDPlayer`, the selected music tracks `[0x65B62C]` (§4.8).
    pub playlist: Vec<u32>,
    /// The front-end settings, for the accessor block.
    pub settings: SavedSettings,
    /// `Tournament`'s four front-end dwords (§4.3).
    pub tournament: TournamentFe,
    /// `OpponentCar.Car` (`FEOpponentCarAccessor`, §6.1): its selection and its seven cars
    /// (`opponent_car_image` / `opponent_car_of`). `None` leaves the accessor out of the block.
    pub opponent_cars: Option<(u32, Vec<SavedCar>)>,
}

/// The four dwords of the `Tournament` module (§4.3) that belong to the front end, not to the
/// `nfs5.trn` table:
///
/// * `[0x691E6C]`, the **selected tournament** — a record index into the table: 0x503310 reads the
///   id at `table + index·0xD40`; the list setters 0x504D00 (0x504DCC) and 0x5055C0 (0x5055EB) and
///   the race end 0x503AD0 (0x503CA8 / 0x503CCB) write it;
/// * `[0x691E7C]`, the **next event** — an event index inside that record: what
///   `Tournament.RemainingTracks`' setter 0x504F00 picks, its getter 0x503330; 0x504D00 puts it on
///   the first undone event and 0x503A50 back to 0;
/// * `[0x5DB768]` / `[0x5DB76C]`, the **running event and tournament** — 0x503940, called once, at
///   0x4D5B4A on the tournament launch after the fee, copies the two above; 0x503A50 (the race end,
///   0x4EDF1A / 0x4EE076) sets both to −1; 0x503960 files the result through them.
///
/// The loader 0x5029F0 stores all four back verbatim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TournamentFe {
    pub selected: u32,
    pub next_event: u32,
    pub current_event: i32,
    pub current_tournament: i32,
}

impl Default for TournamentFe {
    /// what 0x503A50 leaves, with the first record selected
    fn default() -> Self {
        Self { selected: 0, next_event: 0, current_event: -1, current_tournament: -1 }
    }
}

/// The front-end settings the port keeps, for the accessor block (`savegame.md` §6).
#[derive(Debug, Clone, PartialEq)]
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
    /// The `Controller.*` bindings the port has, `(accessor, six fields)` — each an
    /// `FEControllerInputAccessor` (0x51D060: `u32[6]` from `this+0x14`, no selection). The
    /// original keeps the player's rebound controls here, in the profile (`savegame.md` §6.2).
    /// One slot per row: the factory 0x51E030 builds **nineteen** (0x51E04E…0x51E36D), every one
    /// in the accessor map and so every one saved by 0x4B80E0. *(Twelve slots until 2026-09-26,
    /// which left out the eight rows the port bound later — Lights, Pause, the turn signals,
    /// Hazards and the three looks.)*
    pub controls: [Option<(&'static str, [u32; 6])>; CONTROL_ROWS],
    /// Every other option accessor the port keeps, `(name, index)`: the dword each one's generic
    /// Save 0x4BC460 writes is `[this+0x18]`, the **list index** — for an `FEIntAccessor` the value
    /// less its minimum (0x4BA342 hands `SetSelection` `default − min`, the getter 0x4BA370 adds
    /// `[this+0x24]` = min back). A name the fixed fields above already carry is not written twice.
    pub options: Vec<(String, u32)>,
}

/// `Track1.Laps` is a list accessor over the rows "1".."8" (`FEStrings` 420…427, 0x4F9A76) and
/// the launch adds one to its index (0x4FC1BC), so the dword is `laps − 1`.
pub fn laps_to_index(laps: u32) -> u32 {
    laps.saturating_sub(1)
}
pub fn laps_from_index(i: u32) -> u32 {
    i + 1
}
/// `Game.NumKOOpponents` is `0x4BA310("Game.NumKOOpponents", 0, 4, 1, 7, …)` (0x4E556A…): minimum
/// **1**, so the dword is `count − 1`; `Game.NumOpponents` (0x4E5537…) has minimum 0 and stores the
/// count itself.
pub const KO_OPPONENTS_MIN: u32 = 1;

/// `SetSelection` 0x4BC240, which every dword accessor's Load 0x4BC480 ends in: the index is cut to
/// `count - 1` when above it (`jl` at 0x4BC269) and then to 0 when negative (`jge` at 0x4BC275) —
/// both signed compares.
pub fn clamp_index(d: u32, count: u32) -> u32 {
    let (v, last) = (d as i32, count.max(1) as i32 - 1);
    v.min(last).max(0) as u32
}

/// The `FEControllerInputAccessor`s the factory 0x51E030 registers (`controls.md` §4.170).
pub const CONTROL_ROWS: usize = 19;

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
            controls: [None; CONTROL_ROWS],
            options: Vec::new(),
        }
    }
}

/// The `Controller.*` bindings a save carries, by accessor name.
pub fn saved_controls(s: &SaveFile) -> Vec<(String, [u32; 6])> {
    s.accessors
        .as_ref()
        .map(|a| {
            a.entries
                .iter()
                .filter_map(|(n, v)| match v {
                    AccessorValue::ControllerInput(f) if n.starts_with("Controller.") => Some((n.clone(), *f)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn career_to_sav_with(c: &Career, tables: &CareerTables, profile: &Profile, side: &SaveSide) -> SaveFile {
    let (knockout, settings) = (side.knockout.as_ref(), &side.settings);
    let player = PlayerInfo {
        cars: c.garage.iter().enumerate().map(|(i, car)| car_image(tables, car, i as u32, true)).collect(),
        money: c.money.clamp(-MONEY_LIMIT, MONEY_LIMIT) as i32,
        picture: profile.picture as u32,
        player_name: profile.name.clone(),
        driver_name: profile.name.clone(),
        // The original clears all three on a new career (0x5013F9…); the port gates the first
        // (`careerfirst`, `Career::seen_evolution_intro`) and carries the other two and the eight
        // custom colours as loaded — *until 2026-09-26 written as zeros and dropped on load*
        seen_evolution_intro: u8::from(c.seen_evolution_intro),
        seen_factory_welcome: u8::from(c.seen_factory_welcome),
        seen_first_mission: u8::from(c.seen_first_mission),
        colours: c.custom_colours,
    };
    let tournament = TournamentModule {
        year: c.year as u32,
        // the front end's four (`TournamentFe`) — *until 2026-09-26 written as 0 / 0 / −1 / −1*
        remaining_tracks: side.tournament.next_event,
        selected: side.tournament.selected,
        era_type: c.era.string_id(),
        current_event: side.tournament.current_event,
        current_tournament: side.tournament.current_tournament,
        club_mode: u32::from(c.evolution_complete),
        table: tournament_table(tables, c),
    };
    // The accessors the port has real values for, each in the shape its class implies (§6.1/§6.2a).
    let mut entries = vec![
        ("Chronicle.Year".into(), AccessorValue::Dword(c.year as u32)),
        // `Game.GameType` is left out: it holds whatever mode the front end was in when the
        // original saved (0 quick race, 1 quick knockout, 2 knockout, 4 Evolution, 9 factory
        // driver — `controls.md` §SETTYPE*; 0x4D52EA saves after any race), which the career
        // does not know. *(Until 2026-09-26 the port wrote `evolution_complete` as 0/1 here — the
        // club flag `[0x691E54]`, already in the `Tournament` module — so a finished Evolution put
        // the original into quick knockout on load.)*
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
        // each the accessor's index, not the port's count *(until 2026-09-26 the port wrote the
        // lap and knockout-opponent counts themselves, one more than the original reads)*
        ("Track1.Laps".into(), AccessorValue::Dword(laps_to_index(settings.laps))),
        ("Game.NumOpponents".into(), AccessorValue::Dword(settings.opponents)),
        ("Track1.Traffic".into(), AccessorValue::Dword(settings.traffic)),
        ("Game.NumKOOpponents".into(), AccessorValue::Dword(settings.ko_opponents.saturating_sub(KO_OPPONENTS_MIN))),
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
    for (name, fields) in settings.controls.iter().flatten() {
        entries.push(((*name).into(), AccessorValue::ControllerInput(*fields)));
    }
    for (name, index) in &settings.options {
        if !entries.iter().any(|(n, _)| n == name) {
            entries.push((name.clone(), AccessorValue::Dword(*index)));
        }
    }
    if let Some((selection, cars)) = &side.opponent_cars {
        entries.push(("OpponentCar.Car".into(), AccessorValue::OpponentCars { selection: *selection, cars: cars.clone() }));
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
        // §4.2: the player's parts inventory `[0x69A0EC]`, each part its whole `nfs5.prt` record by
        // value (0x517A30) — the table's bytes with the spare's own damage at `+0x164`
        parts_market: Some(spare_records(tables, c)),
        tournament: Some(tournament),
        player: Some(player),
        track_records: true,
        // §4.4, 0x4F7A10: the market's year, then every car of `[0x691B88]` whose `+0x6A0` is 0 —
        // always written, as the module is always registered. The year is `[0x691B58]`, which
        // 0x4F7DC7 sets to the chronicle year each time the market is refreshed after a race.
        // Every change of the chronicle year (0x502A08, 0x503BE8 inside 0x4EDF00) comes before
        // the next refresh or read, so outside the unported year cheats the two are equal and the
        // career's year is the dword (`career.md`, "The market year, settled"). The instance
        // dwords continue after the garage's (see `car_image`).
        used_cars: Some(UsedCarMarket {
            year: c.year as u32,
            cars: c.market.iter().enumerate().map(|(i, car)| car_image(tables, car, (c.garage.len() + i) as u32, false)).collect(),
        }),
        // §4.6: the raw `[0x690410]` block, and §4.5's list of personality ids in play
        knockout: Some(knockout.map(|k| k.to_state()).unwrap_or_default().write()),
        personalities: Some(side.personalities.clone().unwrap_or_else(|| knockout.map(|k| k.personalities.iter().map(|p| *p as u16).collect()).unwrap_or_default())),
        // §4.8: the selected music tracks
        cd_player: Some(side.playlist.clone()),
        accessors: Some(accessors),
        order: Vec::new(),
    }
}

/// The inventory as `PartsMarket` writes it (0x517A30): a 0x168-byte record per spare, copied from
/// the table and carrying the spare's damage. A spare whose id is not in the table is left out.
fn spare_records(tables: &CareerTables, c: &Career) -> Vec<Vec<u8>> {
    c.spares
        .iter()
        .filter_map(|s| {
            let i = tables.parts.iter().position(|p| p.id == s.id)?;
            let mut r = tables.raw.prt.get(i * PART_RECORD_SIZE..(i + 1) * PART_RECORD_SIZE)?.to_vec();
            r[0x164..0x168].copy_from_slice(&s.state.to_le_bytes());
            Some(r)
        })
        .collect()
}

/// Read back what the original's format can hold. Port-side bookkeeping the format has no field
/// for is lost and left at its default: a car's ten zone damages (the damage figure is rebuilt
/// from its parts, 0x4EF960), a tournament's
/// `final_rank` / `prize` / `failed_attempts`, `eras_completed`, a pending finale and
/// `promotions_accepted`. That is why the port's JSON career, not this file, is the live save.
pub fn career_from_sav(s: &SaveFile, tables: &CareerTables) -> Career {
    let mut c = Career::default();
    if let Some(p) = &s.player {
        c.money = p.money as i64;
        c.garage = p.cars.iter().map(|car| owned_from_image(tables, car)).collect();
        // `[0x691DD5]`, `[0x691D94]` and `[0x691DF5]`, the three bytes right after the two names
        // (0x501859, 0x501868, 0x501877), then the eight colours `[0x691D98]` (0x501888)
        c.seen_evolution_intro = p.seen_evolution_intro != 0;
        c.seen_factory_welcome = p.seen_factory_welcome != 0;
        c.seen_first_mission = p.seen_first_mission != 0;
        c.custom_colours = p.colours;
    }
    // 0x4F7750: the market is cleared and rebuilt from the module, each car through the same
    // loader 0x4EE6A0; its price is `+0x68C` whether or not the listed byte is set
    if let Some(m) = &s.used_cars {
        c.market = m
            .cars
            .iter()
            .map(|car| {
                let mut o = owned_from_image(tables, car);
                o.asking_price = Some(car.field_u32(car::ASKING_PRICE));
                o.offer = None;
                o
            })
            .collect();
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
    // 0x517810 reads the records back by value; the id and the damage are all the port keeps
    if let Some(recs) = &s.parts_market {
        c.spares = recs
            .iter()
            .filter(|r| r.len() >= PART_RECORD_SIZE)
            .map(|r| crate::shop::SparePart { id: get_i16(r, 0), state: f32::from_le_bytes([r[0x164], r[0x165], r[0x166], r[0x167]]) })
            .collect();
    }
    if let Some(m) = &s.missions {
        c.missions = (0..m.len() / MISSION_RECORD_SIZE).map(|i| get_i16(m, i * MISSION_RECORD_SIZE + 0xE8)).collect();
    }
    c
}

/// Read back the modules and accessors that are not the career's (`LoadInfo` 0x4DE9B0 hands each
/// module blob to its loader, then the accessor block to 0x4B7FD0):
///
/// * `Knockout` 0x4DDD60 — `rep movsd` of 0x21 dwords into `[0x690410]`, nothing checked;
/// * `Personalities` 0x4E2CA0 — clears `[0x6907D8]`, reads a count (signed, `jle` at 0x4E2CF0)
///   and pushes that many `u16`s;
/// * `CDPlayer` 0x4BE170 — clears `[0x65B62C]`, reads a count (signed, `jle` at 0x4BE1CD) and
///   pushes that many dwords;
/// * every dword accessor — 0x4BC480 stores the dword at `[this+0x18]` and calls
///   `vtbl[+0x2C]` = 0x4BC240, which clamps it to `0 … count − 1` (0x4BC25E…0x4BC277, signed)
///   before it selects. [`SavedSettings::options`] keeps them all, raw; the clamp is the
///   caller's, which knows each list's length.
///
/// A module the file does not carry leaves its field at the default here, as the original leaves
/// the global alone.
pub fn side_from_sav(s: &SaveFile) -> SaveSide {
    let mut side = SaveSide {
        knockout: s.knockout.as_ref().map(|b| KnockoutProgress::from_state(&KnockoutState::parse(b))),
        personalities: s.personalities.clone(),
        playlist: s.cd_player.clone().unwrap_or_default(),
        settings: SavedSettings::default(),
        // 0x5029F0 stores the four back as they are
        tournament: s.tournament.as_ref().map_or_else(TournamentFe::default, |t| TournamentFe { selected: t.selected, next_event: t.remaining_tracks, current_event: t.current_event, current_tournament: t.current_tournament }),
        opponent_cars: None,
    };
    let mut opponent_cars = None;
    let st = &mut side.settings;
    for (name, v) in s.accessors.iter().flat_map(|a| a.entries.iter()) {
        match (name.as_str(), v) {
            (_, AccessorValue::Dword(d)) => {
                let d = *d;
                match name.as_str() {
                    "Audio.MusicLevel" => st.music = clamp_index(d, 128) as u8,
                    "Audio.SfxLevel" => st.sfx = clamp_index(d, 128) as u8,
                    "Audio.EngineLevel" => st.engine = clamp_index(d, 128) as u8,
                    "Audio.SpeechLevel" => st.speech = clamp_index(d, 128) as u8,
                    "Audio.AmbientLevel" => st.ambient = clamp_index(d, 128) as u8,
                    "Audio.Quality" => st.audio_quality = clamp_index(d, 4) as u8,
                    "Graphics.ViewDistance" => st.view_distance = clamp_index(d, 4) as u8,
                    "Track1.Laps" => st.laps = laps_from_index(clamp_index(d, 8)),
                    "Game.NumOpponents" => st.opponents = clamp_index(d, 8),
                    "Track1.Traffic" => st.traffic = clamp_index(d, 4),
                    "Game.NumKOOpponents" => st.ko_opponents = clamp_index(d, 7) + KO_OPPONENTS_MIN,
                    _ => {}
                }
                st.options.push((name.clone(), d));
            }
            // 0x4F56F0: the selection through 0x4BC480, then seven cars through 0x4EE6A0
            (_, AccessorValue::OpponentCars { selection, cars }) => opponent_cars = Some((*selection, cars.clone())),
            (_, AccessorValue::DeadZone { values, .. }) => {
                if let Some((axis, k)) = name.split_once('.').and_then(|(a, m)| Some((DEADZONE_AXES.iter().position(|x| *x == a)?, ["Min", "Max"].iter().position(|x| *x == m)?))) {
                    st.deadzone[axis][k] = values[0];
                }
            }
            _ => {}
        }
    }
    side.opponent_cars = opponent_cars;
    side
}

/// Write `<dir>/<stem>.sav` and `player.lst`, as `SaveInfo` + `LoadInfo`'s tail do.
///
/// `SaveInfo` 0x4DEDA0 first compares the name with the text buffer `[0x5E8E50]` and writes
/// nothing when they match (0x4DEDB9…0x4DEDF4) — the same "empty name" sentinel the name check
/// 0x4DE510 uses (`savegame.md` §2.2, MEDIUM that the buffer is empty then). So an empty name
/// makes no file here either.
pub fn write_profile(dir: &Path, profile: &Profile, save: &SaveFile) -> std::io::Result<()> {
    if profile.name.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "SaveInfo refuses the empty name (0x4DEDB9)"));
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(profile.path(dir), save.write())?;
    write_player_lst(dir, &profile.stem())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §4.2: the parts inventory goes out as whole `nfs5.prt` records carrying each spare's own
    /// damage (0x517A30) and comes back from them (0x517810).
    #[test]
    fn the_parts_inventory_round_trips_as_part_records() {
        let mut prt = vec![0u8; 2 * PART_RECORD_SIZE];
        prt[0..2].copy_from_slice(&7i16.to_le_bytes());
        prt[PART_RECORD_SIZE..PART_RECORD_SIZE + 2].copy_from_slice(&9i16.to_le_bytes());
        prt[PART_RECORD_SIZE + 0x5C..PART_RECORD_SIZE + 0x60].copy_from_slice(&1234u32.to_le_bytes());
        let parts = nfs_formats::career::parse_parts(&prt).unwrap();
        let tables = CareerTables { raw: crate::career::RawTables { prt, ..Default::default() }, tracks: vec![], tournaments: vec![], parts, personalities: vec![], missions: vec![], strings: Default::default(), catalog: vec![], package_count: 0 };
        let mut c = Career::default();
        c.spares = vec![crate::shop::SparePart { id: 9, state: 12.5 }, crate::shop::SparePart { id: 7, state: 0.0 }, crate::shop::SparePart { id: 99, state: 1.0 }];
        let recs = spare_records(&tables, &c);
        assert_eq!(recs.len(), 2, "an id the table lacks is left out");
        assert_eq!(&recs[0][0x5C..0x60], &1234u32.to_le_bytes(), "the record goes by value");
        let s = SaveFile { parts_market: Some(recs), ..SaveFile::default() };
        let back = career_from_sav(&SaveFile::parse(&s.write()).unwrap(), &tables);
        assert_eq!(back.spares, c.spares[..2]);
    }

    /// A rebound control survives the `.sav`: six dwords under its `Controller.*` name, read back
    /// by name (0x51D060 / 0x51D080).
    #[test]
    fn controls_round_trip_through_the_accessor_block() {
        let fields = [3, 0, 0x1E, 0, 0, b'A' as u32];
        let s = SaveFile {
            accessors: Some(AccessorBlock { entries: vec![("Controller.SteerLeft".into(), AccessorValue::ControllerInput(fields)), ("Chronicle.Year".into(), AccessorValue::Dword(1950))] }),
            ..SaveFile::default()
        };
        let back = SaveFile::parse(&s.write()).unwrap();
        assert_eq!(saved_controls(&back), vec![("Controller.SteerLeft".to_string(), fields)]);
    }

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
        // 0x4DF090 tests `cFileName`, `.sav` included: 95 + 4 = 99 is listed, 96 + 4 = 100 is not
        let (ok, long) = (format!("1{}", "a".repeat(94)), format!("2{}", "b".repeat(95)));
        std::fs::write(dir.join(format!("{ok}.sav")), []).unwrap();
        std::fs::write(dir.join(format!("{long}.sav")), []).unwrap();
        let listed = list_profiles(&dir);
        assert!(listed.iter().any(|p| p.stem() == ok));
        assert!(!listed.iter().any(|p| p.stem() == long));
        std::fs::remove_file(dir.join(format!("{ok}.sav"))).unwrap();
        std::fs::remove_file(dir.join(format!("{long}.sav"))).unwrap();
        assert!(!name_taken(&dir, "Otto"));
        assert!(name_taken(&dir, ""), "the empty name is rejected");
        assert!(write_profile(&dir, &Profile::new("", 0), &SaveFile::default()).is_err(), "SaveInfo writes no nameless profile");
        assert!(!dir.join("0.sav").exists());
        write_player_lst(&dir, "4Mike").unwrap();
        assert_eq!(read_player_lst(&dir).as_deref(), Some("4Mike"));
        delete_profile(&dir, &Profile::new("Mike", 4)).unwrap();
        assert_eq!(list_profiles(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
