//! Which cars the quick-race opponents drive — `FEOpponentCarAccessor` (`modes.md` §1.1a).
//!
//! The front end keeps one accessor named `OpponentCar.Car`, built at 0x4F2150 as
//! `new(0x33CC) FEOpponentCarAccessor("OpponentCar.Car", 0x4F26A0)`: a five-item list accessor whose
//! selection lives at `this+0x33C4` (0 when it is built) and whose `+0x33C8` is the callback that
//! hands out the candidate records — `0x4F26A0`, i.e. the vector `[0x6919F0]`. `0x4F4FA0` refills
//! the seven embedded `FECar`s from that selection, and `Race_setup` calls it again on the way into
//! a race (`modes.md` §1.1) before copying car `i` into opponent slot `i`.
//!
//! The five choices, and what the original compares:
//!
//! | value | menu string | rule |
//! |---|---|---|
//! | 0 | 640 "same car" | the record the player has selected — every opponent drives it |
//! | 1 | 642 "same era" | a random candidate with the same `nfs5.car+0x1B0` |
//! | 2 | 643 "same class" | a random candidate with the same `nfs5.car+0x1AC` |
//! | 3 | 644 "same era/class" | both fields must match |
//! | other | 641 "random car" | any candidate at all |
//!
//! The candidate list is built by `0x4F3E40`: every `nfs5.car` record of kind 0 (a racing car, not
//! traffic and not a cop) whose `GameData/CarModel/<model>.crp` exists — which in the shipped data
//! is all 87 of them, bonus and Factory Driver cars included.
//!
//! Colours are drawn per opponent, not copied from the player: a paint set index picks all three
//! body colours together, the interior colour is drawn independently, and the paint package and
//! decal are usually left alone (see [`fill`]).

use nfs_formats::fedata::CarCatalogEntry;

/// The `OpponentCar.Car` selection, as stored at `FEOpponentCarAccessor+0x33C4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpponentChoice {
    SameCar,
    SameEra,
    SameClass,
    SameEraClass,
    RandomCar,
}

impl OpponentChoice {
    /// The list runs over five items; anything the accessor does not recognise (0x4F5201) falls
    /// through to the "any car" path at 0x4F54B2.
    pub fn from_index(i: u32) -> Self {
        match i {
            0 => Self::SameCar,
            1 => Self::SameEra,
            2 => Self::SameClass,
            3 => Self::SameEraClass,
            _ => Self::RandomCar,
        }
    }

    pub fn index(self) -> u32 {
        match self {
            Self::SameCar => 0,
            Self::SameEra => 1,
            Self::SameClass => 2,
            Self::SameEraClass => 3,
            Self::RandomCar => 4,
        }
    }

    /// `FEStrings.csv` 640 / 642 / 643 / 644 / 641.
    /// The `FEStrings` id of this choice's row, in the order `FEOpponentCarAccessor`'s row-text
    /// jump table at **0x4F5788** lists them: index 0 → 640, 1 → **642**, 2 → 643, 3 → 644,
    /// 4 → **641**. Anything outside 0…4 is the literal `"bug in car list"` at 0x5DACAC
    /// (0x4F5780) — the original's own words.
    pub fn string_id(self) -> u32 {
        match self {
            Self::SameCar => 640,
            Self::SameEra => 642,
            Self::SameClass => 643,
            Self::SameEraClass => 644,
            Self::RandomCar => 641,
        }
    }

    /// The five choices in row order.
    pub const ROWS: [Self; 5] = [Self::SameCar, Self::SameEra, Self::SameClass, Self::SameEraClass, Self::RandomCar];

    pub fn label(self) -> &'static str {
        match self {
            Self::SameCar => "SAME CAR",
            Self::SameEra => "SAME ERA",
            Self::SameClass => "SAME CLASS",
            Self::SameEraClass => "SAME ERA/CLASS",
            Self::RandomCar => "RANDOM CAR",
        }
    }
}

/// The front end's own generator, **0x538040** — five 32-bit words plus a counter, rippled together
/// with carry, seeded by `0x538110(seed)`. It is not the simulation's LCG (`crate::rng`): the FE
/// seeds this one from `time(NULL)` (0x4E3182) before drawing, so the opponents differ every run.
#[derive(Debug, Clone)]
pub struct FeRng {
    /// `[0x5DE910] .. [0x5DE924]`
    s: [u32; 6],
}

impl FeRng {
    /// `0x538110(seed)` — the shipped `.data` values are what this leaves for `seed == 0`.
    pub fn new(seed: u32) -> Self {
        let a = seed.wrapping_add(0xF22D_0E56);
        let b = a.wrapping_sub(0x69FB_E76D);
        let c = b.wrapping_add(0x3DF3_B646);
        let d = c.wrapping_add(0x40DD_E76D);
        let e = d.wrapping_sub(0x68CD_851F);
        let f = e.wrapping_add(0xD1A9_FBE7);
        Self { s: [a, b, c, d, e, f] }
    }

    /// Seeded from the wall clock, the way the front end does it.
    pub fn from_clock() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        Self::new(t)
    }

    /// One draw, 0x538040. The carry rides from the counter up through the five words; the value
    /// left in the top word is what the function returns.
    pub fn next_u32(&mut self) -> u32 {
        let [m1, s0, s1, s2, s3, s4] = self.s;
        let (mut a, mut c) = s3.overflowing_add(s4);
        self.s[4] = a;
        let step = |a: &mut u32, c: &mut bool, w: u32| {
            let (v, o1) = a.overflowing_add(w);
            let (v, o2) = v.overflowing_add(*c as u32);
            *c = o1 || o2;
            *a = v;
        };
        step(&mut a, &mut c, s2);
        self.s[3] = a;
        step(&mut a, &mut c, s1);
        self.s[2] = a;
        step(&mut a, &mut c, s0);
        self.s[1] = a;
        step(&mut a, &mut c, m1);
        self.s[0] = a;
        // the counter, and the ripple when it wraps
        self.s[5] = self.s[5].wrapping_add(1);
        if self.s[5] == 0 {
            for i in (0..5).rev() {
                self.s[i] = self.s[i].wrapping_add(1);
                if self.s[i] != 0 {
                    break;
                }
                if i == 0 {
                    a = a.wrapping_add(1);
                }
            }
        }
        a
    }

    /// `rand % n` the way every call site takes it — an unsigned 32-bit remainder.
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next_u32() % n }
    }
}

/// The C runtime's `rand()` (0x5A17C9), which the two percentage rolls use. The default seed of a
/// C program is 1; nothing in the image was found calling `srand`.
pub use crate::rng::CRand;

/// One opponent's car and appearance, as `FECar::init` 0x4EEBA0 receives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpponentPick {
    /// index into the catalogue that was passed in
    pub car: usize,
    /// `rand % nfs5.car[+0x3E6]` — one paint set supplies ExtColour, S1Colour and S2Colour together
    pub paint_set: usize,
    /// `rand % nfs5.car[+0x528]`, drawn separately
    pub interior: usize,
    /// PackageNum: 0, or `rand % packages.ini[header].count` on a 49 % roll
    pub package: u32,
    /// DecalNum: −1, or `rand % 100 − 1` on a 19 % roll
    pub decal: i32,
}

/// Is this record one the opponent list may draw from? `0x4F3E40`'s `switch (rec+0x1F4)` keeps
/// kind 0 and then requires the model file to exist.
pub fn is_candidate(c: &CarCatalogEntry, has_model: impl Fn(&str) -> bool) -> bool {
    c.kind == 0 && has_model(&c.model)
}

/// `0x4F4FA0` — fill the seven opponent cars.
///
/// `selected` indexes `catalog` and is the car the player has chosen (`zMergedList.Car`'s record,
/// read at 0x4F4FB7). `candidates` are indices into the same slice, in list order.
///
/// The search for a matching car is the original's, not a filter-then-pick: it draws a start
/// position `r` in `0..candidates.len()`, then walks the list from index 0, wrapping, counting the
/// entries that match, and stops on the `r`-th one (0x4F523D..0x4F52CA). With no match at all the
/// walk would not terminate in the original; here it gives up after two passes and keeps the
/// player's car, which is what a caller that offers only matching candidates never sees.
pub fn fill(
    catalog: &[CarCatalogEntry],
    candidates: &[usize],
    selected: usize,
    choice: OpponentChoice,
    rng: &mut FeRng,
    crand: &mut CRand,
    package_count: u32,
) -> Vec<OpponentPick> {
    (0..7)
        .map(|_| {
            let car = match choice {
                OpponentChoice::SameCar => selected,
                OpponentChoice::RandomCar => candidates.get(rng.below(candidates.len() as u32) as usize).copied().unwrap_or(selected),
                _ => {
                    let sel = &catalog[selected];
                    let matches = |c: &CarCatalogEntry| match choice {
                        OpponentChoice::SameEra => c.era_string_id == sel.era_string_id,
                        OpponentChoice::SameClass => c.class_string_id == sel.class_string_id,
                        _ => c.era_string_id == sel.era_string_id && c.class_string_id == sel.class_string_id,
                    };
                    let want = rng.below(candidates.len().max(1) as u32) as usize;
                    let mut seen = 0usize;
                    let mut found = selected;
                    for k in 0..candidates.len() * 2 + 1 {
                        let idx = candidates[k % candidates.len().max(1)];
                        if matches(&catalog[idx]) {
                            seen += 1;
                            found = idx;
                            if seen > want {
                                break;
                            }
                        }
                    }
                    found
                }
            };
            let e = &catalog[car];
            let paint_set = if e.paint_sets.is_empty() { 0 } else { rng.below(e.paint_sets.len() as u32) as usize };
            let interior = if e.interior_colours.is_empty() { 0 } else { rng.below(e.interior_colours.len() as u32) as usize };
            // 0x4F5153: `rand() % 100 > 50` — 49 chances in 100 — then a package index
            let package = if crand.next() % 100 > 50 && package_count > 0 { rng.below(package_count) } else { 0 };
            // 0x4F517B: `rand() % 100 > 80` — 19 in 100 — then `rand() % 100 − 1`
            let decal = if crand.next() % 100 > 80 { crand.next() % 100 - 1 } else { -1 };
            OpponentPick { car, paint_set, interior, package, decal }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(era: u32, class: u32) -> CarCatalogEntry {
        let mut e = CarCatalogEntry {
            compare: [0; 5],
            variant: 0,
            cabrio: false,
            abs: false,
            traction: false,
            stock_parts: Vec::new(),
            compatible_parts: Vec::new(),
            display_name: String::new(),
            family: String::new(),
            locale_key: String::new(),
            model: String::new(),
            sim: String::new(),
            showcase: String::new(),
            sim_family: String::new(),
            logo: String::new(),
            sound_set: String::new(),
            body: String::new(),
            displacement: String::new(),
            id: 0,
            year: 0,
            year_end: 0,
            logo_pair: 0,
            kind: 0,
            dealer_flag_204: 1,
            class_string_id: class,
            era_string_id: era,
            interior_colours: vec![[0, 0, 0]; 3],
            value_curve: [0.0; 51],
            age_curve_head: [0.0; 2],
            dash: [0.0; 4],
            dial_kmh: nfs_formats::fedata::DialSpec { style: 0, total_units: 1, needle_style: 0, start_deg: 0, deg_per_unit_x1000: 0, needle_pivot: 0 },
            dial_mph: nfs_formats::fedata::DialSpec { style: 0, total_units: 1, needle_style: 0, start_deg: 0, deg_per_unit_x1000: 0, needle_pivot: 0 },
            dial_tach: nfs_formats::fedata::DialSpec { style: 0, total_units: 1, needle_style: 0, start_deg: 0, deg_per_unit_x1000: 0, needle_pivot: 0 },
            price: 0,
            paint_sets: Vec::new(),
        };
        e.paint_sets = vec![nfs_formats::fedata::PaintSet { package: 0, colours: [[0, 0, 0]; 3], packed: [0; 3] }; 4];
        e
    }

    /// Seed 0 has to reproduce the six words the executable ships in `.data`, because that is what
    /// `0x538110(0)` leaves behind.
    #[test]
    fn the_front_ends_generator_starts_where_the_image_does() {
        let r = FeRng::new(0);
        assert_eq!(r.s, [0xF22D_0E56, 0x8831_26E9, 0xC624_DD2F, 0x0702_C49C, 0x9E35_3F7D, 0x6FDF_3B64]);
    }

    #[test]
    fn every_choice_keeps_to_its_own_era_and_class() {
        // three eras × two classes, one car each
        let catalog: Vec<CarCatalogEntry> = (521..524).flat_map(|era| (501..503).map(move |cl| entry(era, cl))).collect();
        let candidates: Vec<usize> = (0..catalog.len()).collect();
        let selected = 2; // era 522, class 501
        let mut c = CRand::default();
        for (choice, ok) in [
            (OpponentChoice::SameCar, 0),
            (OpponentChoice::SameEra, 1),
            (OpponentChoice::SameClass, 2),
            (OpponentChoice::SameEraClass, 3),
        ] {
            let mut r = FeRng::new(7);
            for p in fill(&catalog, &candidates, selected, choice, &mut r, &mut c, 8) {
                let e = &catalog[p.car];
                match ok {
                    0 => assert_eq!(p.car, selected),
                    1 => assert_eq!(e.era_string_id, 522),
                    2 => assert_eq!(e.class_string_id, 501),
                    _ => assert!(e.era_string_id == 522 && e.class_string_id == 501),
                }
                assert!(p.paint_set < 4);
                assert!(p.package < 8);
                assert!((-1..99).contains(&p.decal));
            }
        }
    }

    /// The decal draw at the instruction (0x4F517B `rand()%100 > 80` → 0x4F518D `rand()%100 − 1`,
    /// both on the C-runtime LCG 0x5A17C9: `seed = seed·0x343FD + 0x269EC3`, return `(seed>>16)&0x7FFF`).
    /// `CRand::default()`'s seed-1 stream drawn by hand (Python, the same recurrence) puts the
    /// first non−1 decal at the 8th opponent — `fill` always returns seven, so that is the first
    /// pick of a *second* call sharing the same `CRand` — with value 94: opponents 0…6 draw test
    /// values (41, 18467), (6334, 26500), (19169, 15724), (11478, 29358), (…)'s `%100` landing at
    /// or below 80 every time; the 8th's decal test (`%100`) clears 80 and its draw is `%100 − 1 =
    /// 94`. This also locks the port's known package/decal split (`car-model.md` discrepancy
    /// "(b)"): the package value is `rng.below` (`FeRng`), not a third `crand.next()`, so exactly
    /// two `crand` calls are spent per opponent whose package test fails and three when the decal
    /// test alone passes — matching this function's code, not the original's own third `rand()`
    /// call at 0x4F516B for the package value.
    #[test]
    fn the_decal_draw_matches_the_generator_at_the_instruction() {
        let catalog = vec![entry(521, 501)];
        let candidates = vec![0];
        let mut r = FeRng::new(0);
        let mut c = CRand::default();
        let first = fill(&catalog, &candidates, 0, OpponentChoice::SameCar, &mut r, &mut c, 8);
        assert_eq!(first.len(), 7);
        assert!(first.iter().all(|p| p.decal == -1), "opponents 0..6: {:?}", first.iter().map(|p| p.decal).collect::<Vec<_>>());
        let second = fill(&catalog, &candidates, 0, OpponentChoice::SameCar, &mut r, &mut c, 8);
        assert_eq!(second[0].decal, 94, "opponent 7 (the second call's first pick)");
    }

    /// "Random car" really does spread over the list.
    #[test]
    fn random_car_uses_the_whole_candidate_list() {
        let catalog: Vec<CarCatalogEntry> = (0..20).map(|i| entry(521 + i % 3, 501 + i % 2)).collect();
        let candidates: Vec<usize> = (0..catalog.len()).collect();
        let mut r = FeRng::new(1);
        let mut c = CRand::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            for p in fill(&catalog, &candidates, 0, OpponentChoice::RandomCar, &mut r, &mut c, 8) {
                seen.insert(p.car);
            }
        }
        assert!(seen.len() > 15, "only {} distinct cars", seen.len());
    }
}
