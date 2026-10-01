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
//! * Era finale: one-off event (5 laps) in the loaned prize car `trn+0x10C` of 0x504990(0x504800());
//!   1st place awards it (0x4EE17D). No finale state is kept: the offer is the front end's stack.
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
/// 2 (2026-09-25): a car carries its stock parts explicitly — `OwnedCar::stock` and the copies
/// installed in `parts` (0x4EE9A0). A version-1 career is brought up by [`Career::upgrade`].
pub const SAVE_VERSION: u32 = 2;

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
    /// `[0x6115F8]`, the number of paint packages: `GameData/CarModel/packages.ini`'s `[header]
    /// count` (0x443A87, default 1 when the key is missing; 8 in the shipped file). The used-car
    /// generator draws a market car's `PackageNum` below it (0x4F8429). Without the file the
    /// original leaves the `.bss` 0 and would divide by it; the port then gives every car package 0.
    pub package_count: u32,
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
        // the same table the front end reads: `FEStrings.loc` in the current language
        let strings = crate::layout::load_strings(paths);
        let raw = RawTables { car: read("nfs5.car")?, trn: read("nfs5.trn")?, fac: read("nfs5.fac")?, prt: read("nfs5.prt")? };
        Ok(Self {
            tracks: career::parse_track_table(&read("nfs5.trk")?).map_err(tbl("nfs5.trk"))?,
            tournaments: career::parse_tournaments(&raw.trn).map_err(tbl("nfs5.trn"))?,
            parts: career::parse_parts(&raw.prt).map_err(tbl("nfs5.prt"))?,
            personalities: career::parse_personalities(&read("nfs5.per")?).map_err(tbl("nfs5.per"))?,
            missions: career::parse_missions(&raw.fac).map_err(tbl("nfs5.fac"))?,
            catalog: nfs_formats::fedata::parse_car_catalog(&raw.car).map_err(tbl("nfs5.car"))?,
            package_count: package_count(&paths.car_model_dir()),
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
    /// **0x504990(era)** — the era's finale record: the first record with `+4 == era` and
    /// `+0xE4 == 2`, or, when none is (an era id 0x504800 did not find, below), the table's
    /// **first record** (0x5049C6 returns the base), tournament 1.
    pub fn finale_record(&self, era: Option<Era>) -> Option<&Tournament> {
        era.and_then(|e| self.tournaments.iter().find(|t| t.kind == TournamentKind::Era(e) && t.is_hidden())).or_else(|| self.tournaments.first())
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
    /// A colour the car wears that is **not a row of its list** — `FECar+0x674` (exterior),
    /// `+0x678` (interior), `+0x67C` (stripe 1), `+0x680` (stripe 2), in that order — as the
    /// accessor dword it was chosen as (`b0<<24 | b1<<16 | b2<<8 | b3`, `b0` the alpha). The
    /// original keeps the colour itself in those four slots: the paint shop's tick 0x511220 hands
    /// each `OwnedCar.*Colour` accessor's `[+0x2C]` to `FEOwnedCarAccessor`'s setters 37…40
    /// (0x511472…0x5114EC) every frame, and a saved custom colour's row puts that RGBA in `[+0x2C]`
    /// (0x4B92ED…0x4B930C). The port keeps a list row where the colour is one
    /// (`paint_set`, `trim`) and the colour here where it is not; `None` means the row.
    #[serde(default)]
    pub custom: [Option<u32>; 4],
    /// `car+0x758`, the **installed** parts as `nfs5.prt` ids (one per category and type name,
    /// 0x518510). A new car's are copies of its stock parts ([`OwnedCar::fit_stock`], 0x4EE9A0).
    #[serde(default)]
    pub parts: Vec<i16>,
    /// `car+0x75C`, the car's **stock** parts — `{id, part+0x164}` of the copies 0x4EE9A0 makes of
    /// the record's `+0x3BC` / `+0x3BE` list in every FE car's constructor (0x4EEBA0 at 0x4EED28).
    /// Nothing but the save's loader writes it afterwards (`controls.md` §4.169.6). The car's
    /// value subtracts it (0x4F8E50), a market car back-fills its empty slots from it (0x4F8590)
    /// and `carsetup.lay` refuses to close while one of its slots is empty (0x4F0220, dialog 650).
    #[serde(default)]
    pub stock: Vec<(i16, f32)>,
    /// `car+0x698`: km driven in career races (+3000 per race).
    #[serde(default)]
    pub mileage: u32,
    /// `car+0x69C`: career wins.
    #[serde(default)]
    pub wins: u32,
    /// Body damage in per cent — the mean of `damage`, the figure the value formula and the
    /// garage screens use.
    #[serde(default)]
    pub damage_pct: f64,
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
    /// `FECar+0x714`, the copy `SAVESETTINGS` takes (0x4EF4E0) and `LOADSETTINGS` restores
    /// (0x4EF4C0); a new car's is the constructor's defaults, as its `setup` is.
    #[serde(default = "default_setup")]
    pub setup_backup: [f32; 17],
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Trim {
    pub interior: u8,
    pub stripe1: u8,
    pub stripe2: u8,
    pub package: u8,
    /// `FECar+0x688`, `OwnedCar.DecalNum` — the race number, −1 for none (a new car's, 0x502710;
    /// the accessor's range is −1…99, 0x4F1FF3…0x4F1FF7)
    #[serde(default = "no_decal")]
    pub decal: i8,
}

fn no_decal() -> i8 {
    -1
}

impl Default for Trim {
    fn default() -> Self {
        Self { interior: 0, stripe1: 0, stripe2: 0, package: 0, decal: -1 }
    }
}

impl OwnedCar {
    pub fn new(car_id: u16, paint_set: u8, purchase_price: u32) -> Self {
        Self { car_id, paint_set, trim: Trim::default(), custom: [None; 4], parts: Vec::new(), stock: Vec::new(), mileage: 0, wins: 0, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, setup_backup: CAR_SETUP_DEFAULT, purchase_price, asking_price: None, offer: None }
    }

    /// The four colours as `FECar+0x674…+0x680` hold them — exterior, interior, stripe 1,
    /// stripe 2 — each an accessor dword (`b0<<24 | b1<<16 | b2<<8 | b3`): [`OwnedCar::custom`]
    /// where one was chosen, else the list row's, `rec+0x3EC` / `+0x530` / `+0x3F0` / `+0x3F4
    /// + 16·row` (`savegame.md` §4.7a). An interior carries alpha 0xFF, as all 443 shipped entries
    /// do (the catalogue keeps its interiors as RGB). 0 for a row the record lacks.
    pub fn colour_words(&self, e: &CarCatalogEntry) -> [u32; 4] {
        let set = |row: u8, k: usize| e.paint_sets.get(row as usize).map_or(0, |p| p.packed[k]);
        let int = e.interior_colours.get(self.trim.interior as usize).map_or(0, |c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32);
        let rows = [set(self.paint_set, 0), int, set(self.trim.stripe1, 1), set(self.trim.stripe2, 2)];
        std::array::from_fn(|k| self.custom[k].unwrap_or(rows[k]))
    }

    /// **0x4EEBA0**, the FE car constructor, which ends in 0x4EE9A0 (0x4EED28): a car with its
    /// stock parts fitted ([`OwnedCar::fit_stock`]). Every car the career makes goes through it —
    /// the dealership's (0x501CF0), a prize car (0x502270), a market car (0x4F82C0).
    pub fn construct(tables: &CareerTables, car_id: u16, paint_set: u8, purchase_price: u32) -> Self {
        let mut car = Self::new(car_id, paint_set, purchase_price);
        car.fit_stock(tables);
        car
    }

    /// **0x4EE9A0** — the stock parts, read 2026-09-25 (`controls.md` §4.169.6), CONFIRMED:
    ///
    /// ```text
    /// 0x4EE9A0  free +0x758's and +0x75C's parts
    /// 0x4EEA37  for i < FECar+0x3C0 (the record's +0x3BC): +0x75C.push(copy of 0x49C470(+0x3C2[i]))
    /// 0x4EEAB9  for each part of +0x75C: +0x758.push(a fresh copy of it)
    /// 0x4EEB32  exchange-sort +0x758: for i, for j > i, swap when 0x49C350(v[j], v[i])
    /// ```
    ///
    /// Each copy carries the table record's `+0x164` (0.0 throughout the shipped `nfs5.prt`).
    pub fn fit_stock(&mut self, tables: &CareerTables) {
        self.stock.clear();
        self.parts.clear();
        self.part_state.clear();
        let Some(record) = tables.car(self.car_id) else { return };
        let stock: Vec<&Part> = record.stock_parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
        self.stock = stock.iter().map(|p| (p.id, p.state as f32)).collect();
        let mut fitted = stock;
        for i in 0..fitted.len() {
            for j in i + 1..fitted.len() {
                if part_less(fitted[j], fitted[i]) {
                    fitted.swap(i, j);
                }
            }
        }
        self.parts = fitted.iter().map(|p| p.id).collect();
        self.part_state = fitted.iter().map(|p| (p.id, p.state as f32)).collect();
    }

    /// Bring a car from a version-1 port career up to [`SAVE_VERSION`] 2. **Port-only** — the
    /// original always writes a car's stock list (`savegame.md` §4.7), so it never actually loads
    /// this exact state; there is no literal trace for it. So the stock list is rebuilt from the
    /// record as 0x4EE9A0 would, and every stock part whose slot no installed part fills (the same
    /// type name and category — 0x4F0220's test, read to the instruction 2026-09-27: it marks each
    /// `+0x75C` entry found by that test among `+0x758`, no more) is put back on the car.
    ///
    /// *(Settled 2026-09-27, order.)* Appended at the end, in the record's own stock order — not
    /// the replaced slot's position. Two CONFIRMED reads of the *live* object settle which of the
    /// three the original's own code does whenever it adds a missing stock part to an
    /// already-populated `+0x758` (the closest a real game gets to this port-only case):
    /// * **0x518510**, installing a part from the spares inventory: the part it displaces (found by
    ///   the same name+category test) is erased from **its own index** in `+0x758` — shifting every
    ///   later element down — and the part going in is **pushed onto the end** (0x518638…0x518708),
    ///   never written back into the vacated slot.
    /// * **0x4F8590**'s back-fill (0x4F89D7…0x4F8C63) for a market car whose random picks left a
    ///   slot empty: for each `+0x75C` entry in turn, unmatched → a fresh copy is **pushed onto the
    ///   end** of `+0x758` (0x4F8B59…0x4F8C48), in the stock list's own order; matched → nothing.
    ///   No sort follows either push.
    /// So "appended at the end, in stock order, no re-sort" is not the port's own choice — it is
    /// the original's rule for this operation, read off the one case it actually runs. (The
    /// exchange-sort at `+0x758`'s **construction** and at **load**, `owned_from_image`'s own
    /// comment, is a different moment and already ported; this function runs after that sort, on
    /// an already-sorted `parts`, matching 0x4F8590's own precondition.) A car that already has a
    /// stock list is left alone.
    pub fn adopt_implicit_stock(&mut self, tables: &CareerTables) {
        if !self.stock.is_empty() {
            return;
        }
        let Some(record) = tables.car(self.car_id) else { return };
        let stock: Vec<&Part> = record.stock_parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
        let installed: Vec<&Part> = self.parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
        let open: Vec<&Part> = stock.iter().copied().filter(|s| !installed.iter().any(|p| p.kind_name == s.kind_name && p.kind == s.kind)).collect();
        for p in open {
            self.parts.push(p.id);
            if !self.part_state.iter().any(|(id, _)| *id == p.id) {
                self.part_state.push((p.id, p.state as f32));
            }
        }
        self.stock = stock.iter().map(|p| (p.id, p.state as f32)).collect();
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

/// **`Knockout_start` 0x4DDF30** on the block `[0x690410]` (`modes.md` §5.3), run by the launch
/// 0x4D52D9 when `Game.GameType` is 2 and no series is running. `n` is `Game.NumKOOpponents`'
/// value (read twice through `vt[0x6C]`, 0x4DDF7A / 0x4DDF8A) and `drawn` the ids its `0x4E3160(n,
/// Game.Difficutly)` draw returned (0x4DE011). CONFIRMED:
///
/// * `active = 1`, `race_index = 0`, `opponents = n` (0x4DDF68…0x4DDF83);
/// * the first **nine** `alive` bytes cleared (a dword, a dword and a byte, 0x4DDF91…0x4DDFA0) —
///   the other seven are left as they were;
/// * `start_pos[i] = −1 − i` and `last_pos[i] = −1` for `i` in 0…8 (0x4DDFA8 loop);
/// * the player `start_pos[0] = n + 1`, `alive[0] = 1`; opponent `i` in 1…n `alive[i] = 1`,
///   `start_pos[i] = i` (0x4DDFBA…0x4DDFE2);
/// * `personalities[j] = drawn[j]` for `j` in 0…n−1, sign-extended words (0x4DE025…0x4DE035).
///   Slots past `n` keep what they held. A draw shorter than `n` (a pool smaller than the series)
///   would read past the vector in the original; the port writes 0 there.
pub fn knockout_start(k: &mut nfs_formats::sav::KnockoutState, n: i32, drawn: &[i16]) {
    k.active = true;
    k.race_index = 0;
    k.opponents = n;
    for a in k.alive.iter_mut().take(9) {
        *a = 0;
    }
    for i in 0..9 {
        k.start_pos[i] = -1 - i as i32;
        k.last_pos[i] = -1;
    }
    k.start_pos[0] = n + 1;
    k.alive[0] = 1;
    for i in 1..=n.min(8) {
        k.alive[i as usize] = 1;
        k.start_pos[i as usize] = i;
    }
    for j in 0..n.clamp(0, 8) as usize {
        k.personalities[j] = drawn.get(j).copied().unwrap_or(0) as i32;
    }
}

/// **`Knockout_afterRace` 0x4DE060(pos)**, run by the race end 0x4EDF00 for a type-2 race that was
/// not aborted. `pos[k]` is the `k`-th racer's 1-based place — racers in slot order, the player
/// first, then the opponents still in the series in series order — padded with 99 to nine
/// (0x690AB0). `live_count` is `Game.NumKOOpponents`' value **now**: both loops ask the accessor
/// (`vt[0x6C]`, 0x4DE0A2 / 0x4DE0CC and 0x4DE0DD / 0x4DE111) rather than the block's `opponents`.
/// CONFIRMED, instruction by instruction:
///
/// ```text
/// remaining = opponents − race_index ; active = 1 ; start_pos[i] = −1 − i  (i in 0…8)
/// if pos[0] > remaining: active = 0 ; return                ; signed `jg`, 0x4DE090
/// k = 0 ; for car in 0…live_count: if alive[car]: last_pos[car] = pos[k++]
/// for car in 0…live_count: if alive[car] and last_pos[car] <= remaining: start_pos[car] = last_pos[car]
///                          else alive[car] = 0
/// race_index += 1 ; if race_index >= opponents: active = 0
/// ```
pub fn knockout_after_race(k: &mut nfs_formats::sav::KnockoutState, pos: &[i32], live_count: i32) {
    let remaining = k.opponents.wrapping_sub(k.race_index);
    k.active = true;
    for i in 0..9 {
        k.start_pos[i] = -1 - i as i32;
    }
    let at = |i: usize| pos.get(i).copied().unwrap_or(99);
    if at(0) > remaining {
        k.active = false;
        return;
    }
    let last = live_count.clamp(-1, 8);
    let mut n = 0;
    for car in 0..=last.max(0) as usize {
        if last < 0 {
            break;
        }
        if k.alive[car] != 0 {
            k.last_pos[car] = at(n);
            n += 1;
        }
    }
    for car in 0..=last.max(0) as usize {
        if last < 0 {
            break;
        }
        if k.alive[car] != 0 && k.last_pos[car] <= remaining {
            k.start_pos[car] = k.last_pos[car];
        } else {
            k.alive[car] = 0;
        }
    }
    k.race_index += 1;
    if k.race_index >= k.opponents {
        k.active = false;
    }
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

/// The installed-part list `car+0x768` the part-damage getters walk (`nfs_physics::specs::
/// InstalledPart`, `physics.md` §10): one entry per part on the car, in list order, each with
/// its **own** zone mask `part+0x48[10]`, its condition `+0x60` and the damage it carries,
/// `+0x164` — the owned car's saved state when it has one, else the table's (`Car_raceInit`
/// 0x41085F copies `carInfo+0x78 … +0xC4` into each part's `+0x164`; the save keeps the same
/// field). An empty list stays empty: 0x49C4C0 rebuilds `car+0x768` as twenty **zeroed** 0x168-byte
/// records and copies a table record only for each non-zero `.PARTINDEX` id (`loaders.md` §1.3), and a
/// zeroed record has type `+0x26` = 0, which none of the getters' kinds 1…10 matches — so a car
/// with no parts (a traffic car: 16 of the 109 `nfs5.car` records — Eden 1…5 and EAS1…11 — carry no
/// stock list; an owned car always keeps its body part, which the install list does not offer to
/// remove, 0x516751) contributes nothing to the wear sums. *(Until 2026-09-30 the empty list took
/// `nfs_physics::specs::fallback_parts`, six invented parts.)* *(Until 2026-09-26
/// this was `zone_masks_for` — one mask per group, the family's first part of the type, whether
/// installed or not — and `part_wear_for`, one summed scale and offset per group.)*
pub fn installed_parts(parts: &[&Part], owned: Option<&OwnedCar>) -> Vec<nfs_physics::specs::InstalledPart> {
    parts
        .iter()
        .map(|p| {
            let state = owned.and_then(|o| o.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s)).unwrap_or(p.state as f32);
            nfs_physics::specs::InstalledPart::new(p.kind, p.zone_mask.map(|b| b != 0), p.condition, state)
        })
        .collect()
}

/// The installed-part list a **layout race** (`Game.GameType` 0, 1, 2, 3 — the single race, the
/// Quick Knockout, the series' round, the network race) starts with for a garage car: the ids of
/// `feCar+0x758` go out as `.PARTINDEX` for every game type (0x4EF3F3: `aiStack_e0[k] = **part`
/// precedes the type test), but `.PARTDAMAGE` is each part's `+0x164` only for a type **other**
/// than 0, 1, 2, 3, 9 (`param_5` tests at 0x4EF3E6…0x4EF40C in 0x4EEDA0) — so `Car_raceInit`'s copy
/// into `part+0x164` (0x41085F…0x410886) brings **0** for every part and the owned car's saved wear
/// stays in the garage. `.DAMAGEZONES` is 0 for the same types (0x4EF420…0x4EF445).
pub fn layout_race_parts(parts: &[&Part]) -> Vec<nfs_physics::specs::InstalledPart> {
    parts.iter().map(|p| nfs_physics::specs::InstalledPart::new(p.kind, p.zone_mask.map(|b| b != 0), p.condition, 0.0)).collect()
}

/// **A freshly built `FECar` raced as it is** — what 0x49CB50 folds into the `carSpecs` block of
/// every car of a race that has no garage record behind it: a catalogue car of the single player's
/// launches (career prize car, test drive, QuickRace / Knockout / series round, the command line),
/// every AI opponent, a cop, a traffic car. CONFIRMED (`RE/notes/loaders.md` §1.3, network.md
/// §11.12b):
///
/// * `FECar::init` 0x4EEBA0 ends **unconditionally** in 0x4EE9A0 (call at 0x4EED28), which fills
///   the installed list `+0x758` with a copy of every part on the record's stock list `+0x75C`
///   (the `nfs5.car` record's `+0x3C0` count, `+0x3C2[i]` ids); 0x4EEDA0 streams those ids as
///   `.PARTINDEX` (0x14B, for every kind) and 0x49C4C0 rebuilds `car+0x768` from them, so
///   0x49CB50's parts loop (0x49CC4C…0x49D1E8) walks the **stock** parts — [`CarSpecs::apply_parts`];
/// * the same constructor writes the neutral sliders (0x4EED2D…0x4EED8D: 50, 50, 50, 0, 0, 0, 1.0,
///   30, 30, zeros — [`CAR_SETUP_DEFAULT`]) which 0x4EEDA0 streams for **every** kind (0x4EF179…
///   0x4EF32F carry no kind test), and 0x49CB50 folds them (0x49D290…0x49D62C) — [`CarSpecs::apply_setup`],
///   which is not the identity: a `.sim` whose `+0x11C` is 0.8 comes out with 0.8·(1+2)/3 and
///   `grip ×(0.8+9)·0.1`;
/// * `automatic` is `.TRANSMISSION` 0x221 → `carinfo+0xC`: 0x4EEDA0 writes the gearbox option for
///   every car whose `param_7` is 0, and all its callers (0x4ED174 cop, 0x4ED460 traffic, 0x4EBCBF /
///   0x4EBF06 / 0x4EC3FF opponent, 0x4EBA4D player) pass 0 in that slot.
///
/// The order is 0x49CB50's: parts, then sliders. [`CarSpecs::parts`] gets the stock list at its
/// carried state (the table's own).
pub fn fold_fresh_car(specs: &mut nfs_physics::specs::CarSpecs, stock_ids: &[i16], table: &[Part], family: &str, automatic: bool) {
    let stock: Vec<&Part> = stock_ids.iter().filter_map(|id| table.iter().find(|p| p.id == *id)).collect();
    specs.apply_parts(&stock, family);
    specs.parts = nfs_physics::specs::stock_parts(stock_ids, table);
    specs.apply_setup(&CAR_SETUP_DEFAULT, automatic);
}

/// [`installed_parts`] for an owned car: its parts looked up in the table.
pub fn installed_parts_for(tables: &CareerTables, owned: &OwnedCar) -> Vec<nfs_physics::specs::InstalledPart> {
    let parts: Vec<&Part> = owned.parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
    installed_parts(&parts, Some(owned))
}

/// Where the purchased parts used to be applied. They are applied to the **`carSpecs` block**
/// now, by `CarSpecs::apply_parts` (`RE/notes/loaders.md` §1.3) — which is where 0x49CB50 does it,
/// and which is the only place the `+0xC0` / `+0xC4` factors of the brake, spring, shock, sway-bar,
/// aero and tyre arms can be applied at all: their targets are block fields with no `.sim`
/// counterpart.
/// The player's money in a tournament as **`HWinnings`** (draw 0x436990) prints it, from the race
/// options it reads:
/// * per event slot `k` (0…9), `TOURNAMENTRACEWINNINGS[k]` `[0x657784 + 4k]`
///   (options 0xDF…) and `TOURNAMENTRACESAFEBONUSES[k]` `[0x6577AC + 4k]` (0xE9…): the front end
///   copies the player's prize `+0x84` and bonus `+0x64` of each stored event record
///   (0x504080 / 0x5040F0 with driver 0, 0x4EC93F…0x4ECAA9), and the race end writes this race's
///   into slot `TOURNRACEINDEX [0x657780]` (0x42775F…0x42778B — the bonus 0 unless the car stayed
///   clean). A slot is listed when either is above 0 (0x4369B3…0x4369BD); an empty one is
///   **skipped** (`jle 0x436BA8`, past the counter's `inc` at 0x436B9F), so the rows are numbered
///   1, 2, … as they are listed — the label counter `[esp+0x14]` — not by the slot;
/// * `TOURNBONUSPRIZEMONEY[row]` `[0x657760 + 4·row]` on the last race (`[0x6577D8]`), `row` the
///   player's standings row handed over by 0x436E90 (0x431E1A…0x431E27);
/// * `TOURNPLAYEREARNINGS1 [0x657740]`, the player's total: the front end's sum of prize + bonus
///   over the events done (0x5035E0), this race's added by 0x427660 and the tournament prize by
///   0x431B80;
/// * `TOURNAMENTNUMRACESCOMPLETED [0x6577D4]` (0xF3): the front end's count of events done
///   (0x503370 − 0x504300, 0x4ECAAE…0x4ECAC5), raised by one at the race end (0x427660).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Winnings {
    /// `(event number, prize, safe bonus)`
    pub events: Vec<(usize, u32, u32)>,
    pub tournament_bonus: Option<u32>,
    pub total: u32,
    pub completed: usize,
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
    pub times: [i32; 8],
    /// The event record this result fills (`trn + 0x3A0 + event·0xA4`). The events need not be
    /// raced in file order — `Tournament.RemainingTracks`' setter 0x504F00 picks any undone one —
    /// so the slot is kept with the result. `None` (a save written before 2026-09-25) is the
    /// result's position in [`TournamentState::events`].
    #[serde(default)]
    pub event: Option<u8>,
}

impl TournamentState {
    /// The event records this attempt has done, by index — the records whose done byte `+0x30`
    /// is set.
    pub fn done_events(&self) -> Vec<usize> {
        self.events.iter().enumerate().map(|(k, r)| r.event.map_or(k, usize::from)).collect()
    }
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

/// What one of `repair.lay`'s ten `QUICKFIX` boxes repairs — the fix function 0x50F110 calls for
/// its slot (0x50F27D…0x50F354). See [`Career::repair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickFix {
    /// 0x4F0490, 0x4F04E0, 0x4F0530, 0x4F0580, 0x4F05D0, 0x4F0620, 0x4F06B0, 0x4F0700: the
    /// installed parts of these categories
    Kinds(&'static [u16]),
    /// 0x4F0750, `QUICKFIX.COSMETIC`: the ten body zones and the `"Body"` part
    Body,
    /// 0x4F0830, `QUICKFIX.OVERALL`: every installed part, then the same as `Body`
    Overall,
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
    // *(Until 2026-09-27 the port also kept `finale` — a pending finale — and `eras_completed`
    // — the eras whose finale was won. Neither is a field of the original: the finale is keyed
    // on 0x504800 alone (the launch, 0x4ED66F / 0x4FC5BF, and the award 0x4EE17D through
    // `PrizeCars.Car` 0x502550), its offer is the front end's screen stack
    // (`[E, VictoryRoom] endofera`, 0x4D4809…0x4D484F), and a won finale leaves nothing but
    // the prize car in the garage (0x502270). An older port JSON's two keys are ignored.)*
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
    /// The player's parts inventory — the vector at **`[0x69A0EC]`**, one `nfs5.prt` record per
    /// part copied **by value** (0x5183C0 `rep movsd`s the 0x168 bytes), so each carries its own
    /// damage `part+0x164`. The parts shop buys into it and sells out of it, `INSTALLPART` moves a
    /// part from it onto a car and `UNINSTALLPART` back (`shop.rs`). The original saves it as the
    /// `PartsMarket` module (0x517A30, `savegame.md` §4.2).
    #[serde(default)]
    pub spares: Vec<crate::shop::SparePart>,
    /// `[0x691DD5]` — the Evolution intro (`careerfirst`) has been shown. `SETTYPECAREERMODE`
    /// 0x510B50 reads it (0x5015B0) to open `tournament` or, the first time, `careerfirst`, and
    /// sets it (0x5015C0) as it opens the latter; the profile's `PlayerInfo` saves it.
    #[serde(default)]
    pub seen_evolution_intro: bool,
    /// `[0x691D94]` — the Factory Driver welcome (`factwelcome`) has been shown: the Factory Driver
    /// screen's constructor 0x5198A0 reads it (0x5015D0 at 0x5198F4) and, when clear, sets it
    /// (0x5015E0 at 0x5198FD) and opens that screen instead of `missionbrief`. `PlayerInfo`'s byte
    /// after `[0x691DD5]` (0x501868).
    #[serde(default)]
    pub seen_factory_welcome: bool,
    /// `[0x691DF5]` — the first-mission text (`factfirst`) has been shown: the same constructor,
    /// when the current mission's record reads 0xBC7 (0x519912), tests it (0x5015F0 at 0x51991A)
    /// and sets it (0x501600 at 0x519923). The byte after `[0x691D94]` (0x501877).
    #[serde(default)]
    pub seen_first_mission: bool,
    /// `[0x691D98]`, the eight custom paint colours of the colour editor, `0xAARRGGBB`, 0 = slot
    /// unused — getter 0x5019B0, setter 0x5019C0, count 0x5019D0; `PlayerInfo`'s last 32 bytes
    /// (`rep movsd` of 8 at 0x501888). Zeroed by the new-career reset (0x501405).
    #[serde(default)]
    pub custom_colours: [u32; 8],
}

fn default_year() -> u16 {
    1950
}

impl Default for Career {
    fn default() -> Self {
        Self { version: SAVE_VERSION, era: Era::Classic, year: 1950, money: STARTING_MONEY, garage: Vec::new(), tournaments: BTreeMap::new(), evolution_complete: false, missions: Vec::new(), promotions_accepted: Vec::new(), market: Vec::new(), spares: Vec::new(), seen_evolution_intro: false, seen_factory_welcome: false, seen_first_mission: false, custom_colours: [0; 8] }
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
        nfs_assets::platform::data_dir().join("career.json")
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

    /// Bring a career loaded from an older port save up to [`SAVE_VERSION`]: a version-1 career's
    /// cars, garage and market, get their stock lists ([`OwnedCar::adopt_implicit_stock`]).
    /// Port-only bookkeeping; the original's own saves carry both lists.
    pub fn upgrade(&mut self, tables: &CareerTables) {
        if self.version < 2 {
            for car in self.garage.iter_mut().chain(self.market.iter_mut()) {
                car.adopt_implicit_stock(tables);
                car.damage_pct = car_damage_pct(tables, car);
            }
        }
        self.version = SAVE_VERSION;
    }

    pub fn state(&self, id: i16) -> TournamentState {
        self.tournaments.get(&id).cloned().unwrap_or_default()
    }

    /// **0x504800** — the highest **completed** era: each of 0x209 / 0x20A / 0x20B in turn is
    /// taken when every record of it that is not hidden (`+0xE4 != 2`) has `+0xD3C != 0`
    /// (0x504824…0x5048C2), each era tested on its own, a later one overriding. What the
    /// end-of-era screens key on — the finale's lines (0x50637E / 0x5063D3), its map pack
    /// (0x506439), `PrizeCars.Car` (0x502550…), the show's music and movie (0x4FD4F0) — so
    /// after the Classic era's last tournament they name the **Classic** finale, while
    /// `[0x691E50]` (`era`) has already moved on. `None` when no era is complete: the original
    /// then answers the stack word its `push ecx` left (0x504800, `[esp+0x10]` at 0x50484A) —
    /// not an era id in practice, so 0x504990 falls to its first record ([`CareerTables::finale_record`]).
    /// *(Until 2026-09-27 the port's notes read this as "the first era with an unfinished
    /// tournament" — the current era — and its finale screens named the next era's finale.)*
    pub fn completed_era(&self, tables: &CareerTables) -> Option<Era> {
        let done = |e: Era| tables.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(e) && !t.is_hidden()).all(|t| self.state(t.id).completed);
        [Era::Modern, Era::Golden, Era::Classic].into_iter().find(|e| done(*e))
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
    /// **0x501AD0** — `OwnedCars.Car`'s list `[0x691DFC]` (its vector getter 0x501AC0): the
    /// garage `[0x691DF8]` in order, except that when `Game.GameType` is 4 (the Evolution,
    /// 0x501B80) a car whose record's first model year `+0x1F8` (signed word, 0x501B91) is later
    /// than the chronicle year 0x5032C0 is left out (`cmp esi, eax / jg`, 0x501B9D) — a Factory
    /// Driver prize car does not show in 1950. Garage indices. The list is rebuilt by 0x4F3E40
    /// (0x4F42E4: SETTYPE*, the profile load, the race end) and by the garage's own changes
    /// (0x501F22 buy, 0x502170 used buy, 0x50225D sale, 0x5022EB award, …); the port derives it
    /// when read, from the same two inputs.
    pub fn owned_list(&self, tables: &CareerTables, evolution: bool) -> Vec<usize> {
        self.garage
            .iter()
            .enumerate()
            .filter(|(_, o)| !evolution || tables.car(o.car_id).is_none_or(|c| c.year as i16 as i32 <= i32::from(self.year)))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn eligible_garage(&self, t: &Tournament) -> Vec<usize> {
        self.garage.iter().enumerate().filter(|(_, c)| t.eligible_cars.contains(&(c.car_id as i16))).map(|(i, _)| i).collect()
    }

    /// Catalogue cars eligible for `t` that the dealer lists this year (0x4F4240).
    pub fn eligible_catalog<'a>(&self, tables: &'a CareerTables, t: &Tournament) -> Vec<&'a CarCatalogEntry> {
        t.eligible_cars.iter().filter_map(|id| tables.car(*id as u16)).filter(|c| !c.sim.is_empty() && self.dealer_lists(c)).collect()
    }

    /// Dealer availability — the test 0x4F3E40 puts every record through before it pushes it onto
    /// the dealer's list `[0x691A00]` (0x4F4240…0x4F428C):
    ///
    /// ```text
    /// 0x4F424B  year <  record+0x1F8 (first model year, signed)  -> only with the cheat flag
    /// 0x4F4256  year >  record+0x202 (last model year, signed)   -> only with the cheat flag
    /// 0x4F426B  record+0x204 == 2 or 3, or record+0x1F4 != 0     -> never
    /// ```
    ///
    /// `year` is the chronicle year 0x5032C0. The cheat flag `[0x691A04]` (set only by the
    /// handler 0x50DD70 → 0x4F4440) is not ported. *(Corrected 2026-09-26: the port widened the
    /// last year to `max(first, last)`, which no shipped record needs but the original does not do.)*
    pub fn dealer_lists(&self, c: &CarCatalogEntry) -> bool {
        let y = self.year as i16;
        c.year as i16 <= y && y <= c.year_end as i16 && !matches!(c.dealer_flag_204, 2 | 3) && c.kind == 0
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
        // 0x501EDF…0x501F1D: −price held to ±999 999 999, then added
        self.add_money(-price);
        let mut car = OwnedCar::construct(tables, car_id, paint_set, price as u32);
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
    /// value = base + m + d + w + Σ 0x518220(installed) − Σ 0x518220(stock)  ; 0x4F8F24…0x4F8FCC
    /// value < base · 0.05f → _ftol(base · 0.05f)                       ; 0x4F8FCE…0x4F8FF1
    /// ```
    ///
    /// Every division is C's, truncating toward zero. **The mileage arm divides by −100** — its
    /// magic is `0xAE147AE1`, the negation of the damage arm's `0x51EB851F` — while the table
    /// holds 0, −10, −15, −25, −30, so mileage *raises* the value by 10…30 %. That is the
    /// original's arithmetic and it is kept. Each table matches `lo <= x <= hi` over integer
    /// bands and a value outside every band adds nothing. The list subtracted, `car+0x75C`, is the
    /// car's **stock** parts (0x4EE9A0 — `controls.md` §4.169.6, which corrects "spares" here),
    /// each at its own `+0x164`; the installed list holds copies of them too, so an undamaged
    /// stock part in its slot cancels out, a damaged one lowers the value and an upgrade adds its
    /// resale over the stock part's. *(Until 2026-09-25 the port kept the stock parts out of both
    /// sums.)*
    pub fn value_of(&self, tables: &CareerTables, owned: &OwnedCar) -> Option<i64> {
        let car = tables.car(owned.car_id)?;
        let installed: i64 = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .map(|p| part_resale(p, self.year, owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)))
            .sum();
        let stock: i64 = owned.stock.iter().filter_map(|(id, s)| tables.parts.iter().find(|p| p.id == *id).map(|p| part_resale(p, self.year, *s))).sum();
        let parts = installed - stock;
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
    pub fn refresh_market(&mut self, tables: &CareerTables, rand: &mut impl FnMut() -> u32, crt: &mut impl FnMut() -> u32) {
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
            let car = self.generate_used_car(tables, record, rand, crt);
            // 0x4F8165…0x4F8174: the count is the **whole** list `[0x691B88]` — the market's own
            // cars and the player's listings, which 0x4F91C0 appends to the same vector — against
            // 25 (`cmp ecx, 0x19 / jb`). At 25 or more, 0x4F817A walks for the first entry whose
            // `car+0x6A0` is 0, a car the game put there, and deletes it; with none (the list all
            // listings) nothing goes. This port keeps the listings in the garage under an asking
            // price, so they are counted there, and the market keeps the list's order, so its
            // first car is the first such entry. *(Corrected 2026-09-26: the listings were not
            // counted.)*
            let listed = self.garage.iter().filter(|c| c.asking_price.is_some()).count();
            if self.market.len() + listed >= USED_CAR_MARKET_MAX && !self.market.is_empty() {
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

    /// One market car — `0x4F82C0(record)`. Its appearance first ([`used_car_trim`]), then, in
    /// the original's order, the mileage (0x4F849D, `USED_MILEAGE`), the parts (0x4F84E9 →
    /// 0x4F8590), the wins (0x4F84F3, `USED_WINS`), `+0x6A0 = 0` (a car the game put there — the
    /// port keeps such cars in `Career::market` instead) and the asking price its own value comes
    /// to (0x4F856F → 0x4F8E50, into `+0x68C`).
    ///
    /// Between the wins and the price, 0x4F8545…0x4F856C walks the five damage bands of
    /// 0x5B7C48, calling `0x4EF960(car)` and truncating it against each band's low end, and when
    /// it is not below calls 0x4EF960 again and pops the result (`fstp st(0)`): nothing is
    /// stored, so the loop changes nothing and is not ported.
    ///
    /// Nothing here damages the car: the generator never touches the body zones, and every
    /// `nfs5.prt` record ships `+0x164 = 0`, so a market car's parts are undamaged and its
    /// condition bars read 100. Its price is discounted by mileage alone.
    pub fn generate_used_car(&self, tables: &CareerTables, record: &CarCatalogEntry, rand: &mut impl FnMut() -> u32, crt: &mut impl FnMut() -> u32) -> OwnedCar {
        let (paint_set, trim) = used_car_trim(record.paint_sets.len(), record.interior_colours.len(), tables.package_count, rand, crt);
        // 0x4F82C0 builds the car through the constructor 0x4EEBA0, so its stock list is there
        // (0x4EE9A0) before 0x4F8590 clears the installed one
        let mut car = OwnedCar::construct(tables, record.id, paint_set, 0);
        car.trim = trim;
        car.mileage = weighted_band(&USED_MILEAGE, rand).max(0) as u32;
        let fitted = self.generate_used_parts(tables, record, &car.stock, rand);
        car.parts = fitted.iter().map(|(id, _)| *id).collect();
        car.part_state = fitted;
        car.wins = weighted_band(&USED_WINS, rand).max(0) as u32;
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
    /// `"Engine"` and `"Engine (Standard)"`, so a car never leaves the market with two picked.
    ///
    /// Then the **back-fill**, 0x4F89D7…0x4F8C63: for each part of the stock list `car+0x75C`
    /// in order, a copy of it (its own `+0x164` with it, `rep movsd` at 0x4F8B80) is pushed onto
    /// the installed list unless an installed part fills its slot — the same type name and
    /// category (0x4F8A1E…0x4F8A60), or both named `"Engine"` (0x4F8A66…0x4F8ACC), or both
    /// `"Engine (Standard)"` (0x4F8AD2…0x4F8B38). So a stock `"Engine (Standard)"` goes back on
    /// beside a picked `"Engine"` of the same category. Nothing sorts the list afterwards.
    /// Returns the installed list as `{id, part+0x164}`.
    fn generate_used_parts(&self, tables: &CareerTables, record: &CarCatalogEntry, stock: &[(i16, f32)], rand: &mut impl FnMut() -> u32) -> Vec<(i16, f32)> {
        let mut out = self.pick_used_parts(tables, record, rand);
        let named = |id: i16| tables.parts.iter().find(|p| p.id == id);
        for (sid, sstate) in stock {
            let Some(s) = named(*sid) else { continue };
            let filled = out.iter().filter_map(|(id, _)| named(*id)).any(|p| {
                (p.kind_name == s.kind_name && p.kind == s.kind)
                    || (s.kind_name == "Engine" && p.kind_name == "Engine")
                    || (s.kind_name == "Engine (Standard)" && p.kind_name == "Engine (Standard)")
            });
            if !filled {
                out.push((*sid, *sstate));
            }
        }
        out
    }

    /// 0x4F8590's random picks (0x4F85F7…0x4F89D4), each a copy of a table record with its
    /// `+0x164`.
    fn pick_used_parts(&self, tables: &CareerTables, record: &CarCatalogEntry, rand: &mut impl FnMut() -> u32) -> Vec<(i16, f32)> {
        let avail = self.parts_for(tables, record);
        // 0x4F8648 draws the share before it looks at the count: a car nothing fits still takes
        // the draw, then 0x4F86A7 leaves with nothing picked
        let pct = weighted_band(&USED_PART_PCT, rand).max(0);
        if avail.is_empty() {
            return Vec::new();
        }
        let want = (pct as f64 * avail.len() as f64 * 0.01) as usize;
        let mut index: Vec<usize> = (0..avail.len()).collect();
        let mut remaining = avail.len();
        let mut out: Vec<(i16, f32)> = Vec::new();
        let engine = |n: &str| n == "Engine" || n == "Engine (Standard)";
        for _ in 0..want {
            let pick = if remaining == 0 { 0 } else { rand() as usize % remaining };
            let cand = avail[index[pick]];
            let clash = out.iter().filter_map(|(id, _)| avail.iter().find(|p| p.id == *id)).any(|p| {
                (p.kind_name == cand.kind_name && p.kind == cand.kind) || (engine(&p.kind_name) && engine(&cand.kind_name))
            });
            if !clash {
                out.push((cand.id, cand.state as f32));
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
        // 0x501FE5…0x502023: −ask held to ±999 999 999, then added
        self.add_money(-price);
        car.purchase_price = price as u32;
        car.asking_price = None;
        car.offer = None;
        self.garage.push(car);
        self.sort_garage(tables);
        Ok(())
    }

    /// Put the damage a race left on the car away with it — the race-over fold 0x493CF0 with its
    /// second argument 0 (`modes.md`, `physics.md` §10, CONFIRMED) and the front end's 0x4EDD40
    /// (from 0x4EDD00 for `[0x690AD4] == 4`), read to the instruction 2026-09-27:
    ///
    /// ```text
    /// race start  0x493CF0(car, 1), 0x41088B:  s = carried − scale·mean(Z0)     (no clamp)
    /// race over   0x493CF0(car, 0), 0x416D31:  s = min(s + scale·mean(Z1), 100); car+0xC48+4i = s
    /// FE          0x4EDD40:  zones = Z1 (record +0x1C4 → FE car +0x6A8);
    ///             part+0x164 = s > 0 ? s : 0 (0x4EDDCF…0x4EDDE8, by the record's part id +0x124);
    ///             body part (0x4F0400) +0x164 = f32(Σ Z1 · 0.1f) (0x4EDD86, 0x4EDD8B, 0x4EDE0B)
    /// ```
    ///
    /// for every part of type 1…10 (0x493D0D…0x493D1A), `scale = 1.5 − 0.005·condition`, `mean`
    /// over the part's own mask (divided only when more than one zone), `carried` the saved
    /// `+0x164` (`.PARTDAMAGE`) and `Z0` the zones the car was launched with (`.DAMAGEZONES`, the
    /// car's own `owned.damage` for a type the launch carries, 0x4EF41B…0x4EF45E). So a part gains
    /// `scale·(mean(Z1) − mean(Z0))` — this race's dents — and racing a dented car again without a
    /// scratch leaves it as it was. *(Until 2026-09-27 the port added `scale·mean(Z1)` whole and
    /// set no body part: every race counted the carried zones again. The type gate and the clamp
    /// at 100 were added 2026-09-26.)* The automatic replay's second fold (0x416D31 again, from
    /// the playback's `Race_finalize(2)`) starts from the same `carried` and `Z0` — `Car_raceInit`
    /// reloads them at every pass — so `zones` is simply the last record's `+0xC98`.
    ///
    /// A part of another type is not folded, and 0x4EDD40 hands it its `car+0xC48` slot all the
    /// same — which holds **0**: the slots' only writers are `Cars_create`'s `rep stosd` over the
    /// whole 0x1254-byte car (0x4111B9, the race load 0x414DB0 → 0x414E68, every launch) and the
    /// fold's own `mov [ebx+0x4E0]` (0x493DAE, types 1…10 only; `.text` scanned for `+0xC48`,
    /// `+0x4E0`, `+0xAD4` and `+0x174`, 2026-09-27). So such a part comes back at 0 (the `else 0`
    /// arm). No shipped part has such a type (`nfs5.prt`: types 1…10 only), so this is for the
    /// rule's sake. *(Until 2026-09-27 the port left such a part as it was.)* 0x4EDD40's inner
    /// loop runs over the installed-part **count** of record slots (`mov edi, edx`, 0x4EDDBC), not
    /// twenty; the race car's slots are filled in the installed order (0x49C5F1), so every
    /// installed part finds its own.
    pub fn record_damage(&mut self, tables: &CareerTables, idx: usize, zones: [f32; 10]) {
        let Some(owned) = self.garage.get(idx) else { return };
        let start = owned.damage;
        let ids = owned.parts.clone();
        let mean = |z: &[f32; 10], mask: &[bool; 10]| {
            let n = mask.iter().filter(|b| **b).count();
            let sum: f32 = z.iter().zip(mask.iter()).filter(|(_, m)| **m).map(|(d, _)| *d).sum();
            if n > 1 { sum / n as f32 } else { sum }
        };
        let mut folded: Vec<(i16, f32)> = Vec::new();
        let mut body = None;
        for id in &ids {
            let Some(p) = tables.parts.iter().find(|p| p.id == *id) else { continue };
            if is_body_part(p) && body.is_none() {
                body = Some(p.id);
            }
            if !(1..=10).contains(&p.kind) {
                // 0x4EDDE2: its `car+0xC48` slot is Cars_create's 0 (0x4111B9), not above 0
                folded.push((p.id, 0.0));
                continue;
            }
            let mask = p.zone_mask.map(|b| b != 0);
            let carried = owned.part_state.iter().find(|(pid, _)| *pid == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32);
            let mut part = nfs_physics::specs::InstalledPart::new(p.kind, mask, p.condition, carried);
            part.unfold(mean(&start, &mask));
            let s = (mean(&zones, &mask) * part.scale + part.state).min(100.0);
            folded.push((p.id, if s > 0.0 { s } else { 0.0 }));
        }
        let body_state = (zones.iter().map(|z| *z as f64).sum::<f64>() * 0.1f32 as f64) as f32;
        let Some(owned) = self.garage.get_mut(idx) else { return };
        owned.damage = zones;
        if let Some(b) = body {
            folded.push((b, body_state));
        }
        for (id, s) in folded {
            match owned.part_state.iter_mut().find(|(pid, _)| *pid == id) {
                Some((_, v)) => *v = s,
                None => owned.part_state.push((id, s)),
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

    /// Carry out one ticked `QUICKFIX` box — its fix function, called by 0x50F110 after the money
    /// check (0x50F232, the ticked boxes' costs summed against the money, `jge` goes on; the
    /// caller does that). Read 2026-09-26:
    ///
    /// ```text
    /// 0x4F0490…0x4F0700  QuickFix::Kinds — 0x5189F0 on every installed part (+0x758) of the
    ///                    box's categories (0x4F0620, BODYWORK, walks 6 and then 7)
    /// 0x4F0750           QuickFix::Body (COSMETIC) — ten times: zone k (FECar+0x6A8+4k) = 0,
    ///                    then 0x5189F0 on the first installed part of category 10 named "Body"
    /// 0x4F0830           QuickFix::Overall — 0x5189F0 on every installed part, then 0x4F0750's loop
    /// 0x5189F0(part)     cb(aged · _ftol(2·part+0x164) / −100) through 0x5012E0; part+0x164 = 0
    /// ```
    ///
    /// So each part pays at the damage it has **then** — a part a box before already fixed pays
    /// nothing — and the body zones are cleared only by COSMETIC and OVERALL, all ten of them.
    /// COSMETIC fixes the one `"Body"` part although its cost (0x4EFF20) counts every category-10
    /// part, rims and roof too. Returns what was spent. *(Corrected 2026-09-26: the port charged
    /// the box's pre-computed cost, fixed every category-10 part for COSMETIC and cleared the
    /// zones under the mask of every part it fixed, whatever the box.)*
    ///
    /// Gap: a car with no `"Body"` part would hand 0x5189F0 a null part (0x4F07E7 `xor esi,
    /// esi`), which reads `[0+0x46]`; every shipped car has one, and the port skips it.
    pub fn repair(&mut self, tables: &CareerTables, idx: usize, fix: QuickFix) -> Result<i64, CareerError> {
        let owned = self.garage.get(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        let parts: Vec<&Part> = owned.parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).collect();
        let body = parts.iter().find(|p| is_body_part(p)).map(|p| p.id);
        let mut order: Vec<i16> = match fix {
            QuickFix::Kinds(kinds) => kinds.iter().flat_map(|k| parts.iter().filter(move |p| p.kind == *k).map(|p| p.id)).collect(),
            QuickFix::Body => Vec::new(),
            QuickFix::Overall => parts.iter().map(|p| p.id).collect(),
        };
        let zones = matches!(fix, QuickFix::Body | QuickFix::Overall);
        if zones {
            // the ten passes of 0x4F0750's loop each fix the body part; after the first it is at 0
            order.extend(body);
        }
        let mut spent = 0;
        for id in order {
            let Some(p) = tables.parts.iter().find(|p| p.id == id) else { continue };
            let owned = &mut self.garage[idx];
            let state = owned.part_state.iter().find(|(pid, _)| *pid == id).map(|(_, s)| *s).unwrap_or(p.state as f32);
            let cost = part_repair_cost(p, self.year, state);
            match owned.part_state.iter_mut().find(|(pid, _)| *pid == id) {
                Some((_, s)) => *s = 0.0,
                None => owned.part_state.push((id, 0.0)),
            }
            self.add_money(-cost);
            spent += cost;
        }
        let owned = &mut self.garage[idx];
        if zones {
            owned.damage = [0.0; 10];
        }
        owned.damage_pct = car_damage_pct(tables, owned);
        Ok(spent)
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
    /// The payment is 0x5012E0's inline twin (0x5021DE…0x502226): the **amount** `car+0x694` is
    /// held to ±999 999 999 and then added to `[0x691D90]`; the sum is not clamped.
    /// *(Corrected 2026-09-26: the port clamped the new balance.)*
    pub fn accept_offer(&mut self, tables: &CareerTables, idx: usize) -> Result<i64, CareerError> {
        let car = self.garage.get(idx).ok_or(CareerError::UnknownCar { id: 0 })?;
        let offer = car.offer.ok_or(CareerError::NoOffer)? as i64;
        self.add_money(offer);
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

    /// The parts shop's first section — **`0x517D10(vec, car, year)`**, which `Parts.AllParts`'s
    /// refresh 0x5153E8 calls with the car `OwnedCars.Car` is on and the chronicle year (0x5032C0).
    /// CONFIRMED, read 2026-09-25:
    ///
    /// ```text
    /// 0x517D58  for each id in the car's compatible list (FECar+0x332 count, +0x334 ids,
    ///           i.e. record +0x32E / +0x330)
    /// 0x517D87    for each nfs5.prt record (0x49C440 count, 0x49C450 base, stride 0x168)
    /// 0x517D93      id matches, and part+0x46 (year) <= year      ; `jg` skips a newer part
    /// 0x517DB3      and not category 10 with type name "Body"     ; strcmp with 0x5DA748
    /// 0x517DF7      -> push
    /// 0x517F05  exchange sort: swap i, j when 0x49C350(v[j], v[i])
    /// ```
    ///
    /// `0x49C350` is the parts' order: category `+0x26`, then price `+0x5C`, then id `+0`, each
    /// ascending and signed ([`part_less`]). *(Corrected 2026-09-25: the port matched the car's
    /// family name against `part+0x04`, took the newest year of each part name, kept categories
    /// 1…8 only and sorted by category, type name and price — none of which is the original's.)*
    pub fn parts_for<'a>(&self, tables: &'a CareerTables, car: &CarCatalogEntry) -> Vec<&'a Part> {
        let mut v: Vec<&Part> = Vec::new();
        for id in &car.compatible_parts {
            for p in &tables.parts {
                if p.id == *id && p.year as i16 <= self.year as i16 && !is_body_part(p) {
                    v.push(p);
                }
            }
        }
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if part_less(v[j], v[i]) {
                    v.swap(i, j);
                }
            }
        }
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

    /// Buy and install `part_id` on garage car `idx` — the port's one-row parts flow
    /// (`career_ui`), which the original splits in two: the shop's buy 0x5183C0
    /// ([`Career::buy_spare`], the aged price into the inventory) and `INSTALLPART` 0x518510
    /// ([`Career::install_spare`]), which moves the part it replaces — a stock part included —
    /// into the inventory. *(Until 2026-09-25 the replaced part was dropped.)*
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
        self.buy_spare(tables, part_id)?;
        let spare = self.spares.len() - 1;
        self.install_spare(tables, idx, spare);
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

    /// Index of the next event to race in `t` — the first undone one, where 0x503AD0 and the
    /// tournament setter 0x504D00 leave `[0x691E7C]` — or `None` when all are done.
    pub fn next_event(&self, t: &Tournament) -> Option<usize> {
        self.undone_events(t).first().copied()
    }

    /// The event records of `t` whose done byte `+0x30` is clear, in file order — the rows of
    /// `Tournament.RemainingTracks` (0x505170) and what its setter 0x504F00 counts through.
    pub fn undone_events(&self, t: &Tournament) -> Vec<usize> {
        let done = self.state(t.id).done_events();
        (0..t.events.len()).filter(|i| !done.contains(i)).collect()
    }

    /// **0x5045C0** — the row of `list` (the vector `Tournament.EventType` picks, 0x5053C0) the
    /// front end puts `Tournament.Tournament` on each time 0x505570 refreshes it. CONFIRMED,
    /// read to the instruction 2026-09-27:
    /// * the first tournament **under way**: its undone events (done byte `+0x30` of each of the
    ///   `+0x39C` records at `+0x3A0 + i·0xA4`, 0x5046B3…0x5046D1) more than none and fewer than
    ///   all (`jle` / `jl`, 0x5046D3…0x5046DB);
    /// * else the first **never completed** one, `trn+0xD3C == 0` (0x50475F);
    /// * else row 0 (the defaults set at 0x50460D / 0x504613 before either walk).
    ///
    /// An id the table does not hold is looked up as record 0 (the walks fall through to
    /// `[0x691E58]`, 0x50467D / 0x50475D); every id of the six vectors is the table's own, so
    /// the port does not model that. An empty list is `None` — the original reads
    /// `list.begin[0]` anyway (0x5045E4), past an empty vector.
    pub fn default_tournament_row(&self, list: &[&Tournament]) -> Option<usize> {
        if list.is_empty() {
            return None;
        }
        let under_way = list.iter().position(|t| {
            let undone = self.undone_events(t).len();
            undone > 0 && undone < t.events.len()
        });
        Some(under_way.or_else(|| list.iter().position(|t| self.state(t.id).completions == 0)).unwrap_or(0))
    }

    /// **0x504F00(k)**: the index of the `k`-th undone event (0x504F43…0x504FFD) — the event
    /// `Tournament.RemainingTracks` row `k` stands for. A row past the end keeps the last one
    /// (the port's rows never go past it: 0x4FA575 wraps them).
    pub fn remaining_event(&self, t: &Tournament, row: usize) -> Option<usize> {
        let u = self.undone_events(t);
        u.get(row.min(u.len().saturating_sub(1))).copied()
    }

    /// Enter `t` and return the event index to race: the first undone one.
    pub fn enter(&mut self, tables: &CareerTables, t: &Tournament) -> Result<usize, CareerError> {
        self.enter_event(tables, t, 0)
    }

    /// Enter `t` for the event on `Tournament.RemainingTracks` row `row` ([`Self::remaining_event`])
    /// and return its index. The fee is paid when no event of the tournament is done yet —
    /// `0x503370(tid) − 0x504300(tid) == 0` at 0x4D591F — so once per attempt (0x4D58FB).
    pub fn enter_event(&mut self, tables: &CareerTables, t: &Tournament, row: usize) -> Result<usize, CareerError> {
        // club events are not gated by career progress — the list accessor 0x504D00 filters them by
        // the selected club era alone (H28, CONFIRMED) — while an Evolution tournament belongs to
        // the era the player is up to. **A completed tournament is raced again**: the type-4 start
        // check 0x4D59F3…0x4D5B3F tests the car restriction (0x504A70 → 813), the part-way
        // tournaments (0x504B60 → 858) and the fee (0x503430 → 857), and nothing else — the
        // filing cleared its events (0x503E40), so the fee is due again, and a second completion
        // counts `+0xD3C` (`TIMESCOMPLETED`) without the year step (`== 1`, 0x503BBC). *(Until
        // 2026-09-27 the port refused a completed tournament.)* The hidden finales are not
        // listed (0x504D00), so never selected.
        let era_ok = t.is_club() || t.era() == self.era;
        if self.availability(tables, t) == Availability::Locked || !era_ok {
            return Err(CareerError::NotAvailable { id: t.id });
        }
        if self.eligible_garage(t).is_empty() {
            return Err(CareerError::NoEligibleCar { id: t.id });
        }
        let event = self.remaining_event(t, row).ok_or(CareerError::NoEvents { id: t.id })?;
        if self.state(t.id).events.is_empty() {
            let fee = t.entry_fee as i64;
            if self.money < fee {
                return Err(CareerError::NoMoney { need: fee, have: self.money });
            }
            // 0x4D5B3F…0x4D5B42: 0x5012E0(−fee)
            self.add_money(-fee);
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
    fn event_result(t: &Tournament, event: usize, order: &[usize], times: &[i32; 8], clean: &[bool; 8]) -> EventResult {
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
        EventResult { position, prize, bonus, points, earned, times: *times, event: Some(event as u8) }
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
    pub fn standings_after(&self, t: &Tournament, event: usize, order: &[usize], times: &[i32; 8], clean: &[bool; 8]) -> Vec<(usize, u32, u32, i32)> {
        let mut events = self.state(t.id).events.clone();
        events.push(Self::event_result(t, event, order, times, clean));
        standings_of(&events, t.time_ranked())
    }

    /// The rows `HTournamentStandings` lists: [`standings_after`](Self::standings_after) cut to the
    /// `racers` in the race (drivers `0..racers`), and on the tournament's last race each row's
    /// winnings raised by the tournament prize of its place. The standings' slot 2 **0x431B80**,
    /// which the race end calls right after the tally 0x427660 (0x426C3F…0x426C4C), sorts the
    /// racers (0x502880) and then, when `TOURNAMENTLASTRACE [0x6577D8]`, adds
    /// `TOURNBONUSPRIZEMONEY[k]` (`[0x657760 + 4k]`, options 0xCE…, the front end's `0x503480(t, k +
    /// 1)` = `t.prize[k]`) to the earnings `[0x657740 + 4·driver]` of the driver in place `k`
    /// (0x431BF9…0x431C16) — the column the WINNINGS cells print (0x431DF4). The order is the
    /// one before the prize.
    #[allow(clippy::too_many_arguments)]
    pub fn standings_page(&self, t: &Tournament, event: usize, order: &[usize], times: &[i32; 8], clean: &[bool; 8], racers: usize, last: bool) -> Vec<(usize, u32, u32, i32)> {
        let mut rows: Vec<_> = self.standings_after(t, event, order, times, clean).into_iter().filter(|(d, ..)| *d < racers.max(1)).collect();
        if last {
            for (k, row) in rows.iter_mut().enumerate() {
                row.2 += t.prize.get(k).copied().unwrap_or(0);
            }
        }
        rows
    }

    /// What **`HWinnings`** (ctor 0x436860, draw 0x436990) lists for the player once this event is
    /// in: the race options it reads, as the front end sets them at launch (0x4EC93F…0x4ECAC5) and
    /// the race end then updates (0x427660, 0x431B80). See [`Winnings`].
    #[allow(clippy::too_many_arguments)]
    pub fn winnings_after(&self, t: &Tournament, event: usize, order: &[usize], times: &[i32; 8], clean: &[bool; 8], racers: usize, last: bool) -> Winnings {
        let st = self.state(t.id);
        // each result in its own event record's slot, this race's in `event`'s
        let mut slots: Vec<(usize, EventResult)> = st.done_events().into_iter().zip(st.events.iter().cloned()).filter(|(k, _)| *k != event).collect();
        slots.push((event, Self::event_result(t, event, order, times, clean)));
        slots.sort_by_key(|(k, _)| *k);
        let page = self.standings_page(t, event, order, times, clean, racers, last);
        let row = page.iter().position(|(d, ..)| *d == 0).unwrap_or(0);
        Winnings {
            // `[esp+0x14]` starts at 1 and steps only on a listed row (0x4369A2, 0x436B9F)
            events: slots.iter().filter(|(k, e)| *k < 10 && (e.prize > 0 || e.bonus > 0)).enumerate().map(|(n, (_, e))| (n + 1, e.prize, e.bonus)).collect(),
            tournament_bonus: last.then(|| t.prize.get(row).copied().unwrap_or(0)),
            total: page.get(row).map_or(0, |r| r.2),
            completed: st.events.len() + 1,
        }
    }

    pub fn record_event(&mut self, tables: &CareerTables, t: &Tournament, event: usize, order: &[usize], times: &[i32; 8], clean: &[bool; 8], garage_idx: Option<usize>) -> EventOutcome {
        let result = Self::event_result(t, event, order, times, clean);
        let (prize, bonus) = (result.prize, result.bonus);
        // 0x4EE040…0x4EE051: the event prize and the bonus, each its own 0x5012E0 call — the
        // amount is clamped, never the balance *(corrected 2026-09-26: the port clamped the sum)*
        self.add_money(prize as i64);
        self.add_money(bonus as i64);
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
        // 0x4EE06E: 0x5012E0(tournament prize) — the amount clamped, then added (`st` still
        // borrows the tournaments, so the clamp is spelt out)
        self.money += (tprize as i64).clamp(-999_999_999, 999_999_999);
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

    /// The finale record the launch and the award use: **0x504990(0x504800())** — the hidden
    /// record of the highest completed era ([`completed_era`](Self::completed_era),
    /// [`CareerTables::finale_record`]). The type-10 launch (0x4D5798 → 0x4D59EA → 0x4D5AA3 →
    /// 0x4D58E3 → 0x4D5B53) writes no profile field — the fee path 0x4D58FB, 0x503940 and the
    /// `[0x691E6C]` / `[0x691E7C]` / `[0x5DB768]` / `[0x5DB76C]` dwords are type 4's — so this is
    /// all the original knows about the finale being raced: `Game.GameType` 10 and 0x504800.
    pub fn finale_tournament<'a>(&self, tables: &'a CareerTables) -> Option<&'a Tournament> {
        tables.finale_record(self.completed_era(tables))
    }

    /// Finale race over: **0x4EE17D** — game type 10 and the player's place 1 → `PrizeCars.Car`'s
    /// car, `0x4F4360(0x5048D0(0x504800()))`, appended by 0x502270 (price 0, first paint set);
    /// nothing else is filed, won or lost — no money, no km / win (the type-4 arm 0x4EDFE6), no
    /// tournament state, no era flag. Returns (won, prize car id). *(Until 2026-09-27 the award
    /// read a port-only pending finale, which a profile load dropped, and a win also filed a
    /// port-only `eras_completed`.)*
    pub fn record_finale(&mut self, tables: &CareerTables, order: &[usize]) -> Option<(bool, u16)> {
        let prize = self.finale_tournament(tables)?.prize_car;
        let won = order.first() == Some(&0);
        if won {
            self.garage.push(OwnedCar::construct(tables, prize, 0, 0));
        }
        Some((won, prize))
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
        self.garage.push(OwnedCar::construct(tables, car, 0, 0));
        Some(car)
    }

    /// Points standings for `t`: `(driver index, points, money, time)` sorted best first.
    pub fn standings(&self, t: &Tournament) -> Vec<(usize, u32, u32, i32)> {
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

/// A market car's appearance — what `0x4F82C0` hands `FECar::init` 0x4EEBA0 as `(ExtColour,
/// IntColour, S1Colour, S2Colour, PackageNum, DecalNum)` (0x4F8483…0x4F8490; `savegame.md` §4.7a),
/// returned as the port's rows: the paint set and the [`Trim`].
///
/// ```text
/// 0x4F82EA  k = rand % rec+0x3E6                  ; one paint set gives all three body colours:
///           ext = rec[0x3EC + 16k], s1 = rec[0x3F0 + 16k], s2 = rec[0x3F4 + 16k]
/// 0x4F83B8  j = rand % rec+0x528                  ; the interior, drawn on its own:
///           int = rec[0x530 + 16j]
/// 0x4F840D  crt_rand() % 100 > 50  →  PackageNum = crt_rand() % [0x6115F8], else 0
/// 0x4F8440  crt_rand() % 100 > 80  →  DecalNum   = crt_rand() % 100 − 1,    else −1
/// ```
///
/// So the stripes are **the exterior's own set's** — the rows `k`, `k` — not paint set 0's as a
/// car bought from the dealer has (0x501CF0); the package is 0…7 about one time in two (49 in
/// 100) and a race number −1…98 about one time in five (19 in 100). A record with no paint sets
/// or no interiors would keep `0xFFFF0000` / `0xFFFFFFFF` colours (0x4F82CA, 0x4F82DC), which rows
/// cannot say; every `nfs5.car` record has at least one of each, so the port gives row 0.
///
/// **Two generators** (0x4F82EA / 0x4F83B8 call 0x538040, 0x4F840D / 0x4F8429 / 0x4F8440 / 0x4F8452
/// call 0x5A17C9 — read to the instruction): `k` and `j` come from the front end's 0x538040
/// (`rand`, `opponents::FeRng`), the four rolls from the CRT's `rand()` 0x5A17C9 (`crt`, the C
/// runtime's shared state, `layout::glints::crt_rand`).
pub fn used_car_trim(paint_sets: usize, interiors: usize, package_count: u32, rand: &mut impl FnMut() -> u32, crt: &mut impl FnMut() -> u32) -> (u8, Trim) {
    let set = if paint_sets > 0 { (rand() as usize % paint_sets) as u8 } else { 0 };
    let interior = if interiors > 0 { (rand() as usize % interiors) as u8 } else { 0 };
    // 0x4F8429 divides by `[0x6115F8]` unguarded; a missing `packages.ini` leaves it 0
    let package = if crt() % 100 > 50 { crt().checked_rem(package_count).unwrap_or(0) as u8 } else { 0 };
    let decal = if crt() % 100 > 80 { (crt() % 100) as i8 - 1 } else { -1 };
    (set, Trim { interior, stripe1: set, stripe2: set, package, decal })
}

/// `packages.ini`'s `[header] count` in `dir` — 0x443A60's read of `[0x6115F8]` (see
/// [`CareerTables::package_count`]): the key's value, 1 when the section lacks it, 0 when there is
/// no file or no `[header]` (0x443A5C / 0x443A76 jump past the store).
pub fn package_count(dir: &Path) -> u32 {
    let Some(text) = GamePaths::find_file(dir, "packages.ini").and_then(|p| std::fs::read_to_string(p).ok()) else { return 0 };
    let ini = nfs_formats::ini::Ini::parse(&text);
    // 0x523AD0: `atoi` of the value, or the default 1 the caller pushes (0x443A7C)
    ini.section("header").map_or(0, |h| h.get("count").map_or(1, |v| v.trim().parse::<i64>().unwrap_or(0).max(0) as u32))
}

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
/// undamaged part, rising with damage. `0x518900` is a different function — the advanced repair
/// screen's **replace** cost, `aged − this` (0x5171C5, 0x5175F8; [`crate::shop::part_replace_cost`]).
/// *(Corrected 2026-09-25: it was said to belong to the parts shop.)*
pub(crate) fn part_resale(p: &Part, year: u16, state: f32) -> i64 {
    part_resale_from(part_aged_price(p, year), state)
}

/// **0x49C350** — the order every parts list keeps: category `+0x26` (signed word, `jge`), then
/// the base price `+0x5C` (`setl`), then the id `+0` (signed word, `setl`). CONFIRMED.
pub fn part_less(a: &Part, b: &Part) -> bool {
    let (ka, kb) = (a.kind as i16, b.kind as i16);
    if ka != kb {
        return ka < kb;
    }
    if a.price != b.price {
        return (a.price as i32) < (b.price as i32);
    }
    a.id < b.id
}

/// A part that is the car's **body** — category 10 with the type name `"Body"` (the string at
/// 0x5DA748). The shop never lists one (0x517DB3, 0x518BE9) and the repair screen's accept knows
/// it by it (0x4F0400).
pub fn is_body_part(p: &Part) -> bool {
    p.kind == 10 && p.kind_name == "Body"
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
/// Each `M_k` is stored as an `f32`; the sum runs on the x87. A new car's installed list holds
/// copies of its stock parts — the FE car constructor 0x4EEBA0 clears `+0x758` (0x4EEC86…) and
/// then 0x4EE9A0 fills it (0x4EED28, `controls.md` §4.169.6; this said "empty" until 2026-09-25)
/// — whose `+0x164` is the table's 0.0, so a new car reads **0 %** however dented its body is;
/// the port's `owned.parts` holds those copies too since 2026-09-25 ([`OwnedCar::fit_stock`]).
/// The value is what `0x4F8E50` and `0x4F7E90` truncate into their damage bands.
/// The result is the x87 value (53-bit, MSVC's precision control) that the callers hand straight
/// to `_ftol` — an `f64` here, never rounded to `f32` on the way. *(Until 2026-09-26 it was stored
/// as `f32`, an APPROXIMATION that could round a total just under an integer up to it.)*
pub fn car_damage_pct(tables: &CareerTables, owned: &OwnedCar) -> f64 {
    let parts: Vec<(u16, f32)> = owned
        .parts
        .iter()
        .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
        .map(|p| (p.kind, owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)))
        .collect();
    damage_pct_from(&parts)
}

/// [`car_damage_pct`] over `(part type, part+0x164)` pairs.
fn damage_pct_from(parts: &[(u16, f32)]) -> f64 {
    let mean = |kind: u16| -> f64 {
        let v: Vec<f64> = parts.iter().filter(|(k, _)| *k == kind).map(|(_, s)| *s as f64).collect();
        let sum: f64 = v.iter().sum();
        (if v.len() > 1 { sum / v.len() as f64 } else { sum }) as f32 as f64
    };
    let total = mean(1) + mean(2) + mean(3) + mean(4) + mean(5) + (mean(6) + mean(7)) * 0.5 + mean(8) + mean(9) + mean(10);
    total * (1.0f32 / 9.0) as f64
}

/// `0x4F8E50`'s arithmetic, given the installed parts' total (see [`Career::value_of`]).
pub fn car_value_from(price: u32, curve_pct: f32, mileage: u32, damage_pct: f64, wins: u32, parts: i64) -> i64 {
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
fn standings_of(events: &[EventResult], time_ranked: bool) -> Vec<(usize, u32, u32, i32)> {
    let mut rows: Vec<(usize, u32, u32, i32)> = (0..8).map(|d| (d, 0u32, 0u32, 0i32)).collect();
    for e in events {
        for d in 0..8 {
            rows[d].1 += e.points[d] as u32;
            rows[d].2 += e.earned[d];
            // the totals are signed dwords (the sort's `jl` / `jg`, 0x5028C6 / 0x50290F)
            rows[d].3 = rows[d].3.wrapping_add(e.times[d]);
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

    /// A visible record of `era` nobody has completed: 0x504800 takes an era with no visible
    /// record as completed (its flag starts at 1), so a test of the finale needs the later eras.
    fn open_in(id: i16, era: Era) -> Tournament {
        Tournament { kind: TournamentKind::Era(era), ..tournament(id, 1, false) }
    }

    fn tables(ts: Vec<Tournament>) -> CareerTables {
        CareerTables { raw: Default::default(), tracks: vec![], tournaments: ts, parts: vec![], personalities: vec![], missions: vec![], strings: Strings::default(), catalog: vec![], package_count: 0 }
    }

    /// 0x4ECAFD…0x4ECB5B: the last race is the one undone event; the end of the era also needs it
    /// to be the era's only open tournament, never completed before.
    #[test]
    fn last_race_and_end_of_era_flags() {
        let a = tournament(1, 2, false);
        let b = tournament(2, 2, false);
        let tables = tables(vec![a.clone(), b.clone()]);
        let mut c = Career::default();
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8], event: None };
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

    /// `HWinnings` (0x436990): one row pair per event with money in it, the tournament prize of the
    /// player's place only on the last race, and the total with this race and that prize in it;
    /// the standings' WINNINGS column carries the same prize (0x431B80).
    #[test]
    fn winnings_panel_lists_the_events_and_the_last_race_prize() {
        let t = tournament(1, 2, false);
        let mut c = Career::default();
        let order = [2, 0, 1, 3, 4, 5, 6, 7];
        let w = c.winnings_after(&t, 0, &order, &[6400; 8], &[true; 8], 8, false);
        assert_eq!(w, Winnings { events: vec![(1, 800, 200)], tournament_bonus: None, total: 1000, completed: 1 });
        let r = Career::event_result(&t, 0, &order, &[6400; 8], &[true; 8]);
        c.tournaments.entry(1).or_default().events.push(r);
        let mut dirty = [true; 8];
        dirty[0] = false;
        let w = c.winnings_after(&t, 1, &[0, 1, 2, 3, 4, 5, 6, 7], &[6400; 8], &dirty, 8, true);
        let page = c.standings_page(&t, 1, &[0, 1, 2, 3, 4, 5, 6, 7], &[6400; 8], &dirty, 8, true);
        let row = page.iter().position(|(d, ..)| *d == 0).unwrap();
        assert_eq!(w.events, vec![(1, 800, 200), (2, 1000, 0)], "a dirty win keeps no bonus");
        assert_eq!(w.tournament_bonus, Some(t.prize[row]));
        assert_eq!(w.total, 800 + 200 + 1000 + t.prize[row]);
        assert_eq!(page[row].2, w.total);
        assert_eq!(w.completed, 2);
        // a slot with nothing in it is skipped and the next row takes its number (0x436BA8)
        let mut c = Career::default();
        let last = [1, 2, 3, 4, 5, 6, 7, 0];
        c.tournaments.entry(1).or_default().events.push(Career::event_result(&t, 0, &last, &[6400; 8], &[true; 8]));
        let w = c.winnings_after(&t, 1, &order, &[6400; 8], &[true; 8], 8, true);
        assert_eq!(w.events, vec![(1, 800, 200)], "an 8th place with no prize lists no row");
    }

    /// 0x431B80: on the last race each place's earnings take the tournament prize of that place,
    /// after the sort; the racers only, and nothing on an earlier race.
    #[test]
    fn standings_page_adds_the_tournament_prize_on_the_last_race() {
        let t = tournament(1, 2, false);
        let c = Career::default();
        let order = [2, 0, 1];
        let before = c.standings_page(&t, 0, &order, &[6400; 8], &[true; 8], 3, false);
        assert_eq!(before.len(), 3, "drivers 0..3 only");
        let after = c.standings_page(&t, 0, &order, &[6400; 8], &[true; 8], 3, true);
        for (k, (a, b)) in before.iter().zip(&after).enumerate() {
            assert_eq!(a.0, b.0, "same order");
            assert_eq!(b.2, a.2 + t.prize[k]);
        }
    }

    /// 0x5045C0: the first tournament under way, else the first never completed, else row 0.
    #[test]
    fn default_tournament_row_order() {
        let (a, b, x) = (tournament(1, 2, false), tournament(2, 3, false), tournament(3, 2, false));
        let list = [&a, &b, &x];
        let mut c = Career::default();
        assert_eq!(c.default_tournament_row(&[]), None);
        assert_eq!(c.default_tournament_row(&list), Some(0), "none under way, none completed: the first");
        c.tournaments.entry(1).or_default().completions = 1;
        assert_eq!(c.default_tournament_row(&list), Some(1), "the first never completed");
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8], event: Some(0) };
        c.tournaments.entry(3).or_default().events.push(r.clone());
        assert_eq!(c.default_tournament_row(&list), Some(2), "one of two done: under way beats never completed");
        c.tournaments.entry(3).or_default().events.push(EventResult { event: Some(1), ..r });
        assert_eq!(c.default_tournament_row(&list), Some(1), "all done is not under way");
        for id in [2, 3] {
            c.tournaments.entry(id).or_default().completions = 1;
        }
        c.tournaments.entry(3).or_default().events.clear();
        assert_eq!(c.default_tournament_row(&list), Some(0), "every one completed: row 0");
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

    fn stock_part(id: i16, kind: u16, kind_name: &str, price: u32) -> Part {
        Part {
            id,
            name_string_id: 5000 + id as u32,
            family: "356".into(),
            unknown_24: 0,
            kind,
            kind_name: kind_name.into(),
            year: 1950,
            zone_mask: [1; 10],
            mass_delta: 0.0,
            top_speed_pct: 0.0,
            price,
            condition: 0.0,
            geometry_swaps: vec![],
            value_c0: 0.0,
            boost: 0.0,
            grip_class: 0,
            forward_gears: 0,
            gearbox_param: 0,
            gear_ratios: [0.0; 8],
            torque_curve: [0.0; 21],
            state: 0,
        }
    }

    /// A layout race brings no part wear (`.PARTDAMAGE` 0 for `Game.GameType` 0…3, 0x4EF3E6…), the
    /// career's own `installed_parts` carries the owned car's saved state.
    #[test]
    fn a_layout_race_starts_every_garage_part_at_zero_wear() {
        let mut p = stock_part(52, 1, "Engine", 100);
        p.state = 5;
        let mut owned = OwnedCar::construct(&stock_tables(vec![p.clone()], &[52], &[]), 76, 0, 0);
        owned.part_state = vec![(52, 7.5)];
        let parts = [&p];
        assert_eq!(installed_parts(&parts, Some(&owned))[0].carried, 7.5);
        let l = layout_race_parts(&parts);
        assert_eq!((l.len(), l[0].kind, l[0].carried, l[0].state), (1, 1, 0.0, 0.0));
    }

    /// A one-car catalogue (id 76, price 10 000, the value curve flat at 100 %) whose record lists
    /// `stock` at `+0x3BC` / `+0x3BE` and `compatible` at `+0x32E` / `+0x330`.
    fn stock_tables(parts: Vec<Part>, stock: &[i16], compatible: &[i16]) -> CareerTables {
        let mut rec = vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE];
        rec[0..2].copy_from_slice(&76u16.to_le_bytes());
        rec[0x3BC..0x3BE].copy_from_slice(&(stock.len() as i16).to_le_bytes());
        for (k, id) in stock.iter().enumerate() {
            rec[0x3BE + 2 * k..0x3C0 + 2 * k].copy_from_slice(&id.to_le_bytes());
        }
        rec[0x32E..0x330].copy_from_slice(&(compatible.len() as i16).to_le_bytes());
        for (k, id) in compatible.iter().enumerate() {
            rec[0x330 + 2 * k..0x332 + 2 * k].copy_from_slice(&id.to_le_bytes());
        }
        let mut catalog = nfs_formats::fedata::parse_car_catalog(&rec).unwrap();
        catalog[0].price = 10_000;
        catalog[0].value_curve = [100.0; 51];
        let mut t = tables(vec![]);
        t.parts = parts;
        t.catalog = catalog;
        t
    }

    fn stock_parts() -> Vec<Part> {
        vec![
            stock_part(1, 9, "", 50),
            stock_part(2, 4, "Springs", 100),
            stock_part(3, 1, "Engine (Standard)", 0),
            stock_part(4, 2, "Brakes", 80),
            stock_part(5, 10, "Body", 3000),
            stock_part(12, 4, "Springs", 300),
            stock_part(13, 1, "Engine", 900),
        ]
    }

    /// 0x4EE9A0 in the constructor: `+0x75C` is the record's stock list in its order, and `+0x758`
    /// copies of it sorted by 0x49C350 (category, then price, then id).
    #[test]
    fn a_new_car_carries_its_stock_list_and_copies_of_it() {
        let t = stock_tables(stock_parts(), &[1, 2, 3, 4, 5], &[]);
        let car = OwnedCar::construct(&t, 76, 0, 0);
        assert_eq!(car.stock, vec![(1, 0.0), (2, 0.0), (3, 0.0), (4, 0.0), (5, 0.0)]);
        assert_eq!(car.parts, vec![3, 4, 2, 1, 5]);
        assert_eq!(car.part_state.len(), 5);
        assert_eq!(car_damage_pct(&t, &car), 0.0);
        // the dealership builds the car the same way
        let mut c = Career::default();
        c.money = 1_000_000;
        c.buy_car(&t, 76, 0, Trim::default()).unwrap();
        assert_eq!(c.garage[0].stock, car.stock);
        assert_eq!(c.garage[0].parts, car.parts);
    }

    /// 0x4F8E50: `Σ 0x518220(+0x758) − Σ 0x518220(+0x75C)` — the stock parts cancel while they are
    /// undamaged in their slots; an upgrade adds its resale over the stock part's, a damaged stock
    /// part takes value off.
    #[test]
    fn the_value_subtracts_the_stock_list() {
        let t = stock_tables(stock_parts(), &[1, 2, 3, 4, 5], &[]);
        let c = Career::default();
        let mut car = OwnedCar::construct(&t, 76, 0, 0);
        // base 10 000, damage 0 costs 1 %
        let bare = car_value_from(10_000, 100.0, 0, 0.0, 0, 0);
        assert_eq!(c.value_of(&t, &car), Some(bare));
        // sport springs (300) in place of the stock ones (100): 80 % of each
        car.parts.retain(|id| *id != 2);
        car.part_state.retain(|(id, _)| *id != 2);
        car.parts.push(12);
        car.part_state.push((12, 0.0));
        assert_eq!(c.value_of(&t, &car), Some(bare + 240 - 80));
        // the stock brakes (80) at 10 damage: 60 % instead of 80 %
        let mut worn = OwnedCar::construct(&t, 76, 0, 0);
        worn.part_state.iter_mut().find(|(id, _)| *id == 4).unwrap().1 = 10.0;
        worn.damage_pct = car_damage_pct(&t, &worn);
        let d = worn.damage_pct;
        assert_eq!(c.value_of(&t, &worn), Some(car_value_from(10_000, 100.0, 0, d, 0, 48 - 64)));
    }

    /// 0x4F8590's back-fill: every stock slot no pick fills gets a copy of its stock part, and a
    /// stock `"Engine (Standard)"` is filled only by another `"Engine (Standard)"`.
    #[test]
    fn a_market_car_back_fills_its_empty_stock_slots() {
        let t = stock_tables(stock_parts(), &[1, 2, 3, 4, 5], &[12, 13]);
        let c = Career::default();
        let stock = vec![(1, 0.0), (2, 0.0), (3, 0.0), (4, 0.0), (5, 0.0)];
        // the share roll 90 → band 76..100, then 21 + 29 % 29 = 21 → 21 % of 2 picks = 0
        let (mut r, _) = script(&[90, 29]);
        assert_eq!(c.generate_used_parts(&t, &t.catalog[0], &stock, &mut r), stock);
        // five fitting (sorted 13, 13, 12, 12, 12) at 21 + 28 = 49 %: two picks, each `rand() % n`
        // = 0 — the first 13, then the 12 moved into slot 0
        let t = stock_tables(stock_parts(), &[1, 2, 3, 4, 5], &[12, 13, 12, 13, 12]);
        let (mut r, _) = script(&[90, 28, 0, 0]);
        let got = c.generate_used_parts(&t, &t.catalog[0], &stock, &mut r);
        // picks: the engine (13, the first of the sorted list) and the springs (12) — the stock
        // springs' slot is filled, the stock engine standard's is not
        let ids: Vec<i16> = got.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![13, 12, 1, 3, 4, 5]);
    }

    /// Older port careers kept the stock parts implicit: `upgrade` rebuilds the list and puts the
    /// stock part back in every empty slot, once.
    #[test]
    fn a_version_1_career_gets_its_stock_parts() {
        let t = stock_tables(stock_parts(), &[1, 2, 3, 4, 5], &[]);
        let mut c = Career::default();
        c.version = 1;
        let mut old = OwnedCar::new(76, 0, 0);
        old.parts = vec![12];
        old.part_state = vec![(12, 3.0)];
        c.garage.push(old);
        c.upgrade(&t);
        assert_eq!(c.version, SAVE_VERSION);
        assert_eq!(c.garage[0].stock.len(), 5);
        assert_eq!(c.garage[0].parts, vec![12, 1, 3, 4, 5]);
        assert_eq!(c.garage[0].part_state[0], (12, 3.0));
        let again = c.clone();
        c.upgrade(&t);
        assert_eq!(c, again);
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
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8], event: None };
        let mut c = Career::default();
        c.tournaments.entry(4).or_default().events.push(r);
        c.forfeit(4);
        assert!(c.state(4).events.is_empty());
        c.garage.push(OwnedCar { car_id: 76, paint_set: 0, trim: Trim::default(), custom: [None; 4], parts: vec![], stock: vec![], mileage: 0, wins: 0, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, setup_backup: CAR_SETUP_DEFAULT, purchase_price: 0, asking_price: Some(9_000), offer: Some(8_000) });
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
        let r = EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8], event: None };
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
        let tables = tables(vec![t.clone(), tournament(31, 1, true), open_in(40, Era::Golden), open_in(41, Era::Modern)]);
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
        // the only visible Classic tournament is done: era advances, year steps, and the finale
        // the end-of-era screens offer is the Classic one (0x504990(0x504800()))
        assert!(o.era_advanced);
        assert_eq!(c.era, Era::Golden);
        assert_eq!(c.year, 1956);
        assert_eq!(c.finale_tournament(&tables).map(|f| f.id), Some(31));
        assert!(c.enter(&tables, &t).is_err(), "a past era's tournament is not listed");
    }

    /// 0x4D59F3…0x4D5B3F: no completed test in the type-4 start check — a completed tournament
    /// is entered again for its fee, and its second completion counts without the year step.
    #[test]
    fn completed_tournament_raced_again() {
        let t = tournament(1, 1, false);
        let other = tournament(2, 1, false);
        let tables = tables(vec![t.clone(), other, tournament(31, 1, true)]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        let order = [0, 1, 2, 3, 4, 5, 6, 7];
        c.enter(&tables, &t).unwrap();
        c.record_event(&tables, &t, 0, &order, &[0; 8], &[true; 8], Some(0));
        assert_eq!((c.state(1).completions, c.year, c.era), (1, 1956, Era::Classic));
        let money = c.money;
        assert_eq!(c.enter(&tables, &t).unwrap(), 0, "a completed tournament is entered again");
        assert_eq!(c.money, money - 500, "for its fee");
        c.record_event(&tables, &t, 0, &order, &[0; 8], &[true; 8], Some(0));
        assert_eq!((c.state(1).completions, c.year), (2, 1956), "no second year step");
    }

    /// 0x504F00: `Tournament.RemainingTracks` row k is the k-th undone event, so the events can be
    /// raced out of file order; the fee is paid while no event is done (0x4D591F), and the flags
    /// and the winnings panel follow the records, not the order they were run in.
    #[test]
    fn events_raced_out_of_order() {
        let t = tournament(1, 3, false);
        let tables = tables(vec![t.clone(), tournament(2, 1, false)]);
        let mut c = Career::default();
        c.garage.push(OwnedCar::new(76, 0, 11_000));
        assert_eq!(c.undone_events(&t), vec![0, 1, 2]);
        assert_eq!(c.enter_event(&tables, &t, 2).unwrap(), 2, "row 2 is event 2");
        assert_eq!(c.money, STARTING_MONEY - 500);
        let times = [6400; 8];
        c.record_event(&tables, &t, 2, &[0, 1, 2, 3, 4, 5, 6, 7], &times, &[true; 8], Some(0));
        assert_eq!(c.undone_events(&t), vec![0, 1]);
        assert_eq!(c.next_event(&t), Some(0));
        assert_eq!(c.remaining_event(&t, 1), Some(1));
        assert_eq!(c.remaining_event(&t, 5), Some(1), "past the end: the last row");
        let money = c.money;
        assert_eq!(c.enter_event(&tables, &t, 1).unwrap(), 1);
        assert_eq!(c.money, money, "no fee once an event is done");
        let w = c.winnings_after(&t, 1, &[0, 1, 2, 3, 4, 5, 6, 7], &times, &[true; 8], 8, false);
        // each result sits in its own record's slot (1 and 2 here); slot 0 is empty, so it is
        // skipped and the rows are numbered as listed (0x436BA8)
        assert_eq!(w.events, vec![(1, 1000, 400), (2, 1000, 400)], "listed in slot order, numbered 1, 2");
        c.record_event(&tables, &t, 1, &[0, 1, 2, 3, 4, 5, 6, 7], &times, &[true; 8], Some(0));
        assert_eq!(c.state(1).done_events(), vec![2, 1]);
        assert!(c.tournament_flags(&tables, &t).0, "event 0 is the last undone one");
        assert_eq!(c.enter(&tables, &t).unwrap(), 0);
        let o = c.record_event(&tables, &t, 0, &[0, 1, 2, 3, 4, 5, 6, 7], &times, &[true; 8], Some(0));
        assert_eq!(o.final_rank, Some(1));
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
        let t = tournament(1, 1, false);
        let finale = Tournament { prize_car: 169, ..tournament(31, 1, true) };
        let tables = tables(vec![t.clone(), finale, open_in(40, Era::Golden), open_in(41, Era::Modern)]);
        let mut c = Career::default();
        c.tournaments.entry(1).or_default().completed = true;
        // lost: nothing filed, and the award is still 0x504800's the next time
        assert_eq!(c.record_finale(&tables, &[1, 0, 2]), Some((false, 169)));
        assert!(c.garage.is_empty());
        assert_eq!(c.record_finale(&tables, &[0, 1, 2]), Some((true, 169)));
        assert_eq!(c.garage.len(), 1);
        assert_eq!(c.garage[0].car_id, 169);
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
        // 0x5021FF…0x502226 clamps the amount, not the balance
        let mut rich = Career { money: 999_999_000, ..Career::default() };
        rich.garage.push(OwnedCar::new(76, 0, 11_000));
        rich.garage[0].offer = Some(5_000);
        rich.accept_offer(&tables, 0).unwrap();
        assert_eq!(rich.money, 1_000_004_000);
    }

    /// 0x501AD0: in the Evolution (`Game.GameType` 4) `OwnedCars.Car` leaves out a car whose
    /// first model year is later than the chronicle year (`jg`, so the year itself is kept);
    /// in any other mode it is the whole garage.
    #[test]
    fn owned_list_hides_later_model_years_in_the_evolution() {
        let mut t = stock_tables(stock_parts(), &[], &[]);
        t.catalog[0].year = 1950;
        let mut boxster = t.catalog[0].clone();
        boxster.id = 77;
        boxster.year = 1996;
        t.catalog.push(boxster);
        let mut c = Career { year: 1950, ..Career::default() };
        c.garage.push(OwnedCar::new(77, 0, 0));
        c.garage.push(OwnedCar::new(76, 0, 0));
        assert_eq!(c.owned_list(&t, true), vec![1]);
        assert_eq!(c.owned_list(&t, false), vec![0, 1]);
        c.year = 1996;
        assert_eq!(c.owned_list(&t, true), vec![0, 1], "the model year itself is listed");
    }

    /// 0x4F8165…0x4F8174: the top-up's cap of 25 counts the whole used-car list, the player's
    /// listings as well as the generated cars, and drops the first generated car to make room.
    #[test]
    fn the_market_cap_counts_the_players_listings() {
        let mut t = stock_tables(stock_parts(), &[], &[]);
        t.catalog[0].year = 1950;
        t.catalog[0].sim = "356_1".into();
        let mut c = Career { year: 1975, ..Career::default() };
        // every draw 30: the rarity roll 30 % 60 = 30 is not below the age-25 entry 5, so the
        // one catalogue car is generated on every refresh
        let mut r = || 30u32;
        let fill = |c: &mut Career| {
            c.market.clear();
            for i in 0..24 {
                let mut car = OwnedCar::new(76, 0, 0);
                car.mileage = i;
                c.market.push(car);
            }
        };
        fill(&mut c);
        c.refresh_market(&t, &mut r, &mut || 0);
        assert_eq!(c.market.len(), 25, "24 on the list: the new car is appended");
        assert_eq!(c.market[0].mileage, 0);
        // one listing makes the list 25 long: the first generated car goes, the new one comes
        fill(&mut c);
        c.garage.push(OwnedCar::new(76, 0, 0));
        c.garage.push(OwnedCar::new(76, 0, 0));
        c.list_for_sale(0, 5_000).unwrap();
        c.refresh_market(&t, &mut r, &mut || 0);
        assert_eq!(c.market.len(), 24);
        assert_eq!(c.market[0].mileage, 1);
        // a list of 25 listings has no generated car to drop (0x4F818B runs off the end): the
        // new one is appended all the same
        c.market.clear();
        for _ in 0..25 {
            c.garage.push(OwnedCar::new(76, 0, 0));
        }
        for i in 1..25 {
            c.list_for_sale(i, 5_000).unwrap();
        }
        assert_eq!(c.garage.iter().filter(|g| g.asking_price.is_some()).count(), 25);
        c.refresh_market(&t, &mut r, &mut || 0);
        assert_eq!(c.market.len(), 1);
    }

    /// 0x50F27D…0x50F354: a category box fixes its parts and leaves the zones; COSMETIC (0x4F0750)
    /// clears the ten zones and fixes the `"Body"` part alone; OVERALL (0x4F0830) fixes every
    /// part and clears the zones. Each part pays `aged · _ftol(2·state) / 100` at the damage it
    /// has then (0x5189F0).
    #[test]
    fn each_quickfix_box_repairs_what_its_fix_function_does() {
        let mut parts = stock_parts();
        parts.push(stock_part(20, 10, "Rims", 500));
        let t = stock_tables(parts, &[1, 2, 3, 4, 5], &[]);
        let mut c = Career { money: 100_000, ..Career::default() };
        let mut car = OwnedCar::construct(&t, 76, 0, 0);
        car.parts.push(20);
        car.part_state.push((20, 30.0));
        for (id, s) in [(4, 10.0), (5, 20.0)] {
            car.part_state.iter_mut().find(|(p, _)| *p == id).unwrap().1 = s;
        }
        car.damage = [7.0; 10];
        c.garage.push(car);
        let state = |c: &Career, id: i16| c.garage[0].part_state.iter().find(|(p, _)| *p == id).unwrap().1;
        // brakes (category 2, $80 at 10): 80 · 20 / 100 = 16; the zones stay
        assert_eq!(c.repair(&t, 0, QuickFix::Kinds(&[2])).unwrap(), 16);
        assert_eq!((state(&c, 4), c.garage[0].damage), (0.0, [7.0; 10]));
        // COSMETIC: the body ($3000 at 20 → 1200) and the zones, not the rims
        assert_eq!(c.repair(&t, 0, QuickFix::Body).unwrap(), 1_200);
        assert_eq!((state(&c, 5), state(&c, 20), c.garage[0].damage), (0.0, 30.0, [0.0; 10]));
        // OVERALL: what is left — the rims, $500 at 30 → 300
        assert_eq!(c.repair(&t, 0, QuickFix::Overall).unwrap(), 300);
        assert_eq!(state(&c, 20), 0.0);
        assert_eq!(c.money, 100_000 - 16 - 1_200 - 300);
    }

    /// 0x4EE040…0x4EE051: the prize and the bonus each go through 0x5012E0, which clamps the
    /// amount and adds it — a balance past 999 999 999 is not cut back.
    #[test]
    fn race_money_clamps_each_amount_not_the_balance() {
        let t = tournament(1, 2, false);
        let tables = tables(vec![t.clone()]);
        let mut c = Career { money: 999_999_000, ..Career::default() };
        let o = c.record_event(&tables, &t, 0, &[0, 1, 2, 3, 4, 5, 6, 7], &[6400; 8], &[true; 8], None);
        assert!(o.result.prize > 0);
        assert_eq!(c.money, 999_999_000 + (o.result.prize + o.result.bonus) as i64);
    }

    /// 0x4F4240…0x4F428C: first year ≤ year ≤ last year, never a bonus / factory record or a
    /// non-playable one.
    #[test]
    fn the_dealer_lists_a_car_in_its_production_years() {
        let mut e = stock_tables(stock_parts(), &[], &[]).catalog.remove(0);
        (e.year, e.year_end) = (1956, 1959);
        let at = |y: u16| Career { year: y, ..Career::default() };
        assert!(!at(1955).dealer_lists(&e));
        assert!(at(1956).dealer_lists(&e));
        assert!(at(1959).dealer_lists(&e));
        assert!(!at(1960).dealer_lists(&e));
        e.dealer_flag_204 = 2;
        assert!(!at(1957).dealer_lists(&e));
        (e.dealer_flag_204, e.kind) = (1, 2);
        assert!(!at(1957).dealer_lists(&e));
        // a last year before the first lists nothing, not even the first year
        (e.kind, e.year_end) = (0, 1950);
        assert!(!at(1956).dealer_lists(&e));
        // the price: `+0x208` up to the first year, then `_ftol(P · [+0x20C + 4·age]) / 100`
        // (0x4EE220), both steps truncating. The shipped '50 356 (id 76): $11 000, +0x20C/+0x210
        // 100/100, and its 1950-based value curve from +0x214 100, 95, 93 — which the dealer
        // indexes by age, so age 1 reads +0x210, age 2 the curve's 1950 entry, age 3 its 1951
        e.price = 11_000;
        e.year = 1950;
        e.age_curve_head = [100.0, 100.0];
        e.value_curve[..3].copy_from_slice(&[100.0, 95.0, 93.0]);
        assert_eq!(e.dealer_price(1950), 11_000);
        assert_eq!(e.dealer_price(1951), 11_000);
        assert_eq!(e.dealer_price(1952), 11_000);
        assert_eq!(e.dealer_price(1953), 10_450);
        assert_eq!(e.dealer_price(1954), 10_230);
    }

    /// A scripted `rand`: each call hands out the next value, and the test checks none is left.
    fn script(v: &[u32]) -> (impl FnMut() -> u32 + '_, std::rc::Rc<std::cell::Cell<usize>>) {
        let at = std::rc::Rc::new(std::cell::Cell::new(0));
        let n = at.clone();
        (move || { let i = n.get(); n.set(i + 1); v[i] }, at)
    }

    /// 0x4F82EA…0x4F8460: one paint set gives the exterior and both stripes, the interior is its
    /// own draw, then the package roll (> 50) and the decal roll (> 80), each with its second draw.
    #[test]
    fn used_car_trim_draws_like_0x4f82c0() {
        // set 5 % 3, interior 7 % 2 from the front end's generator; package roll 51 → 13 % 8,
        // decal roll 81 → 42 % 100 − 1 from the CRT's
        let (mut fe, fe_used) = script(&[5, 7]);
        let (mut crt, crt_used) = script(&[51, 13, 81, 42]);
        let (set, t) = used_car_trim(3, 2, 8, &mut fe, &mut crt);
        assert_eq!(set, 2);
        assert_eq!(t, Trim { interior: 1, stripe1: 2, stripe2: 2, package: 5, decal: 41 });
        assert_eq!((fe_used.get(), crt_used.get()), (2, 4));
        // 50 and 80 fail the rolls (`jle`): package 0, decal −1, and no second draws
        let (mut fe, _) = script(&[0, 0]);
        let (mut crt, used) = script(&[50, 80]);
        assert_eq!(used_car_trim(3, 2, 8, &mut fe, &mut crt).1, Trim { interior: 0, stripe1: 0, stripe2: 0, package: 0, decal: -1 });
        assert_eq!(used.get(), 2);
        // the decal's own draw spans −1…98
        let (mut fe, _) = script(&[0, 0]);
        let (mut crt, _) = script(&[151, 7, 181, 199]);
        let t = used_car_trim(1, 1, 8, &mut fe, &mut crt).1;
        assert_eq!((t.package, t.decal), (7, 98));
        let (mut fe, _) = script(&[0, 0]);
        let (mut crt, _) = script(&[0, 181, 100]);
        assert_eq!(used_car_trim(1, 1, 8, &mut fe, &mut crt).1.decal, -1);
        // no packages.ini: the draw is still taken, the package stays 0
        let (mut fe, _) = script(&[0, 0]);
        let (mut crt, used) = script(&[99, 5, 0]);
        assert_eq!(used_car_trim(1, 1, 0, &mut fe, &mut crt).1.package, 0);
        assert_eq!(used.get(), 3);
    }

    /// 0x4F82C0's draws after the appearance: the mileage band, then the parts' share (taken even
    /// when nothing fits, 0x4F8648), then the wins band (0x4F84F3) — so a car's parts come
    /// between its mileage and its wins in the stream.
    #[test]
    fn used_car_draws_mileage_parts_then_wins() {
        let c = Career { year: 1975, ..Career::default() };
        let tables = tables(vec![]);
        let rec = nfs_formats::fedata::parse_car_catalog(&vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE]).unwrap().remove(0);
        // no paint sets / interiors: the appearance takes the two CRT rolls only (both failing);
        // mileage 10 → band 6…20, 1000 % 13000; share 0 → band 0…5, no second draw; wins 90 →
        // band 81…100, 7 % 39 — all the front end's generator
        let (mut r, used) = script(&[10, 1000, 0, 90, 7]);
        let (mut crt, crt_used) = script(&[0, 0]);
        let car = c.generate_used_car(&tables, &rec, &mut r, &mut crt);
        assert_eq!((used.get(), crt_used.get()), (5, 2));
        assert_eq!((car.mileage, car.wins), (3000, 18));
        assert!(car.parts.is_empty());
        assert_eq!(car.trim, Trim::default());
    }

    /// 0x443A60: `[header] count`, 1 when the key is missing, 0 without the file or the section.
    #[test]
    fn package_count_reads_the_header() {
        let dir = std::env::temp_dir().join(format!("nfs-packages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(package_count(&dir), 0);
        std::fs::write(dir.join("packages.ini"), "[header]\r\ncount=8\r\n\r\n[package1]\r\n").unwrap();
        assert_eq!(package_count(&dir), 8);
        std::fs::write(dir.join("packages.ini"), "[header]\r\n[package1]\r\n").unwrap();
        assert_eq!(package_count(&dir), 1);
        std::fs::write(dir.join("packages.ini"), "[package1]\r\n").unwrap();
        assert_eq!(package_count(&dir), 0);
        let _ = std::fs::remove_dir_all(dir);
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
        c.garage.push(OwnedCar { car_id: 76, paint_set: 2, trim: Trim::default(), custom: [None; 4], parts: vec![13], stock: vec![], mileage: 3000, wins: 1, damage_pct: 0.0, damage: [0.0; 10], part_state: Vec::new(), setup: CAR_SETUP_DEFAULT, setup_backup: CAR_SETUP_DEFAULT, purchase_price: 11_000, asking_price: Some(9_000), offer: None });
        c.tournaments.insert(9, TournamentState { entered: true, ..Default::default() });
        let dir = std::env::temp_dir().join(format!("nfs-career-{}", std::process::id()));
        let path = dir.join("career.json");
        c.save(&path).unwrap();
        assert_eq!(Career::load(&path).unwrap(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_knockout_series_runs_as_the_block_says() {
        let mut k = nfs_formats::sav::KnockoutState::default();
        k.alive[12] = 7; // past the nine bytes the start clears
        knockout_start(&mut k, 3, &[11, 12, 13]);
        assert!(k.active);
        assert_eq!((k.race_index, k.opponents), (0, 3));
        assert_eq!(&k.alive[..5], &[1, 1, 1, 1, 0]);
        assert_eq!(k.alive[12], 7);
        assert_eq!(k.start_pos, [4, 1, 2, 3, -5, -6, -7, -8, -9]);
        assert_eq!(k.last_pos, [-1; 9]);
        assert_eq!(&k.personalities[..4], &[11, 12, 13, 0]);
        // race 1: the player 2nd, opponents 1, 3, 4 — opponent 3 (last) is out
        knockout_after_race(&mut k, &[2, 1, 4, 3, 99, 99, 99, 99, 99], 3);
        assert!(k.active);
        assert_eq!(k.race_index, 1);
        assert_eq!(&k.alive[..4], &[1, 1, 0, 1]);
        assert_eq!(&k.last_pos[..4], &[2, 1, 4, 3]);
        assert_eq!(k.start_pos, [2, 1, -3, 3, -5, -6, -7, -8, -9]);
        // race 2: three cars, the player last — out, the series over, nothing else touched
        knockout_after_race(&mut k, &[3, 1, 2, 99, 99, 99, 99, 99, 99], 3);
        assert!(!k.active);
        assert_eq!(k.race_index, 1);
        assert_eq!(k.start_pos, [-1, -2, -3, -4, -5, -6, -7, -8, -9]);
        assert_eq!(&k.last_pos[..4], &[2, 1, 4, 3]);
    }

    #[test]
    fn the_knockout_final_ends_the_series() {
        let mut k = nfs_formats::sav::KnockoutState::default();
        knockout_start(&mut k, 1, &[5]);
        knockout_after_race(&mut k, &[1, 2, 99, 99, 99, 99, 99, 99, 99], 1);
        assert!(!k.active, "race_index reached the opponent count (0x4DE12E)");
        assert_eq!(k.race_index, 1);
        assert_eq!(&k.alive[..2], &[1, 0]);
        assert_eq!(k.start_pos[0], 1);
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
        // the car's damage figure is 0x4EF960's — its installed parts' means by type (the stock
        // copies 0x4EE9A0 fitted take their share too), not the zones' mean of 60
        let by_kind: Vec<(u16, f32)> = c.garage[0]
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .map(|p| (p.kind, c.part_damage(0, p)))
            .collect();
        assert!((c.garage[0].damage_pct - damage_pct_from(&by_kind)).abs() < 1e-4, "{}", c.garage[0].damage_pct);
        assert!(c.garage[0].damage_pct > 0.0);

        // and that is the number the two damage lists print (0x516EF6's `"%d %"`), where a part the
        // car never raced falls back to the table's own state
        assert!((c.part_damage(0, &part) - state).abs() < 0.01);
        let fresh = tables.parts.iter().find(|p| p.id != part.id && p.kind == part.kind && !c.garage[0].parts.contains(&p.id));
        if let Some(f) = fresh {
            assert_eq!(c.part_damage(0, f), f.state as f32);
        }

        // the race's part list carries each type-1 part with its own mask, condition and state
        let engine: Vec<&Part> = c.garage[0].parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).filter(|p| p.kind == 1).collect();
        let list = installed_parts_for(&tables, &c.garage[0]);
        let listed: Vec<_> = list.iter().filter(|p| p.kind == 1).collect();
        assert_eq!(listed.len(), engine.len());
        for (l, p) in listed.iter().zip(engine.iter()) {
            assert_eq!(l.mask, p.zone_mask.map(|b| b != 0));
            assert!((l.state - c.part_damage(0, p)).abs() < 0.01);
            assert!((l.scale - (1.5 - 0.005 * p.condition)).abs() < 1e-6);
        }
        // and the engine getter 0x4938A0 over it: `(zone1 + Σ scale·mean(own mask) + state) / (1 + n)`
        let mut specs = crate::police::test_specs();
        specs.parts = list.clone();
        let mut v = nfs_physics::vehicle::Vehicle::new(glam::Vec3::ZERO, glam::Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.damage = std::array::from_fn(|i| 10.0 * i as f32);
        let mean = |m: &[u8; 10]| {
            let z: Vec<f32> = (0..10).filter(|i| m[*i] != 0).map(|i| 10.0 * i as f32).collect();
            if z.len() > 1 { z.iter().sum::<f32>() / z.len() as f32 } else { z.iter().sum() }
        };
        let sum: f32 = engine.iter().map(|p| (1.5 - 0.005 * p.condition) * mean(&p.zone_mask) + c.part_damage(0, p)).sum();
        let expect = (v.damage[1] + sum) / (1 + engine.len()) as f32;
        assert!((v.engine_wear(&specs) - expect).abs() < 1e-3, "{} vs {expect}", v.engine_wear(&specs));

        // repairing costs the aged price times twice the damage, and clears both
        let cost = c.repair_cost(&tables, 0, &[1]);
        let expect: i64 = engine.iter().map(|p| part_aged_price(p, c.year) * (2.0 * c.part_damage(0, p)) as i64 / 100).sum();
        assert_eq!(cost, expect);
        assert!(cost > 0);
        let money = c.money;
        assert_eq!(c.repair(&tables, 0, QuickFix::Kinds(&[1])).unwrap(), cost);
        assert_eq!(c.money, money - cost);
        assert_eq!(c.repair_cost(&tables, 0, &[1]), 0);
        assert!(installed_parts_for(&tables, &c.garage[0]).iter().filter(|p| p.kind == 1).all(|p| p.state == 0.0));

        // the engine repair left the zones at 60: the race's start unfolds them (0x41088B), so a
        // race to 100 adds only the 40 more (0x416D31), and racing on at 100 adds nothing
        let scale = 1.5 - 0.005 * part.condition;
        let mask = part.zone_mask.map(|b| b != 0);
        let m = |z: f32| if mask.iter().filter(|b| **b).count() > 0 { z } else { 0.0 };
        c.record_damage(&tables, 0, [100.0; 10]);
        let once = c.part_damage(0, &part);
        assert!((once - (m(100.0) * scale + (m(60.0) * -scale + 0.0)).clamp(0.0, 100.0)).abs() < 0.01, "{once}");
        c.record_damage(&tables, 0, [100.0; 10]);
        assert_eq!(c.part_damage(0, &part), once, "the carried zones are not counted twice");
        // the body part is the zones' sum · 0.1 (0x4EDE0B)
        if let Some(b) = c.garage[0].parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).find(|p| is_body_part(p)) {
            assert_eq!(c.part_damage(0, b), 100.0);
        }
        // 0x493CF0 clamps each part's damage at 100 (0x493D89…0x493D9C), the front end at 0 (0x4EDDCF)
        c.garage[0].damage = [0.0; 10];
        for (id, s) in c.garage[0].part_state.iter_mut() {
            if *id == part.id {
                *s = 90.0;
            }
        }
        c.record_damage(&tables, 0, [100.0; 10]);
        assert_eq!(c.part_damage(0, &part), if m(1.0) > 0.0 { 100.0 } else { 90.0 });
        c.record_damage(&tables, 0, [0.0; 10]);
        assert_eq!(c.part_damage(0, &part), if m(1.0) > 0.0 { 0.0 } else { 90.0 }, "below 0 the front end holds it at 0");
    }
}
