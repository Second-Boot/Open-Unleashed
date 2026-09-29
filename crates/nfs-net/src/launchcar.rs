//! The player's own per-slot car records at the launch — `RE/notes/network.md` §11.12.
//!
//! Every machine's **0x4B4DA0** (`FEGAME_TYPE` 3) walks its launch list and, for the per-slot id
//! bands (`0x13A…0x141`, `0x147…0x14D`, `≥ 0x21E`, `0x247` skipped), sends the records whose slot is
//! **its own player id** (`[net+0xDC]`) with the slot rewritten to its **race index** — `player+0x4C`,
//! its place in the room's list among the players `net+0xE5` marks (0x4B4F25…0x4B4F66, the
//! rewrite `[record+4] = own index` at 0x4B4F59…0x4B4F65). What is in the list for that slot is
//! what the launch chain wrote for the player:
//!
//! 1. **the accessor dump 0x4B7F10** (called at 0x4ED653, before the per-mode chain): every accessor
//!    that has a non-zero id and a per-slot one goes out as `(id, own id, value)` (0x4BC3D0 →
//!    0x4EDC00 at 0x4BC436); the registrations with such an id are `Car.ABS` 0x23A (0x4F25F8),
//!    `Car.Traction` 0x23B (0x4F2635) and `Car.Roof` 0x24B (0x4F2672), each through its
//!    predicate (`[accessor+0x20]`, 0x4BC3F1…0x4BC40F); walked sorted by name, byte-wise;
//! 2. **0x4EEDA0 with kind 1**, called from 0x4EB710 (0x4EBA4D) at the start of 0x4ECF00 (0x4ECF0C),
//!    which appends the car's own record ([`OwnCar::records`]).
//!
//! 0x4B6560 keeps the **last** record of an `(id, slot)`, so `.ABS` 0x23A and `.TRACTION` 0x23B are
//! overwritten by 0x4EEDA0's own `0` / `param_6` (0x4EEFCD, 0x4EEFDB…0x4EEFF1) and only the dump's
//! `.CABRIODOWN` 0x24B survives (0x4EEF0C: 0x4EEDA0 writes it only for a kind other than 1) —
//! `physics.md` §5.4.

use crate::options::record_words;
use crate::slots::string_record;

/// The ids of the per-slot car record (`replay.md` §16.2's carinfo table).
pub mod id {
    pub const DRIVERNAME: u32 = 0x13B;
    pub const LICENSE: u32 = 0x13C;
    pub const FILENAME: u32 = 0x13D;
    pub const SPECNAME: u32 = 0x13E;
    pub const AISPECNAME: u32 = 0x13F;
    pub const SOUNDNAME: u32 = 0x140;
    pub const CAR_NAME: u32 = 0x141;
    pub const CAMERA: u32 = 0x148;
    pub const COLORCONFIG: u32 = 0x149;
    pub const PARTINDEX: u32 = 0x14B;
    pub const PARTDAMAGE: u32 = 0x14C;
    pub const DAMAGEZONES: u32 = 0x14D;
    pub const TYPE: u32 = 0x21F;
    pub const CLASS: u32 = 0x220;
    pub const TRANSMISSION: u32 = 0x221;
    pub const COLOUR: u32 = 0x225;
    pub const FINALRATIO: u32 = 0x227;
    pub const FIRSTRATIO: u32 = 0x228;
    pub const SECONDRATIO: u32 = 0x229;
    pub const THIRDRATIO: u32 = 0x22A;
    pub const FOURTHRATIO: u32 = 0x22B;
    pub const FIFTHRATIO: u32 = 0x22C;
    pub const SIXTHRATIO: u32 = 0x22D;
    pub const REVERSERATIO: u32 = 0x22E;
    pub const FRONTDOWNFORCE: u32 = 0x22F;
    pub const REARDOWNFORCE: u32 = 0x230;
    pub const BRAKEBIAS: u32 = 0x231;
    pub const RIDEHEIGHT: u32 = 0x232;
    pub const SUSPENSIONTRAVEL: u32 = 0x234;
    pub const STEERINGTOE: u32 = 0x235;
    pub const SHOCKRATE: u32 = 0x236;
    pub const FRONTTIREPRESSURE: u32 = 0x237;
    pub const REARTIREPRESSURE: u32 = 0x238;
    pub const ABS: u32 = 0x23A;
    pub const TRACTION: u32 = 0x23B;
    pub const BESTLINE: u32 = 0x23C;
    pub const DRIVER: u32 = 0x23D;
    pub const SPEECHCOLOUR: u32 = 0x248;
    pub const COUNTRY: u32 = 0x249;
    pub const BODYSTYLE: u32 = 0x24A;
    pub const CABRIODOWN: u32 = 0x24B;
    pub const RACENUMBER: u32 = 0x24D;
    pub const STRIPEPACKAGE: u32 = 0x24E;
    pub const TINYCAR: u32 = 0x24F;
    pub const GLASSCAR: u32 = 0x250;
    pub const TACH0RPM: u32 = 0x251;
    pub const TACH1000RPM: u32 = 0x252;
    pub const SPEEDO0KM: u32 = 0x253;
    pub const SPEEDO100KM: u32 = 0x254;
}

/// The seventeen car-setup sliders' ids in `FECar +0x6D0 …` order (0x4EF188…0x4EF32F): the order
/// `CarSpecs::apply_setup` takes its array in.
pub const SETUP_IDS: [u32; 17] = [
    id::FRONTDOWNFORCE,
    id::REARDOWNFORCE,
    id::BRAKEBIAS,
    id::RIDEHEIGHT,
    id::SUSPENSIONTRAVEL,
    id::STEERINGTOE,
    id::SHOCKRATE,
    id::FRONTTIREPRESSURE,
    id::REARTIREPRESSURE,
    id::REVERSERATIO,
    id::FIRSTRATIO,
    id::SECONDRATIO,
    id::THIRDRATIO,
    id::FOURTHRATIO,
    id::FIFTHRATIO,
    id::SIXTHRATIO,
    id::FINALRATIO,
];

/// Every id of the per-slot car record a machine keeps for a *player* at its race index (what
/// [`CarSlot::take`] accepts): 0x4EEDA0's ids, the dump's, and the two the lobby also sends.
pub fn is_car_id(i: u32) -> bool {
    use id::*;
    matches!(
        i,
        DRIVERNAME | LICENSE | FILENAME | SPECNAME | AISPECNAME | SOUNDNAME | CAR_NAME | CAMERA | COLORCONFIG | PARTINDEX | PARTDAMAGE
            | DAMAGEZONES | TYPE | CLASS | TRANSMISSION | COLOUR | ABS | TRACTION | BESTLINE | DRIVER | SPEECHCOLOUR | COUNTRY | BODYSTYLE
            | CABRIODOWN | RACENUMBER | STRIPEPACKAGE | TINYCAR | GLASSCAR | TACH0RPM | TACH1000RPM | SPEEDO0KM | SPEEDO100KM
    ) || SETUP_IDS.contains(&i)
}

/// `(id, slot, count, value × count)` — 0x4EDC30 / 0x4EDC80, the per-slot array band `0x147…0x14D`.
pub fn slot_array_record(id: u32, slot: u32, values: &[u32]) -> Vec<u32> {
    let mut r = vec![id, slot, values.len() as u32];
    r.extend_from_slice(values);
    r
}

/// What 0x4EEDA0 (kind 1) reads for the player, and the accessors the dump reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnCar {
    /// `.DRIVER` 0x23D — kind 1 sends 0x5024E0's value, the profile's picture (0x4EEDBD)
    pub driver: u32,
    /// the argument 0x4EEDA0 sends as `.DRIVERNAME` 0x13B (0x4EEDF1) **and** `.LICENSE` 0x13C (0x4EEE7E):
    /// the driver's name (0x5012A0 with a session up, 0x4EB9DC…0x4EBA1F)
    pub driver_name: Vec<u8>,
    /// `.FEMULTIPLAYERCARNAME` 0x141 (0x4EEE59 / 0x4EEE6E): `car+6`, `"%s *"` of it for a garage car
    pub car_name: Vec<u8>,
    /// `.FILENAME` 0x13D — `car+0x83` (0x4EEE8E)
    pub filename: Vec<u8>,
    /// `.SPECNAME` 0x13E — `car+0xB5`, `"993coupe36"` under the `evenstvn` code (0x4EEEB5)
    pub specname: Vec<u8>,
    /// `.AISPECNAME` 0x13F — `car+0x119`, `"993"` under the code (0x4EEEC6)
    pub aispecname: Vec<u8>,
    /// `.SOUNDNAME` 0x140 — `car+0x17D` (0x4EEEF6)
    pub soundname: Vec<u8>,
    /// `.BODYSTYLE` 0x24A — `movsx word [car+0x1C4]` (0x4EEF22), the record's variant
    pub bodystyle: i16,
    /// `.TRANSMISSION` 0x221 — `Car.Transmission`, 0 when the seventh argument is set (0x4EEF83…)
    pub transmission: u32,
    /// `.COLOUR` 0x225 — `car+0x674`, the exterior colour dword
    pub colour: u32,
    /// `.TRACTION` 0x23B — the sixth argument's byte (0x4EEFDB…0x4EEFF1), set for Factory Driver mission 0xE
    pub traction: bool,
    /// `.CAMERA` 0x148 — `Camera.1 … Camera.4` (0x4EEFF5…0x4EF0F5)
    pub camera: [u32; 4],
    /// `.COLORCONFIG` 0x149 — `car+0x674, +0x67C, +0x680, +0x678`: exterior, stripe 1, stripe 2, interior (0x4EF10x…0x4EF13C)
    pub colorconfig: [u32; 4],
    /// `.STRIPEPACKAGE` 0x24E — `car+0x684`
    pub package: u32,
    /// `.RACENUMBER` 0x24D — `car+0x688` (−1: no number)
    pub racenumber: i32,
    /// the seventeen sliders as `0x4A5400` makes them (`trunc(v · 65536)`), in [`SETUP_IDS`] order
    pub setup: [i32; 17],
    /// `.TACH0RPM … .SPEEDO100KM` 0x251…0x254 — `car+0x2F4 …` raw dwords (floats)
    pub dash: [u32; 4],
    /// `.PARTINDEX` 0x14B — the ids of the installed parts (at most 20, 0x4EF3F3)
    pub parts: Vec<u32>,
    /// `.TINYCAR` 0x24F / `.GLASSCAR` 0x250 — the cheat bits 1 and 2 (0x4EF467…0x4EF4A0), sent only when set
    pub tiny: bool,
    pub glass: bool,
    /// `Car.ABS`, `Car.Traction`, `Car.Roof` as the dump reads them — through the accessors' own
    /// predicates (a car without the capability reads 0; `physics.md` §5.4, `controls.md` §8876)
    pub abs_accessor: u32,
    pub traction_accessor: u32,
    pub roof_accessor: u32,
}

impl OwnCar {
    /// **0x4B7F10** for the per-slot accessors: `(id, slot, value)` for `Car.ABS`, `Car.Roof`,
    /// `Car.Traction` — the map at 0x65B3A8 is sorted byte-wise by name (0x4242C0), which puts
    /// `Car.ABS` < `Car.Roof` < `Car.Traction`. `slot` is the player's own id here (0x4BC420).
    pub fn dump_records(&self, slot: u32) -> Vec<Vec<u32>> {
        vec![
            vec![id::ABS, slot, self.abs_accessor],
            vec![id::CABRIODOWN, slot, self.roof_accessor],
            vec![id::TRACTION, slot, self.traction_accessor],
        ]
    }

    /// **0x4EEDA0** with kind 1 and `Game.GameType` 3, in the function's own order. `.CABRIODOWN`
    /// is not among them (0x4EEF0C: written only for a kind other than 1); the arrays
    /// `.PARTDAMAGE` and `.DAMAGEZONES` are zeros for a game type of 0, 1, 2, 3 or 9 (0x4EF3F3…0x4EF45E).
    pub fn records(&self, slot: u32) -> Vec<Vec<u32>> {
        let n = |i: u32, v: u32| vec![i, slot, v];
        let mut out = vec![
            n(id::DRIVER, self.driver),
            string_record(id::DRIVERNAME, slot, &self.driver_name),
            string_record(id::CAR_NAME, slot, &self.car_name),
            string_record(id::LICENSE, slot, &self.driver_name),
            string_record(id::FILENAME, slot, &self.filename),
            string_record(id::SPECNAME, slot, &self.specname),
            string_record(id::AISPECNAME, slot, &self.aispecname),
            string_record(id::SOUNDNAME, slot, &self.soundname),
            n(id::BODYSTYLE, self.bodystyle as i32 as u32),
            n(id::TRANSMISSION, self.transmission),
            n(id::TYPE, 0xB),
            n(id::COLOUR, self.colour),
            n(id::ABS, 0),
            n(id::TRACTION, u32::from(self.traction)),
            n(id::SPEECHCOLOUR, 0xE),
            n(id::COUNTRY, 0),
            n(id::CLASS, 1),
            slot_array_record(id::CAMERA, slot, &self.camera),
            slot_array_record(id::COLORCONFIG, slot, &self.colorconfig),
            n(id::STRIPEPACKAGE, self.package),
            n(id::RACENUMBER, self.racenumber as u32),
            n(id::BESTLINE, 0),
        ];
        // 0x4EF188…0x4EF32F: the sliders, each through 0x4A5400
        out.extend(SETUP_IDS.iter().zip(self.setup).map(|(&i, v)| n(i, v as u32)));
        for (k, i) in [id::TACH0RPM, id::TACH1000RPM, id::SPEEDO0KM, id::SPEEDO100KM].into_iter().enumerate() {
            out.push(n(i, self.dash[k]));
        }
        let mut parts = [0u32; 20];
        for (d, s) in parts.iter_mut().zip(&self.parts) {
            *d = *s;
        }
        out.push(slot_array_record(id::PARTINDEX, slot, &parts));
        out.push(slot_array_record(id::PARTDAMAGE, slot, &[0; 20]));
        out.push(slot_array_record(id::DAMAGEZONES, slot, &[0; 10]));
        if self.tiny {
            out.push(n(id::TINYCAR, 1));
        }
        if self.glass {
            out.push(n(id::GLASSCAR, 1));
        }
        out
    }

    /// The whole list the launch chain leaves for the player's slot — the dump first, then the
    /// per-car routine — **de-duplicated** the way 0x4B6560 does it (the last record of an
    /// `(id, slot)` stays where it stands). Its `.ABS` and `.TRACTION` are 0x4EEDA0's.
    pub fn launch_list(&self, slot: u32) -> Vec<Vec<u32>> {
        let all: Vec<Vec<u32>> = self.dump_records(slot).into_iter().chain(self.records(slot)).collect();
        crate::options::dedupe_last(all)
    }
}

/// A player's record at a race index, as 0x4B4B80 has written it into the per-slot table: id →
/// the words after the slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarSlot {
    pub words: std::collections::BTreeMap<u32, Vec<u32>>,
}

impl CarSlot {
    /// Write one record (`rec[1]` is the slot, already matched by the caller). `false` for an id
    /// that is not a car field.
    pub fn take(&mut self, rec: &[u32]) -> bool {
        let Some(&i) = rec.first() else { return false };
        if !is_car_id(i) || rec.len() < 3 {
            return false;
        }
        self.words.insert(i, rec[2..].to_vec());
        true
    }

    pub fn num(&self, i: u32) -> Option<u32> {
        self.words.get(&i).and_then(|w| w.first().copied())
    }

    /// A string field: the bytes up to the NUL.
    pub fn text(&self, i: u32) -> Option<Vec<u8>> {
        self.words.get(&i).map(|w| w.iter().flat_map(|x| x.to_le_bytes()).take_while(|&b| b != 0).collect())
    }

    /// An array field: the values after the count.
    pub fn array(&self, i: u32) -> Option<Vec<u32>> {
        let w = self.words.get(&i)?;
        Some(w.iter().skip(1).copied().take(*w.first()? as usize).collect())
    }

    /// The seventeen sliders as floats — `carinfo`'s words are read back with `fild × 2⁻¹⁶`
    /// (`physics.md` §3a). `None` unless every one arrived.
    pub fn setup(&self) -> Option<[f32; 17]> {
        let mut s = [0f32; 17];
        for (d, i) in s.iter_mut().zip(SETUP_IDS) {
            *d = self.num(i)? as i32 as f32 / 65536.0;
        }
        Some(s)
    }

    /// The parts whose ids came (the zero padding dropped).
    pub fn parts(&self) -> Vec<u32> {
        self.array(id::PARTINDEX).unwrap_or_default().into_iter().filter(|&p| p != 0).collect()
    }

    /// Whether the record carries the car the owner races (`.FILENAME`).
    pub fn has_car(&self) -> bool {
        self.words.contains_key(&id::FILENAME)
    }
}

/// Cut a launch list into whole records (each checked by [`record_words`]) — a helper for tests.
pub fn well_formed(list: &[Vec<u32>]) -> bool {
    list.iter().all(|r| record_words(r) == Some(r.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{decode, message};

    fn car() -> OwnCar {
        let mut setup = [0i32; 17];
        for (k, s) in setup.iter_mut().enumerate() {
            *s = ((50 + k as i32) as f32 * 65536.0) as i32;
        }
        OwnCar {
            driver: 8,
            driver_name: b"HOST".to_vec(),
            car_name: b"'94 993 Coupe".to_vec(),
            filename: b"993".to_vec(),
            specname: b"993coupe".to_vec(),
            aispecname: b"993".to_vec(),
            soundname: b"sd2".to_vec(),
            bodystyle: 2,
            transmission: 1,
            colour: 0xFF11_2233,
            traction: false,
            camera: [0, 1, 2, 3],
            colorconfig: [0xFF11_2233, 1, 2, 3],
            package: 0,
            racenumber: -1,
            setup,
            dash: [1, 2, 3, 4],
            parts: vec![7, 9],
            tiny: false,
            glass: false,
            abs_accessor: 1,
            traction_accessor: 1,
            roof_accessor: 1,
        }
    }

    #[test]
    fn the_list_is_whole_records_that_survive_the_wire() {
        let list = car().launch_list(3);
        assert!(well_formed(&list));
        let back = decode(&message(&list.concat()));
        assert_eq!(back, list);
    }

    #[test]
    fn abs_and_traction_are_the_per_car_routines_and_the_roof_is_the_accessors() {
        // `Car.ABS` and `Car.Traction` on: 0x4EEDA0's later records (0, and the sixth argument) win
        let c = car();
        let mut t = CarSlot::default();
        for r in c.launch_list(1) {
            assert!(t.take(&r), "{r:?}");
        }
        assert_eq!(t.num(id::ABS), Some(0));
        assert_eq!(t.num(id::TRACTION), Some(0));
        assert_eq!(t.num(id::CABRIODOWN), Some(1), "the dump's `Car.Roof` stays: 0x4EEDA0 does not write it for kind 1");
        let mut m = c;
        m.traction = true;
        let mut t = CarSlot::default();
        for r in m.launch_list(1) {
            t.take(&r);
        }
        assert_eq!(t.num(id::TRACTION), Some(1));
    }

    #[test]
    fn a_slot_reads_back_what_the_owner_sent() {
        let c = car();
        let mut t = CarSlot::default();
        for r in c.launch_list(2) {
            t.take(&r);
        }
        assert_eq!(t.text(id::FILENAME).unwrap(), b"993".to_vec());
        assert_eq!(t.text(id::LICENSE).unwrap(), b"HOST".to_vec());
        assert_eq!(t.text(id::DRIVERNAME).unwrap(), b"HOST".to_vec());
        assert_eq!(t.num(id::COLOUR), Some(0xFF11_2233));
        assert_eq!(t.num(id::RACENUMBER), Some(u32::MAX));
        assert_eq!(t.array(id::COLORCONFIG).unwrap(), vec![0xFF11_2233, 1, 2, 3]);
        assert_eq!(t.parts(), vec![7, 9]);
        let s = t.setup().unwrap();
        assert_eq!(s[0], 50.0);
        assert_eq!(s[16], 66.0);
        assert!(t.has_car());
    }

    #[test]
    fn the_cheat_records_are_sent_only_when_set() {
        let mut c = car();
        assert!(!c.records(0).iter().any(|r| r[0] == id::TINYCAR || r[0] == id::GLASSCAR));
        c.tiny = true;
        assert!(c.records(0).iter().any(|r| r[0] == id::TINYCAR));
    }
}
