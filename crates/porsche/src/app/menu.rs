//! The race-set-up menu (`Menu`) the `--fe`-less start and the front end's game-setup screens share.

use crate::*;

/// Minimal frontend (M7 first cut): choose track and car, Enter starts the race.
/// The original's `.lay`-driven screens are not reproduced yet; this uses the catalogue tables.
pub(crate) struct Menu {
    /// 0 = quick race, 1 = evolution (career)
    pub(crate) mode: u8,
    /// `[0x691D04]`, the list every `Track<n>.Track` indexes — built by 0x4FBC10
    /// ([`nfs_game::track_list`]): `nfs5.trk`'s order, the era unlocks, no Skidpad
    pub(crate) tracks: Vec<String>,
    /// `[0x691D08]`, `LoopedTrack.Track`'s list: the circuits of the same pass (`trk+0x6E == 1`)
    pub(crate) looped: Vec<String>,
    /// the chronicle year the two lists were last built for (see `App::sync_track_lists`)
    pub(crate) tracks_year: i32,
    /// `GenericCar.Car`'s list `[0x6919EC]` — [`generic_car_list`]: kind-0 records the current
    /// `Game.GameType` admits through the `+0x204` dealer-availability flag, 0x4F0F50-sorted. The
    /// catalogue half of `zMergedList.Car`, `CARSETTINGS.CAR` and the `DealerCar.Car`/
    /// `PrizeCars.Car` fallback arms all read this one (`career.md` "0x4F3E40" audit below).
    /// *(Until this pass: `+0x204` was not read at all, so every kind-0 record showed regardless
    /// of `GameType` — the `[0x6919EC]` `+0x204` question; see [`dealer_flag_admitted`]'s
    /// doc for the resolved arms and the live original check.)*
    pub(crate) cars: Vec<nfs_formats::fedata::CarCatalogEntry>,
    /// `GenericCar2.Car`'s own list `[0x6919F4]` — **a separate rebuild from `cars`, not the same
    /// vector.** 0x4F3E40's per-record loop (`GameType == 4`, the Evolution/career context every
    /// `carcompare.lay` visit is reached through, `career.md` "SETTYPECAREERMODE: type 4") appends
    /// a kind-0 record here whenever the chronicle year has reached its first model year
    /// (0x4F4126…0x4F4156: `chronicle_year >= record+0x1F8`, no class/era/dealer-flag test at all),
    /// then sorts it with the same 0x4F0F50/0x4F1D40 exchange sort as `cars` (0x4F42B3…0x4F42B9).
    /// Rebuilt alongside the track lists (`App::rebuild_track_lists`) since both are the same
    /// 0x4F3E40 call in the original — `cars` is now rebuilt in the same place, on the same
    /// `Game.GameType`, for the same reason.
    pub(crate) cars2: Vec<nfs_formats::fedata::CarCatalogEntry>,
    pub(crate) track_idx: usize,
    pub(crate) car_idx: usize,
    /// **`zMergedList.Car`'s own current index, which half it is in.** `FEMergedCarListAccessor`
    /// keeps one index (`[this+0x18]`, its base's own `[+0x18]`); 0x4F4DF0 (`bind_mergedlist_panel`,
    /// `fe_carsetup_blocked`) tests it against list A's count (`[esi+0x28]->slot9()`) with
    /// `cur = acc->slot10()` — `setl`, `cur < n` — to tell "an owned car" from "one you do not
    /// own". The commit, **0x4F49F0** (§4.171.7 corrected below), sets this the same way it routes:
    /// `true` after a row **below** list A's count (the garage, `OwnedCars.Car`, whose own index is
    /// `Career::car_sel`), `false` after one at or past it (the catalogue, `GenericCar.Car`,
    /// `car_idx` above). *(Until 2026-09-27 the merged selection was always the catalogue's.)*
    pub(crate) car_garage: bool,
    /// **`zMergedList.Car`'s index is still the initial 0.** The accessor's base ctor 0x4BC1F0
    /// stores `[this+0x18] = 0` (0x4BC224) and `0x4FC730` (the `SETTYPE*` reset, `reset_track_rows`)
    /// puts it back to row 0; index 0 is **below list A's count whenever the player owns a car**,
    /// so 0x4F4DF0 (`cur < n`) says "owned" there and the screens show `гараж...` with the garage
    /// car, not the catalogue's `цвет` row, until a row is picked (SC 2026-10-01, the
    /// original's QuickRace opens on the owned `356`; the joiner lobby the same).
    /// While set, `App::sync_merged_zero` keeps `car_garage` equal to "list A is not empty".
    pub(crate) merged_at_zero: bool,
    /// `locationsetup.lay`'s table of knockout rounds: one row per opponent, each with its own
    /// track, laps, direction and traffic. `Knockout.TrackNumber`'s accessor (0x4BA760) asks
    /// `Game.NumKOOpponents` for the row count and answers the empty string for a row past it
    /// (0x4BA7CF), so the list is as long as the opponent count.
    pub(crate) ko: Vec<KoSlot>,
    /// which row of that table the screen's own `Knockout.Track` / `.Laps` / `.Traffic` /
    /// `.Direction` combo boxes edit
    pub(crate) ko_sel: usize,
    /// `GenericCar2.Car` — the **second** car of the compare screen, the one its
    /// `NFS5.ComboBoxSlider` cycles. `carcompare.lay` gives the left column `CarCompare1` (the car
    /// the screen was entered with) and the right column `GenericCar2.Car`, so the two are separate
    /// selections and need separate indices.
    pub(crate) car2_idx: usize,
    /// `Game.NumOpponents` — registered at 0x4E5543 as
    /// `0x4BA310(name, 0, 4, 0, 7, 2, 0)`: **default 4**, minimum **0**, maximum 7. `QuickRace.lay`
    /// is the one layout that names it.
    pub(crate) opponents: u32,
    /// `Game.NumKOOpponents` — a **separate** accessor, registered at 0x4E5576 with the same
    /// shape but a minimum of **1** (0x4E5570): default 4, 1…7. `knockout.lay`,
    /// `locationsetup.lay` and `quickknockout.lay` are the three layouts that name it, and it is
    /// what `Knockout.TrackNumber` measures its table by (0x4BA786).
    pub(crate) ko_opponents: u32,
    /// `Game.Damage` — the **boolean** accessor 0x4B8720 builds at 0x4E55DD from
    /// `("Game.Damage", 0, 1, 0)`. Its third argument is the initial value, fed to the setter
    /// 0x4BC240 as `!= 0`, so the setting starts **on**. Two rows, `FEStrings` 705 "off" at index
    /// 0 and 704 "on" at index 1 — 0x4B8770, the class's row-text slot (§4.102).
    pub(crate) damage: bool,
    /// `Game.Difficutly` (sic) — the list accessor 0x4BB680 builds at 0x4E560A over 0x5D9F08
    /// (`DIFFICULTY_OPTIONS`), whose setter 0x4E561B calls with **1** at registration (before any
    /// profile), so a fresh front end — and a new profile, made from the start-up `XXDefXX` —
    /// opens on 451. A profile's own index comes from its accessor block (0x4BC480 →
    /// `SetSelection`). The launch copies it to `SKILL` for game types 0, 1, 2 (0x4EB470).
    pub(crate) difficulty: u8,
    /// `OpponentCar.Car` — which cars the opponents drive (`nfs-game/src/opponents.rs`,
    /// `modes.md` §1.1a). The original's accessor is built with this at 0, "same car".
    pub(crate) opp_car: u8,
    pub(crate) paint_idx: usize,
    /// `GenericCar.Colour`'s `[+0x2C]` when it is not a row of the car's paint sets — a saved
    /// custom colour (0x4B92ED…0x4B930C), as the accessor dword. The catalogue car's slot 29
    /// (0x4F3840) answers exactly that: `[this+0x30]` names the colour accessor, 0x4F389E finds it
    /// and 0x4B94C0 reads its `[+0x2C]`, so the car wears it in the front end and in the race.
    pub(crate) paint_custom: Option<u32>,
    /// quick-race laps (circuits; point-to-point tracks always run one)
    pub(crate) laps: u32,
    /// `TRAFFIC_DENSITY` 0..3 — none / light / medium / heavy (`traffic.md` §1)
    pub(crate) traffic: u32,
    /// `COPS` — four police cars in a single race (`traffic.md` §17.1)
    pub(crate) cops: bool,
    pub(crate) backwards: bool,
    pub(crate) mirror: bool,
    /// circuit tracks (`nfs5.trk +0x6E`), the only ones a Quick Knockout may use (`LoopedTrack`)
    pub(crate) circuits: Vec<String>,
    /// `LoopedTrack.Track`'s own selection, an index into [`Menu::looped_tracks`]
    pub(crate) looped_idx: usize,
    /// knockout series status line
    pub(crate) knockout_line: String,
    pub(crate) row: u8,
    pub(crate) font: Option<BitmapFont>,
}

impl Menu {
    /// **The selected track, with the original's own out-of-range rule.** `FETrackAccessor`'s row
    /// getters all guard the index the same way — 0x4FA5A0, 0x4FA5E0 and 0x4FA620 each take the
    /// list's length as `(end − begin)/4` and, when the index is **not below** it, use **0**
    /// instead (`cmp ; jb ; xor edi, edi`). Slot 28 does it to the accessor's own current index,
    /// which is this. So a stale selection shows the first track rather than nothing — and rather
    /// than the panic a bare `self.tracks[self.track_idx]` would be.
    pub(crate) fn track(&self) -> &str {
        let i = if self.track_idx < self.tracks.len() { self.track_idx } else { 0 };
        self.tracks.get(i).map(String::as_str).unwrap_or("")
    }

    /// **The selected car, with the original's rule.** The car lists clamp in the *setter*, not the
    /// getter: `FEGenericCarAccessor`'s slot 11, 0x4F3060, takes the count from its own slot 9,
    /// compares the new index with `n − 1` and zeroes it when it is greater (0x4F3070/0x4F3072),
    /// zeroes it again when it is negative (0x4F3078 → 0x4F308B), and then **re-measures the vector
    /// and clamps a third time** (0x4F3087). Because the index can never be out of range, every
    /// getter indexes freely. This port keeps its index in a field nothing clamps, so the clamp has
    /// to live here — same effect for a read, and it is the eight `cars[car_idx]` panics that were
    /// the alternative.
    pub(crate) fn car(&self) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let i = if self.car_idx < self.cars.len() { self.car_idx } else { 0 };
        self.cars.get(i)
    }

    /// `GenericCar2.Car`'s own selected car, `cars2[car2_idx]` with the same out-of-range rule.
    pub(crate) fn car2(&self) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let i = if self.car2_idx < self.cars2.len() { self.car2_idx } else { 0 };
        self.cars2.get(i)
    }

    /// Rebuild `cars` (`[0x6919EC]`, see the field doc and [`generic_car_list`]) from the full
    /// `catalog` for the current `game_type`. Out-of-range `car_idx`/`car_garage` selections are
    /// left to [`Menu::car`]'s own clamp (0x4F3070…0x4F308B, matched) — the same shape as
    /// `set_cars2` below.
    pub(crate) fn set_cars(&mut self, catalog: &[nfs_formats::fedata::CarCatalogEntry], game_type: u8) {
        self.cars = generic_car_list(catalog, game_type);
    }

    /// Rebuild `cars2` (`[0x6919F4]`, see the field doc) from the full `catalog` for the chronicle
    /// `year`: kind-0 records released by `year` (`record.year <= year`), 0x4F0F50-sorted.
    pub(crate) fn set_cars2(&mut self, catalog: &[nfs_formats::fedata::CarCatalogEntry], year: i32) {
        let mut v: Vec<_> = catalog.iter().filter(|c| c.kind == 0 && c.year as i32 <= year).cloned().collect();
        era_class_year_sort(&mut v, era_class_year_key);
        self.cars2 = v;
    }

    /// Keep the seven `Track1..7` rows. The selected row `Knockout.TrackNumber` is **not**
    /// clamped to the count here: nothing does so when `Game.NumKOOpponents` shrinks — only the
    /// track list's draw puts a row at or past the count back on row 0 (0x4FA27B…0x4FA28C,
    /// `bind_indicator_bars`). *(Until 2026-09-27 this clamped it to the last row, citing
    /// 0x4BA786 — which is the `TrackNumber` watch's string formatter, not a clamp.)*
    ///
    /// **The original keeps all seven rows alive** — they are the accessors `Track1..7.*`
    /// registered by 0x4F9790, which a smaller opponent count only hides — so a row the count
    /// hides and shows again keeps its settings; only the reset 0x4FC730 (`reset_track_rows`, run
    /// by `SETTYPEQUICKRACE` / `SETTYPEKNOCKOUT` / `SETTYPEQUICKKNOCKOUT`) puts them back.
    /// *(Until 2026-09-26 the port truncated the table to the count and remade the lost rows.)*
    /// Row 0 is `Track1`, the single race's own row — see [`Self::ko_row`].
    pub(crate) fn fit_ko(&mut self) {
        while self.ko.len() < 7 {
            self.ko.push(KoSlot::RESET);
        }
        self.ko.truncate(7);
    }

    /// Knockout row `i`, i.e. the accessors `Track<i+1>.{Track,Laps,Traffic,Direction}`.
    /// **Row 0 is `Track1`** — the same four accessors the single race's location screen edits
    /// (`location.lay`), which `Knockout.*`' link accessors 0x4B8580 forward to when
    /// `Knockout.TrackNumber` is 0 and 0x4FC0B0 reads for mode 0 as for a knockout's first round
    /// (0x4FC20E formats `Track%d` with `raceIndex + 1`). So the port's single-race fields are row 0.
    /// *(Until 2026-09-26 the knockout table kept a row 0 of its own.)*
    pub(crate) fn ko_row(&self, i: usize) -> KoSlot {
        if i == 0 {
            return KoSlot { track_idx: self.track_idx, laps: self.laps, traffic: self.traffic, backwards: self.backwards, mirror: self.mirror };
        }
        self.ko.get(i).cloned().unwrap_or(KoSlot::RESET)
    }

    pub(crate) fn set_ko_row(&mut self, i: usize, k: KoSlot) {
        if i == 0 {
            (self.track_idx, self.laps, self.traffic, self.backwards, self.mirror) = (k.track_idx, k.laps, k.traffic, k.backwards, k.mirror);
        } else {
            self.fit_ko();
            if let Some(r) = self.ko.get_mut(i) {
                *r = k;
            }
        }
    }

    /// `LoopedTrack.Track`'s list, `[0x691D08]`: the circuits 0x4FBC10 pushes beside
    /// `[0x691D04]` (`nfs5.trk +0x6E == 1`, same order and gate), a separate accessor with its
    /// own selection from `Track1.Track` (registered by 0x4F9790 with the getter 0x4F9F00;
    /// 0x4FC0B0's mode-1 arm reads it).
    pub(crate) fn looped_tracks(&self) -> Vec<&String> {
        self.looped.iter().collect()
    }

    /// Knockout row `i`'s track, with the out-of-range rule of [`Self::track`].
    pub(crate) fn ko_track(&self, i: usize) -> &str {
        let k = self.ko_row(i).track_idx;
        let k = if k < self.tracks.len() { k } else { 0 };
        self.tracks.get(k).map(String::as_str).unwrap_or("")
    }

    /// Put 0x4FBC10's two lists in, for `year` and `Game.GameType` `game_type`.
    pub(crate) fn set_track_lists(&mut self, tables: &nfs_game::track_list::TrackListTables, year: i32, game_type: u8) {
        (self.tracks, self.looped) = tables.build(year, game_type);
        self.tracks_year = year;
    }

    /// The selected `LoopedTrack.Track`, with the out-of-range rule of [`Self::track`].
    pub(crate) fn looped_track(&self) -> &str {
        let l = self.looped_tracks();
        let i = if self.looped_idx < l.len() { self.looped_idx } else { 0 };
        l.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    /// **0x4FC730**, the track rows' reset, and `zMergedList.Car` put to row 0 — what the three
    /// single-player `SETTYPE*` handlers run after setting `Game.GameType` 0 / 2 / 1 and
    /// rebuilding the car lists 0x4F3E40 (0x510C30, 0x510CA0, 0x510D10): every `Track<i>` to
    /// track 0, laps index 1 (two laps), direction 0, traffic 0, `Knockout.TrackNumber` and
    /// `LoopedTrack.Track` to 0 (`modes.md` §2). `default_colour` is
    /// [`default_paint_colour`] — neither `0x4FC730` nor `zMergedList.Car`'s own reset touches
    /// any colour accessor (read to the instruction, `510c30`/`4fc730`), so the colour shown for
    /// the new row-0 car is whichever of *its* rows carries the fixed dword the front end's
    /// `GenericCar.Colour` started on, not row 0 — [`default_paint_row`].
    pub(crate) fn reset_track_rows(&mut self, default_colour: u32) {
        self.track_idx = 0;
        self.laps = 2;
        self.backwards = false;
        self.mirror = false;
        self.traffic = 0;
        self.ko.clear();
        self.ko_sel = 0;
        self.fit_ko();
        self.looped_idx = 0;
        self.car_idx = 0;
        self.merged_at_zero = true;
        self.paint_idx = self.cars.first().map(|c| default_paint_row(default_colour, &c.paint_sets)).unwrap_or(0);
        self.paint_custom = None;
    }

    /// `MAKERANDOMKNOCKOUT`, **0x50D570**: for rows 1…7, each through its accessor's count and
    /// CRT `rand` (0x5A17C9), `Track<i>.Traffic = rand % 4`, `.Direction = rand % 4`, `.Laps =
    /// rand % 8` (the index, so 1…8 laps) and `.Track = rand % tracks`. All seven rows are kept
    /// (`fit_ko`), row 0 being `Track1`, and the direction keeps its mirrored half. *(Until
    /// 2026-09-26 the port dropped the rows past the opponent count and the mirror.)*
    pub(crate) fn randomize_knockout(&mut self, rand: &mut nfs_game::movie_overlay::CRand) {
        let n_tracks = self.tracks.len().max(1) as i32;
        for i in 0..7 {
            let traffic = rand.next() % 4;
            let direction = rand.next() % 4;
            let laps = rand.next() % 8;
            let track = rand.next() % n_tracks;
            let mut k = KoSlot { track_idx: track as usize, laps: laps as u32 + 1, traffic: traffic as u32, backwards: false, mirror: false };
            k.set_direction(direction as usize);
            self.set_ko_row(i, k);
        }
    }

    pub(crate) fn rows(&self) -> u8 {
        match self.mode {
            0 | 4 | 5 => 10,
            3 => 2,
            _ => 4,
        }
    }

    pub(crate) fn sprites(&self, career: Option<&CareerUi>) -> Vec<Sprite> {
        let Some(f) = &self.font else {
            return Vec::new();
        };
        let mut out = f.sprites(
            "PORSCHE UNLEASHED  -  NATIVE",
            [0.05, 0.08],
            0.05,
            [1.0, 1.0, 1.0, 1.0],
        );
        let sel = |r: u8| {
            if self.row == r {
                [1.0, 0.75, 0.1, 1.0]
            } else {
                [0.85, 0.85, 0.85, 1.0]
            }
        };
        out.extend(f.sprites(
            &format!(
                "MODE    < {} >",
                [
                    "QUICK RACE",
                    "EVOLUTION",
                    "TOURNAMENT",
                    "FACTORY DRIVER",
                    "QUICK KNOCKOUT",
                    "KNOCKOUT"
                ][self.mode.min(5) as usize]
            ),
            [0.08, 0.15],
            0.04,
            sel(0),
        ));
        if (1..=3).contains(&self.mode) {
            match career {
                // career rows share the cursor: menu row 1 = tournament, row 2 = car
                Some(c) => out.extend(c.sprites(f, (self.row > 0).then(|| self.row - 1))),
                None => out.extend(f.sprites(
                    "CAREER TABLES UNAVAILABLE",
                    [0.08, 0.30],
                    0.045,
                    [0.85, 0.85, 0.85, 1.0],
                )),
            }
            out.extend(f.sprites(
                "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER BUY / RACE   ESC QUIT",
                [0.08, 0.85],
                0.028,
                [0.6, 0.6, 0.6, 1.0],
            ));
            return out;
        }
        if self.mode == 5 {
            out.extend(f.sprites(&self.knockout_line, [0.08, 0.30], 0.045, sel(1)));
        } else {
            out.extend(f.sprites(
                &format!("TRACK   < {} >", self.track().to_string()),
                [0.08, 0.30],
                0.045,
                sel(1),
            ));
        }
        let Some(c) = self.car() else { return Vec::new() };
        out.extend(f.sprites(
            &format!("CAR     < {} >", c.display_name.to_string()),
            [0.08, 0.40],
            0.045,
            sel(2),
        ));
        let paint = c
            .paint_sets
            .get(self.paint_idx % c.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        out.extend(f.sprites(
            &format!(
                "PAINT   < {} / {} >",
                self.paint_idx % c.paint_sets.len().max(1) + 1,
                c.paint_sets.len()
            ),
            [0.08, 0.48],
            0.045,
            sel(3),
        ));
        out.extend(f.sprites(
            "####",
            [0.42, 0.48],
            0.045,
            [
                paint[0] as f32 / 255.0,
                paint[1] as f32 / 255.0,
                paint[2] as f32 / 255.0,
                1.0,
            ],
        ));
        out.extend(f.sprites(
            &format!("OPPONENTS  < {} >", if self.mode >= 4 { self.ko_opponents } else { self.opponents }),
            [0.08, 0.545],
            0.045,
            sel(4),
        ));
        // `OpponentCar.Car`: FE strings 640..644, default "same car" (`modes.md` §1.1a)
        out.extend(f.sprites(
            &format!(
                "THEY DRIVE < {} >",
                nfs_game::opponents::OpponentChoice::from_index(self.opp_car as u32).label()
            ),
            [0.08, 0.602],
            0.04,
            sel(9),
        ));
        if self.mode == 4 {
            out.extend(f.sprites(
                &format!("LAPS    {}  (ONE CAR OUT PER LAP)", self.ko_opponents.max(1)),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        } else if self.mode == 5 {
            out.extend(f.sprites(
                &format!(
                    "RACES   {}   LAPS < {} >   (LAST CAR OUT EACH RACE)",
                    self.ko_opponents.max(1),
                    self.laps
                ),
                [0.08, 0.66],
                0.04,
                sel(5),
            ));
        } else {
            out.extend(f.sprites(
                &format!("LAPS    < {} >", self.laps),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        }
        // Track1.Direction ids 0x19A..0x19D: forward / backward / forward mirrored / backward mirrored
        let dir = match (self.backwards, self.mirror) {
            (false, false) => "FORWARD",
            (true, false) => "BACKWARD",
            (false, true) => "FORWARD MIRRORED",
            (true, true) => "BACKWARD MIRRORED",
        };
        out.extend(f.sprites(
            &format!("DIRECTION  < {dir} >"),
            [0.08, 0.74],
            0.045,
            sel(6),
        ));
        // FE strings 403/402/401/400; the count is `_ftol(index · 2.5)` = 0/2/5/7 (`traffic.md` §1)
        let density = ["NONE", "LIGHT", "MEDIUM", "HEAVY"][self.traffic.min(3) as usize];
        out.extend(f.sprites(
            &format!(
                "TRAFFIC    < {density} >   ({} CARS)",
                TRAFFIC_COUNT[self.traffic.min(3) as usize]
            ),
            [0.08, 0.80],
            0.04,
            sel(7),
        ));
        out.extend(f.sprites(
            &format!(
                "POLICE     < {} >",
                if self.cops { "ON  (4 CARS)" } else { "OFF" }
            ),
            [0.08, 0.845],
            0.04,
            sel(8),
        ));
        out.extend(f.sprites(
            "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER START   ESC QUIT",
            [0.08, 0.91],
            0.026,
            [0.6, 0.6, 0.6, 1.0],
        ));
        out
    }
}

/// One row of the knockout location table (`locationsetup.lay`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct KoSlot {
    pub(crate) track_idx: usize,
    pub(crate) laps: u32,
    /// `TRAFFIC_DENSITY` 0..3, in the order the screen's own option list gives them
    pub(crate) traffic: u32,
    pub(crate) backwards: bool,
    /// the `Direction` row's mirrored half: 0x4FC0B0 maps the four rows through the tables at
    /// 0x4FC708 / 0x4FC718 to `(REVERSE, MIRROR)` = (0,0) (1,0) (0,1) (1,1)
    pub(crate) mirror: bool,
}

impl KoSlot {
    /// What the reset 0x4FC730 leaves on every row: track 0, laps index 1 (two laps), forward, no
    /// traffic (0x4FC77E, 0x4FC7C2, 0x4FC806, 0x4FC84A).
    pub(crate) const RESET: KoSlot = KoSlot { track_idx: 0, laps: 2, traffic: 0, backwards: false, mirror: false };

    /// `Track<n>.Direction`'s row, `REVERSE + 2·MIRROR` (0x4FC708 / 0x4FC718).
    pub(crate) fn direction(&self) -> usize {
        usize::from(self.backwards) | (usize::from(self.mirror) << 1)
    }

    pub(crate) fn set_direction(&mut self, d: usize) {
        self.backwards = d & 1 != 0;
        self.mirror = d & 2 != 0;
    }
}
