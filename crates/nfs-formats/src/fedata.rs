//! `FEData/Data/nfs5.car` — the car catalogue used by the frontend (109 × 1648-byte records).
//!
//! Status: string fields CONFIRMED by inspection (all 109 records parse, model/sim names exist on
//! disk, except a few sims such as `993coupeS436` which the game resolves to `default.sim`);
//! numeric tail is UNKNOWN except years/price (HYPOTHESIS from values 1950/1954/11000).

use crate::{fixed_str, u16_le, FormatError};

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
    /// +0x208 u16 (11000) HYPOTHESIS: price in dollars.
    pub price: u16,
}

pub fn parse_car_catalog(data: &[u8]) -> Result<Vec<CarCatalogEntry>, FormatError> {
    if data.len() % CAR_RECORD_SIZE != 0 {
        return Err(FormatError::SizeMismatch { what: "nfs5.car", declared: CAR_RECORD_SIZE, actual: data.len() % CAR_RECORD_SIZE });
    }
    let mut out = Vec::with_capacity(data.len() / CAR_RECORD_SIZE);
    for r in data.chunks_exact(CAR_RECORD_SIZE) {
        let s = |o: usize, n: usize| fixed_str(r, o, n, "nfs5.car field");
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
            year: u16_le(r, 0x1F8, "nfs5.car year")?,
            year_end: u16_le(r, 0x202, "nfs5.car year_end")?,
            price: u16_le(r, 0x208, "nfs5.car price")?,
        });
    }
    Ok(out)
}
