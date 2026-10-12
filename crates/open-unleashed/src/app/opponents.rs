//! The race's opponents: the grid, the driver table and the opponent cars' drawing.

use crate::*;

impl App {
    pub(crate) fn clean_racers(&self) -> [bool; 8] {
        // over the records' `+0xC98` once the race is over (0x4276A4) — the replay under the
        // pages has its own damage
        if let Some(r) = &self.results {
            return r.clean;
        }
        let mut clean = [true; 8];
        if let Some(v) = &self.vehicle {
            clean[0] = nfs_game::career::clean_race(self.start_damage[0], &v.damage);
        }
        for (i, o) in self.road.opponents.iter().take(7).enumerate() {
            clean[i + 1] = nfs_game::career::clean_race(self.start_damage[i + 1], &o.vehicle.damage);
        }
        clean
    }

    /// The opponents of a quick race, a quick knockout and a knockout series: `FEOpponentCarAccessor`
    /// 0x4F4FA0 (`modes.md` §1.1a) — the candidate list is every racing-car record with a model on
    /// disk (0x4F3E40), the choice is the menu's `OpponentCar.Car`, and each opponent draws its own
    /// paint set — then one `nfs5.per` driver each (0x4E3160, §1.1). `selected` is the player's
    /// catalogue index. The skill pairs go in with the knockout series' own 1.0 difficulty.
    ///
    /// Returns the opponents, the seven cars 0x4F4FA0 left in the accessor (`App::opp_cars`) and
    /// the ids the draw left in `[0x6907D8]` (the `Personalities` module, `savegame.md` §4.5).
    pub(crate) fn quick_opponents(
        paths: &GamePaths,
        strings: &nfs_formats::locale::Strings,
        opp_choice: u32,
        difficulty: u8,
        catalog: &[nfs_formats::fedata::CarCatalogEntry],
        selected: usize,
        n: usize,
    ) -> (Vec<CareerOpponent>, Vec<nfs_game::opponents::OpponentPick>, Vec<i16>) {
        let mut crand = nfs_game::opponents::CRand::default();
        let picks = Self::draw_opp_cars(paths, opp_choice, catalog, selected, &mut crand);
        let drivers = Self::draw_drivers(paths, difficulty, n, &mut crand);
        let per_table = Self::per_table(paths);
        let opponents = picks
            .iter()
            .take(n)
            .enumerate()
            .map(|(i, p)| Self::opponent_of(strings, &per_table, catalog, p, drivers.get(i).copied()))
            .collect();
        (opponents, picks, drivers)
    }

    /// `nfs5.per`, the 35 driver records (`career.md` §4).
    pub(crate) fn per_table(paths: &GamePaths) -> Vec<nfs_formats::career::Personality> {
        std::fs::read(paths.fe_data().join("Data").join("nfs5.per"))
            .ok()
            .and_then(|d| nfs_formats::career::parse_personalities(&d).ok())
            .unwrap_or_default()
    }

    /// `0x4E3160(n, Game.Difficutly)` — `n` driver ids without replacement from the records of
    /// that difficulty class (`modes.md` §1.1), the CRT `rand` of the caller's draw.
    pub(crate) fn draw_drivers(paths: &GamePaths, difficulty: u8, n: usize, crand: &mut nfs_game::opponents::CRand) -> Vec<i16> {
        nfs_game::career::pick_personalities(&Self::per_table(paths), Some(difficulty as u32), n, |len| crand.next() as usize % len)
    }

    /// One racing opponent from an `OpponentCar.Car` entry and a driver: what 0x4EBBA0's loop
    /// (quick race, 0x4EBC8x…) and 0x4DE150's (knockout, 0x4DE1E7…0x4DE296) hand 0x4EEDA0 and the
    /// `.PERSONALITY` / `.SKILL*` options — the car and its drawn paint set, the driver's name
    /// `FEStrings[per+4]` (0x4E30E0) and the record's skill pairs.
    pub(crate) fn opponent_of(
        strings: &nfs_formats::locale::Strings,
        per_table: &[nfs_formats::career::Personality],
        catalog: &[nfs_formats::fedata::CarCatalogEntry],
        p: &nfs_game::opponents::OpponentPick,
        driver: Option<i16>,
    ) -> CareerOpponent {
        let entry = catalog.get(p.car).cloned().unwrap_or_else(|| catalog[0].clone());
        let paint = entry.paint_sets.get(p.paint_set).map(|s| s.colours[0]).unwrap_or([0x20, 0x40, 0xC0]);
        let driver = driver.and_then(|id| per_table.iter().find(|r| r.id == id));
        let name = driver.and_then(|r| strings.get(r.name_string_id)).map(|n| n.to_string()).unwrap_or_else(|| entry.display_name.clone());
        CareerOpponent {
            car: entry,
            paint,
            name,
            difficulty: 1.0,
            skill: driver.map_or([[1.0, 0.0]; 4], |r| r.skill),
            grid: None,
            personality: driver.map_or(0, |r| r.id),
            parts: Vec::new(),
            decal: p.decal,
        }
    }

    /// **0x4F4FA0** — `FEOpponentCarAccessor` refills its seven cars from its selection
    /// (`modes.md` §1.1a, `nfs_game::opponents::fill`).
    pub(crate) fn draw_opp_cars(
        paths: &GamePaths,
        opp_choice: u32,
        catalog: &[nfs_formats::fedata::CarCatalogEntry],
        selected: usize,
        crand: &mut nfs_game::opponents::CRand,
    ) -> Vec<nfs_game::opponents::OpponentPick> {
        use nfs_game::opponents::{FeRng, OpponentChoice, fill};
        let dir = paths.car_model_dir();
        let candidates: Vec<usize> = catalog
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.kind == 0
                    && !c.sim.is_empty()
                    && GamePaths::find_file(&dir, &format!("{}.crp", c.model))
                        .is_some()
            })
            .map(|(i, _)| i)
            .collect();
        // the paint packages of `GameData/CarModel/packages.ini` (`[header] count`)
        let package_count = std::fs::read_to_string(dir.join("packages.ini"))
            .ok()
            .and_then(|t| {
                t.lines().find_map(|l| {
                    l.trim()
                        .strip_prefix("count=")
                        .and_then(|v| v.trim().parse::<u32>().ok())
                })
            })
            .unwrap_or(0);
        let choice = OpponentChoice::from_index(opp_choice);
        let picks = fill(catalog, &candidates, selected, choice, &mut FeRng::from_clock(), crand, package_count);
        // `0x4EBBA0` then draws `0x4E3160(nOpp, Game.Difficutly)` (`draw_drivers`): one `nfs5.per`
        // driver per opponent, and hands each car `.DRIVERNAME = FEStrings[per+4]` (0x4E30E0 →
        // 0x4EEDA0 → option 0x13B, car-info `+0x17C`) and the record's skill pairs (0x240…0x246).
        tracing::info!(
            choice = choice.label(),
            candidates = candidates.len(),
            player = catalog.get(selected).map_or("", |c| c.display_name.as_str()),
            cars = ?picks.iter().map(|p| (catalog[p.car].display_name.as_str(), p.paint_set, p.package, p.decal)).collect::<Vec<_>>(),
            "opponent cars (0x4F4FA0)"
        );
        picks
    }
}
