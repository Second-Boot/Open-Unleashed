//! **Re-simulation of an input-exchange network race from a room's record** (`RE/notes/lobby.md` section 15, LOBBY-AC4).
//!
//! An input-exchange race (`Comms.Positional` = 0, `network.md` section 11.17) is a pure function of the launch stream (the host's `0xBC`
//! option records and every player's car records), the RNG seed in it and the room's 24-byte input frames: each machine runs the same
//! deterministic simulation on the same frames. A server that heard all of it can therefore play the race again and compare the result
//! with what the clients claim. This module is the *data* half of that: it turns the record into exactly the inputs the port's own network
//! replay already takes (`replay.md` section 18.15: a [`ReplaySetup`] with a [`NetSetup`], and the per-human sample lanes) -- the simulation
//! itself is the application's, unchanged (`App::replay_net_step` reads the samples from here instead of from a replay stream).
//!
//! * [`Package`] -- the record as the server writes it (JSON, hex strings): the room's list at the `0xC5` go (`roster`), every `0xBC` message
//!   in arrival order with whether it came after the first `0xB3` (`options`), and the emitted frames in order (`frames`).
//! * [`resolve`] -- the launch stream through the same tables the client's receiver keeps ([`Received`], 0x4B4B80), the race indices
//!   (0x4B4EBC: the players who are in the race, in the list's order), the `NetRace` of `net_launch_sync` and the cars of `net_race_apply`
//!   / `race_stages` as a [`ReplaySetup`] for the lowest race index (the "local" car of the re-simulation; every other human is a seat).
//! * [`Resim::sample`] -- what ring entry `tick / 2` holds for a race index: the exchange's own receiver ([`nfs_net::exchange::decode`]), twice
//!   per frame, entry 0 the preloaded zero entry (0x415E00, 0x415E80).
//!
//! **TEST GLUE / server tooling, not the original's**: the original has no such thing; the file format is this repository's.

use crate::replay::{CarRecord, NetSeat, NetSetup, NetTraffic, ReplaySetup};
use nfs_formats::fedata::CarCatalogEntry;
use nfs_net::exchange::{self, FRAME};
use nfs_net::launchcar::{CarSlot, id as cid};
use nfs_net::options::{self, Received, id as oid};
use std::collections::BTreeMap;

/// The record of one input-exchange race as the server hands it over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    /// The room's list at the `0xC5` go, in list order: `(player id, in the race)`. The race indices are the in-race players' places
    /// among them (0x4B4EBC); the room's player count is its length (0x48F998 reads `net+0x260`'s size for the per-slot records).
    pub roster: Vec<(u8, bool)>,
    /// Every `0xBC` message of the race's record in arrival order, with whether it came after the race's first `0xB3`
    /// (`net+0xE5 != 0`, which the receiver tests before it files a per-slot record as a car's or a traffic car's).
    pub options: Vec<(bool, Vec<u8>)>,
    /// The frames the room emitted, in order (frame 0 first).
    pub frames: Vec<[u8; FRAME]>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()).collect()
}

impl Package {
    pub const FORMAT: &'static str = "olresim1";

    pub fn to_json(&self) -> String {
        let roster: Vec<serde_json::Value> = self.roster.iter().map(|(p, r)| serde_json::json!({ "pid": p, "in_race": r })).collect();
        let options: Vec<serde_json::Value> = self.options.iter().map(|(l, b)| serde_json::json!({ "launch": l, "hex": hex(b) })).collect();
        let frames: String = self.frames.iter().map(|f| hex(f)).collect();
        serde_json::json!({ "format": Self::FORMAT, "roster": roster, "options": options, "frames": frames }).to_string()
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("not JSON: {e}"))?;
        if v["format"] != Self::FORMAT {
            return Err(format!("not a {} package", Self::FORMAT));
        }
        let roster = v["roster"]
            .as_array()
            .ok_or("no roster")?
            .iter()
            .map(|e| Some((u8::try_from(e["pid"].as_u64()?).ok()?, e["in_race"].as_bool()?)))
            .collect::<Option<Vec<_>>>()
            .ok_or("bad roster")?;
        let options = v["options"]
            .as_array()
            .ok_or("no options")?
            .iter()
            .map(|e| Some((e["launch"].as_bool()?, unhex(e["hex"].as_str()?)?)))
            .collect::<Option<Vec<_>>>()
            .ok_or("bad options")?;
        let raw = unhex(v["frames"].as_str().ok_or("no frames")?).ok_or("bad frames")?;
        if !raw.len().is_multiple_of(FRAME) {
            return Err("frames are not whole".into());
        }
        let frames = raw.chunks_exact(FRAME).map(|c| <[u8; FRAME]>::try_from(c).expect("chunk")).collect();
        Ok(Self { roster, options, frames })
    }
}

/// The inputs of a re-simulation: the replay setup the application starts the race from and the controller samples of every human.
#[derive(Debug, Clone)]
pub struct Resim {
    pub setup: ReplaySetup,
    /// the race index of the car the re-simulation treats as its local one
    pub local_index: u32,
    /// every human car's race index, ascending
    pub humans: Vec<u32>,
    /// per race index: ring entry `e` (0 = the preloaded zero entry, `2 f + 1` / `2 f + 2` = frame `f`'s first / second)
    lanes: BTreeMap<u32, Vec<exchange::Sample>>,
    /// what was assumed or approximated, for the report
    pub notes: Vec<String>,
}

impl Resim {
    /// The game ticks the frames allow: the receiver has `2 * frames` entries, an even tick `t` latches entry `t / 2`, and the ticks
    /// run while `ticks_done <= 2 * received + 1` (`Client::allowed_ticks`, 0x415455...0x415495): `4 * frames + 2` in all.
    pub fn ticks(&self) -> u64 {
        4 * self.lanes.values().next().map_or(0, |l| (l.len() as u64).saturating_sub(1) / 2) + 2
    }

    /// The sample the exchange latches for race index `index` at the even tick `tick` (`None`: past the frames received).
    pub fn sample(&self, index: u32, tick: u64) -> Option<exchange::Sample> {
        self.lanes.get(&index)?.get((tick / 2) as usize).copied()
    }
}

fn text(rec: &CarSlot, id: u32) -> Option<String> {
    rec.text(id).map(|b| b.iter().map(|&c| nfs_formats::locale::cp1252(c)).collect())
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// The catalogue record a car record names, as `race_stages` finds a seat's (`.FILENAME`, then `.SPECNAME`, then `.BODYSTYLE`).
fn catalogue_entry(catalog: &[CarCatalogEntry], rec: &CarSlot) -> Option<CarCatalogEntry> {
    let file = text(rec, cid::FILENAME).filter(|f| !f.is_empty())?;
    let (spec, body) = (text(rec, cid::SPECNAME).unwrap_or_default(), rec.num(cid::BODYSTYLE).map(|v| v as i16));
    let same_file = |c: &&CarCatalogEntry| c.model.eq_ignore_ascii_case(&file);
    let mut e = catalog
        .iter()
        .filter(same_file)
        .find(|c| c.sim.eq_ignore_ascii_case(&spec) && Some(c.variant) == body)
        .or_else(|| catalog.iter().filter(same_file).find(|c| c.sim.eq_ignore_ascii_case(&spec)))
        .or_else(|| catalog.iter().find(same_file))
        .cloned()?;
    if !spec.is_empty() {
        e.sim = spec;
    }
    if let Some(a) = text(rec, cid::AISPECNAME).filter(|a| !a.is_empty()) {
        e.sim_family = a;
    }
    Some(e)
}

/// The launch stream and the frames to the application's inputs. `Err` is "not run" with the reason; the message says what is missing.
pub fn resolve(pkg: &Package, catalog: &[CarCatalogEntry]) -> Result<Resim, String> {
    if pkg.frames.is_empty() {
        return Err("no frames were recorded".into());
    }
    let mut notes = Vec::new();
    // the receiver's tables (`netplay/session.rs`, `Heard::Options`): a per-slot record after the first 0xB3 is a traffic car's or a player's
    let room_len = pkg.roster.len() as u32;
    let mut got = Received::default();
    for (launch, m) in &pkg.options {
        for r in options::decode(m) {
            if *launch && got.take_traffic(&r, room_len) {
                continue;
            }
            if *launch && got.take_car(&r, room_len) {
                continue;
            }
            // (the player-slot records of the lobby, `slots.apply`, are not the race's)
            got.take(&r);
        }
    }
    // 0x4B4EBC / 0x4B4ED2: the race indices, the players in the race in list order
    let index: Vec<(u8, u32)> = pkg.roster.iter().filter(|(_, r)| *r).enumerate().map(|(i, (p, _))| (*p, i as u32)).collect();
    if index.is_empty() {
        return Err("nobody was in the race".into());
    }
    if got.positional.unwrap_or(1) != 0 {
        return Err("the launch stream says positional (COMMSPOSITIONAL != 0)".into());
    }
    if got.derby.unwrap_or(0) != 0 {
        return Err("a Smash Up (DERBY) race: its lap rules are not re-simulated".into());
    }
    if got.rally.unwrap_or(0) != 0 || got.heavy.unwrap_or(0) != 0 {
        return Err("the host's RALLY / HEAVY cheat is on: the launch flags are not carried by the replay setup".into());
    }
    let track = got.track.clone().filter(|t| !t.is_empty()).ok_or("the launch stream names no track")?;
    let seed = got.number(oid::RAND_SEED).ok_or("the launch stream carries no RAND_SEED")?;
    let mut cars: BTreeMap<u32, (CarSlot, CarCatalogEntry)> = BTreeMap::new();
    for &(_, i) in &index {
        let rec = got.cars.get(&i).ok_or_else(|| format!("no car record for race index {i}"))?;
        let e = catalogue_entry(catalog, rec).ok_or_else(|| format!("race index {i}: the car record names no car of this install"))?;
        cars.insert(i, (rec.clone(), e));
    }
    // the car of this machine: the lowest race index (a TINYCAR is a cheat bit of the machine that runs the car, `race_stages`: not carried
    // by the replay setup for the local car)
    let local_index = cars.iter().find(|(_, (r, _))| r.num(cid::TINYCAR).is_none_or(|v| v == 0)).map(|(i, _)| *i).ok_or("every car is a TINYCAR")?;
    if cars.values().any(|(r, _)| r.num(cid::TINYCAR).is_some_and(|v| v != 0)) {
        notes.push("a TINYCAR seat is built from its own record's flag".into());
    }
    let (lrec, lentry) = &cars[&local_index];
    let grid = |i: u32| got.grid.get(&i).map(|g| g.wrapping_sub(1));
    let seats: Vec<NetSeat> = index
        .iter()
        .filter(|(_, i)| *i != local_index)
        .map(|&(_, i)| NetSeat {
            race_index: i,
            name: String::new(),
            grid: grid(i),
            words: cars[&i].0.words.iter().map(|(k, w)| (*k, w.clone())).collect(),
        })
        .collect();
    let traffic: Vec<NetTraffic> = got.traffic.values().map(|t| NetTraffic { model: t.model.clone(), name: t.name.clone(), colour: t.colour }).collect();
    let camera0 = got.cars.get(&0).and_then(|c| c.array(cid::CAMERA));
    let fe_txt: Vec<(String, u32)> = [("weather", oid::WEATHER), ("time", oid::TIME), ("fog", oid::FOG)].into_iter().filter_map(|(k, id)| got.number(id).map(|v| (k.to_string(), v))).collect();
    let driver = text(lrec, cid::DRIVERNAME).unwrap_or_default();
    let paint = lrec.num(cid::COLOUR).map_or([0xD4, 0x10, 0x10], rgb);
    let parts: Vec<i16> = lrec.parts().into_iter().map(|p| p as i16).collect();
    let player = CarRecord {
        car_id: lentry.id,
        paint,
        name: driver.clone(),
        decal: -1,
        // the seats' rule: the listed parts (the stock list when none is listed), the record's sliders (the neutral ones when absent)
        parts: Some(if parts.is_empty() { lentry.stock_parts.clone() } else { parts }),
        setup: lrec.setup(),
        ..Default::default()
    };
    let automatic = lrec.num(cid::TRANSMISSION).is_none_or(|v| v == 1);
    let setup = ReplaySetup {
        track,
        car: lentry.model.clone(),
        sim: lentry.sim.clone(),
        car_id: lentry.id,
        file_car: None,
        paint,
        laps: got.laps.unwrap_or(1).max(1),
        opponents: seats.len() as u32,
        traffic: 0,
        cops: false,
        damage: got.damage.map_or(true, |v| v != 0),
        backwards: got.reverse.is_some_and(|v| v != 0),
        mirror: got.mirror.is_some_and(|v| v != 0),
        seed,
        skill: got.skill.unwrap_or(1).min(2) as u8,
        driver_name: driver,
        net: Some(NetSetup { local_index, grid: grid(local_index), seats, traffic, camera0: camera0.clone(), own_words: lrec.words.iter().map(|(k, w)| (*k, w.clone())).collect(), fe_txt }),
        race_type: Some(0),
        fegame_type: Some(3),
        camera: Some(camera0.unwrap_or_else(|| vec![0, 1, 2, 3])),
        player: Some(player),
        opponent_cars: None,
        scene: Some(0),
        derby: Some(false),
        nocollision: Some(false),
        transmission: Some(automatic),
        opponent_transmission: Some(true),
        mission: None,
    };
    // the lanes: the exchange's receiver (`Client::receive`) pushes, per frame, two entries per car -- the block of the car's player
    // id, decoded with and then without the command bits
    let mut lanes: BTreeMap<u32, Vec<exchange::Sample>> = BTreeMap::new();
    for &(pid, i) in &index {
        let k = usize::from(pid);
        if k >= exchange::SEATS {
            return Err(format!("player id {pid} is outside the exchange's eight slots"));
        }
        let mut lane = Vec::with_capacity(1 + 2 * pkg.frames.len());
        lane.push([0; 4]);
        for f in &pkg.frames {
            let b = [f[3 * k], f[3 * k + 1], f[3 * k + 2]];
            lane.push(exchange::decode(b, false));
            lane.push(exchange::decode(b, true));
        }
        lanes.insert(i, lane);
    }
    Ok(Resim { setup, local_index, humans: index.iter().map(|e| e.1).collect(), lanes, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_round_trips_through_json() {
        let mut f = [0u8; FRAME];
        f[3] = 9;
        let p = Package { roster: vec![(0, true), (3, false)], options: vec![(false, vec![0xBC, 0, 0, 0, 1, 2, 3, 4]), (true, vec![1, 2])], frames: vec![f, [7; FRAME]] };
        assert_eq!(Package::from_json(&p.to_json()), Ok(p));
        assert!(Package::from_json("{}").is_err());
    }

    /// An `0xBC` message of the given records.
    fn message(recs: &[Vec<u32>]) -> Vec<u8> {
        options::message(&recs.concat())
    }

    #[test]
    fn a_record_that_cannot_be_replayed_says_why() {
        let frame = [0u8; FRAME];
        let base = Package { roster: vec![(0, true), (1, true)], options: Vec::new(), frames: vec![frame] };
        assert_eq!(resolve(&Package { frames: Vec::new(), ..base.clone() }, &[]).unwrap_err(), "no frames were recorded");
        assert!(resolve(&Package { roster: vec![(0, false)], ..base.clone() }, &[]).unwrap_err().contains("nobody was in the race"));
        // a positional launch is not an input-exchange race
        let positional = Package { options: vec![(false, message(&[vec![oid::COMMSPOSITIONAL, 1]]))], ..base.clone() };
        assert!(resolve(&positional, &[]).unwrap_err().contains("positional"));
        // the stream names no track
        let none = Package { options: vec![(false, message(&[vec![oid::COMMSPOSITIONAL, 0]]))], ..base.clone() };
        assert!(resolve(&none, &[]).unwrap_err().contains("no track"));
        // a track but no seed
        let track = options::string_record(oid::TRACK_NAME, "alps");
        let unseeded = Package { options: vec![(false, message(&[vec![oid::COMMSPOSITIONAL, 0], track.clone()]))], ..base.clone() };
        assert!(resolve(&unseeded, &[]).unwrap_err().contains("RAND_SEED"));
        // a seed and a track but nobody's car record
        let seeded = Package { options: vec![(false, message(&[vec![oid::COMMSPOSITIONAL, 0], track, vec![oid::RAND_SEED, 8165]]))], ..base };
        assert!(resolve(&seeded, &[]).unwrap_err().contains("no car record for race index 0"));
    }

    #[test]
    fn a_frame_becomes_two_ring_entries_per_car_after_the_zero_entry() {
        // player 1's block: b0 = 0x0D (buttons 5, command 1 bit 3 set), steer 0x20, ...
        let mut f = [0u8; FRAME];
        f[3..6].copy_from_slice(&[0x0D, 0x20, 0x48]);
        let b = [0x0D, 0x20, 0x48];
        let mut lane = vec![[0u8; 4]];
        lane.push(exchange::decode(b, false));
        lane.push(exchange::decode(b, true));
        let r = Resim { setup: ReplaySetup::default(), local_index: 0, humans: vec![0], lanes: BTreeMap::from([(0, lane.clone())]), notes: vec![] };
        // tick 0 latches the zero entry, tick 2 entry 1 (frame 0, with the command bits), tick 4 entry 2 (without)
        assert_eq!(r.sample(0, 0), Some([0; 4]));
        assert_eq!(r.sample(0, 2), Some(lane[1]));
        assert_eq!(r.sample(0, 4), Some(lane[2]));
        assert_eq!(r.sample(0, 6), None);
        assert_eq!(lane[1][0] & !7, 0x08, "command 1 rides the first entry only");
        assert_eq!(lane[2][0] & !7, 0);
        assert_eq!(r.ticks(), 6);
    }
}
