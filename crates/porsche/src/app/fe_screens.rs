//! Building and binding the front end's screens: accessors, layouts, widget binders.

use crate::*;

impl App {
    /// The write half of `accessor_value`: a combo slider moves the port's own setting the accessor
    /// stands for. Only the names the port has state for do anything.
    /// Set a range accessor to an absolute value — what `NFS5.GouraudIndicatorBar`'s pointer
    /// slots do (0x4CA548 and its twin in 0x4CA590) once they have turned the pointer's x into an
    /// index. `bar_at` answers the fraction; the count is the accessor's own `max - min + 1`,
    /// which is the list 0x4BA329 builds, and the index is the value less the minimum (0x4BA34A).
    /// `RE/notes/controls.md` §4.81.
    pub(crate) fn set_accessor_fraction(&mut self, name: &str, f: f32) {
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        let Some((_, lo, hi)) = self.range_value(base) else { return };
        let n = (hi - lo + 1).max(1);
        // 0x5A0F98 truncates toward zero and 0x4CA535 pins the top index, so the bar can never
        // answer `n` itself
        let v = ((n as f32 * f) as i32).min(n - 1).max(0) + lo;
        if base.starts_with("Audio.") {
            let a = &mut self.snd.audio_settings;
            let v = v.clamp(0, 127) as u8;
            match base {
                "Audio.MusicLevel" => a.music_level = v,
                "Audio.SfxLevel" => a.sfx_level = v,
                "Audio.EngineLevel" => a.engine_level = v,
                "Audio.SpeechLevel" => a.speech_level = v,
                "Audio.AmbientLevel" => a.ambient_level = v,
                _ => {}
            }
            if base == "Audio.MusicLevel" {
                self.music_level_changed();
            }
            return;
        }
        if let Some(key) = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == base).map(|(_, k, ..)| *k) {
            self.choices.insert(key.to_string(), v.clamp(lo, hi) as usize);
        }
    }

    /// State 0x2E for the frame about to be drawn: `max(0.25, Graphics.GammaCorrection · 0.01)` —
    /// `fild`, `fmul dword [0x5B3CA0]` (0.01f), `fstp` to a float, the 0.25 of `[0x5B2468]` when
    /// that is not below it — then `[0x6BD918](0x2E, g)`. The original sets it at the mode set and
    /// the race's (0x44ED10, from `[0x657D80]` = `GAMMACORRECTION`), after a profile loads
    /// (0x4DED10), when the Graphics screen is built (0x4E5A70) and on every tick of the options
    /// screen whose value moved (0x511900, cached in `[0x69A058]`) — each time from the same
    /// setting, so setting it per frame from that setting shows the same thing.
    pub(crate) fn sync_gamma(&mut self) {
        let v = self.range_value("Graphics.GammaCorrection").map_or(100, |(v, ..)| v);
        let g = ((v as f64 * f64::from(0.01f32)) as f32).max(0.25);
        if let Some(r) = self.renderer.as_mut() {
            r.set_gamma(g);
        }
        if let Some(run) = self.intro.as_mut() {
            run.renderer.set_gamma(g);
        }
    }

    /// One step of `zMergedList.Car` over the garage half and then the catalogue half (see the
    /// call in `set_accessor`). With an empty garage this is the catalogue step it always was.
    fn step_merged_car(&mut self, delta: i32) {
        self.net.lobby_setsel = true;
        let owned = self.owned_list();
        let car_sel = self.career.as_ref().map_or(0, |c| c.car_sel);
        let Some(m) = self.fe.menu.as_mut() else { return };
        let garage_now = m.car_garage;
        let a = owned.len();
        m.merged_at_zero = false;
        let cur = if garage_now && a > 0 { owned.iter().position(|i| *i == car_sel).unwrap_or(0) } else { a + m.car_idx };
        let n = (a + m.cars.len()).max(1);
        let new = (cur as i32 + delta).rem_euclid(n as i32) as usize;
        if new < a {
            m.car_garage = true;
            if let Some(c) = self.career.as_mut() {
                c.car_sel = owned[new];
            }
        } else {
            m.car_garage = false;
            let before = m.car_idx;
            m.car_idx = new - a;
            // the changed catalogue row shows its own row 0 (see the `GenericCar.Car` arm below)
            if m.car_idx != before || garage_now {
                m.paint_idx = 0;
                m.paint_custom = None;
            }
        }
    }

    pub(crate) fn set_accessor(&mut self, name: &str, delta: i32) {
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        // the seven colour lists step through the colour accessor's own slots (§4.171.5)
        if COLOUR_ACCESSORS.contains(&base) {
            self.colour_step(base, delta);
            return;
        }
        // the paint shop edits the garage car itself — `FEOwnedCarAccessor`'s setters, slots 37…41
        // (0x4F6F90…0x4F7050), write `FECar+0x674…+0x684` (`savegame.md` §4.7a); the four colours
        // went through `colour_step` above, which leaves the package
        if base == "OwnedCar.PackageNum" && self.garage_car().is_some() {
            let n_pkg = CHOICE_ACCESSORS.iter().find(|(n, ..)| *n == "OwnedCar.PackageNum").map_or(8, |(.., o)| o.len()) as i32;
            let Some(c) = self.career.as_mut() else { return };
            let sel = c.car_sel.min(c.career.garage.len().saturating_sub(1));
            let Some(o) = c.career.garage.get_mut(sel) else { return };
            o.trim.package = ((o.trim.package as i32 + delta).rem_euclid(n_pkg)) as u8;
            return;
        }
        if let Some((v, lo, hi)) = self.range_value(base).filter(|_| !base.starts_with("Audio.")) {
            let key = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == base).map(|(_, k, ..)| *k).unwrap_or_default();
            // **one index per press.** An int range is a list of `max − min + 1` entries
            // (0x4BA329 builds exactly that count) whose index *is* the value less the minimum
            // (0x4BA34A / 0x4BA362), and the combo slider adds its own `[widget+0x1A8]` to the
            // accessor's current index (0x4C6CFF…0x4C6D10) — which the `.lay` sets from a
            // `NFS5.CARSETTINGBUTTON`'s `step`, and every one of the 34 shipped buttons carries
            // ±1. So the volume sliders move by one of 128 steps, not by a twentieth of the range
            // as this port used to assume.
            self.choices.insert(key.to_string(), (v + delta).clamp(lo, hi) as usize);
            return;
        }
        if let Some((cur, options)) = self.choice_index(base) {
            let key = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == base).map(|(_, k, _)| *k).unwrap_or_default();
            let next = (cur as i32 + delta).rem_euclid(options.len() as i32) as usize;
            // a Display option goes to the profile's HUD record (0x4CB860 → 0x41EA50), not into an accessor
            if let Some(slot) = Self::hud_cfg2_slot(key) {
                self.hud_cfg_set(slot, next);
                return;
            }
            // the Display screen stores the old layout's options when its combo changes (0x4CB9A0)
            if base == "Display.ScreenLayout" {
                self.hud_cfg_layout_changed();
            }
            self.choices.insert(Self::choice_slot(base, key).to_string(), next);
            return;
        }
        // `CDPlayer.Title` is a combo on the song table (`audioreal.lay`): slot 21 wraps over the
        // table and lands in the setter 0x4BEDC0 (`cd_set_title`) — a click on its caption did nothing
        // before (2026-09-30)
        if base == "CDPlayer.Title" {
            let n = Self::cd_table(&self.cd_names()).len();
            if n > 0 {
                let i = self.fe.cd_title.min(n - 1);
                self.cd_set_title((i as i32 + delta).rem_euclid(n as i32) as usize);
            }
            return;
        }
        // the network screens' own sliders and lists
        if self.step_comms_accessor(base, delta) {
            return;
        }
        // the career lists: the tournament, its remaining events, the dealer car and the garage car
        if self.step_career_list_accessor(base, delta) {
            return;
        }
        // a string list (0x4B8990): the combo adds its step to the index and the setter 0x4BC240
        // clamps it into the list
        if matches!(base, "Controller.AllControllers" | "Controller.AllPads") {
            let pads_only = base == "Controller.AllPads";
            let n = self.gamepads.devices().into_iter().filter(|(_, is_pad)| !pads_only || *is_pad).count();
            let sel = if pads_only { &mut self.fe.pads_sel } else { &mut self.fe.controllers_sel };
            *sel = (*sel as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
            return;
        }
        // `Graphics.Resolutions`: the same wrap every other list combo takes (0x4BC240's clamp;
        // this list is enumerated, not wrapped, in the original — MEDIUM on which one 0x4BC710's
        // own accessor uses, not read this pass); the pick only takes effect when a race is set up
        // ([`App::apply_resolution`], 0x467FC0 at 0x41562E — the front end stays at 640x480, 0x4D3440)
        if base == "Graphics.Resolutions" {
            let n = self.resolution_list().len();
            self.fe.resolution_sel = (self.fe.resolution_sel as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
            self.apply_resolution();
            return;
        }
        // `zMergedList.Car`'s step over both lists (0x4BC690 wraps over `A.count + B.count`, the
        // setter 0x4F49F0 routes the new index to list A's `slot11` — the garage car, `car_sel` —
        // or B's — the catalogue row)
        if base == "zMergedList.Car" {
            self.step_merged_car(delta);
            return;
        }
        self.step_menu_accessor(base, delta);
    }

    /// [`Self::set_accessor`]'s network arms: `true` when one of them took the step.
    fn step_comms_accessor(&mut self, base: &str, delta: i32) -> bool {
        if base == "Comms.Positional"
            && let Some(n) = self.net.session.as_mut()
        {
            n.positional = !n.positional;
            return true;
        }
        if base == "Comms.PlayerList"
            && let Some(n) = self.net.session.as_mut()
        {
            n.step_player(delta);
            return true;
        }
        if let Some(l) = self.net.lobby.as_mut() {
            match base {
                "Comms.Positional" => {
                    l.positional = !l.positional;
                    return true;
                }
                "Comms.IPList" => {
                    let n = l.history.len().max(1);
                    let row = ((l.ip_sel as i32 + delta).rem_euclid(n as i32)) as usize;
                    self.pick_ip_row(row);
                    return true;
                }
                "Comms.ServerList" | "Comms.GamesList" | "Comms.LobbyPlayerList" | "Comms.PlayerList" => {
                    let n = l.found.len().max(1);
                    l.sel = ((l.sel as i32 + delta).rem_euclid(n as i32)) as usize;
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// [`Self::set_accessor`]'s career-list arms: `true` when one of them took the step.
    fn step_career_list_accessor(&mut self, base: &str, delta: i32) -> bool {
        // `Tournament.Tournament`'s combo (`tournament.lay`): the list accessor's step wraps both
        // ways (vtable 0x5B88F0 slots 20 / 21 = 0x4BC660 / 0x4BC690) and lands in its setter
        // **0x504D00(k)**, which makes the k-th record of the list `Tournament.EventType` picks
        // (0x5053C0) the selected tournament `[0x691E6C]` (0x504DCC) and puts `[0x691E7C]` on its
        // first undone event (0x504E79…0x504EB4) — `Tournament.RemainingTracks` back on row 0.
        // A club list (EventType 1…3) is the career's club mode here (`CareerUi::club`).
        if base == "Tournament.Tournament" {
            let ev_type = self.choice_index(choice::TOURNAMENT_EVENT_TYPE).map_or(0, |(i, _)| i);
            if let Some(c) = self.career.as_mut() {
                let list: Vec<i16> = c.event_type_list(ev_type).iter().map(|t| t.id).collect();
                if !list.is_empty() {
                    let cur = c.current().and_then(|t| list.iter().position(|id| *id == t.id)).unwrap_or(0);
                    let id = list[(cur as i32 + delta).rem_euclid(list.len() as i32) as usize];
                    c.club = (1..=3).contains(&c.event_type_kind(ev_type));
                    if let Some(i) = c.tournaments().iter().position(|t| t.id == id) {
                        c.t_idx = i;
                    }
                    self.fe.remaining_sel = 0;
                }
            }
            return true;
        }
        // `Tournament.RemainingTracks` keeps its own row, wrapping (0x4FA575 folds an index at or
        // past the count back to 0)
        if base == "Tournament.RemainingTracks" {
            let n = self.career.as_ref().and_then(|c| {
                let t = c.current()?;
                let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                Some(t.events.len().saturating_sub(done))
            });
            if let Some(n) = n.filter(|n| *n > 0) {
                self.fe.remaining_sel = ((self.fe.remaining_sel.min(n - 1) as i32 + delta).rem_euclid(n as i32)) as usize;
            }
            return true;
        }
        // the dealership's car in a career: it wraps both ways (0x4BC660 / 0x4BC690) and a new car
        // puts the colour rows back to 0 (0x4F30A4…); its two colour lists went through
        // `colour_step` above
        if base == "DealerCar.Car"
            && let Some(v) = self.dealer_cars()
        {
            let count = v.len();
            let n_cars = count.max(1) as i32;
            let d = &mut self.fe.dealer;
            let step = |v: usize, n: i32| ((v.min(n as usize - 1) as i32 + delta).rem_euclid(n)) as usize;
            // 0x4BC660 / 0x4BC690 wrap, then slot 11 (0x4F3060)
            let row = step(d.car, n_cars);
            d.slot11(row as i64, count);
            return true;
        }
        // `OwnedCars.Car` is the garage (`FEOwnedCarAccessor`, §4.144): its index is the garage
        // selection every other owned-car accessor, the logo and the 3D car read. *(Until
        // 2026-09-25 it stepped the quick race's catalogue index, so the garage's and the paint
        // shop's car combos moved nothing on screen.)* Outside a career it keeps that meaning.
        // stepped over 0x501AD0's list (`owned_list`), not the whole garage
        if base == "OwnedCars.Car"
            && self.career.is_some()
        {
            let list = self.owned_list();
            if let Some(c) = self.career.as_mut() {
                if !list.is_empty() {
                    let n = list.len() as i32;
                    let k = list.iter().position(|i| *i == c.car_sel).unwrap_or(0) as i32;
                    c.car_sel = list[(k + delta).rem_euclid(n) as usize];
                }
                c.part_sel = 0;
            }
            return true;
        }
        false
    }

    /// [`Self::set_accessor`]'s last arms: the accessors the race-set-up menu (`fe.menu`) holds,
    /// and the audio levels.
    fn step_menu_accessor(&mut self, base: &str, delta: i32) {
        let Some(m) = self.fe.menu.as_mut() else { return };
        let step = |i: usize, n: usize| ((i as i32 + delta).rem_euclid(n.max(1) as i32)) as usize;
        match base {
            // **`Knockout.*` and `Track1.*` are the same accessors.** The knockout screen's four
            // combo boxes are link accessors (0x4B8580, vtable 0x5B4BE4) whose every slot jumps
            // through `[this+0x24]` to `Track<n>.*` of the selected row (0x4B85B0 / 0x4B85C0 /
            // 0x4B85D0 …), so they step exactly as the single race's do, and row 0 **is** `Track1`
            // (`Menu::ko_row`). *(Until 2026-09-26 the knockout traffic wrapped where `Track1`'s
            // clamps and its direction toggled between two rows.)*
            "Knockout.Track" | "Knockout.Laps" | "Knockout.Traffic" | "Knockout.Direction" | "Track1.Laps" | "Track1.Traffic" | "Track1.Direction" => {
                let (row, field) = match base.split_once('.') {
                    Some(("Knockout", f)) => (m.ko_sel, f),
                    Some((_, f)) => (0, f),
                    None => return,
                };
                let mut k = m.ko_row(row);
                match field {
                    "Track" => k.track_idx = step(k.track_idx, m.tracks.len()),
                    "Laps" => k.laps = (k.laps as i32 + delta).clamp(1, LAPS_MAX as i32) as u32,
                    "Traffic" => k.traffic = (k.traffic as i32 + delta).clamp(0, 3) as u32,
                    // `FEDirectionAccessor`'s four rows, clamped by 0x4BC240 like every list
                    _ => k.set_direction((k.direction() as i32 + delta).clamp(0, 3) as usize),
                }
                m.set_ko_row(row, k);
            }
            // `LoopedTrack.Track` has its own selection over the circuits (`Menu::looped_tracks`)
            "LoopedTrack.Track" => m.looped_idx = step(m.looped_idx.min(m.looped_tracks().len().saturating_sub(1)), m.looped_tracks().len()),
            "Track1.Track" => {
                m.track_idx = step(m.track_idx, m.tracks.len())
            }
            // `Record.Track`'s own list is one row long (0x4FB510 builds it with count 1 through
            // 0x4BC1F0), so its setter 0x4BC240 has nowhere to go and the target is not touched
            "Record.Track" => {}
            // the opponent combo steps the five choices, not a car list
            "OpponentCar.Car" => m.opp_car = ((m.opp_car as i32 + delta).rem_euclid(5)) as u8,
            // the used-car lot steps its own selection, not the catalogue's
            "UsedCar.Car" => {
                let n = self.used_cars().len();
                self.fe.usedcar_sel = step(self.fe.usedcar_sel, n);
                self.bind_indicator_bars();
            }
            // `GenericCar.Car`'s slot 11 (0x4F3060) sends its colour accessors to `slot 11(0)` on
            // every changed row except the very first call (the `[+0x38]` latch, 0x4F30A4…
            // 0x4F3100), so a step of the catalogue car shows the NEW car's own row 0 and drops a
            // picked colour; observed on the original's QuickRace (SC 2026-10-01: 356 → 356 B
            // grey = row 0, red picked, → 356 A Convertible blue = row 0, not red and not the
            // fixed cream dword). A same-row call changes nothing.
            "GenericCar.Car" => {
                let before = m.car_idx;
                m.car_idx = step(m.car_idx, m.cars.len());
                if m.car_idx != before {
                    m.paint_idx = 0;
                    m.paint_custom = None;
                }
            }
            // the garage / dealership / settings accessors: not observed, the by-value row is
            // kept (residual)
            "OwnedCars.Car" | "DealerCar.Car" | "CARSETTINGS.CAR" => {
                // read off `self.paths` (disjoint from `self.fe`, which `m` borrows) — here, for
                // this arm alone: it used to read and parse `nfs5.car` on every step of any
                // accessor of this function
                let default_colour = default_paint_colour(&nfs_formats::fedata::parse_car_catalog(&std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default()).unwrap_or_default());
                m.car_idx = step(m.car_idx, m.cars.len());
                m.paint_idx = m.cars.get(m.car_idx).map(|c| default_paint_row(default_colour, &c.paint_sets)).unwrap_or(0);
                m.paint_custom = None;
            }
            // the compare screen's own combo box moves only the right-hand car
            "GenericCar2.Car" => m.car2_idx = step(m.car2_idx, m.cars2.len()),
            // two accessors, two ranges (0x4E5539 and 0x4E556C): 0…7 for the race, **1**…7 for
            // the knockout table, both starting at 4
            // 0x4BC240 clamps the new index to `count - 1` and then to 0, so the two rows wrap
            // into nothing — the stepper simply runs off each end
            "Game.Damage" => m.damage = (usize::from(m.damage) as i32 + delta).clamp(0, 1) != 0,
            // a list accessor: 0x4BC240 clamps into its three rows
            "Game.Difficutly" => m.difficulty = (m.difficulty as i32 + delta).clamp(0, 2) as u8,
            "Game.NumOpponents" => m.opponents = (m.opponents as i32 + delta).clamp(0, 7) as u32,
            "Game.NumKOOpponents" => {
                m.ko_opponents = (m.ko_opponents as i32 + delta).clamp(1, 7) as u32;
                m.fit_ko();
            }
            _ => {
                // the audio levels are 0..127 and the mixers read them every tick, so a slider
                // changes the mix straight away
                let a = &mut self.snd.audio_settings;
                let lvl = |v: u8| (v as i32 + delta * 8).clamp(0, 127) as u8;
                match base {
                    "Audio.MusicLevel" => a.music_level = lvl(a.music_level),
                    "Audio.SfxLevel" => a.sfx_level = lvl(a.sfx_level),
                    "Audio.EngineLevel" => a.engine_level = lvl(a.engine_level),
                    "Audio.SpeechLevel" => a.speech_level = lvl(a.speech_level),
                    "Audio.AmbientLevel" => a.ambient_level = lvl(a.ambient_level),
                    _ => {}
                }
                if base == "Audio.MusicLevel" {
                    self.music_level_changed();
                }
            }
        }
    }

    pub(crate) fn open_layout(&mut self, name: &str) {
        // `FEChronicleScreen`'s constructor sets `Chronicle.Year` to the group's earliest year
        // (0x506E35…0x506E98), and a new screen object has no drag in flight
        if name.eq_ignore_ascii_case("chronicle") {
            self.fe.chronicle_year_idx = None;
            self.fe.chronicle_slider_drag = None;
        }
        // The bar's own construction (0x4D7E70, called at 0x4D39C4) precedes the first layout's
        // (0x4D67D0, both in 0x4D3420's preamble — `RE/notes/layouts.md`'s "The loading steps"),
        // so its rand draws — the oval flash's wait (`bar_oval.rs`'s `load_bar_oval`) and the four
        // `NFS5.BOTTOMBARBUTTON` sweeps' binds (`activate_glints`, inside `LayoutScreen::load`
        // itself) — land ahead of the opened screen's own. *(Until this pass the port built the
        // bar after `name`'s own load, which put every later screen's `crt_rand()` draws one bar
        // behind the original's — `bar_oval.rs`'s marker; fixed here by building the bar first.)*
        if self.fe.layout_bottom.is_none()
            && let Some(r) = self.renderer.as_mut()
        {
            self.fe.layout_bottom = nfs_game::layout::LayoutScreen::load(r, &self.paths, "bottombar")
                .map_err(|e| tracing::debug!(error = %e, "no bottom bar"))
                .ok();
            if let Some(b) = self.fe.layout_bottom.as_mut() {
                b.set_overlay(true);
                // the bar's constructor 0x4D7E70: the oval flash's art and timer (0x520AD0)
                b.load_bar_oval(r, &self.paths);
                // the bar's slot 21 0x4D7FD0: the career tab pack and its two fonts
                b.load_career_tabs(r, &self.paths);
            }
        }
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        match nfs_game::layout::LayoutScreen::load(r, &self.paths, name) {
            Ok(mut l) => {
                l.cheat_bits = self.cheat_bits;
                l.driver_picture = self.fe.driver_picture;
                let replaced = self.fe.layout_preview.replace(l);
                self.fe.layout_name = Some(name.to_string());
                // **the lobby's entry ritual is per screen instance** (0x509290 / 0x50A4F0 guard
                // `[this+0x285]`, which the constructor clears): a lobby screen built again — after a
                // race, `fe_after_race` — sends its records once more
                if name.eq_ignore_ascii_case("creategame") || name.eq_ignore_ascii_case("joingame") {
                    self.net.lobby_sent = false;
                    self.net.lobby_car = None;
                }
                // the peer-to-peer screen's own latches `[this+0x288]` / `[this+0x289]` (the constructor clears them)
                if name.eq_ignore_ascii_case("peertopeerip") {
                    self.net.connect_pending = false;
                    self.net.connect_opened = false;
                }
                self.fe.layout_sel = 0;
                self.fe.layout_nav_used = false;
                self.fe.layout_open_combo = None;
                // a freshly opened screen needs the device list before its first draw, not after
                let (pads, dz) = (self.fe_pads(), Self::fe_deadzone());
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_pads(pads, self.fe.pads_sel);
                    if let Some(dz) = dz {
                        l.set_deadzone_values(0, dz);
                    }
                }
                self.fe_bind_screen(name);
                // 0x4D1BD0 on every open: a room named differently goes out (slot 0x7C); the
                // first screen's swap makes the room when there is none (0x4D5EFD)
                match (self.fe.room_obj, self.fe_screen_room()) {
                    (None, _) => self.fe_room_create(),
                    (Some((r, t)), Some(want)) if r.id != want.id && !matches!(t, nfs_game::layout::Transition::Out { .. }) => {
                        self.fe.room_obj = Some((r, nfs_game::layout::Transition::Out { frames: nfs_game::layout::TRANS_DELAY_FRAMES, ms: 0 }));
                    }
                    _ => {}
                }
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.transition_in();
                }
                // the screen's music (0x4D1BD0 at 0x4D23DA)
                let list_swapped = self.fe_apply_music();
                // and the bottom bar's buttons (0x4D81C0 at 0x4D25DC)
                self.bind_bottom_bar();
                // the callback-list object's constructor runs when the object is made — i.e. when
                // the **bottom** layer's screen names another list (0x4D1BD0 sends the old one out,
                // 0x4D2570 0x4D27E7…0x4D280B makes the new one); a screen with the same list, or
                // an overlay over a screen, makes none
                let deferred = if list_swapped { self.fe_screen_built() } else { None };
                // the screen it replaces is destroyed — at the tick that swaps them (0x4D2570)
                if let Some(old) = replaced {
                    self.fe_screen_gone(&old);
                }
                if let Some((name, offset)) = deferred {
                    self.fe_open(name, offset, true);
                }
            }
            Err(e) => tracing::warn!(error = %e, name, "layout unavailable"),
        }
    }

    /// `data.callbacklist` of a screen, upper-cased; "" is `ID_NONE` (0x4E0B30).
    pub(crate) fn fe_list_of(l: &nfs_game::layout::LayoutScreen) -> String {
        l.layout.elements.first().and_then(|e| e.get("callbacklist")).filter(|c| !c.is_empty()).unwrap_or("ID_NONE").to_ascii_uppercase()
    }

    /// What a screen's callback list does as it is constructed (`controls.md` §4.169.2), for the
    /// screen now on show. Returns a screen to open once the caller is done — the constructor's
    /// own 0x4D2100, which the port runs after the screen is up.
    ///
    /// * `ID_VICTORYSCREEN` (`VictoryRoom.lay`, creator **0x506B40**, vtable 0x5B89D4): after its
    ///   two callbacks (`TESTDRIVE` 0x506C70, `DECLINE` 0x506CB0) it writes `Game.GameType` **10**
    ///   (slot 0x2C, 0x506BB5), locks BACK (**0x4D5BA0**, 0x506BC0) and opens
    ///   **`0x4D2100("VictoryMessage", 1, 1)`** (0x506BD4) — the finale's message on a layer of
    ///   its own, which its `GOBACKMINUSONE` closes;
    /// * `ID_FDVICTORYSCREEN` (`FactoryVictory.lay`, `levelvictory.lay`, creator **0x51A5F0**,
    ///   vtable 0x5BAABC): after `ACCEPTPRIZECAR` and `FDMSGGOBACK`, the lock (0x51A63F).
    ///
    /// 0x4D5BA0 sets `[fe+0x17C]` and runs 0x4D8230 — the bottom bar's `ID_BACKBUTTON` `[+0x264]`
    /// gets the disabled bit 0x10 when its template has the state (0x4C7E10); 0x4D6320 reads the
    /// byte, so `bind_bottom_bar` carries both.
    pub(crate) fn fe_screen_built(&mut self) -> Option<(&'static str, i32)> {
        // the list object `[fe+0xE0]` is the bottom layer's
        let list = self.fe_base_layout().map(Self::fe_list_of)?;
        match list.as_str() {
            "ID_VICTORYSCREEN" => {
                self.fe.game_type = 10;
                self.fe_back_lock(true);
                Some(("VictoryMessage", 1))
            }
            "ID_FDVICTORYSCREEN" => {
                self.fe_back_lock(true);
                None
            }
            // `ID_FACTORYCBL` (`factorydriver.lay`, `factdone.lay`; creator 0x519830, ctor
            // **0x5198A0**, vtable 0x5BA7C0): after its three callbacks (`CHEATFACTORY`,
            // `PROFILEDEC`, `PROFILEINC`) it opens a layer, `0x4D2100(name, 1, 1)` (0x51993E):
            // `factwelcome` the first time — `[0x691D94]` clear (0x5015D0 at 0x5198F4), then set
            // (0x5015E0) — else `factfirst` when the current mission (0x51AE80: the first record
            // whose `+0xE8` is 0, or the first record when none is) has record `+0` = 0xBC7 and
            // `[0x691DF5]` is clear (0x5015F0 at 0x51991A), then set (0x501600) — else
            // `missionbrief`. The two bytes are `PlayerInfo`'s, saved with the profile.
            // *(Until 2026-09-26 the port carried them and opened nothing.)*
            "ID_FACTORYCBL" => {
                let c = self.career.as_mut()?;
                if !c.career.seen_factory_welcome {
                    c.career.seen_factory_welcome = true;
                    return Some(("factwelcome", 1));
                }
                // 0x51AE80: `cmp word [rec+0xE8], 0` over the records, else the table base
                let current = (0..c.tables.missions.len()).find(|i| c.career.missions.get(*i).copied().unwrap_or(0) == 0).unwrap_or(0);
                let first = c.tables.missions.get(current).is_some_and(|m| m.intro_string_id == 0xBC7);
                if first && !c.career.seen_first_mission {
                    c.career.seen_first_mission = true;
                    return Some(("factfirst", 1));
                }
                Some(("missionbrief", 1))
            }
            _ => None,
        }
    }

    /// A screen destroyed — replaced, or its layer closed (0x4D2570). The two victory lists'
    /// destructors, **0x506C30** and **0x51A690**, lift the lock (**0x4D5BC0**: 0x4D8240 clears
    /// the BACK button's bit 0x10, `[fe+0x17C]` = 0) — whichever screen set it.
    ///
    /// This is the **screen's** half. The callback-list object's destructors (the join screen's
    /// 0x50A260, the victory lists' 0x506C30 / 0x51A690) run when the object is deleted — the
    /// bottom layer's screen named another list ([`Self::fe_list_gone`]) — not when
    /// a screen goes.
    pub(crate) fn fe_screen_gone(&mut self, l: &nfs_game::layout::LayoutScreen) {
        // the options screen holds the Display screen's HUD widget: its `NOTINUSE` stores the
        // Display options (slot 22 0x4CB820, N02)
        if Self::fe_list_of(l) == "ID_OPTIONSSCREEN" {
            self.hud_cfg_apply();
        }
    }

    /// The callback-list object `[fe+0xE0]` deleted (0x4D2570 0x4D2763…0x4D2794, after 0x4D1BD0
    /// sent it out because the bottom layer's screen names another list): the destructors of the
    /// join screen's list (**0x50A260**, [`Self::net_join_screen_gone`]) and of the two victory
    /// lists (**0x506C30** / **0x51A690**: the back lock is lifted).
    pub(crate) fn fe_list_gone(&mut self, list: &str) {
        match list {
            "ID_COMMSJOINSCREEN" => self.net_join_screen_gone(),
            "ID_VICTORYSCREEN" | "ID_FDVICTORYSCREEN" => self.fe_back_lock(false),
            _ => {}
        }
    }

    /// The callback list `[fe+0xE0]` now in force: the **bottom layer's** screen's
    /// `data.callbacklist` (0x4D1BD0 / 0x4D2570, [`Self::fe_base_layout`]), `ID_NONE` for none.
    pub(crate) fn fe_list_now(&self) -> String {
        self.fe_base_layout().map(callback_list_of).unwrap_or_else(|| "ID_NONE".into())
    }

    /// **0x4D5BA0** (`true`) / **0x4D5BC0** (`false`): the back lock `[fe+0x17C]` and the bottom
    /// bar's BACK with it. Set by the victory screens' constructors (0x506BC0, 0x51A63F) and by the
    /// tournament screen's tick while it asks about an offer (0x505F5C); lifted by their
    /// destructors and by the tick once the offer is answered (0x506076). While set, neither
    /// BACK nor the `b` key goes back (0x4D6320, 0x4D29EF).
    pub(crate) fn fe_back_lock(&mut self, on: bool) {
        self.fe.nav.back_lock = on;
        self.bind_bottom_bar();
    }

    /// Everything a screen on show is bound to: the network lobby for the comms screens and the
    /// widgets' data. Run after an open, and when a closed layer uncovers the screen under it.
    fn fe_bind_screen(&mut self, name: &str) {
        // the screen becomes the one in use: `INUSE` reaches the HUD widget of `display.lay`'s
        // panel (slot 21 0x4CB650, N02)
        if name.eq_ignore_ascii_case("options") || name.eq_ignore_ascii_case("display") {
            self.hud_cfg_in_use();
        }
        // the four screens whose `callbacklist=` is a comms one need a browsing socket
        // a browsing socket is only wanted while looking for a game; once a session is
        // open the lobby screens read from it instead
        self.net.lobby = match is_network_layout(name) && self.net.session.is_none() {
            true => netplay::Lobby::open(&self.player_name)
                .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                .ok(),
            false => None,
        };
        // **`Comms.Positional`'s default row is the games screen's** (`network.md` §11.17): the base
        // constructor 0x509CE0 sets row 0 (0x509D96, slot 0x2C on `Comms.Positional`, named at 0x5DA6E8) and the
        // derived ones set theirs after it — `local` (0x50A950) keeps **0**, "input exchange"; `udpip`
        // (0x50B5F0) and `serverlist` (0x50B320) put it on 1; `PeerToPeerIP` does not touch it (the
        // accessor's own start, 1, 0x50BB1C). A session created from the screen carries it.
        if let Some(l) = self.net.lobby.as_mut() {
            l.positional = !name.eq_ignore_ascii_case("local");
        }
        self.bind_screen_widgets();
    }

    /// Every widget's content that comes from code, not the `.lay` file alone — pictures, lists,
    /// bars/knobs, panels, checks, maps. A screen open runs it (`fe_bind_screen`, above); so must
    /// a tab click that loads an `NFS5.PANEL` for the first time (`tab_click`'s two call sites —
    /// the real `NFS5.TABBUTTON`/`NFS5.GRAPHICTABBUTTON` handler and the `NFS_FE_TAB` screenshot
    /// diagnostic that mirrors it): `set_panel`'s load (0x4CEA90) and show (0x4CEC60) are not
    /// gated on which tab was already open, so the original re-evaluates a newly-shown panel's
    /// widgets the same way it does a freshly opened screen's (`layout.rs`'s `tab_click` doc,
    /// `career.md` §11.2c). One central hook here, not a hand-picked pair per call site — the
    /// glancestats miss (2026-09-29: `bind_showcase_pictures` was
    /// missing beside `bind_player_pictures`) showed a hand-picked pair only ever catches the one
    /// widget class someone happened to look for. Auditing the other RADIOGROUP+PANEL screens
    /// (`Controllers.lay`'s six panels, `Options.lay`'s five, `graphics.lay`'s two) this pass
    /// found the same gap for `NFS5.GouraudIndicatorBar` (`bind_indicator_bars`, set by
    /// `set_bar`/`set_knob` from an accessor — `forcefeedback.lay`'s meter, `advgraphics.lay`'s,
    /// `gamectrl.lay`'s `NFS5.CarSettingSlider` knob) which neither call site ever re-ran, so it
    /// stayed at whatever it last held (typically nothing, the widget's own default) until some
    /// unrelated accessor change elsewhere ran `bind_after_accessor`. `NFS5.AccessorWatch` and
    /// `NFS5.CAMERASELECT`/`NFS5.CAMERAPICTURE`'s own live state are not in this set — they read
    /// their accessor at **draw** time (`layout/widgets.rs`'s `AccessorWatch` case, `layout.rs`'s
    /// `camera_select_of`), so a newly-loaded panel already shows them correctly with no bind at
    /// all, same as the original's per-tick `FEAccessorWatch::tick` (0x4BD230) needing no gate.
    pub(crate) fn bind_screen_widgets(&mut self) {
        self.bind_car_logos();
        self.bind_lists();
        self.bind_checks();
        self.bind_indicator_bars();
        self.bind_chronicle_icons();
        self.bind_chronicle_group();
        self.bind_repair_bars();
        self.bind_usedcars_panels();
        self.bind_garage();
        self.bind_carsetup_gearratio();
        self.bind_laps_selector();
        self.bind_player_buttons();
        self.bind_parts_buttons();
        self.bind_mergedlist_panel();
        self.bind_showcase_pictures();
        self.bind_track_maps();
        self.bind_player_pictures();
        self.bind_end_of_era();
        self.bind_picker_wells();
    }

    /// `NFS5.ColorBox`'s eight wells are the profile's custom colours `[0x691D98]` (0x5019B0),
    /// loaded with `PlayerInfo` (0x501888) — into the screen's editor. *(Until 2026-09-26 every
    /// screen's editor started from eight zeros, whatever the profile held.)*
    fn bind_picker_wells(&mut self) {
        let Some(wells) = self.career.as_ref().map(|c| c.career.custom_colours) else { return };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_picker_wells(wells);
        }
    }

    /// …and back: every write the editor makes is 0x5019C0 on the global (0x4C47E6 on a press
    /// or drag, 0x4C49E6…0x4C4A00 on cancel), so the career — `PlayerInfo`'s last 32 bytes —
    /// follows it.
    pub(crate) fn store_picker_wells(&mut self) {
        let Some(wells) = self.fe.layout_preview.as_ref().map(|l| l.picker().wells) else { return };
        if let Some(c) = self.career.as_mut() {
            c.career.custom_colours = wells;
        }
    }

    /// A new layer over the screen on show (0x4D2100 with `offset > 0`): the screen stays
    /// loaded under it, covered (0x4D1E70 → 0x4E08C0), and is drawn beneath (0x4D2BD9…0x4D2C0C
    /// draws every layer's screen, bottom first).
    pub(crate) fn fe_push_under(&mut self) {
        if let (Some(mut l), Some(n)) = (self.fe.layout_preview.take(), self.fe.layout_name.take()) {
            l.set_covered(true);
            // only the screen in front draws the pointer
            l.set_cursor(None);
            self.fe.layout_under.push((n, l));
        }
    }

    /// The top layer closed (0x4D2570 pops it): the screen under it — never freed in the
    /// original — is on show again, uncovered (0x4D1E70 → 0x4E08A0), with 0x4D1BD0's music and
    /// the bottom bar (0x4D81C0 at 0x4D26E2).
    pub(crate) fn fe_pop_under(&mut self) {
        let Some((n, mut l)) = self.fe.layout_under.pop() else { return };
        l.set_covered(false);
        if let Some(old) = self.fe.layout_preview.replace(l) {
            self.fe_screen_gone(&old);
        }
        self.fe.layout_name = Some(n.clone());
        self.fe.layout_sel = 0;
        self.fe.layout_nav_used = false;
        self.fe.layout_open_combo = None;
        self.fe_bind_screen(&n);
        self.fe_apply_music();
        self.bind_bottom_bar();
    }
}
