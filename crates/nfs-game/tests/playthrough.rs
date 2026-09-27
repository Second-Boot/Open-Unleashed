//! A headless playthrough of the whole career.
//!
//! This is not a behaviour test. It answers one question the coverage percentage cannot: **is
//! there anything between a new save and the end of the game that the port cannot get past?** It
//! drives the ported career model the way the front end drives it — buy a car, enter, race every
//! event, take the era finale, then the Factory Driver's three stages — with the player winning
//! everything, and for every event it loads the assets the port would load to start that race
//! (`TrackAssets::load`, `CarAssets::load`, `CarAssets::load_sim`, and a skill test's `.scn`).
//!
//! Anything that stops the walk is collected instead of panicking on the spot, so one run names
//! *every* blocker rather than the first. The test then fails with that list.
use nfs_assets::{car::CarAssets, track::TrackAssets, GamePaths};
use nfs_formats::career::{Era, Tournament};
use nfs_game::career::{Career, CareerTables};
use std::collections::BTreeSet;

struct Run {
    paths: GamePaths,
    tables: CareerTables,
    career: Career,
    blockers: Vec<String>,
    /// Finale records won (the award 0x4EE17D leaves only the prize car behind).
    finales_won: Vec<i16>,
    /// Assets already loaded once, so twenty tournaments on eight tracks stay cheap.
    seen: BTreeSet<String>,
    /// Events whose track has no racing line: the AI has nothing to follow there.
    no_line: BTreeSet<String>,
    races: usize,
}

impl Run {
    /// Load exactly what the port loads to start one race. Errors are recorded, not raised.
    fn check_race(&mut self, track_id: i16, backwards: bool, car_id: u16, scene: Option<String>, what: &str) {
        let Some(track) = self.tables.track(track_id) else {
            self.blockers.push(format!("{what}: no nfs5.trk record for track {track_id}"));
            return;
        };
        let file = track.file.clone();
        let key = format!("track:{file}:{}", scene.as_deref().unwrap_or(""));
        if self.seen.insert(key) {
            match TrackAssets::load(&self.paths, &file) {
                Ok(t) => {
                    if t.racing_line(&self.paths, backwards).is_none() {
                        self.no_line.insert(format!("{file} (backwards={backwards})"));
                    }
                    if let Some(suffix) = &scene {
                        match t.load_scene(suffix) {
                            Ok(Some(_)) => {}
                            Ok(None) => self.blockers.push(format!("{what}: {file}{suffix} is not on disk")),
                            Err(e) => self.blockers.push(format!("{what}: {file}{suffix}: {e}")),
                        }
                    }
                }
                Err(e) => self.blockers.push(format!("{what}: track {file}: {e}")),
            }
        }
        let Some(rec) = self.tables.car(car_id) else {
            self.blockers.push(format!("{what}: no nfs5.car record for car {car_id}"));
            return;
        };
        let (model, sim) = (rec.model.clone(), rec.sim.clone());
        if self.seen.insert(format!("car:{model}")) {
            if let Err(e) = CarAssets::load(&self.paths, &model) {
                self.blockers.push(format!("{what}: car model {model}: {e}"));
            }
        }
        if self.seen.insert(format!("sim:{sim}")) {
            if sim.is_empty() {
                self.blockers.push(format!("{what}: car {model} has no simulation variant"));
            } else if let Err(e) = CarAssets::load_sim(&self.paths, &sim) {
                self.blockers.push(format!("{what}: sim {sim}: {e}"));
            }
        }
        self.races += 1;
    }

    /// A car eligible for `t`, buying one if the garage has none — the player's real options.
    fn ensure_car(&mut self, t: &Tournament) -> Option<usize> {
        if let Some(i) = self.career.eligible_garage(t).first() {
            return Some(*i);
        }
        let choice = self
            .career
            .eligible_catalog(&self.tables, t)
            .iter()
            .map(|c| (self.career.dealer_price(c), c.id))
            .filter(|(price, _)| (*price as i64) <= self.career.money)
            .min();
        let Some((_, id)) = choice else {
            let listed = self.career.eligible_catalog(&self.tables, t).len();
            self.blockers.push(format!(
                "tournament {} ({}): no eligible car — {listed} listed this year, {} in the bank",
                t.id,
                self.tables.tournament_title(t),
                self.career.money
            ));
            return None;
        };
        self.career.buy_car(&self.tables, id, 0, Default::default()).ok()?;
        // the garage is sorted after a purchase (0x4F0EE0): find the new car, fresh from the dealer
        self.career.garage.iter().position(|g| g.car_id == id && g.mileage == 0 && g.wins == 0)
    }

    /// Race every event of `t` with the player first. Returns false when something stopped it.
    fn play(&mut self, id: i16) -> bool {
        let Some(t) = self.tables.tournament(id).cloned() else { return false };
        let Some(idx) = self.ensure_car(&t) else { return false };
        let entered = match self.career.enter(&self.tables, &t) {
            Ok(e) => e,
            Err(e) => {
                self.blockers.push(format!("tournament {id}: enter failed: {e}"));
                return false;
            }
        };
        let car_id = self.career.garage[idx].car_id;
        for ev in entered..t.events.len() {
            let e = &t.events[ev];
            let what = format!("tournament {id} event {ev}");
            self.check_race(e.track_id, e.backwards, car_id, None, &what);
            let order: Vec<usize> = (0..8).collect();
            let times: [u64; 8] = std::array::from_fn(|i| 100_000 + i as u64 * 1000);
            let out = self.career.record_event(&self.tables, &t, ev, &order, &times, &[true; 8], Some(idx));
            // the era step's race end lands on `[E, VictoryRoom] endofera` (0x4D476F): the finale
            // of 0x504800 is offered there and then, and never again (§10.3)
            if out.era_advanced {
                self.play_finale();
            }
            if out.reset {
                self.blockers.push(format!("tournament {id}: reset after winning every event"));
                return false;
            }
        }
        if !self.career.state(id).completed {
            self.blockers.push(format!("tournament {id}: still not completed after all {} events", t.events.len()));
            return false;
        }
        true
    }

    fn play_finale(&mut self) {
        let Some(t) = self.career.finale_tournament(&self.tables).cloned() else {
            self.blockers.push("finale: no record for 0x504800".into());
            return;
        };
        // the finale is raced in the loaned prize car (0x4EB710 modes 10 / 11)
        for (ev, e) in t.events.iter().enumerate() {
            let what = format!("finale {} event {ev}", t.id);
            self.check_race(e.track_id, e.backwards, t.prize_car, None, &what);
        }
        let order: Vec<usize> = (0..8).collect();
        match self.career.record_finale(&self.tables, &order) {
            Some((true, _)) => self.finales_won.push(t.id),
            other => self.blockers.push(format!("finale {}: winning it returned {other:?}", t.id)),
        }
    }

    fn play_factory_driver(&mut self) {
        let total = self.tables.missions.len();
        for _ in 0..total + 8 {
            let Some(i) = self.career.current_mission(&self.tables) else { break };
            let m = self.tables.missions[i].clone();
            let track_file = self.tables.track(m.track_id).map(|t| t.file.clone()).unwrap_or_default();
            let scene = m.scene_file(&track_file).map(|f| f[track_file.len()..].to_string());
            // the mission's car is the first one it names, else whatever the garage holds
            let car_id = m.car_id.max(0) as u16;
            let car = if self.tables.car(car_id).is_some() { car_id } else { self.career.garage.first().map(|c| c.car_id).unwrap_or(car_id) };
            self.check_race(m.track_id, m.backwards, car, scene, &format!("mission {}", m.code));
            self.career.record_mission(&self.tables, i, true);
            if self.career.promotion_due(&self.tables).is_some() && self.career.accept_promotion(&self.tables).is_none() {
                self.blockers.push(format!("mission {}: promotion was due and was not awarded", m.code));
            }
        }
        if let Some(i) = self.career.current_mission(&self.tables) {
            self.blockers.push(format!("Factory Driver stalls at mission {} of {total}", i + 1));
        }
    }
}

#[test]
fn the_whole_career_can_be_played_through() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Ok(tables) = CareerTables::load(&paths) else {
        eprintln!("skipping: tables would not load");
        return;
    };
    let mut run = Run { paths, tables, career: Career::default(), blockers: Vec::new(), finales_won: Vec::new(), seen: BTreeSet::new(), no_line: BTreeSet::new(), races: 0 };

    // Evolution: every listed tournament of the era the player is up to, then the era's finale.
    let mut guard = 0;
    while !run.career.evolution_complete && guard < 200 {
        guard += 1;
        let era = run.career.era;
        let open: Vec<i16> = run
            .tables
            .era_tournaments(era)
            .iter()
            .filter(|t| !t.is_hidden() && !run.career.state(t.id).completed)
            .map(|t| t.id)
            .collect();
        if open.is_empty() {
            run.blockers.push(format!("{era:?} has nothing open, and the career does not advance"));
            break;
        }
        for id in open {
            if !run.play(id) {
                break;
            }
        }
    }
    if !run.career.evolution_complete {
        run.blockers.push(format!("Evolution never completes — stuck in {:?}", run.career.era));
    }

    run.play_factory_driver();

    // Tournament mode: every club event is listed from the first day (H28), so a playthrough has
    // to get through those too — and some of them are the only users of their track.
    let clubs: Vec<i16> = run.tables.club_tournaments().iter().map(|t| t.id).collect();
    let club_total = clubs.len();
    for id in clubs {
        run.play(id);
    }

    // The walk must actually have gone everywhere, or an empty list would pass quietly.
    let era_total = [Era::Classic, Era::Golden, Era::Modern]
        .iter()
        .flat_map(|e| run.tables.era_tournaments(*e))
        .filter(|t| !t.is_hidden())
        .count();
    let era_done = [Era::Classic, Era::Golden, Era::Modern]
        .iter()
        .flat_map(|e| run.tables.era_tournaments(*e))
        .filter(|t| !t.is_hidden() && run.career.state(t.id).completed)
        .count();
    let club_done = run.tables.club_tournaments().iter().filter(|t| run.career.state(t.id).completed).count();
    eprintln!("playthrough: {era_done}/{era_total} Evolution tournaments, {club_done}/{club_total} club events, {}/{} missions", run.career.missions.iter().filter(|m| **m == 1).count(), run.tables.missions.len());
    assert_eq!(era_done, era_total, "every listed Evolution tournament should be completed");
    assert_eq!(club_done, club_total, "every club event should be completable");
    assert!(era_total >= 3 && club_total >= 3 && run.tables.missions.len() >= 3, "the tables did not load: nothing to play");

    eprintln!(
        "playthrough: {} races started, {} in the bank, {} cars, finales won {:?}",
        run.races,
        run.career.money,
        run.career.garage.len(),
        run.finales_won
    );
    if !run.no_line.is_empty() {
        eprintln!("tracks with no racing line (the AI has nothing to follow): {:?}", run.no_line);
    }
    assert!(run.blockers.is_empty(), "a playthrough is blocked:\n  {}", run.blockers.join("\n  "));
    assert_eq!(run.finales_won, vec![31, 32, 33], "all three era finales should be won");
}
