//! `0xBC` — the game's option records over the room's link (`RE/notes/network.md` §11.6).
//!
//! The records are the ones the front end's launch chain builds with `0x4EDB20` / `0x4EDAB0` /
//! `0x4EDC00` (`COVERAGE-2026-09-26.md` §14.19) — `(id, value)`, `(id, string)` and `(id, slot,
//! value)` — and the one consumer of them is `0x4B4B80`, which writes them into the option table at
//! `0x5D1E40`. What this module carries is how they travel:
//!
//! * **Sender.** `NfsNet` keeps one outgoing message at `net+0x160`: byte 0 is `0xBC` (0x48FEE3),
//!   bytes 1…3 are 0 (0x48FEEA…0x48FEF6), the records follow from `+4` and the used length is
//!   `[net+0x250]` (4 when empty, 0x48FCE7). **0x48D490** appends one record — first sending what is
//!   there when the record would take the length past `0xF0` (0x48D4D9) — and **0x48D440** sends
//!   the buffer (`link->vtbl[0](0, net+0x160, len)`, i.e. to every link) and resets it to 4.
//!   **0x48D550** walks a whole stream and sends after **every** record (0x48D641…0x48D672).
//!   The records are raw little-endian dwords (`memcpy`, 0x5323E0): no byte-swapping anywhere.
//! * **Receiver.** 0x48F998 walks `msg+4` while the cursor is below `msg + 4·(len/4)`, applying
//!   each record with 0x4B4B80 and stepping by 0x4B4CD0 — a record's size in dwords, from its id.

use crate::msg::kind;

/// 0x48D4D9 `cmp ecx, 0xF0`: the buffer is sent before a record would take it past this.
pub const FLUSH_AT: usize = 0xF0;

/// **0x4B4CD0** — how many dwords the record starting at `rec[0]` takes, by its id:
///
/// | id | shape | dwords |
/// |---|---|---|
/// | `≥ 0x21E` | `id, slot, value` | 3 |
/// | `0x14E…0x21D` | `id, value` | 2 |
/// | `0x147…0x14D` | `id, slot, count, value × count` | `3 + count` |
/// | `0x142…0x146` | `id, count, value × count` | `2 + count` |
/// | `0x13A…0x141` | `id, slot, string` (NUL, padded to a dword) | `2 + ⌈(len+1)/4⌉` |
/// | `0x10E…0x139` | `id, string` | `1 + ⌈(len+1)/4⌉` |
/// | below | `id, value` | 2 |
///
/// `None` when `rec` ends inside the record.
pub fn record_words(rec: &[u32]) -> Option<usize> {
    let id = *rec.first()?;
    let n = if id >= 0x21E {
        3
    } else if id >= 0x14E {
        2
    } else if id >= 0x142 {
        let base = usize::from(id >= 0x147);
        base + 2 + *rec.get(base + 1)? as usize
    } else if id >= 0x10E {
        let base = usize::from(id >= 0x13A);
        // the bytes after the head, up to and including the NUL
        let mut words = 0;
        for w in rec.get(base + 1..)? {
            words += 1;
            if w.to_le_bytes().contains(&0) {
                break;
            }
            if words > 0x40 {
                return None;
            }
        }
        if words == 0 {
            return None;
        }
        base + 1 + words
    } else {
        2
    };
    (rec.len() >= n).then_some(n)
}

/// The message 0x48D440 sends: `0xBC`, a zero 24-bit value, and the records as they are.
pub fn message(records: &[u32]) -> Vec<u8> {
    let mut m = vec![kind::OPTIONS, 0, 0, 0];
    for w in records {
        m.extend_from_slice(&w.to_le_bytes());
    }
    m
}

/// The records of a received `0xBC` (0x48F998): dwords from `+4` to the last whole dword, cut into
/// records by [`record_words`]. A record that runs off the end is dropped, as it could not have
/// been sent whole.
pub fn decode(m: &[u8]) -> Vec<Vec<u32>> {
    let body = m.get(4..).unwrap_or(&[]);
    let words: Vec<u32> = body.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let Some(n) = record_words(&words[i..]) else { break };
        out.push(words[i..i + n].to_vec());
        i += n;
    }
    out
}

/// The outgoing buffer at `net+0x160` / `[net+0x250]`.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    words: Vec<u32>,
}

impl Outgoing {
    pub fn new() -> Self {
        Self::default()
    }

    fn len(&self) -> usize {
        4 + 4 * self.words.len()
    }

    /// **0x48D490**: append one record; when it would take the buffer past [`FLUSH_AT`] and the
    /// buffer holds something (`[net+0x250] > 4`), that is sent first — returned here.
    pub fn push(&mut self, rec: &[u32]) -> Option<Vec<u8>> {
        let mut sent = None;
        if self.len() + 4 * rec.len() > FLUSH_AT && !self.words.is_empty() {
            sent = self.flush();
        }
        self.words.extend_from_slice(rec);
        sent
    }

    /// **0x48D440**: the buffer as a message, and empty again. `None` when nothing is in it
    /// (`[net+0x250] <= 4`).
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        if self.words.is_empty() {
            return None;
        }
        let m = message(&self.words);
        self.words.clear();
        Some(m)
    }
}

/// **0x48D550**: a whole stream, one message per record.
pub fn messages_each(stream: &[Vec<u32>]) -> Vec<Vec<u8>> {
    stream.iter().map(|r| message(r)).collect()
}

/// The option ids the network launch stream (0x4B4DA0) carries, from the registry table at
/// 0x5D1E40 (`RE/tools/options.py`, `RE/GLOBALS.md`).
pub mod id {
    /// `TRACKLOCALEID0` (global 0x699F30) — the track record's name id, `Track1.Track`
    pub const TRACKLOCALEID0: u32 = 0xC;
    /// `TRACKDIRECTION0` (0x699F2C) — `Track1.Direction`'s row, `REVERSE + 2·MIRROR`
    pub const TRACKDIRECTION0: u32 = 0xD;
    /// `TRACKTRAFFIC0` (0x699F24) — `Track1.Traffic`'s row
    pub const TRACKTRAFFIC0: u32 = 0xE;
    /// `TRACKLAPS0` (0x699FB4) — `Track1.Laps`'s row, laps − 1
    pub const TRACKLAPS0: u32 = 0xF;
    /// `RACE_TYPE` (0x6573EC)
    pub const RACE_TYPE: u32 = 0x10;
    /// `NUM_LAPS` (0x6573F0)
    pub const NUM_LAPS: u32 = 0x11;
    /// `SKILL` (0x6573F4)
    pub const SKILL: u32 = 0x12;
    /// `COMM_MODE` (0x657404) — 4 in a network game (0x4EB4C8)
    pub const COMM_MODE: u32 = 0x13;
    /// `COMMSPOSITIONAL` (0x657408) — `Comms.Positional`'s row
    pub const COMMSPOSITIONAL: u32 = 0x14;
    /// `COPS` (0x657414)
    pub const COPS: u32 = 0x15;
    /// `RALLY` (0x6573F8) — the host's cheat bit 0 (0x4ED7A1: `0x4D8790(0)` → `0x4EDB20(0x19, …, 1)`)
    pub const RALLY: u32 = 0x19;
    /// `HEAVY` (0x6573FC) — the host's cheat bit 8 (`0x4D8790(8)` → `0x4EDB20(0x1A, …, 1)`)
    pub const HEAVY: u32 = 0x1A;
    /// `DERBY` (0x657400)
    pub const DERBY: u32 = 0x1B;
    /// `SCENENUMBER` (0x657C80)
    pub const SCENENUMBER: u32 = 0x1C;
    /// `LINEAR_TRACK` (0x657494)
    pub const LINEAR_TRACK: u32 = 0x1D;
    /// `DAMAGE` (0x657A2C)
    pub const DAMAGE: u32 = 0x20;
    /// `MIRROR` (0x65742C)
    pub const MIRROR: u32 = 0x22;
    /// `REVERSE` (0x657430)
    pub const REVERSE: u32 = 0x23;
    /// `NUMCARS` (0x658080)
    pub const NUMCARS: u32 = 0x26;
    /// `NUMPLAYERRACECARS` (0x658084)
    pub const NUMPLAYERRACECARS: u32 = 0x27;
    /// `NUMOPPONENTRACECARS` (0x658088)
    pub const NUMOPPONENTRACECARS: u32 = 0x28;
    /// `NOCOLLISION` (0x65740C)
    pub const NOCOLLISION: u32 = 0x29;
    /// `TRACK_NAME` (string, 0x10F)
    pub const TRACK_NAME: u32 = 0x10F;
    /// the playlist (int array, 0x143; the count is `NUMPLAYLISTSONGS` 0x3C)
    pub const PLAYLIST: u32 = 0x143;
    /// `.STARTINGPOS` (per slot, 0x247), 1 = pole
    pub const STARTINGPOS: u32 = 0x247;
    /// The loading screen's per-slot progress — 0x4A4A70 sends `(0x256, [0x65741C], step)` at every
    /// step above 0 (0x4A4B20 `push 0x256`), 0x4B4B80's per-slot band ≥ 0x21E stores the value at
    /// `[0x658608 + slot·0x580]`, and the loading screen draws it (0x4A5246)
    pub const LOADING: u32 = 0x256;
    /// `CATCHUP_LOGIC` (0x657420) — read only with two humans on one screen (0x410E8D, 0x416055)
    pub const CATCHUP_LOGIC: u32 = 0x17;
    /// `WEATHER` (0x657A24) — the race load stores 0 into it (0x414DF9)
    pub const WEATHER: u32 = 0x1E;
    /// `FOG` (0x657A28) — nothing reads it
    pub const FOG: u32 = 0x1F;
    /// `TIME` (0x657A30)
    pub const TIME: u32 = 0x21;
    /// `MEASUREMENT` (0x657434) — nothing reads it
    pub const MEASUREMENT: u32 = 0x24;
    /// `RAND_SEED` (0x657A34) — what 0x414EE9 hands `Rand_seed` at every race start
    pub const RAND_SEED: u32 = 0x25;
}

/// `(id, string)` — the record 0x4EDAB0 writes (ids `0x10E…0x139`): the id, then the text and its
/// NUL, padded with zeros to a whole dword.
pub fn string_record(id: u32, text: &str) -> Vec<u32> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let mut r = vec![id];
    r.extend(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
    r
}

/// The text of a `(id, string)` record: the bytes after the id up to the first NUL.
pub fn record_text(rec: &[u32]) -> String {
    let bytes: Vec<u8> = rec.iter().skip(1).flat_map(|w| w.to_le_bytes()).take_while(|&b| b != 0).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `(id, slot, value)` — the record 0x4EDC00 writes (ids `≥ 0x21E`).
pub fn slot_record(id: u32, slot: u32, value: u32) -> Vec<u32> {
    vec![id, slot, value]
}

/// `(id, count, values…)` — the record 0x4EDB50 writes for the array band `0x142…0x146`.
pub fn array_record(id: u32, values: &[u32]) -> Vec<u32> {
    let mut r = vec![id, values.len() as u32];
    r.extend_from_slice(values);
    r
}

/// What the host's launch chain builds for a **network game** (`Game.GameType` 3, `FEGAME_TYPE`
/// 0x30) and 0x4B4DA0 sends to the players (`RE/notes/network.md` §11.7): the records that reach
/// `0x48D490` — ids below `0x2F`, `0x10E…0x125` and `0x142…0x145` (0x4B4F77…0x4B4FA5) — in the
/// order of their last occurrence in the stream (0x4B6560 keeps the last of every id).
///
/// The stream's order is the launch chain's: the accessor dump 0x4B7F10 (the map at 0x65B3A8 is
/// sorted by name, byte-wise: `Comms.Positional`, `Track1.Direction`, `.Laps`, `.Track`,
/// `.Traffic`), 0x4BEA30 (the playlist), 0x4EB470 (`COMM_MODE`, `RACE_TYPE`, `SKILL`, `DAMAGE`),
/// 0x4ECDF0 (`TRACK_NAME`, `LINEAR_TRACK`, `SCENENUMBER`, `REVERSE`, `MIRROR`, `NUM_LAPS`), 0x4ECF00
/// (`NUMPLAYERRACECARS`, `NUMOPPONENTRACECARS`, `COPS`, `NUMCARS`) and 0x4ED5D0's own tail
/// (`NOCOLLISION`, `DERBY`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSettings {
    /// `Comms.Positional`'s row: 0 "input exchange", 1 "positional" (FEStrings 281 / 282)
    pub positional: u32,
    /// `Track1.Direction`'s row, `REVERSE + 2·MIRROR`
    pub direction_row: u32,
    /// `Track1.Laps`'s row, laps − 1
    pub laps_row: u32,
    /// `Track1.Track`'s record `+4`, the track's name string id
    pub track_name_id: u32,
    /// `Track1.Traffic`'s row
    pub traffic_row: u32,
    /// `[0x65B62C]`, the CD player's playlist (song numbers)
    pub playlist: Vec<u32>,
    /// `Game.Difficutly`: 0…2
    pub skill: u32,
    /// `Game.Damage`: 0 or 1
    pub damage: u32,
    /// the track record's file name (`trk+8`)
    pub track: String,
    /// `trk+0x6E != 1` — a point-to-point track
    pub linear: bool,
    /// laps the race runs (`Track1.Laps + 1` on a circuit, else 1)
    pub laps: u32,
    /// `trunc(Track1.Traffic · 2.5)`, before the clamp to nine cars
    pub traffic_cars: u32,
    /// `Comms.Positional == 1 && Comms.Collision == 0` (0x4ED6xx)
    pub nocollision: bool,
    /// `DERBY` `[0x657400]` — the host's "Smash Up" cheat (0x4ED7D9…0x4ED841 also set the track to the
    /// skid pad, the race type 3, damage on, `NOCOLLISION` off): the stream carries it and every
    /// machine's `Race_updateProgress` 0x416310 runs the knockout arm 0x416407…0x41646A
    pub derby: bool,
    /// `RALLY` `[0x6573F8]` / `HEAVY` `[0x6573FC]` — the host's cheat bits 0 and 8, sent **only when set**
    /// (0x4ED5D0's `0x4D8790(0)` / `(8)` → `0x4EDB20(0x19 / 0x1A, …, 1)`, between the `0x32` record and
    /// `NOCOLLISION`); every machine's cars run on the host's flags, never on a cheat of their own
    pub rally: bool,
    pub heavy: bool,
    /// the players in the race (`n` of 0x4B4DA0): the traffic cars' slots are rebased onto it
    pub players: u32,
    /// the kept traffic cars' per-slot records (0x4EEDA0 called from 0x4ED4E0…0x4ED56E), one per
    /// car, in slot order
    pub traffic: Vec<TrafficCar>,
    /// the host's own slot's records as the walk sends them, already at the host's race index
    /// (`NetPlay::own_launch_records`); 0x4ECF00 → 0x4EB710 → 0x4EEDA0 writes them right at its
    /// start, i.e. after `NUM_LAPS` (0x4ECDF0) and before `NUMPLAYERRACECARS` (0x4ED01F)
    pub own_car: Vec<Vec<u32>>,
}

/// The fields of a traffic car's per-slot record (0x4EEDA0, kind 4) this port has an owner for.
/// The record's other ids — `.DRIVERNAME`, `.LICENSE`, the spec names, `.CAMERA`, the style and
/// gearing values … (38 in all) — describe an opponent's set-up, which a traffic car (no driver,
/// 0x43AED4) does not use here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrafficCar {
    /// `.FILENAME` 0x13D — the model's file name (`car+0x83`, 0x4EEE93)
    pub model: String,
    /// `.FEMULTIPLAYERCARNAME` 0x141 — the car's display name (0x4EEE7B: `car+6` in a network
    /// game), one byte per character
    pub name: Vec<u8>,
    /// `.COLOUR` 0x225 — `0xAARRGGBB`
    pub colour: u32,
}

/// The per-slot ids of a traffic car's record the port sends, in 0x4EEDA0's own order
/// (`.FEMULTIPLAYERCARNAME`, `.FILENAME`, `.TYPE`, `.COLOUR`, `.CLASS`).
pub mod traffic_id {
    pub const NAME: u32 = 0x141;
    pub const FILENAME: u32 = 0x13D;
    pub const TYPE: u32 = 0x21F;
    pub const COLOUR: u32 = 0x225;
    pub const CLASS: u32 = 0x220;
    /// `.TYPE` = 0xB for every car (0x4EEF3E), `.CLASS` = the kind, 4 for the traffic (0x4EEF6D)
    pub const TYPE_VALUE: u32 = 0xB;
    pub const CLASS_TRAFFIC: u32 = 4;
}

/// The number of car slots a network game's launch reserves for the players: 8 (0x4ECF00, `mode ==
/// 3`), which 0x4B5079 later cuts down to the players really in the room.
pub const PLAYER_SLOTS: u32 = 8;

impl LaunchSettings {
    /// The traffic cars 0x4ECF00 keeps: `9 < traffic + players + cops + opponents` cuts them to
    /// what is left of nine (0x4ECF00's clamp, 0x4ED389), so **one** with eight player slots.
    pub fn traffic_kept(&self) -> u32 {
        self.traffic_cars.min(9 - PLAYER_SLOTS)
    }

    /// The launch records the host's 0x4B4DA0 sends, in stream order.
    pub fn stream(&self) -> Vec<Vec<u32>> {
        use id::*;
        vec![
            vec![COMMSPOSITIONAL, self.positional],
            vec![TRACKDIRECTION0, self.direction_row],
            vec![TRACKLAPS0, self.laps_row],
            vec![TRACKLOCALEID0, self.track_name_id],
            vec![TRACKTRAFFIC0, self.traffic_row],
            array_record(PLAYLIST, &self.playlist),
            vec![COMM_MODE, 4],
            vec![RACE_TYPE, 0],
            vec![SKILL, self.skill],
            vec![DAMAGE, self.damage],
            string_record(TRACK_NAME, &self.track),
            vec![LINEAR_TRACK, u32::from(self.linear)],
            vec![SCENENUMBER, 0],
            vec![REVERSE, self.direction_row & 1],
            vec![MIRROR, (self.direction_row >> 1) & 1],
            vec![NUM_LAPS, self.laps],
        ]
        .into_iter()
        .chain(self.own_car.iter().cloned())
        .chain([
            vec![NUMPLAYERRACECARS, PLAYER_SLOTS],
            vec![NUMOPPONENTRACECARS, 0],
            vec![COPS, 0],
        ])
        .chain(self.traffic_records())
        .chain([
            vec![NUMCARS, PLAYER_SLOTS + self.traffic_kept()],
        ])
        .chain(self.rally.then(|| vec![RALLY, 1]))
        .chain(self.heavy.then(|| vec![HEAVY, 1]))
        .chain([
            vec![NOCOLLISION, u32::from(self.nocollision)],
            vec![DERBY, u32::from(self.derby)],
        ])
        .collect()
    }

    /// The traffic cars' records as the host's walk sends them (0x4B4F25…0x4B4F75): the slot the
    /// launch chain wrote, `8 + k`, rebased to `slot − 8 + n` (`n` the players in the race).
    pub fn traffic_records(&self) -> Vec<Vec<u32>> {
        use crate::slots::string_record as srec;
        let mut out = Vec::new();
        for (k, c) in self.traffic.iter().take(self.traffic_kept() as usize).enumerate() {
            let slot = self.players + k as u32;
            out.push(srec(traffic_id::NAME, slot, &c.name));
            out.push(srec(traffic_id::FILENAME, slot, c.model.as_bytes()));
            out.push(slot_record(traffic_id::TYPE, slot, traffic_id::TYPE_VALUE));
            out.push(slot_record(traffic_id::COLOUR, slot, c.colour));
            out.push(slot_record(traffic_id::CLASS, slot, traffic_id::CLASS_TRAFFIC));
        }
        out
    }
}

/// The registry's names for the ids the launch walk sends (0x4B4F77…0x4B4FA5: ids below `0x2F`,
/// `0x10E…0x125`, `0x142…0x145`), from the table at 0x5D1E40 (`RE/tools/options.py`); the lookup
/// 0x4B5470 does over the whole table is case-insensitive (0x5AE3C0).
pub const SENT_NAMES: &[(u32, &str)] = &[
    (0x001, "IMPORT"),
    (0x003, "RAMP"),
    (0x004, "GHOSTCAR"),
    (0x005, "COMMPORT"),
    (0x006, "DEBUGMODE"),
    (0x007, "FE"),
    (0x008, "PRELOAD"),
    (0x009, "NOINTROMOVIES"),
    (0x00A, "NOLOADINGSCREEN"),
    (0x00B, "LOCKSIM"),
    (0x00C, "TRACKLOCALEID0"),
    (0x00D, "TRACKDIRECTION0"),
    (0x00E, "TRACKTRAFFIC0"),
    (0x00F, "TRACKLAPS0"),
    (0x010, "RACE_TYPE"),
    (0x011, "NUM_LAPS"),
    (0x012, "SKILL"),
    (0x013, "COMM_MODE"),
    (0x014, "COMMSPOSITIONAL"),
    (0x015, "COPS"),
    (0x016, "TRAFFIC_DENSITY"),
    (0x017, "CATCHUP_LOGIC"),
    (0x018, "SGGE"),
    (0x019, "RALLY"),
    (0x01A, "HEAVY"),
    (0x01B, "DERBY"),
    (0x01C, "SCENENUMBER"),
    (0x01D, "LINEAR_TRACK"),
    (0x01E, "WEATHER"),
    (0x01F, "FOG"),
    (0x020, "DAMAGE"),
    (0x021, "TIME"),
    (0x022, "MIRROR"),
    (0x023, "REVERSE"),
    (0x024, "MEASUREMENT"),
    (0x025, "RAND_SEED"),
    (0x026, "NUMCARS"),
    (0x027, "NUMPLAYERRACECARS"),
    (0x028, "NUMOPPONENTRACECARS"),
    (0x029, "NOCOLLISION"),
    (0x02A, "NUMSMACKABLES"),
    (0x02B, "DISPATCH_SPEECH"),
    (0x02C, "REVERSE_CALL"),
    (0x02D, "LANGUAGE_SPEECH"),
    (0x02E, "SPEECHNUMLOCATIONS"),
    (0x10F, "TRACK_NAME"),
    (0x110, "REPLAY_FILENAME"),
    (0x111, "FE_PLAYER_FILENAME"),
    (0x112, "SPEECHLOCATIONNAME0"),
    (0x113, "SPEECHLOCATIONNAME1"),
    (0x114, "SPEECHLOCATIONNAME2"),
    (0x115, "SPEECHLOCATIONNAME3"),
    (0x116, "SPEECHLOCATIONNAME4"),
    (0x117, "SPEECHLOCATIONNAME5"),
    (0x118, "SPEECHLOCATIONNAME6"),
    (0x119, "SPEECHLOCATIONNAME7"),
    (0x11A, "SPEECHLOCATIONNAME8"),
    (0x11B, "SPEECHLOCATIONNAME9"),
    (0x11C, "SPEECHLOCATIONNAME10"),
    (0x11D, "SPEECHLOCATIONNAME11"),
    (0x11E, "SPEECHLOCATIONNAME12"),
    (0x11F, "SPEECHLOCATIONNAME13"),
    (0x120, "SPEECHLOCATIONNAME14"),
    (0x121, "SPEECHLOCATIONNAME15"),
    (0x122, "SPEECHLOCATIONNAME16"),
    (0x123, "SPEECHLOCATIONNAME17"),
    (0x124, "SPEECHLOCATIONNAME18"),
    (0x125, "SPEECHLOCATIONNAME19"),
    (0x143, "PLAYLIST"),
    (0x144, "SPEECHSTARTSLICE"),
    (0x145, "SPEECHENDSLICE"),
];

/// The name table at **0x5D63A0** (143 entries, `{char *name; int value}`, ended by a null name
/// whose value is 0 — read from the image 2026-09-30): the symbolic values 0x4B5250 accepts for a
/// number, matched case-insensitively (`_stricmp` 0x5AE3C0) against the **rest of the text from
/// the letter on**.
const SYMBOLS: [(&str, u32); 143] = [
    ("SingleRace", 0x0), ("Pursuit", 0x1), ("Tournament", 0x2), ("KnockOut", 0x3),
    ("SkillsTest", 0x4), ("GameSetup_kNoComm", 0x0), ("GameSetup_kSplitScreen", 0x1),
    ("Network", 0x4), ("numCars", 0x9), ("Human", 0x1), ("AI", 0x2), ("Traffic", 0x4),
    ("Cop", 0x8), ("SuperCop", 0x10), ("Smackable", 0x80), ("HumanCop", 0x41), ("manual", 0x0),
    ("auto", 0x1), ("Fly", 0x1), ("English", 0x0), ("German", 0x1), ("French", 0x2),
    ("Spanish", 0x3), ("Italian", 0x4), ("Swedish", 0x5), ("British", 0x6), ("BumperCam", 0x0),
    ("BumperCam2", 0x1), ("TailCam0", 0x2), ("TailCam1", 0x3), ("TailCam2", 0x4),
    ("HeliCam0", 0x5), ("HeliCam1", 0x6), ("HeliCam2", 0x7), ("ShotgunCam2", 0x8),
    ("ShotgunCam3", 0x9), ("CircleCam", 0xA), ("SplineCam", 0xB), ("TVCam", 0xC),
    ("BlimpCam", 0xD), ("AnimCam", 0xE), ("ReverseCam1", 0xF), ("CopCam1", 0x11),
    ("CopCam2", 0x12), ("CopCam3", 0x13), ("DirectorsCam", 0x14), ("LastCam", 0x15), ("UP", 0x10),
    ("RIGHT", 0x20), ("DOWN", 0x40), ("LEFT", 0x80), ("TRIANGLE", 0x1000), ("CIRCLE", 0x2000),
    ("CROSS", 0x4000), ("SQUARE", 0x8000), ("START", 0x8), ("SELECT", 0x1), ("LEFTSHIFT1", 0x400),
    ("LEFTSHIFT2", 0x100), ("RIGHTSHIFT1", 0x800), ("RIGHTSHIFT2", 0x200), ("SC_ESC", 0x1),
    ("SC_1", 0x2), ("SC_2", 0x3), ("SC_3", 0x4), ("SC_4", 0x5), ("SC_5", 0x6), ("SC_6", 0x7),
    ("SC_7", 0x8), ("SC_8", 0x9), ("SC_9", 0xA), ("SC_0", 0xB), ("SC_MINUS", 0xC),
    ("SC_EQUAL", 0xD), ("SC_BS", 0xE), ("SC_TAB", 0xF), ("SC_Q", 0x10), ("SC_W", 0x11),
    ("SC_E", 0x12), ("SC_R", 0x13), ("SC_T", 0x14), ("SC_Y", 0x15), ("SC_U", 0x16), ("SC_I", 0x17),
    ("SC_O", 0x18), ("SC_P", 0x19), ("SC_LBRACKET", 0x1A), ("SC_RBRACKET", 0x1B),
    ("SC_ENTER", 0x1C), ("SC_CTRL", 0x1D), ("SC_A", 0x1E), ("SC_S", 0x1F), ("SC_D", 0x20),
    ("SC_F", 0x21), ("SC_G", 0x22), ("SC_H", 0x23), ("SC_J", 0x24), ("SC_K", 0x25), ("SC_L", 0x26),
    ("SC_COLON", 0x27), ("SC_QUOTE", 0x28), ("SC_BQUOTE", 0x29), ("SC_LSHIFT", 0x2A),
    ("SC_Z", 0x2C), ("SC_X", 0x2D), ("SC_C", 0x2E), ("SC_V", 0x2F), ("SC_B", 0x30), ("SC_N", 0x31),
    ("SC_M", 0x32), ("SC_COMMA", 0x33), ("SC_PERIOD", 0x34), ("SC_QUESTION", 0x35),
    ("SC_RSHIFT", 0x36), ("SC_PRTSRC", 0x37), ("SC_ALT", 0x38), ("SC_SPACE", 0x39),
    ("SC_CAPLOCK", 0x3A), ("SC_F1", 0x3B), ("SC_F2", 0x3C), ("SC_F3", 0x3D), ("SC_F4", 0x3E),
    ("SC_F5", 0x3F), ("SC_F6", 0x40), ("SC_F7", 0x41), ("SC_F8", 0x42), ("SC_F9", 0x43),
    ("SC_F10", 0x44), ("SC_NUMLOCK", 0x45), ("SC_SCROLLLOCK", 0x46), ("SC_HOME", 0x47),
    ("SC_UP", 0x48), ("SC_PGUP", 0x49), ("SC_NPMINUS", 0x4A), ("SC_CENTER", 0x4C),
    ("SC_PLUS", 0x4E), ("SC_END", 0x4F), ("SC_DOWN", 0x50), ("SC_PGDN", 0x51), ("SC_INS", 0x52),
    ("SC_DEL", 0x53), ("SC_LEFT", 0x4B), ("SC_RIGHT", 0x4D),
];

/// **0x4B5250**, the number reader of `fe.txt`'s values, byte for byte (read 2026-09-30). One pass
/// over the text with a value `v`, a sign `s` (1), a radix (10) and a count of commas:
///
/// * `' '` (0x20) — `v = 0`, `s = 1`; `','` — the colour packer, then the same reset: with fewer
///   than two commas before it `pack = ((v << 12) | pack) << 4`, else `pack |= v << 8`, and the
///   radix goes back to 10 (0x4B5298…0x4B52D6); `'-'` — `s = -1` (0x4B52DB), which then multiplies
///   **every digit that follows**; `'x'` / `'X'` — radix 16 when `v` is still 0 (0x4B52E3);
/// * a digit — `v = digit·s + radix·v` (0x4B5300…0x4B530F, wrapping 32 bits); in radix 16 a letter
///   `A`–`F` / `a`–`f` — `v = (v << 4) + (letter − 0x37 / 0x57)·s` (0x4B531D…0x4B5345);
/// * any other letter while `v` is 0 — a **name**: the table [`SYMBOLS`] is searched with
///   `_stricmp` against the text from that letter on, the value becomes the entry's (0 when none
///   matches: the terminator's value), and the scan jumps to the last byte (0x4B535B…0x4B5392);
/// * every other byte is skipped — so `1.5` reads 15 and `10abc` reads 10.
///
/// The result is `pack + v` (0x4B53A9); an empty text is 0 (0x4B5276).
pub fn parse_number(text: &str) -> u32 {
    let b = text.as_bytes();
    let (mut v, mut s, mut radix, mut commas, mut pack) = (0i32, 1i32, 10i32, 0u32, 0i32);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' => (v, s) = (0, 1),
            b',' => {
                pack = if commas < 2 { (v.wrapping_shl(12) | pack).wrapping_shl(4) } else { pack | v.wrapping_shl(8) };
                radix = 10;
                commas += 1;
                (v, s) = (0, 1);
            }
            b'-' => s = -1,
            b'x' | b'X' => {
                if v == 0 {
                    radix = 16;
                }
            }
            b'0'..=b'9' => v = ((c - b'0') as i32).wrapping_mul(s).wrapping_add(radix.wrapping_mul(v)),
            _ if radix == 16 && matches!(c, b'A'..=b'F' | b'a'..=b'f') => {
                let d = if c <= b'F' { c - 0x37 } else { c - 0x57 };
                v = v.wrapping_shl(4).wrapping_add((d as i32).wrapping_mul(s));
            }
            _ if v == 0 && c.is_ascii_alphabetic() => {
                let rest = &b[i..];
                v = SYMBOLS.iter().find(|(n, _)| n.as_bytes().eq_ignore_ascii_case(rest)).map_or(0, |&(_, x)| x as i32);
                i = b.len() - 1;
            }
            _ => {}
        }
        i += 1;
    }
    pack.wrapping_add(v) as u32
}

/// **0x4B5EE0 / 0x4B5470 over `fe.txt`** — the records the file adds to the launch's argument list,
/// in file order, for the ids the walk sends. Each line `name = value` (white space inside a line
/// is dropped, 0x4B5470) whose name the registry knows (case-insensitively) becomes one record by
/// the id's type: `(id, number)` below `0x10E`; `(id, string)` for `0x10E…0x125` (**at most 15
/// characters**, 0x4B5470's cut); `(id, count, numbers…)` for `0x142…0x145`. Lines with an unknown
/// name add nothing. `FE` (7, stored straight into `[0x65B298]`) and `IMPORT` (1, a file name)
/// take their own arms in 0x4B5470 and make no record.
pub fn fe_txt_records(text: &str) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let squeeze = |s: &str| s.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r')).collect::<String>();
        let (k, v) = (squeeze(k), squeeze(v));
        let Some(&(id, _)) = SENT_NAMES.iter().find(|(_, n)| n.eq_ignore_ascii_case(&k)) else { continue };
        if id == 1 || id == 7 {
            continue;
        }
        out.push(match id {
            0x142..=0x146 => {
                let nums: Vec<u32> = v.split(',').filter(|p| !p.is_empty()).map(parse_number).collect();
                array_record(id, &nums)
            }
            0x10E..=0x139 => string_record(id, &v.chars().take(15).collect::<String>()),
            _ => vec![id, parse_number(&v)],
        });
    }
    out
}

/// **0x4B4D60** — the ids whose records are per slot (`0x13A…0x141`, `0x147…0x14D`, `≥ 0x21E`).
pub fn per_slot(id: u32) -> bool {
    id > 0x21D || (id <= 0x14D && (id > 0x146 || (id > 0x139 && id < 0x142)))
}

/// **0x4B6560** — the stream de-duplicated: of every id (and slot, for the per-slot bands) only the
/// **last** record survives, where it stands.
pub fn dedupe_last(stream: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
    let key = |r: &Vec<u32>| (r[0], per_slot(r[0]).then(|| r.get(1).copied()));
    let mut keep = vec![true; stream.len()];
    for i in 0..stream.len() {
        if stream[i + 1..].iter().any(|r| key(r) == key(&stream[i])) {
            keep[i] = false;
        }
    }
    stream.into_iter().zip(keep).filter_map(|(r, k)| k.then_some(r)).collect()
}

/// The whole stream the host's 0x4B4DA0 walks for the broadcast bands: `fe.txt`'s records first
/// (0x4B6660 builds the list from the file, the launch chain appends after it), de-duplicated.
pub fn broadcast_stream(fe_txt: &[Vec<u32>], chain: &[Vec<u32>]) -> Vec<Vec<u32>> {
    dedupe_last(fe_txt.iter().chain(chain).cloned().collect())
}

/// The grid the host draws after the walk (0x4B4FD0…0x4B5043): for each of the `players` the
/// record `(0x247, slot, position)` where `position` is what `rand() % left` picks out of what is
/// left of `0…players−1`. `rand` is the C runtime's (0x5A17C9); the caller supplies it.
pub fn grid_draw(players: u32, mut rand: impl FnMut() -> u32) -> Vec<Vec<u32>> {
    let mut pool: Vec<u32> = (0..players).collect();
    let mut out = Vec::new();
    for slot in 0..players {
        let at = (rand() as usize) % pool.len();
        out.push(slot_record(id::STARTINGPOS, slot, pool.remove(at)));
    }
    out
}

/// The race settings a player has heard from the host's stream, as the option table would hold
/// them (0x4B4B80 writes each record into its global). Fields no record has set are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Received {
    pub positional: Option<u32>,
    pub direction_row: Option<u32>,
    pub laps_row: Option<u32>,
    pub track_name_id: Option<u32>,
    pub traffic_row: Option<u32>,
    pub playlist: Option<Vec<u32>>,
    pub race_type: Option<u32>,
    pub comm_mode: Option<u32>,
    pub skill: Option<u32>,
    pub damage: Option<u32>,
    pub track: Option<String>,
    pub linear: Option<u32>,
    pub reverse: Option<u32>,
    pub mirror: Option<u32>,
    pub laps: Option<u32>,
    pub num_player_cars: Option<u32>,
    pub num_opponent_cars: Option<u32>,
    pub cops: Option<u32>,
    pub num_cars: Option<u32>,
    pub nocollision: Option<u32>,
    /// `DERBY` (0x1B)
    pub derby: Option<u32>,
    /// `RALLY` (0x19) / `HEAVY` (0x1A) — present only when the host's cheat sent them
    pub rally: Option<u32>,
    pub heavy: Option<u32>,
    /// `.STARTINGPOS` per slot (1 = pole)
    pub grid: std::collections::BTreeMap<u32, u32>,
    /// The loading screen's progress per slot (race index): the last step each machine reported
    /// (`[0x658608 + slot·0x580]`, 0 until one came)
    pub progress: std::collections::BTreeMap<u32, u32>,
    /// The traffic cars' per-slot records the host's walk sent (slot ≥ the players in the race):
    /// slot → what they said
    pub traffic: std::collections::BTreeMap<u32, TrafficCar>,
    /// The players' per-slot car records, by **race index** — what each machine's walk sent for
    /// its own slot with the slot rewritten (0x4B4F25…0x4B4F66, `network.md` §11.12)
    pub cars: std::collections::BTreeMap<u32, crate::launchcar::CarSlot>,
    /// Every other record of the sent bands — the `fe.txt` half of the stream (`rand_seed`,
    /// `weather`, `time`, `fog`, `catchup_logic`, `measurement`, …): id → the words after it.
    pub misc: std::collections::BTreeMap<u32, Vec<u32>>,
}

impl Received {
    /// Write one record the way 0x4B4B80 does. Returns `false` for a record no field here holds
    /// (the caller logs those).
    pub fn take(&mut self, rec: &[u32]) -> bool {
        use id::*;
        let Some(&i) = rec.first() else { return false };
        let v = rec.get(1).copied();
        match i {
            COMMSPOSITIONAL => self.positional = v,
            TRACKDIRECTION0 => self.direction_row = v,
            TRACKLAPS0 => self.laps_row = v,
            TRACKLOCALEID0 => self.track_name_id = v,
            TRACKTRAFFIC0 => self.traffic_row = v,
            PLAYLIST => self.playlist = Some(rec.iter().skip(2).copied().take(v.unwrap_or(0) as usize).collect()),
            RACE_TYPE => self.race_type = v,
            COMM_MODE => self.comm_mode = v,
            SKILL => self.skill = v,
            DAMAGE => self.damage = v,
            TRACK_NAME => self.track = Some(record_text(rec)),
            LINEAR_TRACK => self.linear = v,
            REVERSE => self.reverse = v,
            MIRROR => self.mirror = v,
            NUM_LAPS => self.laps = v,
            NUMPLAYERRACECARS => self.num_player_cars = v,
            NUMOPPONENTRACECARS => self.num_opponent_cars = v,
            COPS => self.cops = v,
            NUMCARS => self.num_cars = v,
            NOCOLLISION => self.nocollision = v,
            DERBY => self.derby = v,
            RALLY => self.rally = v,
            HEAVY => self.heavy = v,
            SCENENUMBER => {}
            STARTINGPOS => {
                if let (Some(slot), Some(pos)) = (rec.get(1), rec.get(2)) {
                    self.grid.insert(*slot, *pos);
                }
            }
            LOADING => {
                if let (Some(slot), Some(step)) = (rec.get(1), rec.get(2)) {
                    self.progress.insert(*slot, *step);
                }
            }
            other if SENT_NAMES.iter().any(|(id, _)| *id == other) => {
                self.misc.insert(other, rec[1..].to_vec());
            }
            _ => return false,
        }
        true
    }

    /// **0x4B4B80 for a traffic car's record** during the launch: `rec` is one of the per-slot
    /// records of [`traffic_id`] with a slot at or past `players` (the race indices below it are
    /// the players'). Answers whether it was taken.
    pub fn take_traffic(&mut self, rec: &[u32], players: u32) -> bool {
        let (Some(&i), Some(&slot)) = (rec.first(), rec.get(1)) else { return false };
        if slot < players || !matches!(i, traffic_id::NAME | traffic_id::FILENAME | traffic_id::TYPE | traffic_id::COLOUR | traffic_id::CLASS) {
            return false;
        }
        let text = || -> Vec<u8> { rec.iter().skip(2).flat_map(|w| w.to_le_bytes()).take_while(|&b| b != 0).collect() };
        let car = self.traffic.entry(slot).or_default();
        match i {
            traffic_id::NAME => car.name = text(),
            traffic_id::FILENAME => car.model = text().iter().map(|&b| b as char).collect(),
            traffic_id::COLOUR => car.colour = rec.get(2).copied().unwrap_or(0),
            _ => {}
        }
        true
    }

    /// **0x4B4B80 for a player's per-slot car record** during the launch: `rec` is one of
    /// [`crate::launchcar::is_car_id`]'s ids with a slot below `players` (a race index; the slots
    /// from `players` on are the host's traffic cars, [`Self::take_traffic`]). Answers whether it
    /// was taken.
    pub fn take_car(&mut self, rec: &[u32], players: u32) -> bool {
        let (Some(&i), Some(&slot)) = (rec.first(), rec.get(1)) else { return false };
        if slot >= players || !crate::launchcar::is_car_id(i) {
            return false;
        }
        self.cars.entry(slot).or_default().take(rec)
    }

    /// The number a plain record of the `fe.txt` half carried, if one came.
    pub fn number(&self, id: u32) -> Option<u32> {
        self.misc.get(&id).and_then(|w| w.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loading screen's per-slot progress (`network.md` §11.15): `(0x256, slot, step)` is three
    /// dwords like every record of the per-slot band ≥ 0x21E (0x4EDC00, 0x4B4B80's `slot·0x580 +
    /// table`), and the table keeps the last step of each slot.
    #[test]
    fn a_loading_record_is_three_words_and_the_table_keeps_the_last_step() {
        assert_eq!(record_words(&[id::LOADING, 1, 5]), Some(3));
        let mut r = Received::default();
        assert!(r.take(&[id::LOADING, 1, 5]));
        assert!(r.take(&[id::LOADING, 0, 9]));
        assert!(r.take(&[id::LOADING, 1, 9]));
        assert_eq!(r.progress.get(&0), Some(&9));
        assert_eq!(r.progress.get(&1), Some(&9));
        assert_eq!(r.progress.get(&2), None);
    }

    #[test]
    fn a_record_is_sized_by_its_id_band() {
        assert_eq!(record_words(&[0xC, 0x155]), Some(2));
        assert_eq!(record_words(&[0x255, 1, 9]), Some(3));
        assert_eq!(record_words(&[0x150, 7]), Some(2));
        assert_eq!(record_words(&[0x143, 3, 1, 2, 3]), Some(5));
        assert_eq!(record_words(&[0x148, 1, 2, 5, 6]), Some(5));
        // a string of three characters and its NUL fill one dword, of four they need two
        assert_eq!(record_words(&[0x10F, u32::from_le_bytes(*b"abc\0")]), Some(2));
        assert_eq!(record_words(&[0x10F, u32::from_le_bytes(*b"abcd"), 0]), Some(3));
        assert_eq!(record_words(&[0x13B, 2, u32::from_le_bytes(*b"ab\0\0")]), Some(3));
        // ends inside the record
        assert_eq!(record_words(&[0x255, 1]), None);
        assert_eq!(record_words(&[0xC]), None);
    }

    #[test]
    fn the_buffer_sends_before_it_would_pass_0xf0_and_a_flush_resets_it() {
        let mut o = Outgoing::new();
        assert!(o.flush().is_none());
        let mut sent = Vec::new();
        // 3-dword records are 12 bytes: 4 + 12·19 = 232 <= 0xF0, the 20th would make 244
        for i in 0..20 {
            if let Some(m) = o.push(&[0x255, 0, i]) {
                sent.push(m);
            }
        }
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].len(), 4 + 12 * 19);
        assert_eq!(&sent[0][..4], &[0xBC, 0, 0, 0]);
        let rest = o.flush().unwrap();
        assert_eq!(decode(&rest), vec![vec![0x255, 0, 19]]);
        assert!(o.flush().is_none());
    }

    #[test]
    fn a_message_round_trips_through_decode() {
        let recs = vec![vec![0xC, 0x155], vec![0xD, 412], vec![0xF, 2]];
        let flat: Vec<u32> = recs.iter().flatten().copied().collect();
        assert_eq!(decode(&message(&flat)), recs);
        // a torn record at the end is dropped
        let mut m = message(&flat);
        m.truncate(m.len() - 4);
        assert_eq!(decode(&m), recs[..2].to_vec());
    }

    fn settings() -> LaunchSettings {
        LaunchSettings {
            positional: 1,
            direction_row: 3,
            laps_row: 4,
            track_name_id: 313,
            traffic_row: 2,
            playlist: vec![0, 1, 2],
            skill: 1,
            damage: 1,
            track: "monaco3".into(),
            linear: false,
            laps: 5,
            traffic_cars: 5,
            nocollision: true,
            derby: false,
            rally: false,
            heavy: false,
            players: 2,
            traffic: vec![TrafficCar { model: "sbus".into(), name: b"Bus".to_vec(), colour: 0xFF11_2233 }],
            own_car: Vec::new(),
        }
    }

    #[test]
    fn the_launch_stream_is_whole_records_in_the_broadcast_bands_and_the_receiver_reads_it_back() {
        let s = settings();
        let stream = s.stream();
        let flat: Vec<u32> = stream.iter().flatten().copied().collect();
        // every record is as long as 0x4B4CD0 says
        assert_eq!(decode(&message(&flat)), stream);
        // only what 0x4B4F77…0x4B4FA5 hands to 0x48D490 — and the traffic car's per-slot records
        // (slot 8 rebased to `n`), which the host's walk sends for every slot above 7
        for r in &stream {
            let i = r[0];
            let traffic = per_slot(i) && r[1] >= s.players;
            assert!(traffic || i < 0x2F || (0x10E..0x126).contains(&i) || (0x142..0x146).contains(&i), "{i:#x}");
        }
        let mut got = Received::default();
        for r in &stream {
            if !got.take_traffic(r, s.players) {
                assert!(got.take(r), "{r:?}");
            }
        }
        assert_eq!(got.traffic.len(), 1);
        assert_eq!(got.traffic[&2], TrafficCar { model: "sbus".into(), name: b"Bus".to_vec(), colour: 0xFF11_2233 });
        assert_eq!(got.track.as_deref(), Some("monaco3"));
        assert_eq!(got.playlist, Some(vec![0, 1, 2]));
        assert_eq!((got.reverse, got.mirror, got.laps), (Some(1), Some(1), Some(5)));
        // nine cars less eight player slots leaves one traffic car
        assert_eq!(got.num_cars, Some(9));
        assert_eq!(got.nocollision, Some(1));
    }

    /// `RALLY` 0x19 / `HEAVY` 0x1A ride the stream only when the host's cheat set them (0x4ED5D0's `0x4D8790(0)` /
    /// `(8)` → `0x4EDB20(id, …, 1)`), between the launch's cars and `NOCOLLISION`; a receiver reads them into the
    /// flags every machine's cars run on
    #[test]
    fn the_hosts_rally_and_heavy_cheats_ride_the_stream_only_when_set() {
        let plain = settings();
        assert!(plain.stream().iter().all(|r| r[0] != id::RALLY && r[0] != id::HEAVY));
        let mut s = settings();
        s.rally = true;
        s.heavy = true;
        let stream = s.stream();
        let pos = |i: u32| stream.iter().position(|r| r[0] == i).unwrap();
        assert_eq!(stream[pos(id::RALLY)], vec![id::RALLY, 1]);
        assert_eq!(stream[pos(id::HEAVY)], vec![id::HEAVY, 1]);
        assert!(pos(id::NUMCARS) < pos(id::RALLY) && pos(id::RALLY) < pos(id::HEAVY) && pos(id::HEAVY) < pos(id::NOCOLLISION));
        let mut got = Received::default();
        for r in &stream {
            if !got.take_traffic(r, s.players) {
                assert!(got.take(r), "{r:?}");
            }
        }
        assert_eq!((got.rally, got.heavy), (Some(1), Some(1)));
        let mut none = Received::default();
        for r in &plain.stream() {
            if !none.take_traffic(r, plain.players) {
                assert!(none.take(r));
            }
        }
        assert_eq!((none.rally, none.heavy), (None, None));
    }

    #[test]
    fn fe_txt_lines_become_records_by_the_ids_type_and_the_last_of_an_id_wins() {
        let text = "fe=1\nlocksim=0\nreplay_filename = replay.rpl\nramp = 1\nrand_seed = 8165\nskill = 0\nweather = 2\n\
                    time = 1\nfog=3\ntrack_name = coastal_long_name_here\nplaylist = 0,1,2\ncar[0].abs = 0\nnosuchoption = 5\n# x = 1\nskill = 2\n";
        let recs = fe_txt_records(text);
        // FE and unknown names make none; the string is cut at 15 characters; the array has its count
        assert_eq!(
            recs,
            vec![
                vec![0xB, 0],
                string_record(0x110, "replay.rpl"),
                vec![3, 1],
                vec![id::RAND_SEED, 8165],
                vec![id::SKILL, 0],
                vec![id::WEATHER, 2],
                vec![id::TIME, 1],
                vec![id::FOG, 3],
                string_record(id::TRACK_NAME, "coastal_long_na"),
                array_record(id::PLAYLIST, &[0, 1, 2]),
                vec![id::SKILL, 2],
            ]
        );
        // the chain's records come after the file's, and the last of an id survives where it stands
        let chain = vec![vec![id::SKILL, 1], string_record(id::TRACK_NAME, "monaco3")];
        let s = broadcast_stream(&recs, &chain);
        assert_eq!(s.iter().filter(|r| r[0] == id::SKILL).count(), 1);
        assert_eq!(s[s.len() - 2], vec![id::SKILL, 1]);
        assert_eq!(s[s.len() - 1], string_record(id::TRACK_NAME, "monaco3"));
        assert_eq!(s[0], vec![0xB, 0]);
        // per-slot records are told apart by their slot
        let d = dedupe_last(vec![vec![0x255, 0, 1], vec![0x255, 1, 1], vec![0x255, 0, 0]]);
        assert_eq!(d, vec![vec![0x255, 1, 1], vec![0x255, 0, 0]]);
        // a player takes them into the misc table
        let mut got = Received::default();
        for r in &s {
            assert!(got.take(r), "{r:?}");
        }
        assert_eq!((got.number(id::RAND_SEED), got.number(id::TIME), got.number(id::WEATHER), got.number(id::FOG)), (Some(8165), Some(1), Some(2), Some(3)));
        assert_eq!(parse_number("-1"), u32::MAX);
        assert_eq!(parse_number("0x1F"), 31);
        // 0x4B5250's own readings, worked by hand from the disassembly
        assert_eq!(parse_number(""), 0);
        assert_eq!(parse_number("8165"), 8165);
        assert_eq!(parse_number("-0x10"), (-16i32) as u32); // the sign multiplies every hex digit
        assert_eq!(parse_number("1.5"), 15); // a byte that is nothing is skipped
        assert_eq!(parse_number("10abc"), 10); // a letter after a digit is nothing
        assert_eq!(parse_number("human"), 1); // a name from 0x5D63A0, any case
        assert_eq!(parse_number("SC_ESC"), 1);
        assert_eq!(parse_number("KnockOut"), 3);
        assert_eq!(parse_number("nosuchname"), 0); // the terminator entry's 0
        assert_eq!(parse_number("English"), 0);
        assert_eq!(parse_number("Spanish"), 3);
        // the colour packer: one comma is `(v << 12) << 4`, the value after the last comma is added
        assert_eq!(parse_number("1,2"), 0x1_0002);
        assert_eq!(parse_number("1,2,3"), 0x12_0003);
    }

    #[test]
    fn the_grid_draw_is_a_permutation() {
        let mut n = 7u32;
        let g = grid_draw(4, || {
            n = n.wrapping_mul(1103515245).wrapping_add(12345);
            n >> 16
        });
        let mut pos: Vec<u32> = g.iter().map(|r| r[2]).collect();
        pos.sort();
        assert_eq!(pos, vec![0, 1, 2, 3]);
        assert_eq!(g.iter().map(|r| r[1]).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    }
}
