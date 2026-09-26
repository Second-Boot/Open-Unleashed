//! Evolution-mode screen: era, money, tournaments, garage / dealer, and the bridge between the
//! career tables (`nfs_game::career`) and a race started by the app.
//!
//! Rules and hypotheses are documented in `nfs_game::career`; this file only presents them.

use nfs_assets::GamePaths;
use nfs_formats::career::{ConeRule, Mission, Part, Tournament};
use nfs_formats::fedata::CarCatalogEntry;
use nfs_game::career::{Availability, Career, CareerError, CareerTables};
use nfs_game::save::Profile;
use nfs_game::text::BitmapFont;
use nfs_renderer::Sprite;
use std::path::PathBuf;

/// One opponent of a career event, resolved from the tournament record.
#[derive(Debug, Clone)]
pub struct CareerOpponent {
    pub car: CarCatalogEntry,
    pub paint: [u8; 3],
    pub name: String,
    /// Event difficulty (`event+0x2C`) and the driver's (mean, range) skill pairs (`nfs5.per`).
    pub difficulty: f32,
    pub skill: [[f32; 2]; 4],
    /// Explicit grid slot (knockout series: previous finishing order); None = sequential.
    pub grid: Option<u32>,
    /// `nfs5.per` id of the driver, when one was drawn for this slot (knockout, §5.2).
    pub personality: i16,
    /// The opponent record's installed parts (`opp+0x04`, resolved through the parts table), which
    /// 0x4ED880 puts on its FE car's `+0x758` for the race — specs, grip ratio and style overlay.
    pub parts: Vec<Part>,
}

/// Everything the app needs to start a career event.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub track_file: String,
    pub car: CarCatalogEntry,
    pub paint: [u8; 3],
    pub laps: u32,
    /// Event direction flag (`event+0x04`): the race runs the track backwards — reversed slice
    /// table, `_backwards` start scene and draw keys.
    pub backwards: bool,
    /// The event's second direction byte `event+0x05`, 0x4FC0B0's `MIRROR` for types 4 / 5 / 10
    /// / 11 (`modes.md` §2); 0 in every shipped event. A mission has none (type 9's row).
    pub mirror: bool,
    pub opponents: Vec<CareerOpponent>,
    /// Installed parts of the player's car (applied to the `.sim` before the specs are built).
    pub parts: Vec<Part>,
    /// Factory Driver mission (skill test or plain race).
    pub mission: Option<MissionStart>,
    /// `Game.Difficulty` 0..2 — the `SKILL` the AI catch-up band is keyed on
    pub skill: u8,
    /// Number of traffic cars the race is set up with (`traffic.md` §1: the level indexes
    /// `{0, 2, 4, 8}` for tournament events and factory-driver missions).
    pub traffic: u32,
    /// The tournament's own traffic models, `trn+0x382[0..7]`, when its flag `trn+0x380` is set
    /// (club events 46, 47, 55, 59). 0x4FC4C8 then makes the count the number of non-zero ids and
    /// 0x4ED3E7 builds each traffic car from **these** car ids (`0x4F4400(id)`), not from the
    /// track's `trk+0x7C` list. CONFIRMED.
    pub traffic_ids: Option<Vec<u16>>,
    /// The player's 0-based grid slot (`STARTINGPOS − 1`), when the event sets one.
    pub player_grid: Option<u32>,
}

/// `[0x5DB438]` / `[0x5DB448]` — traffic level → number of cars for career events (`traffic.md` §1).
pub const CAREER_TRAFFIC: [u32; 4] = [0, 2, 4, 8];

#[derive(Debug, Clone)]
pub struct MissionStart {
    pub index: usize,
    pub mission: Mission,
    /// `_st<N>.scn` suffix for skill tests (None for race missions / circuit lap races).
    pub scene_suffix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingEvent {
    pub tournament: i16,
    pub event: usize,
    /// Era finale in the loaned prize car.
    pub finale: bool,
    /// Garage slot of the player's car (mileage / wins).
    pub garage_idx: Option<usize>,
    /// Factory Driver mission index.
    pub mission: Option<usize>,
    /// Started by the layout front end's `PLAYGAME` (`start_selected_event`), whose garage
    /// selection is `OwnedCars.Car` and must survive the race — the text screen's `car_sel` is an
    /// index into `car_choices` instead and is put back to 0.
    pub layout: bool,
}

/// The three bytes 0x4D3420's type-4 branch reads when it comes back from a tournament race
/// (0x4D4757…0x4D476F, `controls.md` §4.163).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TournamentRaceFlags {
    /// `TOURNAMENTLASTRACE` `[0x6577D8]` (option 0xB9), set at the launch (0x4ECAFD…): exactly one
    /// event of the tournament was left undone
    pub last_race: bool,
    /// `TOURNAMENTENDOFERA` `[0x6577DC]` (option 0xBA), set at the launch beside it
    pub end_of_era: bool,
    /// 0x505800 = `[0x691E81]`: 0x503AD0 clears it on every call (0x503B01) and sets it when the
    /// tournament it files closes the era (0x503C69)
    pub era_changed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CarChoice {
    /// Garage slot index and car id.
    Owned(usize, u16),
    /// Car id and price at the dealer.
    Buy(u16, u32),
    /// Finale car, loaned for the event.
    Loaned(u16),
}

pub struct CareerUi {
    pub tables: CareerTables,
    pub career: Career,
    pub save_path: PathBuf,
    pub t_idx: usize,
    pub car_sel: usize,
    /// Parts picked in the shop but not paid for yet — the original's `Parts.ShoppingList`, whose
    /// `(TOTAL)` watch shows their price sum (`parts.lay`). Enter adds a part to it, Backspace on
    /// the parts row buys everything in it.
    pub basket: Vec<i16>,
    /// 0 = tournament row, 1 = car row, 2 = parts row.
    pub row: u8,
    pub part_sel: usize,
    /// Tournament mode: list the club events instead of the Evolution tournaments.
    pub club: bool,
    /// Factory Driver screen.
    pub factory: bool,
    pub message: String,
    /// A dialog the start check wants up — `(FEStrings id, two buttons)` (§4.147's raise sites in
    /// 0x4D5730): 813 the car does not meet the restrictions, 858 another tournament is
    /// part-way through, 857 not enough credits for the fee. The app raises it and takes it.
    pub dialog: Option<(u32, bool)>,
    /// `[screen+0x174]` — the player said "yes" to 858 (0x4D2C94), so the check does not ask again
    pub reset_confirmed: bool,
    pub pending: Option<PendingEvent>,
    /// What the front end's way back from a tournament race reads (0x4D3420).
    pub race_flags: TournamentRaceFlags,
    /// The profile the career belongs to: `SaveData/<digit><Name>.sav` (`RE/notes/savegame.md` §2).
    pub profile: Profile,
    pub save_dir: PathBuf,
    /// What the `.sav` carries beside the career — the `Knockout`, `Personalities` and
    /// `CDPlayer` modules and the settings screens' accessors (`savegame.md` §4.5, §4.6, §4.8,
    /// §6) — as the last load left it and the front end keeps it in step.
    pub side: nfs_game::save::SaveSide,
}

impl CareerUi {
    pub fn load(paths: &GamePaths) -> Result<Self, CareerError> {
        let tables = CareerTables::load(paths)?;
        let save_path = std::env::var_os("NFS5_CAREER_SAVE")
            .map(PathBuf::from)
            .unwrap_or_else(Career::default_save_path);
        // The profile is the one `player.lst` last selected, as `RebuildUserList` 0x4DEF60 does.
        let dir = nfs_game::save::save_dir();
        let profile = std::env::var("NFS5_PROFILE")
            .ok()
            .map(|n| Profile::new(n, nfs_game::save::DEFAULT_PICTURE))
            .or_else(|| nfs_game::save::read_player_lst(&dir).and_then(|s| Profile::from_stem(&s)))
            .or_else(|| nfs_game::save::list_profiles(&dir).into_iter().next())
            .unwrap_or_else(|| Profile::new("DRIVER", nfs_game::save::DEFAULT_PICTURE));
        let career = match Career::load(&save_path) {
            Ok(mut c) => {
                // an older port career keeps its cars' stock parts implicit (`Career::upgrade`)
                c.upgrade(&tables);
                tracing::info!(path = %save_path.display(), era = ?c.era, money = c.money, cars = c.garage.len(), "career loaded");
                c
            }
            // No JSON yet: fall back to the original-format save for this profile if one exists.
            Err(CareerError::Io { .. }) => match std::fs::read(profile.path(&dir))
                .ok()
                .and_then(|d| nfs_formats::sav::SaveFile::parse(&d).ok())
            {
                Some(s) => {
                    let c = nfs_game::save::career_from_sav(&s, &tables);
                    tracing::info!(path = %profile.path(&dir).display(), money = c.money, cars = c.garage.len(), "career imported from the original .sav");
                    c
                }
                None => Career::default(),
            },
            Err(e) => {
                tracing::warn!(error = %e, "career save unreadable, starting a new one");
                Career::default()
            }
        };
        Ok(Self {
            tables,
            career,
            save_path,
            save_dir: dir,
            profile,
            side: nfs_game::save::SaveSide::default(),
            t_idx: 0,
            car_sel: 0,
            row: 0,
            part_sel: 0,
            club: false,
            factory: false,
            basket: Vec::new(),
            message: String::new(),
            dialog: None,
            reset_confirmed: false,
            pending: None,
            race_flags: TournamentRaceFlags::default(),
        })
    }

    pub fn tournaments(&self) -> Vec<&Tournament> {
        if self.club {
            self.tables.club_tournaments()
        } else {
            self.tables
                .era_tournaments(self.career.era)
                .into_iter()
                .filter(|t| !t.is_hidden())
                .collect()
        }
    }

    /// Which of 0x5053C0's vectors `Tournament.EventType`'s index `ev_type` stands for, as the
    /// jump table at 0x505450 reads it: 0 the career era's list (0x505404, by `[0x691E50]`), 1…3
    /// the classic, golden and modern club lists (0x505447 / 0x505441 / 0x50543B). Once the
    /// Evolution is finished (`[0x691E54]`, `Career::evolution_complete`) the index is taken one
    /// down: 0…2 are the three club lists (0x50542B…0x50543B) and anything else the era list.
    pub fn event_type_kind(&self, ev_type: usize) -> usize {
        if self.career.evolution_complete { ev_type + 1 } else { ev_type }
    }

    /// The vector **0x5053C0** returns for `Tournament.EventType`'s index (`event_type_kind`):
    /// the era's list with the era finale left out (0x504D00 skips `+0xE4 == 2`), or one of the
    /// three club lists.
    pub fn event_type_list(&self, ev_type: usize) -> Vec<&Tournament> {
        use nfs_formats::career::Era;
        match self.event_type_kind(ev_type) {
            1 => self.tables.club_tournaments_of(Era::Classic),
            2 => self.tables.club_tournaments_of(Era::Golden),
            3 => self.tables.club_tournaments_of(Era::Modern),
            _ => self.tables.era_tournaments(self.career.era).into_iter().filter(|t| !t.is_hidden()).collect(),
        }
    }

    /// `Tournament.EventType`'s rows (`FEStrings` ids). The accessor is built (0x4FD13E →
    /// 0x506A70 → 0x4BB6F0) over the table 0x5B81A4 — 470…473, four rows — and every refresh
    /// 0x505570 cuts its count by the era (**0x506AA0**: `setCount` 0x4BC310 of 1, 2, 3 for
    /// 521, 522, 523): the club races of an era are offered from the next era on, the modern
    /// ones not until the Evolution is over. Then 0x4FD1A0 rebuilds it over **0x5B81CC** —
    /// 471, 472, 473, the three club lists alone (short forms 0x5B81DC) — and the era count
    /// is 3 again.
    pub fn event_type_rows(&self) -> &'static [u32] {
        use nfs_formats::career::Era;
        if self.career.evolution_complete {
            return &[471, 472, 473];
        }
        let n = match self.career.era {
            Era::Classic => 1,
            Era::Golden => 2,
            Era::Modern => 3,
        };
        &[470, 471, 472, 473][..n]
    }

    /// **0x505570**'s tournament half: `Tournament.Tournament` onto the row 0x5045C0 picks from
    /// the list `Tournament.EventType` shows (`Career::default_tournament_row`), and the
    /// selected tournament `[0x691E6C]` onto that record (0x5055CD…0x5055F9). A club list makes
    /// the port's club mode, as the combo's own step does.
    pub fn refresh_tournament(&mut self, ev_type: usize) {
        let list = self.event_type_list(ev_type);
        let Some(id) = self.career.default_tournament_row(&list).map(|k| list[k].id) else { return };
        self.club = (1..=3).contains(&self.event_type_kind(ev_type));
        if let Some(i) = self.tournaments().iter().position(|t| t.id == id) {
            self.t_idx = i;
        }
    }

    /// Menu mode: 1 Evolution, 2 Tournament (club events), 3 Factory Driver.
    pub fn set_mode(&mut self, mode: u8) {
        let (club, factory) = (mode == 2, mode == 3);
        if self.club != club || self.factory != factory {
            self.club = club;
            self.factory = factory;
            self.t_idx = 0;
            self.car_sel = 0;
            self.part_sel = 0;
            self.message.clear();
        }
    }

    pub fn narrator_name(&self, n: i16) -> String {
        self.tables.strings.text(18176 + n.clamp(0, 5) as u32)
    }

    /// Start the current Factory Driver mission (or accept a due promotion).
    pub fn activate_mission(&mut self) -> Option<StartRequest> {
        if self.career.promotion_due(&self.tables).is_some() {
            if let Some(car) = self.career.accept_promotion(&self.tables) {
                self.message = format!(
                    "PRIZE CAR ACCEPTED: {}",
                    self.tables
                        .car(car)
                        .map(|c| c.display_name.to_string())
                        .unwrap_or_default()
                );
                self.save();
            }
            return None;
        }
        // `NFS_MISSION=<idx>` overrides the current mission (smoke runs)
        let idx = std::env::var("NFS_MISSION")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|i| *i < self.tables.missions.len())
            .or_else(|| self.career.current_mission(&self.tables))?;
        let m = self.tables.missions[idx].clone();
        let car = self.tables.car(m.car_id as u16)?.clone();
        if car.sim.is_empty() {
            self.message = "MISSION CAR HAS NO SIMULATION DATA".into();
            return None;
        }
        let Some(track) = self.tables.track(m.track_id) else {
            self.message = format!("UNKNOWN TRACK {}", m.track_id);
            return None;
        };
        let c = m.colours[0];
        let paint = if c[..3] == [0, 0, 0] {
            car.paint_sets
                .first()
                .map(|p| p.colours[0])
                .unwrap_or([0xD4, 0x10, 0x10])
        } else {
            [c[0], c[1], c[2]]
        };
        let parts: Vec<Part> = m
            .parts
            .iter()
            .filter_map(|id| self.tables.parts.iter().find(|p| p.id == *id).cloned())
            .collect();
        let opponents: Vec<CareerOpponent> = m
            .opponents
            .iter()
            .filter_map(|(per, car_id)| {
                let car = self.tables.car(*car_id as u16)?.clone();
                if car.sim.is_empty() {
                    return None;
                }
                let skill = self
                    .tables
                    .personalities
                    .iter()
                    .find(|p| p.id == *per)
                    .map(|p| p.skill)
                    .unwrap_or([[1.0, 0.0]; 4]);
                let paint = car
                    .paint_sets
                    .first()
                    .map(|p| p.colours[0])
                    .unwrap_or([0x20, 0x40, 0xC0]);
                Some(CareerOpponent {
                    car,
                    paint,
                    name: self.tables.driver_name(*per),
                    difficulty: 1.0,
                    skill,
                    grid: None,
                    personality: *per,
                    parts: Vec::new(),
                })
            })
            .collect();
        let scene_suffix = m
            .scene_file(&track.file)
            .map(|f| f[track.file.len()..].to_string());
        self.pending = Some(PendingEvent {
            tournament: 0,
            event: 0,
            finale: false,
            garage_idx: None,
            mission: Some(idx),
            layout: false,
        });
        tracing::info!(mission = m.code, track = track.file, scene = ?scene_suffix, car = car.model, opponents = opponents.len(), "mission start");
        Some(StartRequest {
            track_file: track.file.clone(),
            car,
            paint,
            laps: m.laps.max(1) as u32,
            backwards: m.backwards,
            mirror: false,
            opponents,
            parts,
            mission: Some(MissionStart {
                index: idx,
                mission: m.clone(),
                scene_suffix,
            }),
            traffic: CAREER_TRAFFIC[(m.traffic_level.max(0) as usize).min(3)],
            traffic_ids: None,
            player_grid: None,
            skill: m.skill.clamp(0, 2) as u8,
        })
    }

    /// Mission over: `passed` from `SkillTest::evaluate` / the race position.
    /// `text` is the result line the race's `HMissionSummary` chose (0x4293F0), and `driver` the
    /// name its `%s` takes.
    pub fn finish_mission(&mut self, idx: usize, passed: bool, time_ticks: u64, cones: usize, text: Option<u32>, driver: &str) {
        self.pending = None;
        let m = self.tables.missions[idx].clone();
        let pick = text;
        let promotion = self.career.record_mission(&self.tables, idx, passed);
        let secs = time_ticks as f32 / 64.0;
        let mut msg = format!(
            "{}: {}  ({}:{:05.2}, {} CONES)  ",
            if passed { "PASSED" } else { "FAILED" },
            m.code.to_string(),
            (secs / 60.0) as u32,
            secs % 60.0,
            cones
        );
        if let Some(id) = pick {
            msg += &self.tables.strings.text(id).replacen("%s", driver, 1);
        }
        if let Some(stage) = promotion {
            msg += &format!("   STAGE {stage} COMPLETE: PROMOTION!");
        }
        self.message = msg;
        self.save();
    }

    fn factory_sprites(&self, f: &BitmapFont, active: Option<u8>, out: &mut Vec<Sprite>) {
        let grey = [0.85, 0.85, 0.85, 1.0];
        let dim = [0.55, 0.55, 0.55, 1.0];
        let sel = |r: u8| {
            if active == Some(r) {
                [1.0, 0.75, 0.1, 1.0]
            } else {
                grey
            }
        };
        let done = self.career.missions.iter().filter(|r| **r == 1).count();
        let titles = [
            "JUNIOR TEST DRIVER",
            "TEST DRIVER",
            "CHIEF TEST DRIVER",
            "ACE TEST DRIVER",
        ];
        let rank = self.career.promotions_accepted.len().min(3);
        out.extend(f.sprites(
            &format!(
                "FACTORY DRIVER   {}   {}/{} MISSIONS",
                titles[rank],
                done,
                self.tables.missions.len()
            ),
            [0.08, 0.20],
            0.04,
            [0.9, 0.9, 1.0, 1.0],
        ));
        if let Some(p) = self.career.promotion_due(&self.tables) {
            let car = self
                .tables
                .car(p.prize_car as u16)
                .map(|c| c.display_name.to_string())
                .unwrap_or_default();
            out.extend(f.sprites(
                &format!("PROMOTION  < ACCEPT THE {car} >"),
                [0.08, 0.30],
                0.045,
                sel(0),
            ));
            for (i, line) in wrap(
                &self
                    .tables
                    .strings
                    .text(p.promotion_string_id)
                    .replace("%s", "DRIVER"),
                66,
            )
            .iter()
            .take(7)
            .enumerate()
            {
                out.extend(f.sprites(
                    line,
                    [0.08, 0.37 + i as f32 * 0.036],
                    0.026,
                    dim,
                ));
            }
        } else if let Some(idx) = self.career.current_mission(&self.tables) {
            let m = &self.tables.missions[idx];
            let title = self.tables.strings.text(m.title_string_id());
            out.extend(f.sprites(
                &format!(
                    "MISSION {}  < {} >",
                    m.code.to_string(),
                    title.to_string()
                ),
                [0.08, 0.30],
                0.045,
                sel(0),
            ));
            let car = self
                .tables
                .car(m.car_id as u16)
                .map(|c| c.display_name.to_string())
                .unwrap_or_default();
            let mut rules = vec![format!(
                "{} - {}",
                self.tables.track_name(m.track_id).to_string(),
                car
            )];
            if let Some(t) = m.time_limit_s {
                rules.push(format!("GOAL TIME {}:{:02}", t / 60, t % 60));
            }
            if let Some(ConeRule::PenaltySeconds(s)) = m.cone_rule {
                rules.push(format!("+{s} S PER CONE"));
            }
            if let Some(ConeRule::Limit(n)) = m.cone_rule {
                rules.push(format!("MAX {n} CONES"));
            }
            if let Some(d) = m.damage_limit_pct {
                rules.push(format!("DAMAGE UNDER {d}%"));
            }
            if let Some(r) = m.min_rank {
                rules.push(format!(
                    "FINISH {}",
                    if r == 1 {
                        "1ST".to_string()
                    } else {
                        format!("TOP {r}")
                    }
                ));
            }
            if m.laps > 1 {
                rules.push(format!("{} LAPS", m.laps));
            }
            let rule_lines = wrap(&rules.join("   "), 70);
            for (i, line) in rule_lines.iter().take(2).enumerate() {
                out.extend(f.sprites(line, [0.12, 0.36 + i as f32 * 0.035], 0.028, dim));
            }
            let y0 = 0.36 + rule_lines.len().min(2) as f32 * 0.035 + 0.02;
            out.extend(f.sprites(
                &format!("{}:", self.narrator_name(m.narrator).to_string()),
                [0.08, y0],
                0.028,
                grey,
            ));
            for (i, line) in wrap(
                &self
                    .tables
                    .strings
                    .text(m.intro_string_id)
                    .replace("%s", "DRIVER"),
                66,
            )
            .iter()
            .take(7)
            .enumerate()
            {
                out.extend(f.sprites(
                    line,
                    [0.08, y0 + 0.04 + i as f32 * 0.036],
                    0.026,
                    dim,
                ));
            }
        } else {
            out.extend(f.sprites("ALL MISSIONS COMPLETE", [0.08, 0.30], 0.045, sel(0)));
        }
        if !self.message.is_empty() {
            for (i, line) in wrap(&self.message, 70).iter().take(4).enumerate() {
                out.extend(f.sprites(
                    line,
                    [0.08, 0.72 + i as f32 * 0.033],
                    0.026,
                    [0.4, 1.0, 0.5, 1.0],
                ));
            }
        }
    }

    pub fn current(&self) -> Option<&Tournament> {
        let list = self.tournaments();
        list.get(self.t_idx.min(list.len().saturating_sub(1)))
            .copied()
    }

    pub fn car_choices(&self) -> Vec<CarChoice> {
        if let Some(f) = &self.career.finale {
            return self
                .tables
                .tournament(f.tournament)
                .map(|t| vec![CarChoice::Loaned(t.prize_car)])
                .unwrap_or_default();
        }
        let Some(t) = self.current() else {
            return Vec::new();
        };
        let mut v: Vec<CarChoice> = self
            .career
            .eligible_garage(t)
            .into_iter()
            .map(|i| CarChoice::Owned(i, self.career.garage[i].car_id))
            .collect();
        for c in self.career.eligible_catalog(&self.tables, t) {
            v.push(CarChoice::Buy(c.id, self.career.dealer_price(c)));
        }
        v
    }

    /// Repair every category of the selected car (the `QUICKFIX` screen's OVERALL row).
    pub fn repair_all(&mut self) {
        let Some(i) = self.selected_garage() else {
            self.message = "SELECT AN OWNED CAR FIRST".into();
            return;
        };
        let kinds: Vec<u16> = (1..=10).collect();
        let cost = self.career.repair_cost(&self.tables, i, &kinds);
        if cost == 0 {
            self.message = "NOTHING TO REPAIR".into();
            return;
        }
        // 0x50F232: the repair total above the money -> one-button 809
        if self.career.money < cost {
            self.dialog = Some((809, false));
            return;
        }
        match self.career.repair(&self.tables, i, nfs_game::career::QuickFix::Overall) {
            Ok(paid) => {
                self.message = format!("REPAIRED FOR ${paid}");
                self.save();
            }
            Err(e) => self.message = e.to_string().to_uppercase(),
        }
    }

    /// The parts in the basket, in the order they were added.
    pub fn basket_parts(&self) -> Vec<&Part> {
        self.basket
            .iter()
            .filter_map(|id| self.tables.parts.iter().find(|p| p.id == *id))
            .collect()
    }

    /// `Parts.ShoppingList(TOTAL)`.
    pub fn basket_total(&self) -> i64 {
        self.basket_parts().iter().map(|p| self.career.part_buy_price(p)).sum()
    }

    /// Pay for the basket and install everything in it, in order. Stops at the first part that
    /// cannot be bought (no money, not eligible) and leaves the rest in the basket.
    pub fn buy_basket(&mut self) {
        let Some(i) = self.selected_garage() else {
            self.message = "SELECT AN OWNED CAR FIRST".into();
            return;
        };
        if self.basket.is_empty() {
            // an empty transaction list on the parts row means the other thing this shop does
            self.repair_all();
            return;
        }
        // 0x517630: the whole list's `TOTAL` against the money first — above it, one-button 810
        // and nothing is bought (the port used to buy part by part until the money ran out)
        if self.basket_total() > self.career.money {
            self.dialog = Some((810, false));
            return;
        }
        let mut bought = 0;
        let mut spent = 0i64;
        while let Some(id) = self.basket.first().copied() {
            let price = self
                .tables
                .parts
                .iter()
                .find(|p| p.id == id)
                .map(|p| self.career.part_buy_price(p))
                .unwrap_or(0);
            match self.career.buy_part(&self.tables, i, id) {
                Ok(()) => {
                    self.basket.remove(0);
                    bought += 1;
                    spent += price;
                }
                Err(e) => {
                    self.message = e.to_string().to_uppercase();
                    break;
                }
            }
        }
        if bought > 0 {
            self.message = format!(
                "INSTALLED {bought} PART{}  (${spent})",
                if bought == 1 { "" } else { "S" }
            );
            self.save();
        }
    }

    /// Backspace / East button: decline a pending finale, or sell the selected garage car.
    pub fn back(&mut self) {
        if self.career.finale.is_some() {
            self.career.decline_finale();
            self.message = "FINALE DECLINED".into();
            self.save();
            return;
        }
        if self.row == 2 {
            self.buy_basket();
            return;
        }
        // used-car market (career.md §10.5): list the car, withdraw it, or accept a standing offer.
        // The original asks for the asking price in an entry box; this port lists at the car's current
        // value (UI choice — the offer rules themselves are the original's).
        if self.row == 1
            && let Some(i) = self.selected_garage()
        {
            let name = self
                .tables
                .car(self.career.garage[i].car_id)
                .map(|c| c.display_name.to_string())
                .unwrap_or_default();
            let car = &self.career.garage[i];
            if car.offer.is_some() {
                match self.career.accept_offer(&self.tables, i) {
                    Ok(v) => {
                        self.message = format!("SOLD {name} FOR ${v}");
                        self.car_sel = 0;
                        self.save();
                    }
                    Err(e) => self.message = e.to_string().to_uppercase(),
                }
            } else if car.asking_price.is_some() {
                self.career.withdraw_from_sale(i);
                self.message = format!("{name} TAKEN OFF THE MARKET");
                self.save();
            } else {
                let ask = self.career.car_value(&self.tables, i).unwrap_or(0).max(0) as u32;
                match self.career.list_for_sale(i, ask) {
                    Ok(()) => {
                        self.message =
                            format!("{name} LISTED AT ${ask}   (OFFERS COME IN AFTER A RACE)");
                        self.save();
                    }
                    // 0x50FCF1: `push 0x2BE` (the "ok" label, 702) / `push 0x3C` (the text, 60)
                    Err(nfs_game::career::CareerError::LastCar) => self.dialog = Some((60, false)),
                    Err(e) => self.message = e.to_string().to_uppercase(),
                }
            }
        }
    }

    /// Garage index of the selected car, if it is an owned car.
    fn selected_garage(&self) -> Option<usize> {
        match self.car_choices().get(self.car_sel) {
            Some(CarChoice::Owned(i, _)) => Some(*i),
            _ => None,
        }
    }

    /// Parts on offer for the selected garage car.
    pub fn part_choices(&self) -> Vec<&Part> {
        let Some(i) = self.selected_garage() else {
            return Vec::new();
        };
        let Some(car) = self.tables.car(self.career.garage[i].car_id) else {
            return Vec::new();
        };
        self.career.parts_for(&self.tables, car)
    }

    pub fn move_col(&mut self, delta: i32) {
        match self.row {
            0 => {
                let n = self.tournaments().len().max(1);
                self.t_idx = ((self.t_idx as i32 + delta).rem_euclid(n as i32)) as usize;
                self.car_sel = 0;
                self.part_sel = 0;
            }
            1 => {
                let n = self.car_choices().len().max(1);
                self.car_sel = ((self.car_sel as i32 + delta).rem_euclid(n as i32)) as usize;
                self.part_sel = 0;
            }
            _ => {
                let n = self.part_choices().len().max(1);
                self.part_sel = ((self.part_sel as i32 + delta).rem_euclid(n as i32)) as usize;
            }
        }
        self.message.clear();
    }

    pub fn save(&mut self) {
        if let Err(e) = self.career.save(&self.save_path) {
            tracing::error!(error = %e, "career save failed");
            self.message = format!("SAVE FAILED: {e}");
        }
        // `Tournament`'s front-end dwords (`save::TournamentFe`): the selected tournament
        // `[0x691E6C]` is the record's index in the table, and the running pair `[0x5DB768]` /
        // `[0x5DB76C]` is set only by the tournament launch 0x503940 (0x4D5B4A) — so a mission or a
        // finale leaves the −1s 0x503A50 put there. The next event `[0x691E7C]` is the front
        // end's (`Tournament.RemainingTracks`), which keeps it in `side` as it moves.
        let index_of = |id: i16| self.tables.tournaments.iter().position(|t| t.id == id);
        if let Some(i) = self.current().and_then(|t| index_of(t.id)) {
            self.side.tournament.selected = i as u32;
        }
        // The pair is a pair of globals, not a function of the pending race: 0x503940 sets it at a
        // tournament launch, 0x503A50 puts the −1s back at that race's end (`finish` / `abort`),
        // and nothing else writes it — so a pair a loaded file carried stays until then, and is
        // saved again as it is. Nothing reads it outside a tournament race's end (0x503960 at
        // 0x4EE007, the getters 0x5056D0…0x5057D0 called from 0x4EE040…0x4EE068), so **a saved
        // running event is not turned back into a race**: the original drops the attempt the same
        // way (the fee was paid before that save). *(Until 2026-09-26 the port rewrote the pair
        // from the pending race on every save, so a loaded pair was lost at the next save.)*
        if let Some(r) = self.pending.as_ref().filter(|p| p.mission.is_none() && !p.finale).and_then(|p| Some((p.event as i32, index_of(p.tournament)? as i32))) {
            (self.side.tournament.current_event, self.side.tournament.current_tournament) = r;
        }
        // …and the same career in the original's format, so `SaveData/` matches what the game
        // itself would leave behind (`RE/notes/savegame.md` §3).
        let sav = nfs_game::save::career_to_sav_with(
            &self.career,
            &self.tables,
            &self.profile,
            &self.side,
        );
        if let Err(e) = nfs_game::save::write_profile(&self.save_dir, &self.profile, &sav) {
            tracing::warn!(error = %e, "writing the .sav profile failed");
        }
    }

    /// The selected tournament `[0x691E6C]` a loaded save carries (0x5029F0), as this screen's row:
    /// the record at that index in the table, when the list on show holds it. Returns the event
    /// row `Tournament.RemainingTracks` is on — the saved next event `[0x691E7C]` among the undone
    /// ones — or `None` when that event is not an undone one of it.
    pub fn apply_tournament_fe(&mut self, fe: nfs_game::save::TournamentFe) -> Option<usize> {
        let id = self.tables.tournaments.get(fe.selected as usize)?.id;
        let row = self.tournaments().iter().position(|t| t.id == id)?;
        self.t_idx = row;
        let t = self.current()?;
        self.career.undone_events(t).iter().position(|e| *e == fe.next_event as usize)
    }

    /// `LoadInfo` 0x4DE9B0 with `autosave = 0` (`RE/notes/savegame.md` §2.4, §3): read
    /// `SaveData\<stem>.sav` and make it the career; `player.lst` takes the stem (0x4DEC7B).
    /// False when the file is missing or does not parse — the callers' 806.
    ///
    /// The autosave arm never fires from the player screens: it tests `<i><stem>.sav` for a stem
    /// that already carries its digit, which no file matches. The port's JSON career is rewritten
    /// with the loaded one so the next start agrees with `player.lst`.
    pub fn switch_profile(&mut self, p: &Profile) -> bool {
        let Some(s) = std::fs::read(p.path(&self.save_dir))
            .ok()
            .and_then(|d| nfs_formats::sav::SaveFile::parse(&d).ok())
        else {
            return false;
        };
        self.career = nfs_game::save::career_from_sav(&s, &self.tables);
        self.profile = p.clone();
        // the modules `LoadInfo` hands their own loaders — `Knockout` 0x4DDD60, `Personalities`
        // 0x4E2CA0, `CDPlayer` 0x4BE170 — and the accessor block 0x4B7FD0
        self.side = nfs_game::save::side_from_sav(&s);
        if nfs_game::save::write_player_lst(&self.save_dir, &p.stem()).is_err() {
            return false;
        }
        if let Err(e) = self.career.save(&self.save_path) {
            tracing::warn!(error = %e, "career save failed");
        }
        true
    }

    /// `0x4DE5B0(name, picture)` (`savegame.md` §2.2): refused when any `<i><name>.sav` exists;
    /// otherwise the pristine career (`XXDefXX`, the defaults 0x4DE880 saved at start-up), the
    /// name, the picture, and `SaveInfo` under `<picture><name>`. The per-profile HUD copies it
    /// also makes are not ported.
    #[cfg(test)]
    pub fn create_profile(&mut self, name: &str, picture: u8) -> bool {
        self.create_profile_from(name, picture, &nfs_game::save::SaveSide::default())
    }

    /// [`Self::create_profile`] with `XXDefXX`'s modules and accessors — the front end's start-up
    /// state 0x4DE880 saved (0x4D3B76) and 0x4DE5B0 loads before it saves the new profile — so the
    /// new file carries those options, not the ones of the profile that was up.
    pub fn create_profile_from(&mut self, name: &str, picture: u8, template: &nfs_game::save::SaveSide) -> bool {
        if (0..=9).any(|d| self.save_dir.join(format!("{d}{name}.sav")).exists()) {
            return false;
        }
        let old = (std::mem::take(&mut self.career), self.profile.clone());
        self.profile = Profile::new(name, picture);
        let sav = nfs_game::save::career_to_sav_with(&self.career, &self.tables, &self.profile, template);
        if nfs_game::save::write_profile(&self.save_dir, &self.profile, &sav).is_err() {
            (self.career, self.profile) = old;
            return false;
        }
        true
    }

    /// Enter pressed: buy the selected car or part, or start the next event of the selected tournament.
    /// The "yes" to 858 (0x4D2C94…0x4D2CA3): clear the other part-way tournaments (0x504BD0)
    /// and remember the answer, so the start check that runs again does not ask.
    pub fn confirm_reset(&mut self) {
        if let Some(t) = self.current().cloned() {
            self.career.reset_partial_except(&self.tables, t.id);
            self.reset_confirmed = true;
            self.save();
        }
    }

    pub fn activate(&mut self) -> Option<StartRequest> {
        if self.factory {
            return self.activate_mission();
        }
        if self.row == 2 {
            let (Some(_), Some(part)) = (
                self.selected_garage(),
                self.part_choices()
                    .get(self.part_sel)
                    .map(|p| (p.id, self.tables.strings.text(p.name_string_id).to_string())),
            ) else {
                self.message = "SELECT AN OWNED CAR FIRST".into();
                return None;
            };
            // the shop fills a basket and charges for it on confirmation
            if let Some(k) = self.basket.iter().position(|id| *id == part.0) {
                self.basket.remove(k);
                self.message = format!("REMOVED {}", part.1.to_string());
            } else {
                self.basket.push(part.0);
                self.message = format!(
                    "ADDED {}  (TOTAL ${})",
                    part.1.to_string(),
                    self.basket_total()
                );
            }
            return None;
        }
        let choices = self.car_choices();
        let choice = choices
            .get(self.car_sel.min(choices.len().saturating_sub(1)))
            .cloned();
        let t = self.current()?.clone();
        match choice {
            Some(CarChoice::Buy(id, _)) => {
                match self.career.buy_car(&self.tables, id, 0, nfs_game::career::Trim::default()) {
                    Ok(()) => {
                        let name = self
                            .tables
                            .car(id)
                            .map(|c| c.display_name.clone())
                            .unwrap_or_default();
                        self.message = format!("BOUGHT {}", name.to_string());
                        self.car_sel = self
                            .car_choices()
                            .iter()
                            .position(|c| matches!(c, CarChoice::Owned(_, cid) if *cid == id))
                            .unwrap_or(0);
                        self.save();
                    }
                    // 0x501D41 / 0x501F86: not enough credits for the car -> one-button 802
                    Err(nfs_game::career::CareerError::NoMoney { .. }) => self.dialog = Some((802, false)),
                    // 0x501D94: eighty cars already -> one-button 61
                    Err(nfs_game::career::CareerError::GarageFull) => self.dialog = Some((61, false)),
                    Err(e) => self.message = e.to_string().to_uppercase(),
                }
                None
            }
            Some(CarChoice::Owned(_, id)) | Some(CarChoice::Loaned(id)) => {
                let car = self.tables.car(id)?.clone();
                if car.sim.is_empty() {
                    self.message = "CAR HAS NO SIMULATION DATA".into();
                    return None;
                }
                let finale = self.career.finale.clone();
                let (t, event, garage_idx) = match &finale {
                    Some(f) => (self.tables.tournament(f.tournament)?.clone(), 0usize, None),
                    None => {
                        // the start check 0x4D5730 in the original's order: the car against the
                        // tournament's restrictions (0x504A70 → 813), another tournament part-way
                        // through (0x504B60 → 858, unless already confirmed), then the fee on the
                        // first event of an attempt (0x503430 against 0x5012D0 → 857)
                        if let Some(CarChoice::Owned(i, _)) = choice
                            && !self.career.eligible_garage(&t).contains(&i)
                        {
                            self.dialog = Some((813, false));
                            return None;
                        }
                        if !self.reset_confirmed && self.career.partial_elsewhere(&self.tables, t.id) {
                            self.dialog = Some((858, true));
                            return None;
                        }
                        let event = match self.career.enter(&self.tables, &t) {
                            Ok(e) => e,
                            Err(nfs_game::career::CareerError::NoMoney { .. }) => {
                                self.dialog = Some((857, false));
                                return None;
                            }
                            Err(nfs_game::career::CareerError::NoEligibleCar { .. }) => {
                                self.dialog = Some((813, false));
                                return None;
                            }
                            Err(e) => {
                                self.message = e.to_string().to_uppercase();
                                return None;
                            }
                        };
                        self.reset_confirmed = false;
                        (
                            t,
                            event,
                            match choice {
                                Some(CarChoice::Owned(i, _)) => Some(i),
                                _ => None,
                            },
                        )
                    }
                };
                let req = self.event_request(car, &t, event, garage_idx, finale.is_some(), false);
                // the launch's `SaveInfo` (0x4D52EA), after the fee and 0x503940
                self.save();
                req
            }
            None => {
                self.message = "NO CAR AVAILABLE".into();
                None
            }
        }
    }

    /// **`PLAYGAME` on the layout front end for `Game.GameType` 4** — `prerace.lay` or
    /// `tournament.lay`, both `canplaygame=2`. The handler 0x50CE90's single-player tail
    /// (0x50D25E…0x50D274) runs the start check **0x4D5730** and races only when it passes:
    ///
    /// * the car is `OwnedCars.Car` (0x4D5A01…0x4D5A1B, the garage selection `garage_idx`), tested
    ///   against the selected tournament `[0x691E6C]` by 0x504A70 → one-button **813** and no race
    ///   (0x4D5A88);
    /// * 0x504B60, another tournament part-way through, unless `[screen+0x174]` already said yes →
    ///   two-button **858** (0x4D5AE8);
    /// * type 4 only (0x4D58FB): when no event of the tournament is done yet
    ///   (`0x503370 − 0x504300 == 0`, 0x4D591F) the fee 0x503430 against the money 0x5012D0 —
    ///   short → one-button **857** (0x4D5967), else `0x5012E0(−fee)` (0x4D5B42); then **0x503940**,
    ///   the current tournament and event `[0x5DB76C]` / `[0x5DB768]` = the selected ones
    ///   `[0x691E6C]` / `[0x691E7C]`.
    ///
    /// Then the launch 0x4ED5D0 builds the race from them ([`Self::event_request`]). The event
    /// is `[0x691E7C]`:
    /// `Tournament.RemainingTracks` row `row`'s undone event (0x504F00, `Career::enter_event`). The
    /// dialogs are raised through `self.dialog`.
    pub fn start_selected_event(&mut self, garage_idx: usize, row: usize) -> Option<StartRequest> {
        let t = self.current()?.clone();
        let owned = self.career.garage.get(garage_idx)?;
        let car = self.tables.car(owned.car_id)?.clone();
        if !self.career.eligible_garage(&t).contains(&garage_idx) {
            self.dialog = Some((813, false));
            return None;
        }
        if !self.reset_confirmed && self.career.partial_elsewhere(&self.tables, t.id) {
            self.dialog = Some((858, true));
            return None;
        }
        let event = match self.career.enter_event(&self.tables, &t, row) {
            Ok(e) => e,
            Err(CareerError::NoMoney { .. }) => {
                self.dialog = Some((857, false));
                return None;
            }
            Err(e) => {
                tracing::info!(error = %e, "tournament event not started");
                return None;
            }
        };
        self.reset_confirmed = false;
        let req = self.event_request(car, &t, event, Some(garage_idx), false, true);
        // **the launch saves the profile after 0x503940** (0x4D5B4A sets the running pair, then
        // the front end's shut-down runs `SaveInfo` at 0x4D52EA), so the file holds the event
        // being raced. *(Until 2026-09-26 the port saved before the event was pending, writing
        // the −1s.)*
        self.save();
        req
    }

    /// A tournament event's race, as 0x4ED5D0 sets it up for game type 4 (and 10 for a finale):
    /// the event's track, laps and direction (0x4FC0B0), the seven opponents of the record at the
    /// event's difficulty (0x4EBDA0), the grid by the standings (0x4ED040) and the traffic. Marks
    /// the event pending, and for a tournament sets `TOURNAMENTLASTRACE` / `TOURNAMENTENDOFERA`
    /// as the launch does (0x4ECAFD…0x4ECB5B).
    fn event_request(&mut self, car: CarCatalogEntry, t: &Tournament, event: usize, garage_idx: Option<usize>, finale: bool, layout: bool) -> Option<StartRequest> {
        let ev = &t.events[event];
        let Some(track) = self.tables.track(ev.track_id) else {
            self.message = format!("UNKNOWN TRACK {}", ev.track_id);
            return None;
        };
        let paint = self.paint_for(
            &car,
            garage_idx
                .map(|i| self.career.garage[i].paint_set as usize)
                .unwrap_or(0),
        );
        // a garage car in a saved custom colour wears that, `FECar+0x674` (`OwnedCar::custom`)
        let paint = match garage_idx.and_then(|i| self.career.garage[i].custom[0]) {
            Some(c) => [(c >> 16) as u8, (c >> 8) as u8, c as u8],
            None => paint,
        };
        // **The grid is the standings.** 0x4ED040's mode-4 arm gives racer slot `i` the
        // `STARTINGPOS` `0x503660(tid, i)` — slot 0 the player, slot `i` opponent record
        // `i`. With no event of the attempt done that function returns **8** for the
        // player and `i` for opponent `i` (0x503750, 0x503745), the usual "player last";
        // after that it is each driver's 1-based rank in the table, so the leader starts
        // on pole. `None` for a finale, which is not a tournament of the standings.
        let ranks: Option<[u32; 8]> = (!self.career.state(t.id).events.is_empty() && !finale).then(|| {
            let mut r = [8u32; 8];
            for (pos, row) in self.career.standings(&t).iter().enumerate() {
                if row.0 < 8 {
                    r[row.0] = pos as u32 + 1;
                }
            }
            r
        });
        let opponents: Vec<CareerOpponent> = t
            .opponents
            .iter()
            .enumerate()
            .filter_map(|(slot, o)| {
                let car = self.tables.car(o.car_id as u16)?.clone();
                if car.sim.is_empty() {
                    return None;
                }
                let skill = self
                    .tables
                    .personalities
                    .iter()
                    .find(|p| p.id == o.driver)
                    .map(|p| p.skill)
                    .unwrap_or([[1.0, 0.0]; 4]);
                let c = o.colours[0];
                // opponent colour record is RGBA (bytes r g b a); an all-zero entry means "catalogue default"
                let paint = if c[..3] == [0, 0, 0] {
                    car.paint_sets
                        .first()
                        .map(|p| p.colours[0])
                        .unwrap_or([0x20, 0x40, 0xC0])
                } else {
                    [c[0], c[1], c[2]]
                };
                Some(CareerOpponent {
                    car,
                    paint,
                    name: self.tables.driver_name(o.driver),
                    difficulty: ev.difficulty,
                    skill,
                    grid: ranks.map(|r| r[slot + 1] - 1),
                    personality: o.driver,
                    parts: o.parts.iter().filter_map(|id| self.tables.parts.iter().find(|p| p.id == *id).cloned()).collect(),
                })
            })
            .collect();
        // 0x4ECAFD…0x4ECB5B, before the race: the last race when one event is left undone
        // (0x504300), the end of the era beside it. `[0x691E81]` keeps whatever the last 0x503AD0
        // left in it until the race is filed. A finale (type 10) leaves the pair alone; the way
        // back from it does not read them.
        if !finale {
            let (last_race, end_of_era) = self.career.tournament_flags(&self.tables, t);
            self.race_flags = TournamentRaceFlags { last_race, end_of_era, era_changed: self.race_flags.era_changed };
        }
        self.pending = Some(PendingEvent {
            tournament: t.id,
            event,
            finale,
            garage_idx,
            mission: None,
            layout,
        });
        let parts: Vec<Part> = garage_idx
            .map(|i| {
                self.career
                    .installed_parts(&self.tables, i)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        tracing::info!(
            tournament = t.id,
            event,
            finale,
            track = track.file,
            laps = ev.laps,
            backwards = ev.backwards,
            car = car.model,
            opponents = opponents.len(),
            parts = parts.len(),
            "career event start"
        );
        Some(StartRequest {
            track_file: track.file.clone(),
            car,
            paint,
            laps: ev.laps.max(1) as u32,
            backwards: ev.backwards,
            mirror: ev.direction_flag != 0,
            opponents,
            parts,
            mission: None,
            traffic: match &t.custom_traffic {
                Some(ids) => ids.len() as u32,
                None => CAREER_TRAFFIC[(t.traffic_level as usize).min(3)],
            },
            player_grid: ranks.map(|r| r[0] - 1),
            traffic_ids: t
                .custom_traffic
                .as_ref()
                .map(|ids| ids.iter().map(|id| *id as u16).collect()),
            skill: t.difficulty_level.min(2) as u8,
        })
    }

    /// CLI smoke helper: buy the cheapest eligible car of the selected tournament if the garage
    /// holds none.
    pub fn ensure_car(&mut self) {
        let choices = self.car_choices();
        if choices
            .iter()
            .any(|c| matches!(c, CarChoice::Owned(..) | CarChoice::Loaned(_)))
        {
            return;
        }
        if let Some(CarChoice::Buy(id, _)) = choices
            .iter()
            .filter(|c| matches!(c, CarChoice::Buy(..)))
            .min_by_key(|c| {
                if let CarChoice::Buy(_, p) = c {
                    *p
                } else {
                    u32::MAX
                }
            })
            .cloned()
        {
            let _ = self
                .career
                .buy_car(&self.tables, id, 0, nfs_game::career::Trim::default())
                .map_err(|e| tracing::warn!(error = %e, "ensure_car"));
        }
    }

    fn paint_for(&self, car: &CarCatalogEntry, set: usize) -> [u8; 3] {
        car.paint_sets
            .get(set % car.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10])
    }

    /// Race over: `order` = finishing order of driver indices (0 = player, 1..=7 = opponents in
    /// tournament order), `times[driver]` = finishing time in ticks (0 = did not finish).
    /// The car comes back from a race with the damage it took (`Career::record_damage`).
    pub fn park(&mut self, zones: [f32; 10]) {
        if let Some(i) = self.pending.as_ref().and_then(|p| p.garage_idx) {
            self.career.record_damage(&self.tables, i, zones);
        }
    }

    /// A race left from the pause menu (`[0x657E34]` set). The front end still runs the race end
    /// 0x4EDD00 on its way back (0x4D3EEC, before it tests the flag at 0x4D3EF1), and for a
    /// tournament race (`[0x690AD4] == 4`) that is more than filing nothing: 0x4EDD40 (0x4EDD0E)
    /// puts the race car's damage — folded into its parts by `Race_finalize(1)`'s 0x493CF0,
    /// which the quit runs too (0x41574E) — back on the owned car, 0x4EDF00's abort arm
    /// (0x4EDF11…0x4EDF2D) only resets the current event (0x503A50, 0x503AD0), and 0x4F7DB0 still
    /// ticks the used-car market with the year (0x4EDD1D…0x4EDD2C). *(Until the audit of
    /// 2026-09-26 the port dropped the damage and skipped the market.)* Returns whether the race
    /// was a tournament race.
    pub fn abort(&mut self, zones: Option<[f32; 10]>) -> bool {
        let Some(p) = self.pending.take() else { return false };
        if p.mission.is_some() || p.finale {
            return false;
        }
        if let (Some(i), Some(z)) = (p.garage_idx, zones) {
            self.career.record_damage(&self.tables, i, z);
        }
        // 0x4F7DB0 seeds the front end's generator from the clock (0x5A17EB → 0x538110)
        let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u32).unwrap_or(1) | 1;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed >> 8
        };
        self.career.generate_offers(&self.tables, &mut rand);
        self.career.reprice_market(&self.tables);
        self.career.refresh_market(&self.tables, &mut rand);
        // 0x503AD0 clears the era-changed byte `[0x691E81]` (0x503B01) and files nothing
        self.race_flags.era_changed = false;
        // 0x503A50 at 0x4EDF1A: the running pair back to −1
        (self.side.tournament.current_event, self.side.tournament.current_tournament) = (-1, -1);
        self.save();
        true
    }

    pub fn finish(&mut self, order: &[usize], times: &[u64; 8], clean: &[bool; 8]) {
        let Some(p) = self.pending.take() else { return };
        if p.mission.is_some() {
            // missions are settled through `finish_mission`
            self.pending = Some(p);
            return;
        }
        if p.finale {
            self.message = match self.career.record_finale(&self.tables, order) {
                Some((true, car)) => format!(
                    "FINALE WON: {} IS YOURS",
                    self.tables
                        .car(car)
                        .map(|c| c.display_name.to_string())
                        .unwrap_or_default()
                ),
                Some((false, _)) => "FINALE LOST".into(),
                None => String::new(),
            };
            self.save();
            self.t_idx = 0;
            self.car_sel = 0;
            return;
        }
        let Some(t) = self.tables.tournament(p.tournament).cloned() else {
            return;
        };
        let o = self
            .career
            .record_event(&self.tables, &t, p.event, order, times, clean, p.garage_idx);
        // 0x503A50 at 0x4EE076, once the result is filed through the pair: −1 again
        (self.side.tournament.current_event, self.side.tournament.current_tournament) = (-1, -1);
        // used-car offers are refreshed after every career race (0x4F7DB0)
        let mut seed = (o.result.position as u32).wrapping_mul(2_654_435_761)
            ^ (times[0] as u32).wrapping_mul(40_503)
            ^ 0x9E37_79B9;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed >> 8
        };
        self.career.generate_offers(&self.tables, &mut rand);
        // 0x4F7DB0 then re-prices every market car at its current value (0x4F7E63) and calls
        // 0x4F80C0 to top the market up — so the used-car lot fills after the first career race,
        // not when the career is created
        self.career.reprice_market(&self.tables);
        self.career.refresh_market(&self.tables, &mut rand);
        let mut msg = format!("RESULT: P{}  PRIZE ${}", o.result.position, o.result.prize);
        if o.result.bonus > 0 {
            msg += &format!("  BONUS ${}", o.result.bonus);
        }
        if let Some(rank) = o.final_rank {
            msg += &format!("   TOURNAMENT: RANK {rank}  ${}", o.tournament_prize);
            if o.reset {
                msg += "   (RANK > 3: TOURNAMENT RESET)";
            }
        }
        if o.era_advanced {
            msg += &format!("   {:?} ERA COMPLETE", t.era()).to_uppercase();
        }
        self.message = msg;
        // 0x505800 reads what 0x503AD0 left in `[0x691E81]`: cleared at 0x503B01, set at 0x503C69
        // when the era moved on
        self.race_flags.era_changed = o.era_advanced;
        self.save();
        // **The selected tournament `[0x691E6C]` moves only when one is completed** (rank ≤ 3 in
        // an era tournament): 0x503C76…0x503CA8 walks the records of the career's era, now the
        // next one when this closed it, and takes the first visible one not completed yet, else
        // the last completed one (0 when the era has none). A reset (rank > 3, 0x503CB3), a club
        // event (0x503D23) and an event that is not the last (0x503CD3) leave it where it was.
        // (Until 2026-09-25 the port moved to the first open tournament after every event.)
        // The layout front end then re-picks the row on its way back all the same: 0x505570 at
        // 0x4D5267 (`refresh_tournament`) — this pick is what the text menu keeps.
        if o.final_rank.is_some() && !o.reset && !t.is_club() {
            let list = self.tournaments();
            let mut pick = 0;
            for (i, x) in list.iter().enumerate() {
                if self.career.availability(&self.tables, x) == Availability::Open {
                    pick = i;
                    break;
                }
                pick = i;
            }
            self.t_idx = pick;
        }
        // the text screen's car row indexes `car_choices`, which the tournament decides; the
        // layout front end's is `OwnedCars.Car`, which the race end does not move
        if !p.layout {
            self.car_sel = 0;
        }
    }

    pub fn sprites(&self, f: &BitmapFont, active: Option<u8>) -> Vec<Sprite> {
        if self.factory {
            let mut out = Vec::new();
            self.factory_sprites(f, active, &mut out);
            return out;
        }
        let grey = [0.85, 0.85, 0.85, 1.0];
        let dim = [0.55, 0.55, 0.55, 1.0];
        let sel = |r: u8| {
            if active == Some(r) {
                [1.0, 0.75, 0.1, 1.0]
            } else {
                grey
            }
        };
        let mut out = f.sprites(
            &format!(
                "{}   {}   ${}   GARAGE {}",
                self.tables.strings.text(self.career.era.string_id()),
                self.career.year,
                self.career.money,
                self.career.garage.len()
            ),
            [0.08, 0.20],
            0.04,
            [0.9, 0.9, 1.0, 1.0],
        );
        if let Some(fin) = &self.career.finale
            && let Some(t) = self.tables.tournament(fin.tournament)
        {
            let car = self
                .tables
                .car(t.prize_car)
                .map(|c| c.display_name.to_string())
                .unwrap_or_default();
            out.extend(f.sprites(
                &format!(
                    "ERA FINALE  < {} >",
                    self.tables.tournament_title(t).to_string()
                ),
                [0.08, 0.30],
                0.045,
                sel(0),
            ));
            out.extend(f.sprites(
                &format!(
                    "WIN IT IN THE LOANED {car} AND KEEP THE CAR   ({} LAPS)",
                    t.events.first().map(|e| e.laps).unwrap_or(5)
                ),
                [0.12, 0.36],
                0.03,
                dim,
            ));
            out.extend(f.sprites(
                &format!("CAR  < {car} >  (LOANED)"),
                [0.08, 0.54],
                0.045,
                sel(1),
            ));
            out.extend(f.sprites("ENTER RACE   BACKSPACE DECLINE", [0.08, 0.60], 0.035, dim));
            if !self.message.is_empty() {
                out.extend(f.sprites(&self.message, [0.08, 0.66], 0.035, [0.4, 1.0, 0.5, 1.0]));
            }
            return out;
        }
        let list = self.tournaments();
        let Some(t) = self.current() else {
            out.extend(f.sprites("NO TOURNAMENTS", [0.08, 0.30], 0.045, grey));
            return out;
        };
        let st = self.career.state(t.id);
        let status = match self.career.availability(&self.tables, t) {
            Availability::Completed => format!("DONE  RANK {}", st.final_rank.unwrap_or(0)),
            Availability::Locked => "LOCKED".to_string(),
            Availability::Open if st.entered => {
                format!("EVENT {}/{}", st.events.len() + 1, t.events.len())
            }
            Availability::Open => format!("FEE ${}  {} EVENTS", t.entry_fee, t.events.len()),
        };
        let era_tag = if self.club {
            format!(
                "{} ",
                self.tables
                    .strings
                    .text(t.era().string_id())
                    .replace(" ERA", "")
            )
        } else {
            String::new()
        };
        out.extend(f.sprites(
            &format!(
                "{} {}/{}  < {}{} >",
                if self.club {
                    "CLUB EVENT"
                } else {
                    "TOURNAMENT"
                },
                self.t_idx + 1,
                list.len(),
                era_tag,
                self.tables.tournament_title(t).to_string()
            ),
            [0.08, 0.30],
            0.045,
            sel(0),
        ));
        out.extend(
            f.sprites(
                &format!(
                    "{}   {}",
                    status,
                    self.tables
                        .strings
                        .text(t.restriction_string_id)
                        .to_string()
                ),
                [0.12, 0.36],
                0.03,
                dim,
            ),
        );
        if let Some(e) = self.career.next_event(t).and_then(|i| t.events.get(i)) {
            out.extend(f.sprites(
                &format!(
                    "NEXT: {}   1ST ${}  BONUS ${}",
                    self.tables.track_name(e.track_id).to_string(),
                    e.prize[0],
                    e.bonus
                ),
                [0.12, 0.41],
                0.03,
                dim,
            ));
        }
        if st.entered && !st.completed {
            let standings = self.career.standings(t);
            let pts: Vec<String> = standings
                .iter()
                .take(4)
                .map(|(d, p, ..)| {
                    format!(
                        "{} {}",
                        if *d == 0 {
                            "YOU".to_string()
                        } else {
                            t.opponents
                                .get(d - 1)
                                .map(|o| self.tables.driver_name(o.driver).to_string())
                                .unwrap_or_default()
                        },
                        p
                    )
                })
                .collect();
            out.extend(f.sprites(
                &format!("POINTS: {}", pts.join("  ")),
                [0.12, 0.46],
                0.03,
                dim,
            ));
        }
        let choices = self.car_choices();
        let car_line = match choices.get(self.car_sel.min(choices.len().saturating_sub(1))) {
            Some(CarChoice::Owned(i, id)) => {
                let name = self
                    .tables
                    .car(*id)
                    .map(|c| c.display_name.to_string())
                    .unwrap_or_default();
                let value = self.career.car_value(&self.tables, *i).unwrap_or(0);
                let car = &self.career.garage[*i];
                match (car.offer, car.asking_price) {
                    (Some(o), _) => format!("CAR  < {name} >  OFFER ${o}  (BACKSPACE = ACCEPT)"),
                    (None, Some(a)) => {
                        format!("CAR  < {name} >  LISTED AT ${a}  (BACKSPACE = WITHDRAW)")
                    }
                    (None, None) => {
                        format!("CAR  < {name} >  (GARAGE, VALUE ${value}, BACKSPACE = SELL)")
                    }
                }
            }
            Some(CarChoice::Buy(id, price)) => format!(
                "BUY  < {} >  ${}",
                self.tables
                    .car(*id)
                    .map(|c| c.display_name.to_string())
                    .unwrap_or_default(),
                price
            ),
            Some(CarChoice::Loaned(id)) => format!(
                "CAR  < {} >  (LOANED FOR THE FINALE)",
                self.tables
                    .car(*id)
                    .map(|c| c.display_name.to_string())
                    .unwrap_or_default()
            ),
            None => "NO ELIGIBLE CAR".to_string(),
        };
        out.extend(f.sprites(&car_line, [0.08, 0.54], 0.045, sel(1)));
        let parts = self.part_choices();
        let part_line = match (
            self.selected_garage(),
            parts.get(self.part_sel.min(parts.len().saturating_sub(1))),
        ) {
            (Some(i), Some(p)) => {
                let installed = self.career.garage[i].parts.contains(&p.id);
                let mark = if installed {
                    "  (INSTALLED)".to_string()
                } else if self.basket.contains(&p.id) {
                    "  (IN LIST)".to_string()
                } else {
                    String::new()
                };
                let total = if !self.basket.is_empty() {
                    format!(
                        "   LIST {} = ${}  (BACKSPACE = BUY)",
                        self.basket.len(),
                        self.basket_total()
                    )
                } else {
                    let kinds: Vec<u16> = (1..=10).collect();
                    match self.career.repair_cost(&self.tables, i, &kinds) {
                        0 => String::new(),
                        c => format!("   REPAIR ${c}  (BACKSPACE)"),
                    }
                };
                format!(
                    "PART {}/{}  < {}: {} >  ${}{}{}",
                    self.part_sel + 1,
                    parts.len(),
                    p.kind_name.to_string(),
                    self.tables.strings.text(p.name_string_id).to_string(),
                    self.career.part_buy_price(p),
                    mark,
                    total
                )
            }
            (Some(_), None) => "PART  -  NONE FOR THIS CAR".to_string(),
            (None, _) => "PART  -  (OWNED CARS ONLY)".to_string(),
        };
        out.extend(f.sprites(&part_line, [0.08, 0.60], 0.035, sel(2)));
        if !self.message.is_empty() {
            out.extend(f.sprites(&self.message, [0.08, 0.66], 0.035, [0.4, 1.0, 0.5, 1.0]));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CREATEPLAYERDONE` / `SETNAMETHENBACK` / `REMOVEPLAYER` over their three helpers:
    /// 0x4DE5B0 refuses a taken name, `LoadInfo` reads the file back, 0x4DE8A0 deletes it.
    #[test]
    fn profiles_are_made_loaded_and_removed() {
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let dir = std::env::temp_dir().join(format!("nfs-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ui = CareerUi::load(&paths).unwrap();
        ui.save_dir = dir.clone();
        ui.save_path = dir.join("career.json");
        ui.career.money = 1;
        assert!(ui.create_profile("Hans", 8));
        assert!(dir.join("8Hans.sav").exists());
        assert_eq!(ui.career.money, 11_000, "the pristine career");
        assert!(!ui.create_profile("Hans", 3), "a taken name, whatever the picture");
        ui.career.money = 5;
        let p = Profile::new("Hans", 8);
        assert!(ui.switch_profile(&p));
        assert_eq!(ui.career.money, 11_000);
        assert_eq!(nfs_game::save::read_player_lst(&dir).as_deref(), Some("8Hans"));
        nfs_game::save::delete_profile(&dir, &p).unwrap();
        assert!(!ui.switch_profile(&p), "806: nothing to load");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 0x506AA0 / 0x4FD1A0: `Tournament.EventType` has one row per era reached, the club lists
    /// alone once the Evolution is over — and then its index 0 is the classic club list
    /// (0x50542B). 0x505570 → 0x5045C0 puts the screen on the first tournament under way.
    #[test]
    fn event_type_rows_follow_the_era() {
        use nfs_formats::career::Era;
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let mut ui = CareerUi::load(&paths).unwrap();
        ui.career = Career::default();
        assert_eq!(ui.event_type_rows(), &[470]);
        ui.career.era = Era::Golden;
        assert_eq!(ui.event_type_rows(), &[470, 471]);
        ui.career.era = Era::Modern;
        assert_eq!(ui.event_type_rows(), &[470, 471, 472]);
        ui.career.evolution_complete = true;
        assert_eq!(ui.event_type_rows(), &[471, 472, 473]);
        let classic_clubs: Vec<i16> = ui.tables.club_tournaments_of(Era::Classic).iter().map(|t| t.id).collect();
        let listed: Vec<i16> = ui.event_type_list(0).iter().map(|t| t.id).collect();
        assert_eq!(listed, classic_clubs);
        ui.career.evolution_complete = false;
        ui.career.era = Era::Classic;
        let era: Vec<i16> = ui.event_type_list(0).iter().map(|t| t.id).collect();
        assert!(era.len() > 1);
        // the second tournament of the era under way: the refresh lands on it
        let second = ui.event_type_list(0)[1].clone();
        let r = nfs_game::career::EventResult { position: 1, prize: 0, bonus: 0, points: [0; 8], earned: [0; 8], times: [0; 8], event: Some(0) };
        ui.career.tournaments.entry(second.id).or_default().events.push(r);
        ui.refresh_tournament(0);
        assert_eq!(ui.current().map(|t| t.id), Some(second.id));
        assert!(!ui.club);
    }

    /// The picture a player clicks on `createplayer` (`[0x5D8F2C]`, 0x4CF940) is what
    /// `CREATEPLAYERDONE` hands 0x4DE5B0 (0x510874): it becomes `[0x5DB748]` (0x4DE82F), so it is
    /// both the digit of `<N><name>.sav` and `PlayerInfo`'s picture field, and the profile list
    /// reads it back out of the file name. `savegame.md` §2.2, `controls.md` §4.162.
    #[test]
    fn the_chosen_picture_is_the_file_digit_and_the_saved_field() {
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let dir = std::env::temp_dir().join(format!("nfs-picture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ui = CareerUi::load(&paths).unwrap();
        ui.save_dir = dir.clone();
        ui.save_path = dir.join("career.json");
        assert!(ui.create_profile("Greta", 5));
        let file = dir.join("5Greta.sav");
        assert!(file.exists());
        let sav = nfs_formats::sav::SaveFile::parse(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(sav.player.as_ref().map(|p| p.picture), Some(5));
        let listed = nfs_game::save::list_profiles(&dir);
        assert_eq!(listed, vec![Profile::new("Greta", 5)]);
        assert!(ui.switch_profile(&listed[0]));
        assert_eq!(ui.profile.picture, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `Tournament`'s front-end dwords (`savegame.md` §4.3): the selected tournament `[0x691E6C]`
    /// is the record's index in the table and comes back as the screen's row; the next event
    /// `[0x691E7C]` comes back as the `Tournament.RemainingTracks` row that stands for it; the
    /// running pair is −1 unless a tournament event was launched (0x503940 at 0x4D5B4A).
    #[test]
    fn the_tournament_selection_rides_in_the_save() {
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let dir = std::env::temp_dir().join(format!("nfs-tourfe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ui = CareerUi::load(&paths).unwrap();
        ui.save_dir = dir.clone();
        ui.save_path = dir.join("career.json");
        ui.career = Career::default();
        assert!(ui.tournaments().len() >= 2, "the classic era lists several tournaments");
        ui.t_idx = 1;
        let t = ui.current().unwrap().clone();
        let table_index = ui.tables.tournaments.iter().position(|x| x.id == t.id).unwrap() as u32;
        ui.side.tournament.next_event = ui.career.remaining_event(&t, 1).unwrap() as u32;
        ui.save();
        let sav = nfs_formats::sav::SaveFile::parse(&std::fs::read(ui.profile.path(&dir)).unwrap()).unwrap();
        let tm = sav.tournament.as_ref().unwrap();
        assert_eq!(tm.selected, table_index);
        assert_eq!(tm.remaining_tracks, ui.side.tournament.next_event);
        assert_eq!((tm.current_event, tm.current_tournament), (-1, -1), "nothing launched");
        // …and back: the screen's row and the remaining-events row
        ui.t_idx = 0;
        let fe = nfs_game::save::side_from_sav(&sav).tournament;
        assert_eq!(ui.apply_tournament_fe(fe), Some(1));
        assert_eq!(ui.t_idx, 1);
        // a pair a file carried (the launch's save, 0x4D52EA after 0x503940) is only globals: no
        // race comes back from it, and the next save writes it again (0x503A50 is the only reset)
        ui.side.tournament.current_event = 2;
        ui.side.tournament.current_tournament = table_index as i32;
        ui.save();
        assert!(ui.pending.is_none(), "not turned back into a race");
        let sav = nfs_formats::sav::SaveFile::parse(&std::fs::read(ui.profile.path(&dir)).unwrap()).unwrap();
        let tm = sav.tournament.as_ref().unwrap();
        assert_eq!((tm.current_event, tm.current_tournament), (2, table_index as i32));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_classic_tournament_flow() {
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let tmp = std::env::temp_dir().join(format!("nfs-career-ui-{}.json", std::process::id()));
        unsafe { std::env::set_var("NFS5_CAREER_SAVE", &tmp) };
        let mut ui = CareerUi::load(&paths).unwrap();
        // the profile's `.sav` and `player.lst` go to a scratch dir, never the real save dir
        let dir = std::env::temp_dir().join(format!("nfs-career-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ui.save_dir = dir.clone();
        ui.save_path = tmp.clone();
        ui.career = Career::default();
        assert_eq!(ui.career.money, 11_000);
        let list = ui.tournaments();
        assert_eq!(list[0].id, 1, "356 Challenge first");
        assert!(
            list.iter().all(|t| !t.is_hidden()),
            "finales are not listed"
        );
        // no car yet: the car row offers the eligible 356s at the dealer
        ui.row = 1;
        let choices = ui.car_choices();
        assert!(matches!(choices[0], CarChoice::Buy(_, _)));
        let CarChoice::Buy(id, price) = choices
            .iter()
            .find(|c| matches!(c, CarChoice::Buy(_, 11_000)))
            .unwrap()
            .clone()
        else {
            unreachable!()
        };
        assert_eq!(ui.career.year, 1950);
        ui.car_sel = choices
            .iter()
            .position(|c| *c == CarChoice::Buy(id, price))
            .unwrap();
        assert!(ui.activate().is_none(), "buying does not start a race");
        assert_eq!(ui.career.money, 0);
        assert_eq!(ui.career.garage.len(), 1);
        assert!(matches!(
            ui.car_choices()[ui.car_sel],
            CarChoice::Owned(0, _)
        ));
        // parts row: the 356 has engine-group parts on offer
        ui.row = 2;
        let n_parts = ui.part_choices().len();
        assert!(n_parts > 10, "{n_parts} parts for the 356");
        ui.career.money = 100_000;
        let pid = ui.part_choices()[0].id;
        let price = ui.career.part_buy_price(&ui.part_choices()[0].clone());
        // Enter fills the transaction list, Backspace pays for it (the original's parts.lay flow)
        assert!(ui.activate().is_none());
        assert_eq!(ui.basket, vec![pid]);
        assert_eq!(ui.basket_total(), price);
        // a new car carries copies of its stock parts (0x4EE9A0)
        let stock_fitted = ui.career.garage[0].parts.clone();
        assert!(!stock_fitted.is_empty());
        assert!(ui.career.spares.is_empty(), "nothing is bought before the list is accepted");
        assert!(ui.activate().is_none());
        assert!(ui.basket.is_empty(), "Enter again takes it back out");
        assert!(ui.activate().is_none());
        assert_eq!(ui.career.garage[0].parts, stock_fitted);
        ui.back();
        // bought and installed: the part it replaces (0x518510) goes to the inventory
        assert!(ui.career.garage[0].parts.contains(&pid));
        let fitted = ui.career.garage[0].parts.len();
        assert_eq!(fitted + ui.career.spares.len(), stock_fitted.len() + 1);
        assert_eq!(ui.career.money, 100_000 - price);
        assert!(ui.basket.is_empty());
        assert!(ui.message.starts_with("INSTALLED"));
        ui.row = 1;
        let req = ui.activate().expect("event starts");
        // the race gets every installed part, the stock copies with the bought one (0x49CB50)
        assert_eq!(req.parts.len(), fitted);
        assert_eq!(req.opponents.len(), 7);
        assert!(["farmland", "coastal"].contains(&req.track_file.as_str()));
        assert!(
            req.opponents
                .iter()
                .all(|o| (0.3..=1.0).contains(&o.difficulty))
        );
        assert_eq!(req.laps, 1, "point-to-point stage");
        assert!(ui.pending.is_some());
        ui.finish(&[0, 1, 2, 3, 4, 5, 6, 7], &[6400; 8], &[true; 8]);
        assert!(ui.pending.is_none());
        let st = ui.career.state(1);
        assert_eq!(st.events.len(), 1);
        assert_eq!(st.events[0].position, 1);
        assert_eq!(ui.career.garage[0].mileage, 3000);
        assert!(ui.career.money > 0);
        assert!(ui.message.starts_with("RESULT: P1"));
        assert_eq!(Career::load(&tmp).unwrap(), ui.career);
        let _ = std::fs::remove_file(tmp);
    }
}

/// Word-wrap `text` to lines of at most `width` characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}
