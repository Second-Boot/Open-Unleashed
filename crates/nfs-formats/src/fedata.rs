//! `FEData/Data/nfs5.car` — the car catalogue used by the frontend (109 × 1648-byte records).
//!
//! Status: string fields CONFIRMED by inspection (all 109 records parse, model/sim names exist on
//! disk, except a few sims such as `993coupeS436` which the game resolves to `default.sim`);
//! numeric tail is UNKNOWN except years/price (HYPOTHESIS from values 1950/1954/11000).

use crate::{fixed_str, u16_le, u32_le, FormatError};

pub const CAR_RECORD_SIZE: usize = 1648;

#[derive(Debug, Clone, PartialEq)]
pub struct CarCatalogEntry {
    /// +0x02 display name, e.g. `'50 356 1100 Cabriolet`.
    pub display_name: String,
    /// +0x34 family, e.g. `356`.
    pub family: String,
    /// +0x4D localisation key / `FEData/Locale/<key>.loc`, e.g. `1950356Cabrio11`.
    pub locale_key: String,
    /// +0x7F model name → `GameData/CarModel/<model>.crp|D.fsh|L.fsh|.tpg`, e.g. `356_1`.
    pub model: String,
    /// +0xB1 simulation variant → `GameData/Simulation/CarData/<sim>.sim`, e.g. `356cabrio11`.
    pub sim: String,
    /// +0xE3 — the car's **showcase pack**: `FEData/Showcase/<key>.fsh`, e.g. `356-14`. The
    /// showcase screens' `NFS5.ShowcasePicture` widgets take entry `number` of it, and each
    /// widget's `size` matches that entry's own dimensions on all eight. CONFIRMED; 86 of the 87
    /// racing records name a pack that is on disk.
    pub showcase: String,
    /// +0x115 the car's **family key**, e.g. `356`, `993T`, `boxster`, `GT2` — what the AI's
    /// per-track steering table is keyed on (0x4A2950, compared case-insensitively) and what the
    /// car-vs-car tables group by. CONFIRMED.
    pub sim_family: String,
    /// +0x147 the logo pack: `FEData/Art/<logo>.fsh` (`NFS5.CarLogo` builds the name with
    /// `"%s%s.fsh"` at 0x4C19CD). CONFIRMED.
    pub logo: String,
    /// +0x179 engine sound set → `GameData/Sounds/<set>.viv`, e.g. `sd1b`.
    pub sound_set: String,
    /// +0x1B2 body style, e.g. `Cabrio`.
    pub body: String,
    /// +0x1C2 engine displacement text, e.g. `1100`.
    pub displacement: String,
    /// +0x00 u16 record id (0x4C, 0x4D, … sequential).
    pub id: u16,
    /// +0x1F8 u16 (1950…) HYPOTHESIS: first model year.
    pub year: u16,
    /// +0x202 u16 (1954…) HYPOTHESIS: last model year.
    pub year_end: u16,
    /// +0x1F6 i16: the car's logo **pair** inside the `logo` pack. `NFS5.CarLogo` reads it at
    /// 0x4C1A63 and uses entries `2*logo_pair` and `2*logo_pair + 1`, drawn side by side
    /// (`RE/notes/layouts.md` §4). CONFIRMED.
    /// `+0x1C0` — the car's place among the variants of one model generation, 0…12, handed out by
    /// `FEGenericCarAccessor`'s slot 38 (0x4F31F6) and three more accessors. The generation is
    /// wider than [`Self::family`]: the 356 A, its Convertible and its Speedster share one run.
    /// `RE/notes/controls.md` §4.145. Nothing in this port reads it yet.
    pub variant: i16,
    /// `+0x1BF` u8 — the car is a **convertible**: `Car.Roof`'s predicate 0x4F1E40 reads it off the
    /// car `CARSETTINGS.CAR` names and forces the roof to 0 when it is clear. 1 on every Cabriolet,
    /// Speedster, Roadster, Convertible D and the 964 / 993 cabriolets, 0 on the coupés and Targas.
    pub cabrio: bool,
    /// `+0x3BC` i16 count, `+0x3BE` i16 ids: the record's stock parts, which the 3D car widget
    /// walks for a car that is not a garage record (0x4C036A…0x4C0405) to build its style overlay.
    pub stock_parts: Vec<i16>,
    pub logo_pair: i16,
    /// +0x1F4 u16 — what the car *is*, and which of the front end's lists it joins (the `switch` at
    /// 0x4F400E): **0 a racing car** (87 records, the player's, the opponents' and the dealer's),
    /// **1 traffic** (16: `eden1..5`, `EAS1..11`), **2 a cop** (6). Only kind 0 may be listed or
    /// raced. CONFIRMED.
    pub kind: u16,
    /// +0x204 u16 — availability: `2` is a bonus car (959, 928 GTS, GT2, GT3) and `3` a Factory
    /// Driver car (`FBoxster`, `F901`, `F996`); the dealer skips both (0x4F4240).
    pub dealer_flag_204: u16,
    /// +0x1AC u32 — the car's **class**, as an `FEStrings.csv` id: 501 CLASS 3, 502 CLASS 2,
    /// 503 CLASS 1, 504 RACE CLASS. `FEOpponentCarAccessor` matches on it (`modes.md` §1.1a).
    pub class_string_id: u32,
    /// +0x1B0 u32 — the car's **era**, as an `FEStrings.csv` id: 521 CLASSIC, 522 GOLDEN,
    /// 523 MODERN. Traffic and cop records carry 0 in both.
    pub era_string_id: u32,
    /// +0x528 u16 count, +0x530 16-byte entries — the interior colours, in the same
    /// `{u32 packed, …}` shape as a paint set's colours (`OwnedCar.IntColour`, 0x4F0CE0/0x4F0D20).
    pub interior_colours: Vec<[u8; 3]>,
    /// +0x214 … +0x2DC: value curve in per cent for the years 1950..=2000 (`0x4F8E50`); the dealer
    /// indexes the same table by age from +0x20C (`age_curve`).
    pub value_curve: [f32; 51],
    /// +0x20C, +0x210: the two entries before the value curve (dealer ages 0 and 1).
    pub age_curve_head: [f32; 2],
    /// +0x2F0 … +0x2FC f32 — the dash needles' calibration, in turns: rest angle and per-1000-rpm
    /// of the rev counter, rest angle and scale of the speedometer. The per-car routine 0x4EEDA0
    /// sends them as options 0x251…0x254 `.TACH0RPM`, `.TACH1000RPM`, `.SPEEDO0KM`,
    /// `.SPEEDO100KM` (0x4EF331…0x4EF37B, off `this+0x2F4`: the FE car object keeps a 4-byte
    /// header before the record) into `carinfo+0x524…+0x530`, which slot 9 turns into channels
    /// 11 and 12 (`car-model.md` "Slot 9"). The 993: −0.37, 0.09, −0.34, 0.08.
    pub dash: [f32; 4],
    /// +0x300 / +0x30C / +0x318: km/h, mph and tach dial records (`RE/notes/hud.md` §3.1).
    pub dial_kmh: DialSpec,
    pub dial_mph: DialSpec,
    pub dial_tach: DialSpec,
    /// +0x324 … +0x32C — five i16 ratings on a **0..100** scale, in the order the car-compare
    /// screen's jump table at 0x4CAA48 dispatches them: **acceleration, top speed, braking,
    /// handling, overall**. The accessor `CarCompare.Data(<n>CarCompareCar<m>)` (0x4CA790) takes
    /// the leading digit of its source name as `n` and returns the matching field with `movsx`,
    /// and `carcompare.lay` labels the five rows with `FEStrings` 10702, 10701, 10700, 10706 and
    /// 10705 — ACCELERATION, TOP SPEED, BRAKING, HANDLING, OVERALL, top to bottom. The bar widget
    /// reading them has stops `c0`, `c50` and `c100`, which is what fixes the scale. CONFIRMED.
    pub compare: [i16; 5],
    /// +0x208 u32 price in dollars (11000 for the '50 356, 750000 for the GT1 — matches the tournament
    /// prize scale; the u16 reading overflowed for cars above $65535). HIGH.
    pub price: u32,
    /// +0x3E6 u16 count, +0x3E8 16-byte sets `{u32 package id, u32 ARGB c1, c2, c3}` (bytes `ff r g b`) —
    /// the paint colour sets compiled from the car's `.clr` (RE/notes/car-model.md §5, MEDIUM);
    /// `c1` is the body paint used for the tint slots.
    pub paint_sets: Vec<PaintSet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaintSet {
    /// The set's first dword. Not the paint shop's stripe `PackageNum` (`car+0x684`, the row of
    /// `packages.ini` 0x443DB0 tints by — `car-model.md` §5.0a): over `nfs5.car` it is only ever 0
    /// or 205 (`examples/paintpkgs.rs`). What it is was not read.
    pub package: u32,
    pub colours: [[u8; 3]; 3],
    /// The same three colours in the dword form the front-end's colour accessors hand out and the
    /// FE car object stores at `car+0x674` / `+0x67C` / `+0x680`: `b0<<24 | b1<<16 | b2<<8 | b3`
    /// of the stored bytes (0x4F0C60, CONFIRMED). Kept so a save game can be written byte-exactly.
    pub packed: [u32; 3],
}

/// One HUD dial definition (12 bytes of i16, CONFIRMED): face style (`speedK`/`speedM`/`tach.fsh`
/// entry index), total units (km/h, mph, or rpm/10), needle style (`needle*.fsh` entry), start
/// angle in degrees, degrees per unit × 1000, needle pivot in pixels above the image bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialSpec {
    pub style: i16,
    pub total_units: i16,
    pub needle_style: i16,
    pub start_deg: i16,
    pub deg_per_unit_x1000: i16,
    pub needle_pivot: i16,
}

impl DialSpec {
    pub fn parse(r: &[u8], off: usize) -> Result<Self, FormatError> {
        let v = |i: usize| u16_le(r, off + i * 2, "nfs5.car dial").map(|x| x as i16);
        Ok(Self { style: v(0)?, total_units: v(1)?, needle_style: v(2)?, start_deg: v(3)?, deg_per_unit_x1000: v(4)?, needle_pivot: v(5)? })
    }
    /// The angle `HDial`'s constructor stored for a value, in degrees (`RE/notes/hud.md` §3.3).
    ///
    /// The precompute loop 0x4214A0 walks the table **backwards** — it starts at the last element and
    /// decrements the write pointer by one vertex triple each turn — while the angle accumulates
    /// forward from `start_deg` in steps of `deg_per_unit`. Unit `v` therefore holds
    /// `start + (total − 1 − v)·step`, not `start + v·step`.
    pub fn angle(&self, value: i32) -> f32 {
        let last = (self.total_units as i32 - 1).max(0);
        let v = value.clamp(0, last);
        self.start_deg as f32 + (last - v) as f32 * self.deg_per_unit_x1000 as f32 / 1000.0
    }

    /// Where the needle points, in degrees clockwise from twelve o'clock.
    ///
    /// The vertices are screen-space (`XYZRHW`, y down) and `setValue` 0x421650 emits
    /// `(cx + A, cy − B)` with `A = L·cos φ`, `B = L·sin φ`, so the tip sits at `90° − φ` clockwise
    /// from the top. Together with the reversed table that makes the needle sweep **clockwise** by
    /// `deg_per_unit` per unit, which is what every shipped face draws — including the Boxster and
    /// 996 dials, whose positive `start_deg` used to look like an exception.
    pub fn screen_cw_deg(&self, value: i32) -> f32 {
        90.0 - self.angle(value)
    }
}

impl CarCatalogEntry {
    /// Dealer price in `year` (`0x4EE220`): the catalogue price while the car is new, then the
    /// age-indexed curve (`+0x20C + 4·age`) in per cent.
    pub fn dealer_price(&self, year: u16) -> u32 {
        if year <= self.year {
            return self.price;
        }
        let age = (year - self.year) as usize;
        let pct = match age {
            1 => self.age_curve_head[1],
            a => self.value_curve.get(a - 2).copied().unwrap_or(100.0),
        };
        // 0x4EE24C…0x4EE273: `_ftol(price · curve)` — the float product truncated — and then a C
        // integer `/ 100`, so both steps truncate; the port used to round the quotient
        ((self.price as f64 * pct as f64) as i64 / 100) as u32
    }

    /// Value in per cent of the catalogue price in `year` (`0x4F8E50`, index `year − 1950`).
    pub fn value_pct(&self, year: u16) -> f32 {
        self.value_curve.get((year.max(1950) - 1950) as usize).copied().unwrap_or(100.0)
    }
}

pub fn parse_car_catalog(data: &[u8]) -> Result<Vec<CarCatalogEntry>, FormatError> {
    if !data.len().is_multiple_of(CAR_RECORD_SIZE) {
        return Err(FormatError::SizeMismatch { what: "nfs5.car", declared: CAR_RECORD_SIZE, actual: data.len() % CAR_RECORD_SIZE });
    }
    let mut out = Vec::with_capacity(data.len() / CAR_RECORD_SIZE);
    for r in data.chunks_exact(CAR_RECORD_SIZE) {
        let s = |o: usize, n: usize| fixed_str(r, o, n, "nfs5.car field");
        let count = u16_le(r, 0x3E6, "nfs5.car paint count")? as usize;
        let mut paint_sets = Vec::new();
        for i in 0..count.min(40) {
            let o = 0x3E8 + i * 16;
            if o + 16 > r.len() {
                break;
            }
            let package = crate::u32_le(r, o, "nfs5.car paint package")?;
            let c = |k: usize| [r[o + 4 + k * 4 + 1], r[o + 4 + k * 4 + 2], r[o + 4 + k * 4 + 3]];
            let packed = |k: usize| {
                let b = &r[o + 4 + k * 4..o + 8 + k * 4];
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            };
            paint_sets.push(PaintSet { package, colours: [c(0), c(1), c(2)], packed: [packed(0), packed(1), packed(2)] });
        }
        let n_int = u16_le(r, 0x528, "nfs5.car interior count")? as usize;
        let mut interior_colours = Vec::new();
        for i in 0..n_int.min(40) {
            let o = 0x530 + i * 16;
            if o + 4 > r.len() {
                break;
            }
            interior_colours.push([r[o + 1], r[o + 2], r[o + 3]]);
        }
        out.push(CarCatalogEntry {
            display_name: s(0x02, 50)?,
            family: s(0x34, 25)?,
            locale_key: s(0x4D, 50)?,
            model: s(0x7F, 50)?,
            sim: s(0xB1, 50)?,
            showcase: s(0xE3, 50)?,
            sim_family: s(0x115, 50)?,
            logo: s(0x147, 50)?,
            sound_set: s(0x179, 57)?,
            body: s(0x1B2, 16)?,
            displacement: s(0x1C2, 16)?,
            id: u16_le(r, 0, "nfs5.car id")?,
            variant: u16_le(r, 0x1C0, "nfs5.car +0x1C0")? as i16,
            cabrio: r[0x1BF] != 0,
            stock_parts: {
                let n = (u16_le(r, 0x3BC, "nfs5.car +0x3BC")? as i16).clamp(0, 40) as usize;
                (0..n).filter_map(|k| u16_le(r, 0x3BE + 2 * k, "nfs5.car +0x3BE").ok().map(|v| v as i16)).collect()
            },
            logo_pair: u16_le(r, 0x1F6, "nfs5.car +0x1F6")? as i16,
            kind: u16_le(r, 0x1F4, "nfs5.car +0x1F4")?,
            dealer_flag_204: u16_le(r, 0x204, "nfs5.car +0x204")?,
            class_string_id: u32_le(r, 0x1AC, "nfs5.car +0x1AC")?,
            era_string_id: u32_le(r, 0x1B0, "nfs5.car +0x1B0")?,
            interior_colours,
            value_curve: {
                let mut c = [0f32; 51];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = f32::from_bits(u32_le(r, 0x214 + i * 4, "nfs5.car value curve")?);
                }
                c
            },
            age_curve_head: [f32::from_bits(u32_le(r, 0x20C, "nfs5.car +0x20C")?), f32::from_bits(u32_le(r, 0x210, "nfs5.car +0x210")?)],
            dash: {
                let mut d = [0f32; 4];
                for (i, v) in d.iter_mut().enumerate() {
                    *v = f32::from_bits(u32_le(r, 0x2F0 + i * 4, "nfs5.car dash")?);
                }
                d
            },
            dial_kmh: DialSpec::parse(r, 0x300)?,
            dial_mph: DialSpec::parse(r, 0x30C)?,
            dial_tach: DialSpec::parse(r, 0x318)?,
            compare: {
                let mut c = [0i16; 5];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = u16_le(r, 0x324 + i * 2, "nfs5.car compare rating")? as i16;
                }
                c
            },
            year: u16_le(r, 0x1F8, "nfs5.car year")?,
            year_end: u16_le(r, 0x202, "nfs5.car year_end")?,
            price: u32_le(r, 0x208, "nfs5.car price")?,
            paint_sets,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod dial_tests {
    use super::DialSpec;

    fn spec(total: i16, start: i16, step: i16) -> DialSpec {
        DialSpec { style: 0, total_units: total, needle_style: 0, start_deg: start, deg_per_unit_x1000: step, needle_pivot: 18 }
    }

    /// Both dial families read off their own faces once the reversed table is taken into account:
    /// the '72 911's `300k` face has 0 at about half past seven and 300 at about twenty past four,
    /// and the Boxster's `boxS` face has 0 at a quarter to eight and 260 just past one.
    #[test]
    fn needles_sweep_clockwise_like_the_faces() {
        let s = spec(300, -40, 910);
        assert!((s.screen_cw_deg(0).rem_euclid(360.0) - 217.9).abs() < 0.5, "{}", s.screen_cw_deg(0));
        assert!((s.screen_cw_deg(299) - 130.0).abs() < 0.5);
        let b = spec(260, 55, 620);
        assert!((b.screen_cw_deg(0).rem_euclid(360.0) - 234.4).abs() < 0.5, "{}", b.screen_cw_deg(0));
        assert!((b.screen_cw_deg(259).rem_euclid(360.0) - 35.0).abs() < 0.5);
        // clockwise everywhere, by exactly one step per unit
        for d in [s, b] {
            let step = d.deg_per_unit_x1000 as f32 / 1000.0;
            assert!((d.screen_cw_deg(10) - d.screen_cw_deg(9) - step).abs() < 1e-3);
        }
    }

    /// Values outside the table clamp to its ends rather than running off the face.
    #[test]
    fn value_clamps_to_the_table() {
        let s = spec(300, -40, 910);
        assert_eq!(s.screen_cw_deg(-5), s.screen_cw_deg(0));
        assert_eq!(s.screen_cw_deg(999), s.screen_cw_deg(299));
    }
}
