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
    /// +0xE3 e.g. `356-14` (chronicle/showcase key?).
    pub unknown_e3: String,
    /// +0x115 e.g. `356`.
    pub unknown_115: String,
    /// +0x147 e.g. `finallogos2` (logo texture pack?).
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
    /// +0x1F6 u16 (4) unknown.
    pub unknown_1f6: u16,
    /// +0x1F4 u16: must be 0 for the dealer to list the car (0x4F4240).
    pub dealer_flag_1f4: u16,
    /// +0x204 u16: dealer skips values 2 and 3 (0x4F4240).
    pub dealer_flag_204: u16,
    /// +0x214 … +0x2DC: value curve in per cent for the years 1950..=2000 (`0x4F8E50`); the dealer
    /// indexes the same table by age from +0x20C (`age_curve`).
    pub value_curve: [f32; 51],
    /// +0x20C, +0x210: the two entries before the value curve (dealer ages 0 and 1).
    pub age_curve_head: [f32; 2],
    /// +0x300 / +0x30C / +0x318: km/h, mph and tach dial records (`RE/notes/hud.md` §3.1).
    pub dial_kmh: DialSpec,
    pub dial_mph: DialSpec,
    pub dial_tach: DialSpec,
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
    pub package: u32,
    pub colours: [[u8; 3]; 3],
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
    /// Needle angle in degrees for a value (`φ = start + v·deg`, v clamped to `total − 1`).
    pub fn angle(&self, value: i32) -> f32 {
        let v = value.clamp(0, (self.total_units as i32 - 1).max(0));
        self.start_deg as f32 + v as f32 * self.deg_per_unit_x1000 as f32 / 1000.0
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
        (self.price as f64 * pct as f64 / 100.0).round() as u32
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
            paint_sets.push(PaintSet { package, colours: [c(0), c(1), c(2)] });
        }
        out.push(CarCatalogEntry {
            display_name: s(0x02, 50)?,
            family: s(0x34, 25)?,
            locale_key: s(0x4D, 50)?,
            model: s(0x7F, 50)?,
            sim: s(0xB1, 50)?,
            unknown_e3: s(0xE3, 50)?,
            unknown_115: s(0x115, 50)?,
            logo: s(0x147, 50)?,
            sound_set: s(0x179, 57)?,
            body: s(0x1B2, 16)?,
            displacement: s(0x1C2, 16)?,
            id: u16_le(r, 0, "nfs5.car id")?,
            unknown_1f6: u16_le(r, 0x1F6, "nfs5.car +0x1F6")?,
            dealer_flag_1f4: u16_le(r, 0x1F4, "nfs5.car +0x1F4")?,
            dealer_flag_204: u16_le(r, 0x204, "nfs5.car +0x204")?,
            value_curve: {
                let mut c = [0f32; 51];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = f32::from_bits(u32_le(r, 0x214 + i * 4, "nfs5.car value curve")?);
                }
                c
            },
            age_curve_head: [f32::from_bits(u32_le(r, 0x20C, "nfs5.car +0x20C")?), f32::from_bits(u32_le(r, 0x210, "nfs5.car +0x210")?)],
            dial_kmh: DialSpec::parse(r, 0x300)?,
            dial_mph: DialSpec::parse(r, 0x30C)?,
            dial_tach: DialSpec::parse(r, 0x318)?,
            year: u16_le(r, 0x1F8, "nfs5.car year")?,
            year_end: u16_le(r, 0x202, "nfs5.car year_end")?,
            price: u32_le(r, 0x208, "nfs5.car price")?,
            paint_sets,
        });
    }
    Ok(out)
}
