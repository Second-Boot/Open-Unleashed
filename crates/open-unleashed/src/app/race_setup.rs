//! **The launch's race options in one place**. The original fills a new option buffer
//! for every race (0x4ED5D0 → 0x4EB470 → 0x4ECDF0 → 0x4ECF00), so nothing of the last race's
//! options survives there; the port keeps the same options as loose `App` fields, and each launch
//! path (replay, test drive, career event, knockout round, menu race, network race, command line)
//! used to write its own subset by hand — a field one path forgot kept the last race's value
//! (the knockout arm did not write `player_car_id`; EV1: a test drive kept the last skill).
//!
//! A [`RaceSetup`] names every one of those fields, and each path builds the whole struct: a field
//! the path deliberately leaves alone is written as `kept.<field>` (the value the race had before,
//! [`App::race_setup`]), so what a launch keeps is visible at the call site instead of implied by
//! an absent assignment. [`App::apply_race_setup`] writes them back. Plain field writes only: no
//! RNG, no load, no log — where a path wrote them is where it applies them, so the order of side
//! effects in the launch (and in `start_race`) is untouched.

use crate::*;

/// The player car's colours as the race draws them: the body colour the race compares against
/// (`App::paint`), the owned / custom paint (`App::owned_paint`) and the race number
/// (`App::owned_decal`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayerLook {
    pub(crate) paint: [u8; 3],
    pub(crate) owned_paint: Option<Paint>,
    pub(crate) owned_decal: Option<i8>,
}

impl PlayerLook {
    /// A catalogue colour with no owned paint and no race number (a replay, a career event).
    pub(crate) fn plain(paint: [u8; 3]) -> Self {
        Self { paint, owned_paint: None, owned_decal: None }
    }

    /// The menu's colour row of `car` (`m.paint_idx` over its paint sets, red when it has none) —
    /// or the menu's custom colour (`App::menu_custom_paint`), which is then the paint the race
    /// compares against too. No race number. The text menu's and the layout front end's single
    /// race and the text menu's knockout round all took this.
    pub(crate) fn from_menu(m: &Menu, car: &nfs_formats::fedata::CarCatalogEntry) -> Self {
        let mut look = Self::plain(car.paint_sets.get(m.paint_idx % car.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]));
        if let Some(p) = App::menu_custom_paint(m, car) {
            (look.paint, look.owned_paint) = (p.exterior, Some(p));
        }
        look
    }
}

/// The race options every launch path writes (see the module doc). Field ↔ `App` field:
/// `opponents` `num_opponents`, `laps` `num_laps`, `backwards` / `mirror` the same, `traffic_count`
/// / `traffic_ids` / `cops_enabled` / `cops_race` / `cop_count` / `damage` (`damage_on`) on
/// `App::road`, `skill` `race_skill`, `quick_knockout`, `career_opponents`, `player_grid`
/// `player_grid_override`, `look` the three paint fields.
#[derive(Debug, Clone)]
pub(crate) struct RaceSetup {
    pub(crate) opponents: u32,
    pub(crate) laps: u32,
    pub(crate) backwards: bool,
    pub(crate) mirror: bool,
    pub(crate) traffic_count: u32,
    pub(crate) traffic_ids: Option<Vec<u16>>,
    pub(crate) cops_enabled: bool,
    pub(crate) cops_race: bool,
    pub(crate) cop_count: u32,
    pub(crate) damage: bool,
    pub(crate) skill: u8,
    pub(crate) quick_knockout: bool,
    pub(crate) career_opponents: Option<Vec<CareerOpponent>>,
    pub(crate) player_grid: Option<u32>,
    pub(crate) look: PlayerLook,
}

/// A test drive's event (the tournament / finale drives, 0x4FC0B0's arms 5 and 11): laps,
/// reverse, mirror, traffic count and the tournament's own traffic models.
pub(crate) type TestDriveEvent = (u32, bool, bool, u32, Option<Vec<u16>>);

impl RaceSetup {
    /// **A playback** (`start_replay`): the recording's block (`Replay_begin` 0x413CF0's restore),
    /// its car in its recorded colour with no owned paint; `RACE_TYPE` / `FEGAME_TYPE`'s
    /// `quick_knockout` / `cops_race` / `cop_count` are `apply_replay_launch_block`'s (kept), the
    /// grid `apply_replay_net`'s (kept, written after).
    pub(crate) fn replay(s: &nfs_game::replay::ReplaySetup, kept: RaceSetup) -> Self {
        Self {
            opponents: s.opponents,
            laps: s.laps.max(1),
            backwards: s.backwards,
            mirror: s.mirror,
            traffic_count: TRAFFIC_COUNT[(s.traffic as usize).min(3)],
            traffic_ids: None,
            cops_enabled: s.cops,
            cops_race: kept.cops_race,
            cop_count: kept.cop_count,
            damage: s.damage,
            // `SKILL` travels in the block as well (0x413DCF)
            skill: s.skill,
            quick_knockout: kept.quick_knockout,
            career_opponents: None,
            player_grid: kept.player_grid,
            look: PlayerLook::plain(s.paint),
        }
    }

    /// **A test drive**, `Game.GameType` 5…8 / 11 (`start_test_drive`): 0x4FC0B0's race for 6…8 —
    /// one lap, forward, **five** traffic cars — or the event's for 5 / 11, with no opponents
    /// (0x4ECF00) and no cops. `SKILL` / `DAMAGE` are `launch_skill_damage`'s, written after (kept);
    /// the grid is left as it stands.
    pub(crate) fn test_drive(event: Option<TestDriveEvent>, look: PlayerLook, kept: RaceSetup) -> Self {
        let (laps, backwards, mirror, traffic_count, traffic_ids) = event.unwrap_or((1, false, false, 5, None));
        Self {
            opponents: 0,
            laps,
            backwards,
            mirror,
            traffic_count,
            traffic_ids,
            cops_enabled: false,
            cops_race: false,
            cop_count: 4,
            damage: kept.damage,
            skill: kept.skill,
            quick_knockout: false,
            career_opponents: None,
            player_grid: kept.player_grid,
            look,
        }
    }

    /// **A career event** — a tournament event, a finale or a Factory Driver mission
    /// (`load_career_race`): the options 0x4ED5D0 sends. **Career police**: 0x4ED0A5 gives cops
    /// only to game type 9 — a factory-driver mission — and only when its record's `+0x26` is set,
    /// and then two of them (0x4ED0BC…0x4ED0E7); a tournament has none. `DAMAGE` is
    /// `launch_skill_damage`'s, written by the callers after (kept).
    pub(crate) fn career(req: &career_ui::StartRequest, kept: RaceSetup) -> Self {
        Self {
            opponents: req.opponents.len() as u32,
            laps: req.laps,
            backwards: req.backwards,
            mirror: req.mirror,
            traffic_count: req.traffic,
            traffic_ids: req.traffic_ids.clone(),
            cops_enabled: req.mission.as_ref().is_some_and(|m| m.mission.police),
            cops_race: false,
            cop_count: 2,
            damage: kept.damage,
            skill: req.skill,
            quick_knockout: false,
            career_opponents: Some(req.opponents.clone()),
            player_grid: req.player_grid,
            look: PlayerLook::plain(req.paint),
        }
    }

    /// **A knockout round** (`knockout_round`, 0x4ED5D0 for `Game.GameType` 2 and 0x4DE150): the
    /// round's row — laps, `REVERSE` / `MIRROR`, traffic — no cops, the surviving opponents on
    /// their grid slots and the player's `.STARTINGPOS`. `SKILL` and the colour stay as the launch
    /// set them (kept).
    pub(crate) fn knockout_round(row: &KoSlot, laps: u32, damage: bool, opponents: Vec<CareerOpponent>, player_grid: u32, kept: RaceSetup) -> Self {
        Self {
            opponents: opponents.len() as u32,
            laps,
            backwards: row.backwards,
            mirror: row.mirror,
            traffic_count: TRAFFIC_COUNT[row.traffic.min(3) as usize],
            traffic_ids: None,
            cops_enabled: false,
            cops_race: false,
            cop_count: 4,
            damage,
            skill: kept.skill,
            quick_knockout: false,
            career_opponents: Some(opponents),
            player_grid: Some(player_grid),
            look: kept.look,
        }
    }

    /// **The menu's own options** for a single race / Quick Knockout (`start_from_layout`'s arm for
    /// game types 0 / 1, the text menu's `Enter`): direction, mirror, traffic level, damage, four
    /// cops' worth of slots, no career opponents and no grid override. The caller fills what its
    /// arm decides (`opponents`, `laps`, `quick_knockout`, `cops_race`, `cops_enabled`, `look`)
    /// with `..RaceSetup::menu(m, kept)`; anything it does not name is the race's as it stood.
    pub(crate) fn menu(m: &Menu, kept: RaceSetup) -> Self {
        Self {
            backwards: m.backwards,
            mirror: m.mirror,
            traffic_count: TRAFFIC_COUNT[m.traffic.min(3) as usize],
            traffic_ids: None,
            cop_count: 4,
            damage: m.damage,
            career_opponents: None,
            player_grid: None,
            ..kept
        }
    }

    /// **A network race** (`net_race_apply`, what 0x4B4B80 writes from the launch stream): the
    /// host's direction, mirror, laps (99 for a Smash Up, which counts none), damage,
    /// skill and traffic; no cops (0x4ECF00), the room's other humans as the opponents
    /// (0x4B5079's `NUMPLAYERRACECARS`), the host's drawn `.STARTINGPOS` for this machine. The
    /// rest stays as the menu arm left it (kept).
    pub(crate) fn net(r: &NetRaceOptions, traffic_ids: Option<Vec<u16>>, kept: RaceSetup) -> Self {
        Self {
            opponents: r.players.saturating_sub(1),
            laps: if r.derby { 99 } else { r.laps.max(1) },
            backwards: r.backwards,
            mirror: r.mirror,
            traffic_count: r.traffic_cars,
            traffic_ids,
            cops_enabled: false,
            cops_race: false,
            cop_count: kept.cop_count,
            damage: r.damage,
            skill: r.skill,
            quick_knockout: kept.quick_knockout,
            career_opponents: kept.career_opponents,
            player_grid: r.grid,
            look: kept.look,
        }
    }
}

/// The network launch's numbers [`RaceSetup::net`] reads (a view of `NetRace`, so the builder is
/// testable without a session).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NetRaceOptions {
    pub(crate) players: u32,
    pub(crate) laps: u32,
    pub(crate) derby: bool,
    pub(crate) backwards: bool,
    pub(crate) mirror: bool,
    pub(crate) traffic_cars: u32,
    pub(crate) damage: bool,
    pub(crate) skill: u8,
    pub(crate) grid: Option<u32>,
}

impl From<&NetRace> for NetRaceOptions {
    fn from(r: &NetRace) -> Self {
        Self {
            players: r.players,
            laps: r.laps,
            derby: r.derby,
            backwards: r.backwards,
            mirror: r.mirror,
            traffic_cars: r.traffic_cars,
            damage: r.damage,
            skill: r.skill,
            grid: r.grid,
        }
    }
}

impl App {
    /// The race options as they stand — what a launch path keeps of the last race (`kept`).
    pub(crate) fn race_setup(&self) -> RaceSetup {
        RaceSetup {
            opponents: self.num_opponents,
            laps: self.num_laps,
            backwards: self.backwards,
            mirror: self.mirror,
            traffic_count: self.road.traffic_count,
            traffic_ids: self.road.traffic_ids.clone(),
            cops_enabled: self.road.cops_enabled,
            cops_race: self.road.cops_race,
            cop_count: self.road.cop_count,
            damage: self.road.damage_on,
            skill: self.race_skill,
            quick_knockout: self.quick_knockout,
            career_opponents: self.career_opponents.clone(),
            player_grid: self.player_grid_override,
            look: PlayerLook { paint: self.paint, owned_paint: self.owned_paint, owned_decal: self.owned_decal },
        }
    }

    /// Writes every field of `rs` onto the race (plain field writes, nothing else).
    pub(crate) fn apply_race_setup(&mut self, rs: RaceSetup) {
        let RaceSetup {
            opponents,
            laps,
            backwards,
            mirror,
            traffic_count,
            traffic_ids,
            cops_enabled,
            cops_race,
            cop_count,
            damage,
            skill,
            quick_knockout,
            career_opponents,
            player_grid,
            look,
        } = rs;
        self.num_opponents = opponents;
        self.num_laps = laps;
        self.backwards = backwards;
        self.mirror = mirror;
        self.road.traffic_count = traffic_count;
        self.road.traffic_ids = traffic_ids;
        self.road.cops_enabled = cops_enabled;
        self.road.cops_race = cops_race;
        self.road.cop_count = cop_count;
        self.road.damage_on = damage;
        self.race_skill = skill;
        self.quick_knockout = quick_knockout;
        self.career_opponents = career_opponents;
        self.player_grid_override = player_grid;
        self.apply_player_look(look);
    }

    /// Writes the player car's colours (`paint`, `owned_paint`, `owned_decal`).
    pub(crate) fn apply_player_look(&mut self, look: PlayerLook) {
        self.paint = look.paint;
        self.owned_paint = look.owned_paint;
        self.owned_decal = look.owned_decal;
    }
}

/// **Field parity**: each builder against the assignments its launch path made
/// before the struct existed — copied statement for statement onto [`Old`], an `App`-shaped
/// stand-in with the same field names, from the same `kept` state (one made deliberately unlike
/// every builder's output, so a field a path forgets or invents shows).
#[cfg(test)]
mod parity {
    use super::*;

    #[derive(Debug, Clone)]
    struct Road {
        traffic_count: u32,
        traffic_ids: Option<Vec<u16>>,
        cops_enabled: bool,
        cops_race: bool,
        cop_count: u32,
        damage_on: bool,
    }

    #[derive(Debug, Clone)]
    struct Old {
        num_opponents: u32,
        num_laps: u32,
        backwards: bool,
        mirror: bool,
        road: Road,
        race_skill: u8,
        quick_knockout: bool,
        career_opponents: Option<Vec<CareerOpponent>>,
        player_grid_override: Option<u32>,
        paint: [u8; 3],
        owned_paint: Option<Paint>,
        owned_decal: Option<i8>,
        cheat_bits: u32,
    }

    impl Old {
        /// `App::race_setup`, field for field
        fn setup(&self) -> RaceSetup {
            RaceSetup {
                opponents: self.num_opponents,
                laps: self.num_laps,
                backwards: self.backwards,
                mirror: self.mirror,
                traffic_count: self.road.traffic_count,
                traffic_ids: self.road.traffic_ids.clone(),
                cops_enabled: self.road.cops_enabled,
                cops_race: self.road.cops_race,
                cop_count: self.road.cop_count,
                damage: self.road.damage_on,
                skill: self.race_skill,
                quick_knockout: self.quick_knockout,
                career_opponents: self.career_opponents.clone(),
                player_grid: self.player_grid_override,
                look: PlayerLook { paint: self.paint, owned_paint: self.owned_paint, owned_decal: self.owned_decal },
            }
        }
    }

    fn car(id: u16, colours: &[[u8; 3]]) -> nfs_formats::fedata::CarCatalogEntry {
        let mut c = nfs_formats::fedata::parse_car_catalog(&vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE]).unwrap().remove(0);
        c.id = id;
        c.paint_sets = colours.iter().map(|&k| nfs_formats::fedata::PaintSet { package: 0, colours: [k, k, k], packed: [0; 3] }).collect();
        c
    }

    fn opponent(name: &str) -> CareerOpponent {
        CareerOpponent { car: car(7, &[]), paint: [1, 2, 3], name: name.into(), difficulty: 0.5, skill: [[0.0; 2]; 4], grid: None, personality: -1, parts: Vec::new(), decal: -1 }
    }

    /// a state no builder produces as a whole: every field set to something a launch must overwrite or keep
    fn kept() -> Old {
        Old {
            num_opponents: 77,
            num_laps: 66,
            backwards: true,
            mirror: true,
            road: Road { traffic_count: 55, traffic_ids: Some(vec![9, 9]), cops_enabled: true, cops_race: true, cop_count: 44, damage_on: false },
            race_skill: 33,
            quick_knockout: true,
            career_opponents: Some(vec![opponent("stale")]),
            player_grid_override: Some(22),
            paint: [11, 11, 11],
            owned_paint: Some(Paint::from_catalogue(&car(1, &[[5, 5, 5]]), 0)),
            owned_decal: Some(3),
            cheat_bits: 0,
        }
    }

    fn menu() -> Menu {
        Menu {
            mode: 0,
            tracks: vec!["alps".into(), "coastal".into()],
            looped: vec!["monaco1".into()],
            tracks_year: 1950,
            cars: Vec::new(),
            cars2: Vec::new(),
            track_idx: 1,
            car_idx: 0,
            car_garage: false,
            merged_at_zero: false,
            car2_idx: 0,
            ko: Vec::new(),
            ko_sel: 0,
            opponents: 3,
            ko_opponents: 5,
            damage: true,
            difficulty: 1,
            opp_car: 0,
            paint_idx: 1,
            paint_custom: None,
            laps: 4,
            traffic: 2,
            cops: false,
            backwards: false,
            mirror: false,
            circuits: vec!["coastal".into()],
            looped_idx: 0,
            knockout_line: String::new(),
            row: 0,
            font: None,
        }
    }

    fn same(new: &RaceSetup, old: &Old) {
        assert_eq!(format!("{new:#?}"), format!("{:#?}", old.setup()));
    }

    #[test]
    fn replay() {
        let s = nfs_game::replay::ReplaySetup { paint: [4, 5, 6], laps: 0, opponents: 2, traffic: 9, cops: true, damage: false, backwards: false, mirror: true, skill: 2, ..Default::default() };
        let new = RaceSetup::replay(&s, kept().setup());
        // `start_replay` before (the launch block's three and the grid untouched)
        let mut o = kept();
        o.num_opponents = s.opponents;
        o.num_laps = s.laps.max(1);
        o.backwards = s.backwards;
        o.mirror = s.mirror;
        o.owned_paint = None;
        o.owned_decal = None;
        o.paint = s.paint;
        o.road.traffic_count = TRAFFIC_COUNT[(s.traffic as usize).min(3)];
        o.road.traffic_ids = None;
        o.road.cops_enabled = s.cops;
        o.road.damage_on = s.damage;
        o.race_skill = s.skill;
        o.career_opponents = None;
        same(&new, &o);
    }

    #[test]
    fn test_drive() {
        for event in [None, Some((3, true, true, 4, Some(vec![12u16, 13])))] {
            let look = PlayerLook { paint: [8, 8, 8], owned_paint: None, owned_decal: Some(5) };
            let new = RaceSetup::test_drive(event.clone(), look, kept().setup());
            // `start_test_drive` before
            let mut o = kept();
            (o.paint, o.owned_decal, o.owned_paint) = (look.paint, look.owned_decal, look.owned_paint);
            o.num_opponents = 0;
            o.num_laps = 1;
            o.backwards = false;
            o.mirror = false;
            o.road.traffic_count = 5;
            o.road.traffic_ids = None;
            if let Some((laps, rev, mir, traffic, ids)) = event {
                o.num_laps = laps;
                o.backwards = rev;
                o.mirror = mir;
                o.road.traffic_count = traffic;
                o.road.traffic_ids = ids;
            }
            o.road.cops_enabled = false;
            o.road.cop_count = 4;
            o.road.cops_race = false;
            o.career_opponents = None;
            o.quick_knockout = false;
            same(&new, &o);
        }
    }

    #[test]
    fn career() {
        let req = career_ui::StartRequest {
            track_file: "alps".into(),
            car: car(3, &[]),
            paint: [7, 7, 7],
            laps: 3,
            backwards: true,
            mirror: false,
            opponents: vec![opponent("a"), opponent("b")],
            parts: Vec::new(),
            mission: None,
            skill: 1,
            traffic: 4,
            traffic_ids: Some(vec![21]),
            player_grid: Some(6),
        };
        let new = RaceSetup::career(&req, kept().setup());
        // `load_career_race` before
        let mut o = kept();
        o.num_opponents = req.opponents.len() as u32;
        o.num_laps = req.laps;
        o.owned_paint = None;
        o.owned_decal = None;
        o.paint = req.paint;
        o.career_opponents = Some(req.opponents.clone());
        o.road.traffic_count = req.traffic;
        o.road.traffic_ids = req.traffic_ids.clone();
        o.player_grid_override = req.player_grid;
        let mission_cops = req.mission.as_ref().is_some_and(|m| m.mission.police);
        o.road.cops_enabled = mission_cops;
        o.road.cop_count = 2;
        o.race_skill = req.skill;
        o.backwards = req.backwards;
        o.mirror = req.mirror;
        o.quick_knockout = false;
        o.road.cops_race = false;
        same(&new, &o);
    }

    #[test]
    fn knockout_round() {
        let row = KoSlot { track_idx: 1, laps: 3, traffic: 7, backwards: true, mirror: true };
        let opponents = vec![opponent("x"), opponent("y"), opponent("z")];
        let new = RaceSetup::knockout_round(&row, 3, true, opponents.clone(), 2, kept().setup());
        // `knockout_round` before
        let mut o = kept();
        o.player_grid_override = Some(2);
        o.num_opponents = opponents.len() as u32;
        o.career_opponents = Some(opponents);
        o.road.traffic_count = TRAFFIC_COUNT[row.traffic.min(3) as usize];
        o.road.traffic_ids = None;
        o.road.damage_on = true;
        o.road.cops_enabled = false;
        o.road.cops_race = false;
        o.road.cop_count = 4;
        o.quick_knockout = false;
        o.num_laps = 3;
        o.backwards = row.backwards;
        o.mirror = row.mirror;
        same(&new, &o);
    }

    /// `start_from_layout`'s arm for game types 0 / 1, the old way, onto `o`
    fn old_layout_arm(o: &mut Old, m: &Menu, quick: bool) {
        o.player_grid_override = None;
        o.career_opponents = None;
        o.backwards = m.backwards;
        o.mirror = m.mirror;
        o.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
        o.road.traffic_ids = None;
        o.road.damage_on = m.damage;
        o.road.cop_count = 4;
        if quick {
            o.num_opponents = m.ko_opponents;
            o.num_laps = m.ko_opponents.max(1);
            o.road.cops_race = false;
            o.road.cops_enabled = false;
            o.quick_knockout = true;
        } else {
            let t = m.track().to_string();
            o.num_opponents = m.opponents;
            o.num_laps = fe_laps(&m.circuits, &t, m.laps);
            o.road.cops_race = o.cheat_bits & CHEAT_COPS != 0;
            o.road.cops_enabled = m.cops || o.road.cops_race;
            o.quick_knockout = false;
        }
    }

    #[test]
    fn layout_arms() {
        for (quick, cheat, cops) in [(false, 0, false), (false, CHEAT_COPS, false), (false, 0, true), (true, CHEAT_COPS, true)] {
            let m = Menu { cops, ..menu() };
            let mut o = Old { cheat_bits: cheat, ..kept() };
            // the new arm, as `start_from_layout` builds it
            let new = if quick {
                RaceSetup { opponents: m.ko_opponents, laps: m.ko_opponents.max(1), cops_race: false, cops_enabled: false, quick_knockout: true, ..RaceSetup::menu(&m, o.setup()) }
            } else {
                let cops_race = o.cheat_bits & CHEAT_COPS != 0;
                RaceSetup {
                    opponents: m.opponents,
                    laps: fe_laps(&m.circuits, m.track(), m.laps),
                    cops_race,
                    cops_enabled: m.cops || cops_race,
                    quick_knockout: false,
                    ..RaceSetup::menu(&m, o.setup())
                }
            };
            old_layout_arm(&mut o, &m, quick);
            same(&new, &o);
        }
    }

    #[test]
    fn text_menu_enter() {
        let entry = car(4, &[[1, 1, 1], [2, 2, 2]]);
        for (mode, cheat, cops, custom) in [(0, CHEAT_COPS, false, None), (0, 0, true, Some(0x00AABBCC)), (4, CHEAT_COPS, true, None)] {
            let m = Menu { mode, cops, paint_custom: custom, ..menu() };
            let mut o = Old { cheat_bits: cheat, ..kept() };
            // the new `Enter` arm (`player_grid_override` cleared at its top, as before)
            o.player_grid_override = None;
            let cops_race = m.mode == 0 && o.cheat_bits & CHEAT_COPS != 0;
            let new = RaceSetup {
                opponents: if m.mode >= 4 { m.ko_opponents } else { m.opponents },
                laps: if m.mode == 4 { m.ko_opponents.max(1) } else { fe_laps(&m.circuits, m.track(), m.laps) },
                cops_race,
                cops_enabled: m.cops || cops_race,
                quick_knockout: m.mode == 4,
                look: PlayerLook::from_menu(&m, &entry),
                ..RaceSetup::menu(&m, o.setup())
            };
            // `menu_key`'s `Enter` before
            o.num_opponents = if m.mode >= 4 { m.ko_opponents } else { m.opponents };
            o.quick_knockout = m.mode == 4;
            o.num_laps = if m.mode == 4 { m.ko_opponents.max(1) } else { fe_laps(&m.circuits, m.track(), m.laps) };
            o.backwards = m.backwards;
            o.mirror = m.mirror;
            o.career_opponents = None;
            o.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
            o.road.traffic_ids = None;
            o.road.damage_on = m.damage;
            o.road.cops_race = m.mode == 0 && o.cheat_bits & CHEAT_COPS != 0;
            o.road.cops_enabled = m.cops || o.road.cops_race;
            o.road.cop_count = 4;
            o.owned_paint = None;
            o.owned_decal = None;
            o.paint = entry.paint_sets.get(m.paint_idx % entry.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
            if let Some(p) = App::menu_custom_paint(&m, &entry) {
                (o.paint, o.owned_paint) = (p.exterior, Some(p));
            }
            same(&new, &o);
        }
    }

    #[test]
    fn net() {
        for derby in [false, true] {
            let r = NetRaceOptions { players: 3, laps: 0, derby, backwards: true, mirror: false, traffic_cars: 4, damage: true, skill: 2, grid: Some(1) };
            let ids = Some(vec![30u16, 31]);
            let new = RaceSetup::net(&r, ids.clone(), kept().setup());
            // `net_race_apply` before
            let mut o = kept();
            o.backwards = r.backwards;
            o.mirror = r.mirror;
            o.num_laps = r.laps.max(1);
            if r.derby {
                o.num_laps = 99;
            }
            o.road.damage_on = r.damage;
            o.race_skill = r.skill;
            o.road.cops_race = false;
            o.road.cops_enabled = false;
            o.road.traffic_count = r.traffic_cars;
            o.road.traffic_ids = None;
            if ids.is_some() {
                o.road.traffic_ids = ids;
            }
            o.num_opponents = r.players.saturating_sub(1);
            o.player_grid_override = r.grid;
            same(&new, &o);
        }
    }
}
