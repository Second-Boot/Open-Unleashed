//! The original save game: `SaveData\<N><Name>.sav` (`RE/notes/savegame.md`, CONFIRMED).
//!
//! The file is a flat, self-delimiting sequence of `"<ModuleName>\0" + blob` written by
//! `SaveInfo` 0x4DEDA0 and walked by `LoadInfo` 0x4DE9B0 until the file length runs out. There is
//! **no header, no version stamp, no checksum and no obfuscation** (§1) — every field is stored
//! little-endian exactly as it sits in memory. Module order is irrelevant on load (the reader is
//! name-driven), but the writer always emits [`MODULE_ORDER`], and an unknown module name aborts
//! the whole load.
//!
//! Nine modules contribute bytes; `TrackRecords` contributes none (its save fn writes
//! `SaveData\nfs5.trk` as a side effect and its load fn is `mov al,1; ret`). The optional trailing
//! `"accessor\0" u32 len` block holds the front-end accessors — options, selections, the chronicle
//! year — in `std::map` (byte-wise `strcmp`) order. All 223 statically registered names and the class that
//! serialises each one are in [`ACCESSORS`] (`savegame.md` §6.2a), so the block is fully typed
//! here: [`AccessorBlock`] parses and writes it, and a *subset* is legal because the original looks
//! every name up in its own registry and anything absent keeps its default (§9.6).

use crate::bytes::{get_u16, get_u32, Cursor};
use crate::FormatError;

const WHAT: &str = "sav";

/// On-disk module order (`SaveInfo`'s traversal of the save list `[0x65B638]`, §3.1).
pub const MODULE_ORDER: [&str; 9] =
    ["Missions", "PartsMarket", "Tournament", "PlayerInfo", "TrackRecords", "UsedCarMarket", "Personalities", "Knockout", "CDPlayer"];

/// `sizeof(FECar)` is 0x760; the writer stores the first 0x758 bytes — the two trailing dwords are
/// the part-vector pointers, saved as the id/state lists instead (§4.7).
pub const FE_CAR_IMAGE: usize = 0x758;
/// `nfs5.prt` record size, stored by value in `PartsMarket`.
pub const PART_RECORD: usize = 0x168;
/// `Knockout`'s blob: 33 dwords at `[0x690410]`.
pub const KNOCKOUT_BLOB: usize = 0x84;

/// The `Knockout` module's 0x84-byte state block (`modes.md` §5.2, CONFIRMED). The module is the raw
/// image of `[0x690410]`, so this is a typed view of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KnockoutState {
    /// +0x00 — the series is running.
    pub active: bool,
    /// +0x04 — index of the next race, 0-based.
    pub race_index: i32,
    /// +0x08 — opponents, which is also the number of races.
    pub opponents: i32,
    /// +0x0C — `alive[car]`, car 0 = the player.
    pub alive: [u8; 16],
    /// +0x1C — the personality id of opponents 1..n, drawn once at the start of the series.
    pub personalities: [i32; 8],
    /// +0x3C — the grid position for the next race, −1 when the car is out.
    pub start_pos: [i32; 9],
    /// +0x60 — where each car finished the last race, −1 before the first.
    pub last_pos: [i32; 9],
}

impl KnockoutState {
    pub fn parse(b: &[u8]) -> Self {
        let u32at = |o: usize| get_u32(b, o).unwrap_or(0);
        let i32at = |o: usize| u32at(o) as i32;
        let mut s = Self { active: b.first().copied().unwrap_or(0) != 0, race_index: i32at(4), opponents: i32at(8), ..Default::default() };
        for (i, a) in s.alive.iter_mut().enumerate() {
            *a = b.get(0x0C + i).copied().unwrap_or(0);
        }
        for (i, p) in s.personalities.iter_mut().enumerate() {
            *p = i32at(0x1C + i * 4);
        }
        for (i, p) in s.start_pos.iter_mut().enumerate() {
            *p = i32at(0x3C + i * 4);
        }
        for (i, p) in s.last_pos.iter_mut().enumerate() {
            *p = i32at(0x60 + i * 4);
        }
        s
    }

    pub fn write(&self) -> Vec<u8> {
        let mut b = vec![0u8; KNOCKOUT_BLOB];
        b[0] = u8::from(self.active);
        b[4..8].copy_from_slice(&self.race_index.to_le_bytes());
        b[8..12].copy_from_slice(&self.opponents.to_le_bytes());
        b[0x0C..0x1C].copy_from_slice(&self.alive);
        for (i, p) in self.personalities.iter().enumerate() {
            b[0x1C + i * 4..0x20 + i * 4].copy_from_slice(&p.to_le_bytes());
        }
        for (i, p) in self.start_pos.iter().enumerate() {
            b[0x3C + i * 4..0x40 + i * 4].copy_from_slice(&p.to_le_bytes());
        }
        for (i, p) in self.last_pos.iter().enumerate() {
            b[0x60 + i * 4..0x64 + i * 4].copy_from_slice(&p.to_le_bytes());
        }
        b
    }
}

/// The sequential reader every block parser here uses: the shared [`Cursor`], whose errors are
/// `Truncated { what: "sav", offset: pos, needed }` exactly as this module's own reader reported.
type Reader<'a> = Cursor<'a>;

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_cstr(out: &mut Vec<u8>, s: &str) {
    out.extend(s.chars().map(|c| c as u32 as u8));
    out.push(0);
}

/// One part on a saved car: only the id and the one float at `part+0x164` survive — the loader
/// rebuilds the rest from `nfs5.prt` (§4.7, CONFIRMED).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SavedPart {
    /// Sign-extended `i16` from `part+0x00`.
    pub id: i32,
    /// `part+0x164`, written as a raw dword and restored with `fld`/`fstp`.
    pub state: f32,
}

/// `OwnedCar` — writer 0x4EE8D0, reader 0x4EE6A0. Used by `PlayerInfo`, `UsedCarMarket` and the
/// `OpponentCar.Car` accessor.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedCar {
    /// The 0x758-byte FE car object verbatim: `+0x00` an instance dword, then the 1648-byte
    /// `nfs5.car` record at `+0x04` (`FECar::init` 0x4EEBDA `lea edi, [ebp+4]`).
    pub image: Vec<u8>,
    /// `car+0x75C` — written first: the car's **stock** parts (0x4EE9A0; `controls.md` §4.169.6
    /// corrects `career.md` §10's "spare/removed list"). The field keeps its old name.
    pub removed: Vec<SavedPart>,
    /// `car+0x758` — the installed list.
    pub installed: Vec<SavedPart>,
}

impl SavedCar {
    pub fn new(image: Vec<u8>) -> Self {
        Self { image, removed: Vec::new(), installed: Vec::new() }
    }

    fn read(r: &mut Reader) -> Result<Self, FormatError> {
        let image = r.take(FE_CAR_IMAGE)?.to_vec();
        let n1 = r.u32()? as usize;
        let n2 = r.u32()? as usize;
        let part = |r: &mut Reader| -> Result<SavedPart, FormatError> { Ok(SavedPart { id: r.i32()?, state: r.f32()? }) };
        let removed = (0..n1).map(|_| part(r)).collect::<Result<_, _>>()?;
        let installed = (0..n2).map(|_| part(r)).collect::<Result<_, _>>()?;
        Ok(Self { image, removed, installed })
    }

    fn write(&self, out: &mut Vec<u8>) {
        let mut image = self.image.clone();
        image.resize(FE_CAR_IMAGE, 0);
        out.extend_from_slice(&image);
        put_u32(out, self.removed.len() as u32);
        put_u32(out, self.installed.len() as u32);
        for p in self.removed.iter().chain(self.installed.iter()) {
            put_u32(out, p.id as u32);
            put_u32(out, p.state.to_bits());
        }
    }

    /// Read a dword of the FE car image (`car+0x68C` … `car+0x6A4`, `career.md` §10).
    pub fn field_u32(&self, off: usize) -> u32 {
        get_u32(&self.image, off).unwrap_or(0)
    }
    pub fn set_field_u32(&mut self, off: usize, v: u32) {
        if self.image.len() < off + 4 {
            self.image.resize(FE_CAR_IMAGE, 0);
        }
        self.image[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn field_u8(&self, off: usize) -> u8 {
        self.image.get(off).copied().unwrap_or(0)
    }
    pub fn set_field_u8(&mut self, off: usize, v: u8) {
        if self.image.len() < off + 1 {
            self.image.resize(FE_CAR_IMAGE, 0);
        }
        self.image[off] = v;
    }
    /// `nfs5.car` record id, the record's first word at `car+0x04` (0x4EEBDA).
    pub fn car_id(&self) -> u16 {
        get_u16(&self.image, 4).unwrap_or(0)
    }
}

/// `PlayerInfo` (save 0x5018B0 / load 0x501610) — the core of the career.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerInfo {
    /// The garage, `[0x691DF8]`.
    pub cars: Vec<SavedCar>,
    /// `[0x691D90]`, 11 000 in a fresh career, clamped to ±999 999 999 by 0x5012E0.
    pub money: i32,
    /// `[0x5DB748]` driver picture 0..9 — the same value as the file name's leading digit, **not**
    /// a format version (§5.1). Default 8.
    pub picture: u32,
    /// `[0x691DC8]` — the displayed profile name (= file stem minus the digit).
    pub player_name: String,
    /// `[0x691DE8]` — set once, at profile creation; copied into race-result records.
    pub driver_name: String,
    /// `[0x691DD5]` — "already seen the Evolution/tournament intro".
    pub seen_evolution_intro: u8,
    /// `[0x691D94]` — "already seen the Factory Driver welcome".
    pub seen_factory_welcome: u8,
    /// `[0x691DF5]` — "already seen the first-mission text".
    pub seen_first_mission: u8,
    /// `[0x691D98..0x691DB8]` — the eight user-defined paint colours (0 = slot unused).
    pub colours: [u32; 8],
}

impl PlayerInfo {
    fn read(r: &mut Reader) -> Result<Self, FormatError> {
        let n = r.u32()? as usize;
        let cars = (0..n).map(|_| SavedCar::read(r)).collect::<Result<_, _>>()?;
        let money = r.i32()?;
        let picture = r.u32()?;
        let player_name = r.cstr()?;
        let driver_name = r.cstr()?;
        let seen_evolution_intro = r.u8()?;
        let seen_factory_welcome = r.u8()?;
        let seen_first_mission = r.u8()?;
        let mut colours = [0u32; 8];
        for c in colours.iter_mut() {
            *c = r.u32()?;
        }
        Ok(Self { cars, money, picture, player_name, driver_name, seen_evolution_intro, seen_factory_welcome, seen_first_mission, colours })
    }

    fn write(&self, out: &mut Vec<u8>) {
        put_u32(out, self.cars.len() as u32);
        for c in &self.cars {
            c.write(out);
        }
        put_u32(out, self.money as u32);
        put_u32(out, self.picture);
        put_cstr(out, &self.player_name);
        put_cstr(out, &self.driver_name);
        out.push(self.seen_evolution_intro);
        out.push(self.seen_factory_welcome);
        out.push(self.seen_first_mission);
        for c in self.colours {
            put_u32(out, c);
        }
    }
}

/// `Tournament` (0x502AE0) — the seven runtime dwords plus the whole `nfs5.trn` table with its
/// standings, `done` flags and completion markers (§4.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TournamentModule {
    /// `[0x5DB764]` chronicle year (0x79E = 1950 in a fresh career).
    pub year: u32,
    /// `[0x691E7C]` next event index (`Tournament.RemainingTracks`).
    pub remaining_tracks: u32,
    /// `[0x691E6C]` selected tournament index.
    pub selected: u32,
    /// `[0x691E50]` era type: 521 Classic / 522 Golden / 523 Modern.
    pub era_type: u32,
    /// `[0x5DB768]` current event index (−1 = none).
    pub current_event: i32,
    /// `[0x5DB76C]` current tournament index (−1 = none).
    pub current_tournament: i32,
    /// `[0x691E54]` club-mode flag.
    pub club_mode: u32,
    /// `[0x691E68]` — the whole `nfs5.trn` table image (35 × 0xD40).
    pub table: Vec<u8>,
}

impl TournamentModule {
    fn read(r: &mut Reader) -> Result<Self, FormatError> {
        let year = r.u32()?;
        let remaining_tracks = r.u32()?;
        let selected = r.u32()?;
        let era_type = r.u32()?;
        let current_event = r.i32()?;
        let current_tournament = r.i32()?;
        let club_mode = r.u32()?;
        let size = r.u32()? as usize;
        let table = r.take(size)?.to_vec();
        Ok(Self { year, remaining_tracks, selected, era_type, current_event, current_tournament, club_mode, table })
    }

    fn write(&self, out: &mut Vec<u8>) {
        for v in [self.year, self.remaining_tracks, self.selected, self.era_type, self.current_event as u32, self.current_tournament as u32, self.club_mode] {
            put_u32(out, v);
        }
        put_u32(out, self.table.len() as u32);
        out.extend_from_slice(&self.table);
    }
}

/// `UsedCarMarket` (0x4F7A10): a count, the market's year, then the cars (§4.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsedCarMarket {
    /// `[0x691B58]`, 0x79E = 1950 on reset.
    pub year: u32,
    pub cars: Vec<SavedCar>,
}

/// A parsed save game. Every module is optional: the reader is bounded by the file size, so a file
/// that stops after the modules it wants leaves the rest at their defaults (§3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveFile {
    /// The whole `nfs5.fac` table image including the runtime "current mission" markers at `+0xE8`.
    pub missions: Option<Vec<u8>>,
    /// The parts currently on sale, each a 0x168-byte `nfs5.prt` record by value.
    pub parts_market: Option<Vec<Vec<u8>>>,
    pub tournament: Option<TournamentModule>,
    pub player: Option<PlayerInfo>,
    /// `TrackRecords` writes no bytes; this only records that the module name was present.
    pub track_records: bool,
    pub used_cars: Option<UsedCarMarket>,
    /// The `nfs5.per` personality ids currently in use.
    pub personalities: Option<Vec<u16>>,
    /// 33 dwords of Knockout state.
    pub knockout: Option<Vec<u8>>,
    /// The selected music-track list.
    pub cd_player: Option<Vec<u32>>,
    /// The accessor block, `None` when the file has none. Writing a subset is legal: the original
    /// looks every name up in its own registry and anything absent keeps its default.
    pub accessors: Option<AccessorBlock>,
    /// Module names seen in the file, in file order (a byte-identical rewrite needs the original
    /// order; [`SaveFile::write`] uses [`MODULE_ORDER`]).
    pub order: Vec<String>,
}

impl SaveFile {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data, "sav");
        let mut out = Self::default();
        while r.pos() < data.len() {
            let name = r.cstr()?;
            out.order.push(name.clone());
            match name.as_str() {
                "Missions" => {
                    let n = r.u32()? as usize;
                    out.missions = Some(r.take(n)?.to_vec());
                }
                "PartsMarket" => {
                    let n = r.u32()? as usize;
                    out.parts_market = Some((0..n).map(|_| r.take(PART_RECORD).map(<[u8]>::to_vec)).collect::<Result<_, _>>()?);
                }
                "Tournament" => out.tournament = Some(TournamentModule::read(&mut r)?),
                "PlayerInfo" => out.player = Some(PlayerInfo::read(&mut r)?),
                "TrackRecords" => out.track_records = true,
                "UsedCarMarket" => {
                    let n = r.u32()? as usize;
                    let year = r.u32()?;
                    let cars = (0..n).map(|_| SavedCar::read(&mut r)).collect::<Result<_, _>>()?;
                    out.used_cars = Some(UsedCarMarket { year, cars });
                }
                "Personalities" => {
                    let n = r.u32()? as usize;
                    out.personalities = Some((0..n).map(|_| r.u16()).collect::<Result<_, _>>()?);
                }
                "Knockout" => out.knockout = Some(r.take(KNOCKOUT_BLOB)?.to_vec()),
                "CDPlayer" => {
                    let n = r.u32()? as usize;
                    out.cd_player = Some((0..n).map(|_| r.u32()).collect::<Result<_, _>>()?);
                }
                "accessor" => {
                    let n = r.u32()? as usize;
                    out.accessors = Some(AccessorBlock::parse(r.take(n)?)?);
                }
                // The original's reader fails hard on a name it does not know, because it cannot
                // size the payload that follows (0x4DEBBC → return false).
                _ => {
                    return Err(FormatError::UnexpectedValue {
                        what: "sav",
                        offset: r.pos(),
                        expected: format!("one of {MODULE_ORDER:?} or \"accessor\""),
                        found: name,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for name in MODULE_ORDER {
            match name {
                "Missions" => {
                    if let Some(m) = &self.missions {
                        put_cstr(&mut out, name);
                        put_u32(&mut out, m.len() as u32);
                        out.extend_from_slice(m);
                    }
                }
                "PartsMarket" => {
                    if let Some(p) = &self.parts_market {
                        put_cstr(&mut out, name);
                        put_u32(&mut out, p.len() as u32);
                        for rec in p {
                            let mut rec = rec.clone();
                            rec.resize(PART_RECORD, 0);
                            out.extend_from_slice(&rec);
                        }
                    }
                }
                "Tournament" => {
                    if let Some(t) = &self.tournament {
                        put_cstr(&mut out, name);
                        t.write(&mut out);
                    }
                }
                "PlayerInfo" => {
                    if let Some(p) = &self.player {
                        put_cstr(&mut out, name);
                        p.write(&mut out);
                    }
                }
                "TrackRecords" => {
                    if self.track_records {
                        put_cstr(&mut out, name);
                    }
                }
                "UsedCarMarket" => {
                    if let Some(m) = &self.used_cars {
                        put_cstr(&mut out, name);
                        put_u32(&mut out, m.cars.len() as u32);
                        put_u32(&mut out, m.year);
                        for c in &m.cars {
                            c.write(&mut out);
                        }
                    }
                }
                "Personalities" => {
                    if let Some(p) = &self.personalities {
                        put_cstr(&mut out, name);
                        put_u32(&mut out, p.len() as u32);
                        for id in p {
                            out.extend_from_slice(&id.to_le_bytes());
                        }
                    }
                }
                "Knockout" => {
                    if let Some(k) = &self.knockout {
                        put_cstr(&mut out, name);
                        let mut k = k.clone();
                        k.resize(KNOCKOUT_BLOB, 0);
                        out.extend_from_slice(&k);
                    }
                }
                "CDPlayer" => {
                    if let Some(l) = &self.cd_player {
                        put_cstr(&mut out, name);
                        put_u32(&mut out, l.len() as u32);
                        for v in l {
                            put_u32(&mut out, *v);
                        }
                    }
                }
                _ => unreachable!("{WHAT}: {name}"),
            }
        }
        if let Some(a) = &self.accessors {
            let payload = a.write();
            put_cstr(&mut out, "accessor");
            put_u32(&mut out, payload.len() as u32);
            out.extend_from_slice(&payload);
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// The accessor block (§6, §6.1, §6.2a)
// ---------------------------------------------------------------------------------------------

/// What one accessor writes, by the class that backs it (`RE/notes/savegame.md` §6.1, CONFIRMED).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    /// The generic `FEAccessor::Save`/`Load` 0x4BC460 / 0x4BC480: the dword at `this+0x18`.
    Dword,
    /// `FEStringAccessor` 0x4BCBF0: a NUL-terminated string.
    Text,
    /// `FECharVectAccessor` 0x4B8D70: `u32 n`, `n × (char[] NUL)`, `u32 selection` — or just the
    /// selection when the accessor carries a list it does not own (`this+0x34 != 0`).
    CharVect,
    /// `FEColourAccessor` 0x4B9520: selection and RGBA.
    Colour,
    /// `FEOwnedCarAccessor` 0x4F5B30: the car count and the selection (the cars live in `PlayerInfo`).
    OwnedCarSel,
    /// `FEOpponentCarAccessor` 0x4F56C0: a selection then seven embedded cars.
    OpponentCars,
    /// `FEControllerDeadZoneAccessor` 0x51BEC0: a selection then sixteen dwords.
    DeadZone,
    /// `FEControllerInputAccessor` 0x51D060: six dwords and no selection.
    ControllerInput,
    /// `Factory.Data`, `Comms.Data`, `Showcase.Text` and `Tournament.Data`: their Save and Load are
    /// both `0x50BBE0`, which is `ret 4` — they write nothing at all.
    Empty,
}

/// Every accessor the front end registers, with the class that serialises it — the byte-wise
/// ascending order the `std::map` at `[0x65B3A8]` keeps them in, which is the order the block must
/// be written in. Extracted from the image: each name's push site names its constructor, the
/// constructor's vtable store names the class through its RTTI descriptor (`savegame.md` §6.2a).
pub const ACCESSORS: [(&str, AccessorKind); 223] = [
    ("ABOOL", AccessorKind::Dword), // FEBoolAccessor
    ("ANINTEGER", AccessorKind::Dword), // FEIntAccessor
    ("ANOTHER", AccessorKind::Text), // FEStringAccessor
    ("Audio.AmbientLevel", AccessorKind::Dword), // FEIntAccessor
    ("Audio.EngineLevel", AccessorKind::Dword), // FEIntAccessor
    ("Audio.Mode", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Audio.MusicLevel", AccessorKind::Dword), // FEIntAccessor
    ("Audio.Quality", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Audio.SfxLevel", AccessorKind::Dword), // FEIntAccessor
    ("Audio.SpeechLevel", AccessorKind::Dword), // FEIntAccessor
    ("BrakeBias", AccessorKind::Dword), // FEFloatAccessor
    ("CARSETTINGS.CAR", AccessorKind::Dword), // FECarSettingsCarAccessor
    ("CDPlayer.AvailableList", AccessorKind::Dword), // FEAvailableSongsAccessor
    ("CDPlayer.Options", AccessorKind::Dword), // FELocalizedTextAccessor
    ("CDPlayer.SelectedList", AccessorKind::Dword), // FESelectedSongsAccessor
    ("CDPlayer.Title", AccessorKind::Dword), // FEPositionableAccessor (2026-09-28: found missing in a genuine save, see savegame.md "Genuine save 2026-09-28")
    ("CDPlayer.View", AccessorKind::Dword), // FELocalizedTextAccessor
    ("COMMS.ACCOUNTID", AccessorKind::Text), // FEStringAccessor
    ("COMMS.PASSWORD", AccessorKind::Text), // FEStringAccessor
    ("Camera.1", AccessorKind::Dword), // FEPositionableAccessor
    ("Camera.2", AccessorKind::Dword), // FEPositionableAccessor
    ("Camera.3", AccessorKind::Dword), // FEPositionableAccessor
    ("Camera.4", AccessorKind::Dword), // FEPositionableAccessor
    ("Camera.Current", AccessorKind::Dword), // FEPositionableAccessor
    ("Car.ABS", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Car.Roof", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Car.Traction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Car.Transmission", AccessorKind::Dword), // FELocalizedTextAccessor
    ("CarCompare.Data", AccessorKind::Dword), // FECarCompareDataAccessor
    ("Chronicle.Year", AccessorKind::Dword), // FEIntAccessor
    ("Comms.ChatInput", AccessorKind::Text), // FEStringAccessor
    ("Comms.Collision", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Comms.Data", AccessorKind::Empty), // FEMultiplayerDataAccessor
    ("Comms.GameMode", AccessorKind::Dword), // FEPositionableAccessor
    ("Comms.GameName", AccessorKind::Text), // FEStringAccessor
    ("Comms.GamesList", AccessorKind::Dword), // FEGameListAccessor
    ("Comms.IP", AccessorKind::Text), // FEStringAccessor
    ("Comms.IPList", AccessorKind::CharVect), // FECharVectAccessor
    ("Comms.LobbyPlayerList", AccessorKind::Dword), // FELobbyPlayerListAccessor
    ("Comms.ModemList", AccessorKind::CharVect), // FECharVectAccessor
    ("Comms.PhoneList", AccessorKind::CharVect), // FECharVectAccessor
    ("Comms.PhoneNumber", AccessorKind::Text), // FEStringAccessor
    ("Comms.PlayerList", AccessorKind::Dword), // FEPlayerListAccessor
    ("Comms.Positional", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Comms.PrimaryServerInput", AccessorKind::Text), // FEStringAccessor
    ("Comms.ServerList", AccessorKind::Dword), // FECommInfoAccessor
    ("Controller.Accel", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.AllControllers", AccessorKind::CharVect), // FECharVectAccessor
    ("Controller.AllPads", AccessorKind::CharVect), // FECharVectAccessor
    ("Controller.Brake", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.CameraView", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.HandBrake", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.Hazards", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.Horn", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.LeftTurnSignal", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.Lights", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.LookBehind", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.LookForward", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.LookLeft", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.LookRight", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.Pause", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.ResetCar", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.RightTurnSignal", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.ShiftDown", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.ShiftUp", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.SteerLeft", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("Controller.SteerRight", AccessorKind::ControllerInput), // FEControllerInputAccessor
    ("DealerCar.Car", AccessorKind::Dword), // FEGenericCarAccessor
    ("DealerCar.Colour", AccessorKind::Colour), // FEColourAccessor (2026-09-28: corrected from FEGenericCarAccessor/Dword — its ctor 0x4B91F0 stores vtable 0x5B4E60, whose +0x1C/+0x20 are 0x4B9520/0x4B9550, the FEColourAccessor Save/Load; found via a genuine save, savegame.md "Genuine save 2026-09-28")
    ("DealerCar.IntColour", AccessorKind::Colour), // FEColourAccessor
    ("Display.Damage", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.DispCols", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.Gear", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.Map", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.MapScale", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.OppList", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.RearView", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.RecordTime", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.ScreenLayout", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.SpeedUnits", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.Speedometer", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.SplitTimes", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.Tach", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Display.Translucency", AccessorKind::Dword), // FELocalizedTextAccessor
    ("FDPrizeCars.Car", AccessorKind::Dword), // FEFactoryDriverPrizeCarAccessor
    ("FDownForce", AccessorKind::Dword), // FEFloatAccessor
    ("FF.CollisionVolume", AccessorKind::Dword), // FEIntAccessor
    ("FF.EngineVolume", AccessorKind::Dword), // FEIntAccessor
    ("FF.RoadGripVolume", AccessorKind::Dword), // FEIntAccessor
    ("FF.RoadVolume", AccessorKind::Dword), // FEIntAccessor
    ("FF.TotalVolume", AccessorKind::Dword), // FEIntAccessor
    ("FTirePressure", AccessorKind::Dword), // FEFloatAccessor
    ("Factory.Data", AccessorKind::Empty), // FEFactoryDriverDataAccessor
    ("Factory.MissionCheat", AccessorKind::Dword), // FEIntAccessor
    ("Factory.ProfileNum", AccessorKind::Dword), // FEPositionableAccessor
    ("FactoryDriver.Car", AccessorKind::Dword), // FEFactoryDriverCarAccessor
    ("Game.Damage", AccessorKind::Dword), // FEBoolAccessor
    ("Game.Difficutly", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Game.GameType", AccessorKind::Dword), // FEIntAccessor
    ("Game.MaxHumanCars", AccessorKind::Dword), // FEIntAccessor
    ("Game.NumKOOpponents", AccessorKind::Dword), // FEIntAccessor
    ("Game.NumOpponents", AccessorKind::Dword), // FEIntAccessor
    ("Gear0", AccessorKind::Dword), // FEFloatAccessor
    ("Gear1", AccessorKind::Dword), // FEFloatAccessor
    ("Gear2", AccessorKind::Dword), // FEFloatAccessor
    ("Gear3", AccessorKind::Dword), // FEFloatAccessor
    ("Gear4", AccessorKind::Dword), // FEFloatAccessor
    ("Gear5", AccessorKind::Dword), // FEFloatAccessor
    ("Gear6", AccessorKind::Dword), // FEFloatAccessor
    ("Gear7", AccessorKind::Dword), // FEFloatAccessor
    ("GenericCar.Car", AccessorKind::Dword), // FEGenericCarAccessor
    ("GenericCar.Colour", AccessorKind::Colour), // FEColourAccessor (2026-09-28: corrected the same way as DealerCar.Colour — ctor 0x4B91F0, vtable 0x5B4E60)
    ("GenericCar2.Car", AccessorKind::Dword), // FEGenericCarAccessor
    ("Graphics.CarChrome", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.CarDetail", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.DetailedShadows", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.GammaCorrection", AccessorKind::Dword), // FEIntAccessor
    ("Graphics.Headlights", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.LensFlare", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.Reflections", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.Resolutions", AccessorKind::CharVect), // FEScreenResolutionAccessor
    ("Graphics.ShadowMapping", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.SkyRendering", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.SpecialFX", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.TextureDetail", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.TexturedDamage", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.TripleBuffer", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.ViewDistance", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Graphics.WideScreen", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Knockout.Direction", AccessorKind::Dword), // FEAliasedPositionableAccessor
    ("Knockout.Laps", AccessorKind::Dword), // FEAliasedPositionableAccessor
    ("Knockout.Track", AccessorKind::Dword), // FEAliasedPositionableAccessor
    ("Knockout.TrackNumber", AccessorKind::Dword), // FEKnockoutTrackNumberAccessor
    ("Knockout.Traffic", AccessorKind::Dword), // FEAliasedPositionableAccessor
    ("LoopedTrack.Track", AccessorKind::Dword), // FETrackAccessor
    ("OpponentCar.Car", AccessorKind::OpponentCars), // FEOpponentCarAccessor
    ("OwnedCar.DecalNum", AccessorKind::Dword), // FEIntAccessor
    ("OwnedCar.ExtColour", AccessorKind::Colour), // FEColourAccessor
    ("OwnedCar.IntColour", AccessorKind::Colour), // FEColourAccessor
    ("OwnedCar.PackageNum", AccessorKind::Dword), // FELocalizedTextAccessor
    ("OwnedCar.S1Colour", AccessorKind::Colour), // FEColourAccessor
    ("OwnedCar.S2Colour", AccessorKind::Colour), // FEColourAccessor
    ("OwnedCars.Car", AccessorKind::OwnedCarSel), // FEOwnedCarAccessor
    ("Parts.AllParts", AccessorKind::Dword), // FEPartsAllPartsAccessor
    ("Parts.InstallList", AccessorKind::Dword), // FEPartsCarSetupInventoryAccessor
    ("Parts.OwnedDamageRepairList", AccessorKind::Dword), // FEPartsDamageRepairAccessor
    ("Parts.RepairShoppingList", AccessorKind::Dword), // FEPartsDamageRepairShoppingList (2026-09-28: found missing in a genuine save — ctor 0x516FE0, vtable 0x5BA434, RTTI ".?AVFEPartsDamageRepairShoppingList@@", Save/Load 0x4BC460/0x4BC480)
    ("Parts.ShoppingList", AccessorKind::Dword), // FEPartsShoppingBasket (2026-09-28: found missing in a genuine save — ctor 0x515DF0, vtable 0x5BA2E0, RTTI ".?AVFEPartsShoppingBasket@@", Save/Load 0x4BC460/0x4BC480)
    ("Parts.UsedDamageList", AccessorKind::Dword), // FEPartsDamageRepairAccessor
    ("PrizeCars.Car", AccessorKind::Dword), // FEPrizeCarAccessor
    ("QUICKFIX.BODYWORK", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.BRAKES", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.COSMETIC", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.ENGINE", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.OVERALL", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.SHOCKS", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.SUSPENSION", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.SWAYBARS", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.TIRES", AccessorKind::Dword), // FEBoolAccessor
    ("QUICKFIX.TRANSMISSION", AccessorKind::Dword), // FEBoolAccessor
    ("RDownForce", AccessorKind::Dword), // FEFloatAccessor
    ("RTirePressure", AccessorKind::Dword), // FEFloatAccessor
    ("Record.Track", AccessorKind::Dword), // FETrackRecordsDescripAccessor
    ("Replay.List", AccessorKind::CharVect), // FECharVectAccessor
    ("RideHeight", AccessorKind::Dword), // FEFloatAccessor
    ("ShockRate", AccessorKind::Dword), // FEFloatAccessor
    ("Showcase.Text", AccessorKind::Empty), // FEShowcaseTextAccessor
    ("SliderA.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("SliderA.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("SliderB.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("SliderB.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("SteeringToe", AccessorKind::Dword), // FEFloatAccessor
    ("Suspension", AccessorKind::Dword), // FEFloatAccessor
    ("Test.Windowed", AccessorKind::Dword), // FEBoolAccessor
    ("Tournament.Data", AccessorKind::Empty), // FETournamentDataAccessor
    ("Tournament.EventType", AccessorKind::Dword), // FETournamentEventTypeAccessor
    ("Tournament.RemainingTracks", AccessorKind::Dword), // FETrackAccessor
    ("Tournament.Tournament", AccessorKind::Dword), // FETournyAccessor
    ("Track.Records", AccessorKind::Dword), // FETrackRecordsAccessor
    ("Track1.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track1.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track1.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track1.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track2.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track2.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track2.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track2.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track3.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track3.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track3.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track3.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track4.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track4.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track4.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track4.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track5.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track5.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track5.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track5.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track6.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track6.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track6.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track6.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track7.Direction", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track7.Laps", AccessorKind::Dword), // FELocalizedTextAccessor
    ("Track7.Track", AccessorKind::Dword), // FETrackAccessor
    ("Track7.Traffic", AccessorKind::Dword), // FELocalizedTextAccessor
    ("UsedCar.Car", AccessorKind::OwnedCarSel), // FEOwnedCarAccessor
    ("UserList.NameInput", AccessorKind::Text), // FEStringAccessor
    ("UserList.UserList", AccessorKind::CharVect), // FESpecialUserListCharVectAccessor
    ("XAxis.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("XAxis.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("XRot.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("XRot.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("YAxis.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("YAxis.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("YRot.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("YRot.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("ZAxis.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("ZAxis.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("ZRot.Max", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("ZRot.Min", AccessorKind::DeadZone), // FEControllerDeadZoneAccessor
    ("zMergedList.Car", AccessorKind::Dword), // FEMergedCarListAccessor
];

/// The class that backs `name` (the `(MODIFIER)` suffix a layout may add is not part of the key).
///
/// Besides the static set, two slider widget classes register one accessor each when they are
/// built, named after the widget: `sprintf("%s.SliderBar", widget)` (0x4C2BEF, an `FEIntAccessor`
/// through 0x4BA310) and `sprintf("%s.Sliderbar", widget)` (0x4C50B0, an `FEPositionableAccessor`
/// through 0x4BC1F0). Both go through the base constructor's map insert, so a save the original
/// makes while such a widget exists carries them — each a plain dword (0x4BC460).
pub fn accessor_kind(name: &str) -> Option<AccessorKind> {
    if name.ends_with(".SliderBar") || name.ends_with(".Sliderbar") {
        return Some(AccessorKind::Dword);
    }
    let key = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
    ACCESSORS.iter().find(|(n, _)| *n == key).map(|(_, k)| *k)
}

/// Whether a `FECharVectAccessor`-class entry writes its list (`u32 n`, `n × char[] NUL`, then the
/// selection) or only the selection. Save 0x4B8D70 / Load 0x4B8E00 test `this+0x34`, which the
/// constructor 0x4B8990 stores from its **third argument** (0x4B89CE) and nothing else writes.
/// The eight constructions push (CONFIRMED, 2026-09-26):
///
/// | accessor | site | `+0x34` |
/// |---|---|---|
/// | `Comms.IPList` | 0x50B8BE | 0 — owns its list |
/// | `Comms.PhoneList` | 0x50B920 | 0 — owns its list |
/// | `Comms.ModemList` | 0x50BB88 | 1 |
/// | `Replay.List` | 0x4E5DF7 | 1 |
/// | `Controller.AllControllers` / `Controller.AllPads` | 0x51E4A3 / 0x51E4D6 | 1 |
/// | `UserList.UserList` (`FESpecialUserListCharVectAccessor`) | 0x4B8F8F | 1 |
/// | `Graphics.Resolutions` (`FEScreenResolutionAccessor`) | 0x4BC723 | 1 |
///
/// *(Until 2026-09-26 the reader guessed the shape from the bytes — a plausible count followed by
/// that many strings — and read a selection-only entry whose selection is small, `Replay.List` = 0
/// say, as an empty list, taking the next entry's name as its selection and desyncing the rest.)*
pub fn char_vect_owns_list(name: &str) -> bool {
    matches!(name, "Comms.IPList" | "Comms.PhoneList")
}

/// One accessor's stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessorValue {
    Dword(u32),
    Text(String),
    /// `Some(items)` when the accessor owns its list, `None` when it only stores the selection.
    CharVect { items: Option<Vec<String>>, selection: u32 },
    Colour { selection: u32, rgba: u32 },
    OwnedCarSel { cars: u32, selection: u32 },
    OpponentCars { selection: u32, cars: Vec<SavedCar> },
    DeadZone { selection: u32, values: [u32; 16] },
    ControllerInput([u32; 6]),
    Empty,
}

impl AccessorValue {
    pub fn kind(&self) -> AccessorKind {
        match self {
            Self::Dword(_) => AccessorKind::Dword,
            Self::Text(_) => AccessorKind::Text,
            Self::CharVect { .. } => AccessorKind::CharVect,
            Self::Colour { .. } => AccessorKind::Colour,
            Self::OwnedCarSel { .. } => AccessorKind::OwnedCarSel,
            Self::OpponentCars { .. } => AccessorKind::OpponentCars,
            Self::DeadZone { .. } => AccessorKind::DeadZone,
            Self::ControllerInput(_) => AccessorKind::ControllerInput,
            Self::Empty => AccessorKind::Empty,
        }
    }
}

/// The `"accessor\0"` region: `{ "<Name>\0" + payload }*`, in `std::map` order. A subset is legal —
/// the reader looks every name up in its own registry and anything absent keeps its default — but a
/// name the original does not know desyncs the rest, so [`AccessorBlock::write`] skips unknown ones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccessorBlock {
    pub entries: Vec<(String, AccessorValue)>,
}

impl AccessorBlock {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data, "sav");
        let mut entries = Vec::new();
        while r.pos() < data.len() {
            let name = r.cstr()?;
            let Some(kind) = accessor_kind(&name) else {
                return Err(FormatError::UnexpectedValue { what: "sav accessors", offset: r.pos(), expected: "a registered accessor name".into(), found: name });
            };
            let value = match kind {
                AccessorKind::Dword => AccessorValue::Dword(r.u32()?),
                AccessorKind::Text => AccessorValue::Text(r.cstr()?),
                AccessorKind::CharVect => {
                    // The shape is fixed per accessor by its constructor's `+0x34` (see
                    // [`char_vect_owns_list`]), exactly as the original's Load 0x4B8E00 decides.
                    if char_vect_owns_list(&name) {
                        let n = r.u32()? as usize;
                        let items = (0..n).map(|_| r.cstr()).collect::<Result<Vec<_>, _>>()?;
                        AccessorValue::CharVect { items: Some(items), selection: r.u32()? }
                    } else {
                        AccessorValue::CharVect { items: None, selection: r.u32()? }
                    }
                }
                AccessorKind::Colour => AccessorValue::Colour { selection: r.u32()?, rgba: r.u32()? },
                AccessorKind::OwnedCarSel => AccessorValue::OwnedCarSel { cars: r.u32()?, selection: r.u32()? },
                AccessorKind::OpponentCars => {
                    let selection = r.u32()?;
                    let cars = (0..7).map(|_| SavedCar::read(&mut r)).collect::<Result<_, _>>()?;
                    AccessorValue::OpponentCars { selection, cars }
                }
                AccessorKind::DeadZone => {
                    let selection = r.u32()?;
                    let mut values = [0u32; 16];
                    for v in values.iter_mut() {
                        *v = r.u32()?;
                    }
                    AccessorValue::DeadZone { selection, values }
                }
                AccessorKind::ControllerInput => {
                    let mut values = [0u32; 6];
                    for v in values.iter_mut() {
                        *v = r.u32()?;
                    }
                    AccessorValue::ControllerInput(values)
                }
                AccessorKind::Empty => AccessorValue::Empty,
            };
            entries.push((name, value));
        }
        Ok(Self { entries })
    }

    /// The payload region (without the `u32` length the container puts in front of it), in the
    /// `std::map` order the original writes: byte-wise ascending by name.
    pub fn write(&self) -> Vec<u8> {
        let mut entries: Vec<&(String, AccessorValue)> = self.entries.iter().filter(|(n, v)| accessor_kind(n) == Some(v.kind())).collect();
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut out = Vec::new();
        for (name, value) in entries {
            put_cstr(&mut out, name);
            match value {
                AccessorValue::Dword(v) => put_u32(&mut out, *v),
                AccessorValue::Text(s) => put_cstr(&mut out, s),
                AccessorValue::CharVect { items, selection } => {
                    // the shape is the accessor's, not the value's (0x4B8D70 tests `+0x34`)
                    if char_vect_owns_list(name) {
                        let items = items.as_deref().unwrap_or(&[]);
                        put_u32(&mut out, items.len() as u32);
                        for i in items {
                            put_cstr(&mut out, i);
                        }
                    }
                    put_u32(&mut out, *selection);
                }
                AccessorValue::Colour { selection, rgba } => {
                    put_u32(&mut out, *selection);
                    put_u32(&mut out, *rgba);
                }
                AccessorValue::OwnedCarSel { cars, selection } => {
                    put_u32(&mut out, *cars);
                    put_u32(&mut out, *selection);
                }
                AccessorValue::OpponentCars { selection, cars } => {
                    put_u32(&mut out, *selection);
                    for c in cars.iter().take(7) {
                        c.write(&mut out);
                    }
                    for _ in cars.len()..7 {
                        SavedCar::new(vec![0; FE_CAR_IMAGE]).write(&mut out);
                    }
                }
                AccessorValue::DeadZone { selection, values } => {
                    put_u32(&mut out, *selection);
                    for v in values {
                        put_u32(&mut out, *v);
                    }
                }
                AccessorValue::ControllerInput(values) => {
                    for v in values {
                        put_u32(&mut out, *v);
                    }
                }
                AccessorValue::Empty => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SaveFile {
        let mut car = SavedCar::new(vec![0u8; FE_CAR_IMAGE]);
        car.set_field_u32(0x698, 6000);
        car.set_field_u8(0x6A0, 1);
        car.installed = vec![SavedPart { id: 42, state: 100.0 }, SavedPart { id: -1, state: 0.5 }];
        car.removed = vec![SavedPart { id: 7, state: 95.0 }];
        SaveFile {
            missions: Some(vec![1, 2, 3, 4, 5]),
            parts_market: Some(vec![vec![0xAA; PART_RECORD], vec![0xBB; PART_RECORD]]),
            tournament: Some(TournamentModule { year: 1950, remaining_tracks: 2, selected: 1, era_type: 521, current_event: -1, current_tournament: -1, club_mode: 0, table: vec![9; 100] }),
            player: Some(PlayerInfo {
                cars: vec![car],
                money: 11_000,
                picture: 8,
                player_name: "Hans".into(),
                driver_name: "Hans".into(),
                seen_evolution_intro: 1,
                seen_factory_welcome: 0,
                seen_first_mission: 1,
                colours: [0x11223344, 0, 0, 0, 0, 0, 0, 0],
            }),
            track_records: true,
            used_cars: Some(UsedCarMarket { year: 1950, cars: vec![SavedCar::new(vec![3u8; FE_CAR_IMAGE])] }),
            personalities: Some(vec![31, 32, 33]),
            knockout: Some(vec![7; KNOCKOUT_BLOB]),
            cd_player: Some(vec![1, 2, 3]),
            accessors: Some(AccessorBlock {
                entries: vec![
                    ("Chronicle.Year".into(), AccessorValue::Dword(1950)),
                    ("UserList.NameInput".into(), AccessorValue::Text("Hans".into())),
                    ("OwnedCars.Car".into(), AccessorValue::OwnedCarSel { cars: 2, selection: 1 }),
                    ("Game.Damage".into(), AccessorValue::Dword(1)),
                    ("Factory.Data".into(), AccessorValue::Empty),
                    ("Controller.Accel".into(), AccessorValue::ControllerInput([1, 2, 3, 4, 5, 6])),
                    ("OwnedCar.ExtColour".into(), AccessorValue::Colour { selection: 2, rgba: 0x00FF0000 }),
                ],
            }),
            order: Vec::new(),
        }
    }

    #[test]
    fn the_accessor_table_matches_the_registry() {
        // 223 names, byte-wise ascending — the order the original's std::map keeps and the writer
        // has to reproduce.
        assert_eq!(ACCESSORS.len(), 223);
        assert_eq!(accessor_kind("UVWj"), None, "0x4FD0FF pushes a callback, not a name");
        for w in ACCESSORS.windows(2) {
            assert!(w[0].0.as_bytes() < w[1].0.as_bytes(), "{} !< {}", w[0].0, w[1].0);
        }
        assert_eq!(accessor_kind("Chronicle.Year"), Some(AccessorKind::Dword));
        assert_eq!(accessor_kind("Track1.Track(SHORT)"), Some(AccessorKind::Dword), "a layout modifier is not part of the key");
        assert_eq!(accessor_kind("OwnedCars.Car"), Some(AccessorKind::OwnedCarSel));
        assert_eq!(accessor_kind("OpponentCar.Car"), Some(AccessorKind::OpponentCars));
        assert_eq!(accessor_kind("Controller.Accel"), Some(AccessorKind::ControllerInput));
        assert_eq!(accessor_kind("XAxis.Max"), Some(AccessorKind::DeadZone));
        assert_eq!(accessor_kind("UserList.UserList"), Some(AccessorKind::CharVect));
        assert_eq!(accessor_kind("UserList.NameInput"), Some(AccessorKind::Text));
        // the four whose Save and Load are both `ret 4`
        for n in ["Factory.Data", "Comms.Data", "Showcase.Text", "Tournament.Data"] {
            assert_eq!(accessor_kind(n), Some(AccessorKind::Empty), "{n}");
        }
        assert_eq!(accessor_kind("Nonsense.Accessor"), None);
        // the per-widget slider accessors (0x4C2BEF / 0x4C50B0) carry one dword
        assert_eq!(accessor_kind("NFS5.MUSICSLIDER.SliderBar"), Some(AccessorKind::Dword));
        assert_eq!(accessor_kind("NFS5.GAMMA.Sliderbar"), Some(AccessorKind::Dword));
    }

    #[test]
    fn the_accessor_block_round_trips_every_shape() {
        let block = AccessorBlock {
            entries: vec![
                ("Chronicle.Year".into(), AccessorValue::Dword(1969)),
                ("Comms.ChatInput".into(), AccessorValue::Text("hello".into())),
                ("Comms.Data".into(), AccessorValue::Empty),
                ("Controller.Brake".into(), AccessorValue::ControllerInput([9, 8, 7, 6, 5, 4])),
                ("Comms.IPList".into(), AccessorValue::CharVect { items: Some(vec!["10.0.0.1".into(), "10.0.0.2".into()]), selection: 1 }),
                ("Graphics.Resolutions".into(), AccessorValue::CharVect { items: None, selection: 2 }),
                ("OwnedCar.IntColour".into(), AccessorValue::Colour { selection: 3, rgba: 0x11223344 }),
                ("OwnedCars.Car".into(), AccessorValue::OwnedCarSel { cars: 4, selection: 2 }),
                ("XAxis.Min".into(), AccessorValue::DeadZone { selection: 1, values: [7; 16] }),
            ],
        };
        let bytes = block.write();
        let back = AccessorBlock::parse(&bytes).expect("parse");
        let mut expect = block.clone();
        expect.entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        assert_eq!(back, expect);
        assert_eq!(back.write(), bytes);
        // an unknown name cannot be sized, exactly as it cannot be in the original
        let mut bad = b"Nonsense\0".to_vec();
        bad.extend_from_slice(&[0, 0, 0, 0]);
        assert!(AccessorBlock::parse(&bad).is_err());
        // and the writer refuses to emit one, because it would desync everything after it
        let skipped = AccessorBlock { entries: vec![("Nonsense".into(), AccessorValue::Dword(1)), ("Chronicle.Year".into(), AccessorValue::Dword(2))] };
        assert_eq!(AccessorBlock::parse(&skipped.write()).unwrap().entries.len(), 1);
    }

    /// A selection-only list accessor whose selection is 0 is followed by the next name at once;
    /// the shape comes from the constructor's `+0x34`, not from the bytes (0x4B8E00).
    #[test]
    fn a_selection_only_list_does_not_swallow_the_next_entry() {
        let mut bytes = b"Replay.List\0".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"RideHeight\0");
        bytes.extend_from_slice(&1.5f32.to_bits().to_le_bytes());
        let b = AccessorBlock::parse(&bytes).expect("parse");
        assert_eq!(
            b.entries,
            vec![("Replay.List".into(), AccessorValue::CharVect { items: None, selection: 0 }), ("RideHeight".into(), AccessorValue::Dword(1.5f32.to_bits()))]
        );
        assert_eq!(b.write(), bytes);
        assert!(char_vect_owns_list("Comms.IPList") && char_vect_owns_list("Comms.PhoneList"));
        for n in ["Comms.ModemList", "Replay.List", "Controller.AllControllers", "Controller.AllPads", "UserList.UserList", "Graphics.Resolutions"] {
            assert!(!char_vect_owns_list(n), "{n}");
        }
    }

    /// A file shaped the way `SaveInfo` 0x4DEDA0 lays one out — all nine modules in the save
    /// list's order and an accessor block holding every payload class of §6.1, including the
    /// selection-only lists and a per-widget slider accessor — parses and re-writes byte for byte.
    #[test]
    fn an_original_shaped_file_round_trips_byte_for_byte() {
        let mut s = sample();
        let mut opp = SavedCar::new(vec![5u8; FE_CAR_IMAGE]);
        opp.installed = vec![SavedPart { id: 3, state: 0.0 }];
        s.accessors = Some(AccessorBlock {
            entries: vec![
                ("Audio.MusicLevel".into(), AccessorValue::Dword(100)),
                ("Chronicle.Year".into(), AccessorValue::Dword(1962)),
                ("Comms.IPList".into(), AccessorValue::CharVect { items: Some(vec!["1.2.3.4".into()]), selection: 0 }),
                ("Comms.ModemList".into(), AccessorValue::CharVect { items: None, selection: 0 }),
                ("Comms.PhoneList".into(), AccessorValue::CharVect { items: Some(vec![]), selection: 0 }),
                ("Controller.AllPads".into(), AccessorValue::CharVect { items: None, selection: 1 }),
                ("Controller.Brake".into(), AccessorValue::ControllerInput([3, 0, 0x1F, 0, 0, 0])),
                ("DealerCar.IntColour".into(), AccessorValue::Colour { selection: 0, rgba: 0xFF00FF00 }),
                ("Factory.Data".into(), AccessorValue::Empty),
                ("Graphics.Resolutions".into(), AccessorValue::CharVect { items: None, selection: 3 }),
                ("NFS5.MUSIC.SliderBar".into(), AccessorValue::Dword(64)),
                ("OpponentCar.Car".into(), AccessorValue::OpponentCars { selection: 2, cars: vec![opp.clone(); 7] }),
                ("OwnedCars.Car".into(), AccessorValue::OwnedCarSel { cars: 1, selection: 0 }),
                ("Replay.List".into(), AccessorValue::CharVect { items: None, selection: 0 }),
                ("UserList.NameInput".into(), AccessorValue::Text("Hans".into())),
                ("UserList.UserList".into(), AccessorValue::CharVect { items: None, selection: 0 }),
                ("XAxis.Min".into(), AccessorValue::DeadZone { selection: 0, values: [0; 16] }),
            ],
        });
        let bytes = s.write();
        let back = SaveFile::parse(&bytes).expect("parse");
        assert_eq!(back.accessors.as_ref().unwrap().entries.len(), 17);
        assert_eq!(back.write(), bytes);
    }

    #[test]
    fn container_round_trips() {
        let s = sample();
        let bytes = s.write();
        let back = SaveFile::parse(&bytes).expect("parse");
        assert_eq!(back.order, ["Missions", "PartsMarket", "Tournament", "PlayerInfo", "TrackRecords", "UsedCarMarket", "Personalities", "Knockout", "CDPlayer", "accessor"]);
        let mut expect = s.clone();
        expect.order = back.order.clone();
        // the writer puts the accessors in the std::map order the original keeps them in
        if let Some(a) = expect.accessors.as_mut() {
            a.entries.sort_by(|x, y| x.0.as_bytes().cmp(y.0.as_bytes()));
        }
        assert_eq!(back, expect);
        assert_eq!(back.write(), bytes, "writing a parsed file reproduces it byte for byte");
    }

    #[test]
    fn module_names_are_nul_terminated_and_order_is_the_originals() {
        let bytes = sample().write();
        assert!(bytes.starts_with(b"Missions\0"), "the first module is Missions");
        // TrackRecords contributes its name and nothing else.
        let at = bytes.windows(13).position(|w| w == b"TrackRecords\0").expect("TrackRecords");
        assert_eq!(&bytes[at + 13..at + 13 + 14], b"UsedCarMarket\0", "no payload between them");
    }

    #[test]
    fn a_truncated_subset_still_loads() {
        // §3: modules may be omitted, and the loop is bounded by the file length.
        let s = SaveFile { player: sample().player, ..Default::default() };
        let back = SaveFile::parse(&s.write()).expect("parse");
        assert_eq!(back.player, s.player);
        assert!(back.tournament.is_none() && back.accessors.is_none());
    }

    #[test]
    fn an_unknown_module_name_is_a_hard_failure() {
        let mut bytes = b"Nonsense\0".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        assert!(SaveFile::parse(&bytes).is_err());
    }

    #[test]
    fn parts_are_written_removed_list_first() {
        let s = sample();
        let bytes = s.write();
        let back = SaveFile::parse(&bytes).unwrap();
        let car = &back.player.unwrap().cars[0];
        assert_eq!(car.removed.len(), 1);
        assert_eq!(car.installed.len(), 2);
        assert_eq!(car.installed[1].id, -1, "part ids are sign-extended i16");
        assert_eq!(car.field_u32(0x698), 6000);
        assert_eq!(car.field_u8(0x6A0), 1);
        // the record sits after the instance dword
        let mut c = SavedCar::new(vec![0u8; FE_CAR_IMAGE]);
        c.set_field_u32(0, 3);
        c.image[4..6].copy_from_slice(&993u16.to_le_bytes());
        assert_eq!(c.car_id(), 993);
    }
}
