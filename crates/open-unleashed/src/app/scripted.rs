//! TEST GLUE: the scripted-run helpers and diagnostics (`NFS_*` environment variables); none of it is in the original.

use crate::*;

impl App {
    /// Why a widget the sweep found blank is blank in the original as well, in the state the
    /// sweep runs in — or `None` when it is a gap of the port's. `icons` is how many of the
    /// screen's `NFS5.ChroniclePictureIcon`s still stand past the media list.
    fn sweep_blank_in_original(&self, class: &str, src: &str, icons: &mut usize) -> Option<&'static str> {
        let acc = src.strip_prefix("accessor=").unwrap_or("");
        match class {
            // 0x51AE00 answers −1 while mission 0 is unpassed; 0x4CF3ED / 0x4CF61D stop the draw
            "NFS5.BADGE" | "NFS5.MINIBADGE" if self.badge_stage().is_none() => Some("licence badge, no mission passed (0x51AE00 = -1, 0x4CF3ED / 0x4CF61D)"),
            "NFS5.MINIPLAYERPICTURE" if self.badge_stage().is_none() => Some("mini portrait, no mission passed (0x51AE00 = -1, 0x4CFCA3 -> 0x4CFD4D)"),
            // the bind 0x5087CF stops at `number >= [0x699D00]`, the media list's count
            "NFS5.ChroniclePictureIcon" if *icons > 0 => {
                *icons -= 1;
                Some("chronicle icon past the media list (0x5087CF)")
            }
            // `[0x699C98]` is written only for kind 2 (0x507642) and both callers pass 0 or 1
            // (0x5080F8, 0x508137); `chroniclemovie.lay` is named by no destination and no string
            "NFS5.MOVIEBOX" if src == "othersource=chronicle" => Some("chronicle clip [0x699C98] never built (0x507642; callers 0x5080F8 / 0x508137)"),
            // an empty lot: the logo's car is null (0x4F5980), ASKINGPRICE is the empty string
            // (0x4F61C6), and a bar at slot 6's 0 (0x4F6EC2) paints nothing (0x521F26 / 0x521F65 /
            // 0x521FB9)
            "NFS5.CarLogo" | "NFS5.GouraudIndicatorBar" | "NFS5.AccessorWatch"
                if acc.starts_with("UsedCar.Car")
                    && self.used_car().is_none()
                    && (class != "NFS5.AccessorWatch" || acc == "UsedCar.Car(ASKINGPRICE)") =>
            {
                Some("used-car lot empty (0x4F5980, 0x4F61C6, 0x4F6EC2 -> 0x521CB0)")
            }
            // an empty list: 0x4C53A0 clamps the panel to `count x 14` = 0 and the row loop
            // 0x4C670B stops at the count
            "NFS5.ListBox"
                if matches!(acc, "Parts.ShoppingList" | "Parts.RepairShoppingList" | "CDPlayer.SelectedList")
                    && self.list_rows(acc).is_some_and(|(r, _)| r.is_empty()) =>
            {
                Some("empty basket / play list (0x4C53A0, 0x4C670B)")
            }
            // row 0 is a section header whose arms leave the description's out-parameter unset
            // (0x5152EE, 0x515346): the original prints an uninitialised stack slot (0x515DCA)
            "NFS5.WORDWRAPACCESSORWATCH" if acc == "Parts.AllParts(DESC)" => Some("undefined in the original: uninitialised stack (0x515DCA)"),
            _ => None,
        }
    }

    /// `--layout-sweep`: load every shipped `.lay`, draw it once with the port's real accessors and
    /// print every widget class that produced no sprite at all, with the source it names. A widget
    /// lands here when the original places its content from code and the file carries neither a
    /// shape nor a string for it — which is exactly the list of front-end things still to port.
    pub(crate) fn run_layout_sweep(&mut self) {
        // The career is put into the states the blank count is otherwise waiting on — a car with
        // damage on every part, and a used-car market — so the widgets that need them are
        // measured instead of assumed. It only ever touches the in-memory career of this one
        // diagnostic run, never the save. (A diagnostic that answers with the starting state of a
        // synthetic career is measuring the harness, not the port: bare, the headline was 61, of
        // which 42 were `UsedCar.Car` and damage widgets blank only because nothing had been bought,
        // damaged or listed.)
        if let Some(c) = self.career.as_mut() {
            let parts: Vec<(i16, f32)> = {
                let car = c.career.garage.first().map(|o| o.car_id);
                let family = car.and_then(|id| c.tables.car(id)).and_then(nfs_game::career::part_family).map(str::to_string);
                c.tables
                    .parts
                    .iter()
                    .filter(|p| Some(p.family.as_str()) == family.as_deref())
                    .map(|p| (p.id, 40.0))
                    .collect()
            };
            if let Some(o) = c.career.garage.first_mut() {
                o.damage_pct = 37.0;
                o.damage = [37.0; 10];
                o.parts = parts.iter().map(|(id, _)| *id).collect();
                o.part_state = parts;
                o.mileage = 12_345;
                o.asking_price = Some(9_500);
            }
            // …and a used-car market, which the original only has after the first career race
            // (0x4F7DB0 → 0x4F80C0). Without it the used-car screens have nothing to read.
            if c.career.market.is_empty() {
                let mut seed = 0x1234_5678u32;
                let mut rand = move || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed >> 8
                };
                let CareerUi { career, tables, .. } = c;
                career.refresh_market(tables, &mut rand, &mut || nfs_game::layout::glints::crt_rand() as u32);
            }
        }
        let dir = self.paths.fe_data().join("Layouts");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.strip_suffix(".lay").or_else(|| n.strip_suffix(".LAY")).map(str::to_string)
            })
            .collect();
        names.sort();
        let mut total: std::collections::BTreeMap<(String, String), usize> = Default::default();
        let mut empty_combos: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mute_combos: std::collections::BTreeMap<String, usize> = Default::default();
        let mut mute_bars: std::collections::BTreeMap<String, usize> = Default::default();
        // `NFS5.3DCAR`s, counted out of the blank total and reported on their own
        let mut cars3d = 0usize;
        let mut ko_past = 0usize;
        let mut explained: std::collections::BTreeMap<&'static str, usize> = Default::default();
        let mut screens = 0usize;
        for name in &names {
            self.open_layout(name);
            if self.fe.layout_preview.is_none() {
                continue;
            }
            screens += 1;
            // the same probe fills the browser with one session, so the four `Comms.*` lists and
            // the watches beside them can be measured too. The record is what a `'d'` answer
            // carries (`nfs_net::comm::Describe`), filled as a two-player game would fill it.
            if let Some(lobby) = self.net.lobby.as_mut()
                && lobby.found.is_empty()
            {
                // a game as 0x509B00 / 0x48DD97 describe it: `HANS'S ~3.5`, positional (index 1),
                // forward (410) and the first track (300)
                let name = nfs_net::comm::desc::name_field("HANS'S ", "3.5");
                lobby.found.push((
                    std::net::SocketAddr::from(([127, 0, 0, 1], 1106)),
                    nfs_net::comm::Describe {
                        request: 0,
                        from: nfs_net::wire::Addr::ip([127, 0, 0, 1], 1106),
                        conn_dword: 0,
                        conn_words: [0, 0],
                        conn_raw: [0, 0],
                        name,
                        bytes: [0, 1, 0, 0],
                        words: [410, 300],
                        dwords: [0, 0],
                        player_limit: 2,
                        slot_mask: 1,
                    },
                ));
            }
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
            if nfs_game::config::cfg().nfs_hit_sweep.as_str().is_some()
                && let Some(l) = self.fe.layout_preview.as_mut()
            {
                for line in l.hit_sweep() {
                    println!("  hit {name:<20} {line}");
                }
            }
            if let Some(l) = self.fe.layout_preview.as_ref() {
                cars3d += l.car_3d().len();
            }
            // **The blank count cannot see an empty drop-down.** A combo always draws its own
            // box, so however empty its list is it never counts as a widget that drew nothing —
            // which is how 61 of the 97 went unnoticed until §4.91. Counted here as its own axis.
            if let Some(l) = self.fe.layout_preview.as_ref() {
                for w in l.combo_sliders() {
                    let a = w.accessor.clone().unwrap_or_default();
                    let n = self
                        .choice_rows(&a)
                        .or_else(|| self.combo_rows(&a))
                        .or_else(|| self.list_rows(&a))
                        .map(|(r, _)| r.len())
                        .unwrap_or(0);
                    if n == 0 {
                        *empty_combos.entry(a.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                    // and the caption is blind in the same way: a combo whose accessor answers
                    // nothing still draws its box, so the blank count never sees it either
                    if self.accessor_value(&a).is_none() {
                        *mute_combos.entry(a.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                }
                // the same blindness, one class along: a `NFS5.GouraudIndicatorBar` paints its two
                // bands whatever its accessor says, so a bar the port cannot answer sits at zero
                // and the blank count never sees it (§4.94)
                for (_, src, is_other) in l.indicator_bars() {
                    let v = if is_other {
                        self.compare_rating(&src)
                    } else {
                        let name = src.split_once('(').map_or(src.as_str(), |(b, _)| b.trim());
                        // the bind's own order (`bind_indicator_bars`): slot 6's integer first —
                        // the damage family answers it where its text is "n/a" (0x4F6B50)
                        match (self.accessor_number(&src), self.range_value(name)) {
                            (Some(v), _) => Some(v),
                            (None, Some((_, lo, hi))) if hi > lo => Some(0),
                            _ => self
                                .accessor_value(&src)
                                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                                .map(|v| v as i32),
                        }
                    };
                    if v.is_none() {
                        *mute_bars.entry(src.split('(').next().unwrap_or("").trim().to_string()).or_insert(0usize) += 1;
                    }
                }
            }
            let Some(l) = self.fe.layout_preview.as_ref() else { continue };
            // how many `NFS5.ChroniclePictureIcon`s of this screen stand past the media list
            let mut icons = {
                let media = self.chronicle_media().len() as i64;
                l.numbered_widgets("NFS5.ChroniclePictureIcon").into_iter().filter(|(_, n)| *n >= media).count()
            };
            for (class, src) in l.blank_widgets(nfs_game::layout::DESIGN.0 / nfs_game::layout::DESIGN.1, &self.fe.strings, &|a| self.accessor_value(a)) {
                // **A knockout row past the opponent count draws nothing in the original too.**
                // `locationsetup.lay` carries seven fixed rows of watches, rows 0…6, but
                // `FEKnockoutTrackNumberAccessor`'s getter reads `Game.NumKOOpponents`
                // (0x4BA786) and, at 0x4BA7CF, `cmp ebp, [esp+0x44]; jl …` — a row at or past
                // that count falls straight through to `mov eax, 0x5E8E50`, the shared empty
                // string. With the default count of 4 that is rows 4, 5 and 6, five fields
                // each. They are not blanks the port owes anyone; counting them made the sweep
                // chase its own harness, the same way `NFS5.3DCAR` did.
                if let Some(row) = ko_row_of(&src)
                    && row >= self.fe.menu.as_ref().map(|m| m.ko_opponents).unwrap_or(4)
                {
                    ko_past += 1;
                    continue;
                }
                // **Blank in the original too, in this state** — each with the address that makes
                // it so, `RE/notes/layouts.md` "The sweep's blanks, row by row" (2026-09-26). The
                // rest of the list is what the port still owes.
                if let Some(why) = self.sweep_blank_in_original(&class, &src, &mut icons) {
                    *explained.entry(why).or_default() += 1;
                    continue;
                }
                *total.entry((class, src)).or_default() += 1;
            }
        }
        let mut by_class: std::collections::BTreeMap<String, usize> = Default::default();
        for ((c, _), n) in &total {
            *by_class.entry(c.clone()).or_default() += n;
        }
        let mut rows: Vec<(usize, String)> = by_class.into_iter().map(|(c, n)| (n, c)).collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        println!("layouts swept: {screens} of {}", names.len());
        println!("widgets that drew nothing, by class:");
        for (n, c) in &rows {
            let srcs: Vec<String> = total.iter().filter(|((k, _), _)| k == c).map(|((_, s), n)| format!("{s} x{n}")).take(4).collect();
            println!("  {n:5}  {c:<32} {}", srcs.join(", "));
        }
        println!("total: {}", rows.iter().map(|(n, _)| n).sum::<usize>());
        // the second axis: containers the blank count is blind to (§4.92)
        let ec: usize = empty_combos.values().sum();
        println!("\ncombos with an empty drop-down: {ec}");
        let mut ecs: Vec<(usize, String)> = empty_combos.into_iter().map(|(a, n)| (n, a)).collect();
        ecs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &ecs {
            println!("  {n:4}  {a}");
        }
        let mc: usize = mute_combos.values().sum();
        println!("\ncombos whose caption answers nothing: {mc}");
        let mut mcs: Vec<(usize, String)> = mute_combos.into_iter().map(|(a, n)| (n, a)).collect();
        mcs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &mcs {
            println!("  {n:4}  {a}");
        }
        // `NFS5.3DCAR` is not in the blank count — it is never a 2D sprite in either build
        // (`FECar3DControl`'s draw 0x4C0990 calls no painter at all) — so it is reported here
        // instead of going missing.
        println!("\n3D car widgets (never a sprite, drawn by the 3D pass — 0x4C0990): {cars3d}");
        println!("knockout rows past Game.NumKOOpponents (empty in the original — 0x4BA7CF): {ko_past}");
        println!("blank in the original too, in this state: {}", explained.values().sum::<usize>());
        for (why, n) in &explained {
            println!("  {n:4}  {why}");
        }
        let mb: usize = mute_bars.values().sum();
        println!("\nindicator bars whose accessor answers nothing: {mb}");
        let mut mbs: Vec<(usize, String)> = mute_bars.into_iter().map(|(a, n)| (n, a)).collect();
        mbs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (n, a) in &mbs {
            println!("  {n:4}  {a}");
        }
        println!("\nby source:");
        let mut srcs: Vec<(usize, String, String)> = total.iter().map(|((c, s), n)| (*n, c.clone(), s.clone())).collect();
        srcs.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        for (n, c, s) in &srcs {
            println!("  {n:4}  {c:<30} {s}");
        }
    }

    /// One scripted left click at design point `(x, y)` (tooling: `NFS_FE_CLICKS`, and the
    /// `click:x,y` token of `NFS_FE_KEYS`, which runs in order with the `keys:` around it).
    pub(crate) fn fe_scripted_click(&mut self, x: f32, y: f32, event_loop: &ActiveEventLoop) {
        self.fe.cursor = Some((x, y));
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.script_cursor(Some((x, y)));
            // let any transition finish and the hover settle, as a player's pointer would have
            for _ in 0..60 {
                l.advance_transition(16);
            }
        }
        let before = self.fe.layout_preview.as_ref().map(|l| (l.hovered(), l.open_combo(), l.combo_row_at(x, y)));
        self.fe_left_button(event_loop, true);
        self.fe_left_button(event_loop, false);
        let after = self.fe.layout_preview.as_ref().map(|l| (l.hovered(), l.open_combo()));
        tracing::info!(x, y, ?before, ?after, open = ?self.fe.layout_open_combo, "NFS_FE_CLICKS click");
        for _ in 0..4 {
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.advance_transition(16);
            }
        }
    }

    /// The analogue devices the front end can see. `NFS5.DEADZONE`'s draw returns before painting
    /// anything when there is none (0x51B60D), which is what an empty list gives — and that is
    /// what this port has on a machine with no pad. `NFS_FE_PAD=1` stands one in, all eight axes
    /// present and each a tenth of the way along, so `deadzone.lay` can be looked at without one.
    pub(crate) fn fe_pads(&self) -> Vec<nfs_game::layout::FePad> {
        if nfs_game::config::cfg().nfs_fe_pad.as_str().is_some() {
            return vec![nfs_game::layout::FePad { analogue: true, axes: 0xFF, values: [32767; 8] }];
        }
        // `NFS_FE_DZ=<min>,<max>` puts the sixteen values somewhere other than the factory's
        // 0 / 0x80, so a still shows what a trimmed bar looks like
        self.gamepads
            .axis_sets()
            .into_iter()
            .map(|a| nfs_game::layout::FePad { analogue: a.mask != 0, axes: a.mask, values: a.values })
            .collect()
    }

    /// `NFS_FE_DZ=<min>,<max>` — the sixteen dead-zone values for a still, in the accessors' own
    /// 0…0x80. Without it they are the factory's 0 and 0x80 (0x51E546, 0x51E550).
    pub(crate) fn fe_deadzone() -> Option<[[i32; 2]; 8]> {
        let v = nfs_game::config::cfg().nfs_fe_dz.as_str()?;
        let (a, b) = v.split_once(',')?;
        let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        Some([[a, b]; 8])
    }

    /// diagnostic: a scripted press on the replay bar (`NFS_REPLAY_PRESS`), the pointer put in the
    /// cell's middle; 7 / 8 click `HReplaySave`'s buttons and 9 / 10 the overwrite question's. A
    /// cell of the bar's own only latches the press (`App::replay_bar_latch`), same as a real
    /// click — its caller runs `replay_bar_frame` right after, in the same tick, which answers it;
    /// `NFS_REPLAY_TYPE` is typed into the name once that opens `HReplaySave` (the caller's check).
    pub(crate) fn run_bar_script(&mut self) {
        if let Some(&(t, cell)) = self.hover_script.first()
            && self.tick >= t
            && self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
        {
            self.hover_script.remove(0);
            let (cells, _) = Hud::bar_geometry();
            self.fe.cursor = Some(cell.and_then(|c| cells.get(c)).map_or((0.0, 0.0), |&(x, y)| (x + 10.0, y + 9.0)));
            tracing::info!(tick = self.tick, cell = ?cell, "diagnostic: the pointer on the replay bar (NFS_REPLAY_HOVER)");
        }
        if let Some(&(t, cell)) = self.bar_script.first()
            && self.tick >= t
            && self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
        {
            self.bar_script.remove(0);
            let (cells, _) = Hud::bar_geometry();
            match cell {
                7 | 8 => self.replay_save_click(cell - 7),
                9 | 10 => self.replay_overwrite_click(cell - 9),
                _ => {
                    if let Some(&(x, y)) = cells.get(cell) {
                        self.fe.cursor = Some((x + 10.0, y + 9.0));
                        self.replay_bar_latch();
                    }
                }
            }
        }
    }
}

/// Set by `main` for a scripted run: the window must not take the focus (`NFS_NO_FOCUS`).
pub(crate) static NO_FOCUS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set by `main` from `NFS_PLAY_INTRO`: a scripted run (`--autotest`/`--screenshot`/
/// `--layout-sweep`/`--record`/`NFS_NO_FOCUS`) otherwise never plays the start-up movies (`main`'s
/// own `scripted` gate on `resume_inner`'s intro branch), so no scripted check ever walked the
/// intro → front-end handoff — the class of hang the release app hit after the movies (548aa02b).
/// This flag keeps the intro for a scripted run too, and `intro_event` auto-taps Esc every frame
/// (the same key path a player's skip takes) so the walk does not sit through the whole movie.
pub(crate) static PLAY_INTRO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `NFS_TRAFFIC=0..3`: the traffic density of a menu-less start (the one place the variable is read; the registry's
/// `traffic_density` otherwise).
pub(crate) fn env_traffic() -> Option<u32> {
    std::env::var("NFS_TRAFFIC").ok().and_then(|v| v.parse().ok())
}

/// `NFS_TRAFFIC_SEED=<n>`: pins the traffic / police seed (a repeatable run; the original seeds from `fe.txt`'s `rand_seed`).
pub(crate) fn env_traffic_seed() -> Option<u32> {
    nfs_game::config::cfg().nfs_traffic_seed.as_str().and_then(|v| v.parse().ok())
}

/// `NFS_COPS=0|1`: the cops of a menu-less start (`0` off, anything else on; the registry's `cops` otherwise).
pub(crate) fn env_cops() -> Option<bool> {
    std::env::var("NFS_COPS").ok().map(|v| v != "0")
}

/// **TEST GLUE** (not in the original): the still's geometry — `NFS_SHOT_SIZE=WxH` physical pixels
/// (default 1280x800, the goldens' size) and `NFS_SHOT_SCALE=<s>` (default 1, the HiDPI factor the
/// HUD's logical resolution is taken with, see [`App::hud_geometry`]).
pub(crate) fn shot_geometry() -> (u32, u32, f32) {
    let (w, h) = nfs_game::config::cfg().nfs_shot_size.as_str()
        .and_then(|v| {
            let (a, b) = v.split_once('x')?;
            Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?))
        })
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((1280, 800));
    let s = nfs_game::config::cfg().nfs_shot_scale.as_str().and_then(|v| v.trim().parse::<f32>().ok()).filter(|s| *s >= 1.0).unwrap_or(1.0);
    (w, h, s)
}

/// diagnostic (port-only): `NFS_RACE_PLACE=<p>` — the place (1-based) the player finishes at, in
/// any race. A scripted race's bot cannot drive past anyone, so this is how a scripted career race
/// reaches the winnings of a better place, a tournament won and the era's advance.
/// `NFS_RACE_PLACE=<p>:<tick>` also flags the player's car finished at race tick `tick`
/// ([`race_finish_diagnostic`]), so race over and the result pages come. Unset (the goldens,
/// every normal run) it does nothing; read once.
pub(crate) fn race_place_diagnostic() -> Option<usize> {
    race_place_spec().0
}

pub(crate) fn race_finish_diagnostic() -> Option<u64> {
    race_place_spec().1
}

fn race_place_spec() -> (Option<usize>, Option<u64>) {
    static SPEC: std::sync::OnceLock<(Option<usize>, Option<u64>)> = std::sync::OnceLock::new();
    *SPEC.get_or_init(|| {
        let Some(v) = nfs_game::config::cfg().nfs_race_place.as_str() else { return (None, None) };
        let (p, t) = v.split_once(':').map_or((v, None), |(p, t)| (p, Some(t)));
        let place = p.trim().parse::<usize>().ok().filter(|p| *p >= 1);
        (place, place.and(t.and_then(|t| t.trim().parse::<u64>().ok())))
    })
}

/// `NFS_RACE_PLACE`'s edit of `Race_finalize`'s output: the player (driver 0) moved to place `p`
/// of `order`, every racer's `+0xCD4` renumbered to its row, and the times dealt out again in the
/// new order — the timed racers' totals sorted ascending go to the timed rows first, so a
/// time-ranked club event ranks the same way (the lap times and the best lap stay each car's own).
pub(crate) fn place_player(order: &mut Vec<usize>, racers: &mut [nfs_game::race::RacerResult], p: usize) {
    let Some(at) = order.iter().position(|d| *d == 0) else { return };
    let d = order.remove(at);
    order.insert(p.clamp(1, order.len() + 1) - 1, d);
    let mut times: Vec<i32> = order.iter().filter_map(|d| racers.get(*d)).map(|r| r.time).collect();
    // no time (0) sorts last
    times.sort_by_key(|t| if *t == 0 { i32::MAX } else { *t });
    for (row, d) in order.iter().enumerate() {
        if let Some(r) = racers.get_mut(*d) {
            r.place = row as u32 + 1;
            r.time = times[row];
        }
    }
}

/// A `NFS_HUD_KEYS` script entry: an ordinary HUD toggle key, the pause (left through its
/// continue), or the pause left through the pause menu's restart instead — the port diagnostic
/// added for `replay.md` §18.12's restart audio-pick count.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ScriptedHudKey {
    Toggle(nfs_input::Action),
    Pause,
    PauseRestart,
    /// **TEST GLUE** (`pausequit`): the pause, left through the pause menu's quit (`leave_race`)
    PauseQuit,
    /// **TEST GLUE** (`cam`): a key pressed and released on that tick, as one keystroke
    Tap(nfs_input::Action),
    /// test glue for the network pad bytes (`NFS_HUD_KEYS`'s `horn` / `hornoff` / `reset`): a button pressed /
    /// released on that tick
    Pad(nfs_input::Action, bool),
}

impl App {
    /// **TEST GLUE, the start of a tick's car work**: the diagnostic placement `NFS_SMACK_RAM`, which sets a
    /// smackable down ahead of the car. Moved out of [`Self::simulate`] verbatim:
    /// it runs between the same two statements (after the exchange registration, before the 32 Hz
    /// block's `v.race_started = …`), and `simulate` borrows the car and the world only after it.
    pub(crate) fn apply_tick_glue(&mut self) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        // **TEST GLUE** `NFS_SMACK_RAM=k@t:deg`: on game tick `t` the player's car is sent along its own
        // heading at 12 m/s and the track's k-th smackable is set down 6 m ahead of it, on the car's
        // ground, its face turned `deg` degrees off head-on — the same scenario the reference run
        // sets up on the original by memory writes (`RE/notes/REFERENCE-2026-10-17-SIGN.md`)
        if let Some((k, t, deg)) = nfs_game::config::cfg().nfs_smack_ram.as_str().and_then(|s| {
            let (k, rest) = s.split_once('@')?;
            let (t, deg) = rest.split_once(':')?;
            Some((k.trim().parse::<usize>().ok()?, t.trim().parse::<u64>().ok()?, deg.trim().parse::<f32>().ok()?))
        }) && self.tick == t
            && let Some(p) = self.smackables.props.get_mut(k)
        {
            let fwd = v.orientation.row(2);
            let h = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::Z);
            let ang = |a: Vec3| a.z.atan2(a.x);
            let rows = [p.body.orientation.row(0), p.body.orientation.row(1), p.body.orientation.row(2)];
            let d = ang(-h) + deg.to_radians() - ang(rows[2]);
            let (s, c) = d.sin_cos();
            let rot = |a: Vec3| Vec3::new(a.x * c - a.z * s, a.y, a.x * s + a.z * c);
            let o = glam::Mat3::from_cols(rot(rows[0]), rot(rows[1]), rot(rows[2])).transpose();
            let low = p.body.body_points.iter().map(|q| q.y).fold(f32::MAX, f32::min);
            let at = Vec3::new(v.position.x + h.x * 6.0, v.ground_h - low + 0.088866, v.position.z + h.z * 6.0);
            p.relocate(&w.collision, at, o);
            v.velocity = Vec3::new(h.x * 12.0, v.velocity.y, h.z * 12.0);
            tracing::info!(target: "open_unleashed::app::simulate", k, ?at, ?h, "NFS_SMACK_RAM: the smackable set ahead of the car");
        }
    }

    /// **The scripted run** (`--autotest <secs>`, the diagnostic runs and the fixtures' races): the race run in a loop of
    /// one-tick iterations instead of the window's frames, with the `NFS_*` test glue the windowed frame does not have
    /// (the held scripted pause, the network leave, the pace, the drawn frames of `NFS_FX_FRAMES`, the stops, the
    /// traces), then the race's end as the interactive path files it. Moved out of `start_race` unchanged; the end of
    /// each iteration is [`Self::race_frame_epilogue`], the windowed frame's own. `true` when `start_race` must return
    /// (the run ended the program, left for the front end or started another race), `false` to go on to the screenshot.
    pub(crate) fn run_scripted_race(&mut self, secs: f32, event_loop: &ActiveEventLoop) -> bool {
        let ticks = (secs * TICKS_PER_SEC_F32) as u32;
        // diagnostic: with `NFS_FX_FRAMES` the scripted run draws a frame every other tick, so the cars' effects
        // (driven by each drawn frame, `nfs_game::car_effects`) run as in a windowed race
        let smack_frames = nfs_game::config::cfg().nfs_fx_frames.is_set();
        let mut pause_hold = 0;
        // **TEST GLUE** (not in the original): `NFS_NET_PACE=1` holds a scripted network race to
        // the game's own 64 Hz, so the two machines' tick counters stay comparable — the link
        // classification (0x490140) measures a packet's age against the game tick, and an unpaced
        // headless run lets one machine's tick run far ahead of the other's
        let exchange_run = self.net.session.as_ref().is_some_and(|n| n.clocked());
        let pace = (nfs_game::config::cfg().nfs_net_pace.is_set() && self.net.session.is_some() && !exchange_run).then(Instant::now);
        for t in 0..ticks {
            self.net_race_notice();
            // **TEST GLUE** `NFS_NET_LOCKSTEP=<D>`: the remote cars' blocks arrive at fixed receiver ticks (`NetPlay::lockstep_wait`)
            if let Some(n) = self.net.session.as_mut() {
                n.lockstep_wait(self.tick);
            }
            // an input-exchange race runs a tick when the frames and the host's clock allow it
            if exchange_run && !self.net_exchange_wait() {
                tracing::warn!(target: "open_unleashed::app::race_start", tick = self.tick, "the input exchange stopped: the scripted run ends");
                break;
            }
            if let Some(t0) = pace
                && let Some(wait) = (t0 + Duration::from_micros(u64::from(t) * 15_625)).checked_duration_since(Instant::now())
            {
                std::thread::sleep(wait);
            }
            // diagnostic `NFS_HUD_KEYS=tick:pause` in a network race: the press has asked to leave
            if std::mem::take(&mut self.scripted_hud.net_leave) {
                if !self.leave_race_as(false) {
                    tracing::info!(target: "open_unleashed::app::race_start", "network race left by a scripted pause press: no front end to return to");
                }
                event_loop.exit();
                return true;
            }
            // diagnostic `NFS_HUD_KEYS=tick:pause`: the headless run holds the pause for 64 of
            // its iterations (no tick runs), the audio paused and resumed as a frame would
            if self.scripted_hud.pause {
                self.sync_audio_pause();
                pause_hold += 1;
                if pause_hold >= 64 {
                    pause_hold = 0;
                    // `NFS_HUD_KEYS=tick:pauserestart` — the pause menu's restart
                    // (command 3, 0x415721) instead of its continue, for the restart's
                    // audio-pick count (`replay.md` §18.12): this recurses into a fresh
                    // `start_race` (`self.autotest` is already taken, so it runs the
                    // ordinary — not the scripted — set-up), so the outer loop's own
                    // `ticks`/`t` no longer describe a live race. The extra `sync_audio_pause`
                    // right after stands for the windowed loop's next `RedrawRequested` —
                    // the frame where the divergence's stray resume pick used to fire once
                    // `restart_race` had already made its own race-start pick — then the
                    // scripted run ends (scripted runs never stay open, as the ordinary
                    // race-end path does at its own `event_loop.exit()`).
                    if self.end_scripted_pause(event_loop) {
                        self.sync_audio_pause();
                        event_loop.exit();
                        return true;
                    }
                    // **TEST GLUE** `NFS_HUD_KEYS=tick:pausequit` — the pause menu's quit (its question
                    // answered yes): the race left for the front end (`leave_race`), then, in a
                    // scripted session (`NFS_AUTOTEST_RACES`), `PLAYGAME` again for the next race
                    if std::mem::take(&mut self.scripted_hud.quit) {
                        let game_type = self.fe.race_game_type.unwrap_or(self.fe.game_type);
                        if self.leave_race() && self.session_races > 1 {
                            self.session_races -= 1;
                            self.session_replayed = false;
                            tracing::info!(target: "open_unleashed::app::race_start", left = self.session_races, "session: quit from the pause menu, next race");
                            // the script's own keys are let go, as `session_step` does
                            self.input.release(Action::Throttle);
                            self.input.release(Action::SteerRight);
                            self.rearm_hud_keys();
                            self.autotest = Some(secs);
                            self.session_next_from_fe(game_type, event_loop);
                            return true;
                        }
                        event_loop.exit();
                        return true;
                    }
                    self.sync_audio_pause();
                }
                continue;
            }
            if smack_frames && t % 2 == 1 {
                // the windowed frame's billboard key first, as `RedrawRequested` takes it
                self.billboard_key_frame();
                self.rebuild_scene();
                self.shot_at_tick();
            }
            let phase = t as f32 / ticks as f32;
            if t % 2 == 0 {
                self.input.press(Action::Throttle);
                // **TEST GLUE** (not in the original): `NFS_NO_STEER=1` keeps the scripted run's
                // last third straight, for a still of the wheel and the hands at rest
                if !self.follow_road && phase > 0.66 && !nfs_game::config::cfg().nfs_no_steer.is_set() {
                    self.input.press(Action::SteerRight);
                }
            }
            self.simulate();
            self.start_auto_replay_if_due(event_loop);
            // diagnostic: `NFS_RESULTS_STOP=<n>[:<ticks>]` stops a scripted run `ticks` (96)
            // after the n-th result page of the queue (0-based) was first drawn, its slide-in
            // over, so `--screenshot` shows it (with `NFS_FX_FRAMES`, which draws the frames
            // that open and time the pages)
            if let Some((n, wait)) = nfs_game::config::cfg().nfs_results_stop.as_str().and_then(|v| {
                let (n, w) = v.split_once(':').map_or((v, "96"), |(n, w)| (n, w));
                Some((n.trim().parse::<u8>().ok()?, w.trim().parse::<u64>().unwrap_or(96)))
            }) && self.results.is_some()
                && self.pages.page == n
                && self.pages.seen == Some(n)
                && self.pages.open_tick.is_some_and(|t0| self.tick >= t0 + wait)
            {
                tracing::info!(target: "open_unleashed::app::race_start", tick = self.tick, page = ?self.result_pages().get(n as usize), "diagnostic: the scripted run stops on the result page");
                break;
            }
            // a scripted run's iteration is a frame of one tick
            if self.race_frame_epilogue(event_loop) {
                return true;
            }
            // NFS_AI_TRACE=<opponent index>: per-quarter-second trace of one AI car (target vs speed, pedals, slip)
            if t % 16 == 0
                && let Some(idx) = nfs_game::config::cfg().nfs_ai_trace.as_str()
                    .and_then(|v| v.parse::<usize>().ok())
                && let Some(o) = self.road.opponents.get(idx)
            {
                let target = match &o.controller {
                    Controller::Ai(a) => a.target_speed * 3.6,
                    _ => -1.0,
                };
                let (n_ahead, t_off, t_dot) = match &o.controller {
                    Controller::Ai(a) => (a.dbg_n, a.dbg_offset, a.dbg_dot),
                    _ => (0, 0.0, 0.0),
                };
                let car_off = self
                    .slices
                    .as_ref()
                    .map(|t| {
                        let sl = &t.slices[o.progress.slice];
                        (o.vehicle.position - sl.centre).dot(sl.lateral)
                    })
                    .unwrap_or(0.0);
                let lat = o.vehicle.v_local.x;
                tracing::info!(target: "open_unleashed::app::race_start", t = t as f32 / TICKS_PER_SEC_F32, slice = o.progress.slice, speed = (o.vehicle.speed() * 3.6) as i32, target = target as i32, thr = (o.vehicle.throttle * 100.0) as i32, brk = (o.vehicle.brake * 100.0) as i32, steer = o.vehicle.steer, n_ahead, t_off = (t_off * 10.0).round() / 10.0, t_dot = (t_dot * 100.0).round() / 100.0, car_off = (car_off * 10.0).round() / 10.0, v_lat = lat, slip = ?o.vehicle.wheels.iter().map(|w| (w.slip * 10.0) as i32).collect::<Vec<_>>(), locked = ?o.vehicle.wheels.iter().map(|w| w.locked as u8).collect::<Vec<_>>(), walls = o.wall_hits, "ai trace");
            }
            if t % 64 == 0
                && let Some(v) = &self.vehicle
            {
                tracing::info!(target: "open_unleashed::app::race_start", t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, damage = ?v.damage.map(|d| d as i32), opp = ?self.road.opponents.iter().map(|o| (o.progress.slice, (o.vehicle.speed() * 3.6) as i32, match &o.controller { Controller::Ai(a) => (a.target_speed * 3.6) as i32, _ => -1 }, o.vehicle.gear, o.wall_hits, o.resets, o.vehicle.wheels[0].steer as i32, (o.vehicle.throttle * 100.0) as i32, (o.vehicle.brake * 100.0) as i32, o.vehicle.position.x as i32, o.vehicle.position.z as i32)).collect::<Vec<_>>(), wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), police = ?self.road.police.as_ref().map(|p| p.cars.iter().filter(|c| c.active).map(|c| (c.slice, (c.speed * 3.6) as i32, (c.position - v.position).length() as i32, c.mode)).collect::<Vec<_>>()), violation = ?self.road.police.as_ref().map(|p| p.violation), traffic = ?self.road.traffic.as_ref().map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| (c.slice, c.dir as i32, (c.speed * 3.6) as i32, (c.lateral * 10.0) as i32, (c.position - v.position).length() as i32, (c.position.y - v.position.y) as i32)).collect::<Vec<_>>()), pos = ?v.position, "autotest");
            }
        }
        if self.session_step(event_loop, secs) {
            return true;
        }
        // 0x4EDF00's record arm runs on every race end, career or not, type 10 (the finale)
        // included ([`App::file_track_record`]) — `end_finished_race` calls it first for the
        // interactive path; this scripted fallthrough (the last race of an `--autotest`
        // session, or a single one) skipped it entirely until now, so a scripted finale (or
        // any other scripted race that runs off the end of its ticks here) filed the career
        // event but never the track record. *(Until 2026-09-27 this diagnostic path called
        // neither `file_track_record` nor `end_finished_race`.)*
        self.file_track_record();
        // no front end follows a scripted run, so no launch will save: write the buffer now (TEST GLUE)
        if let Err(e) = Records::flush_staged() {
            tracing::warn!(error = %e, "records save failed");
        }
        self.rebuild_scene();
        self.update_damage_decals();
        // 0x501340 + `LoadInfo` (0x4D3B52), same as the interactive `end_finished_race` path
        self.reload_career_at_fe_entry();
        self.settle_mission();
        if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
            let (order, times) = self.finishing_order();
            let clean = self.clean_racers();
            let zones = self.result_damage();
            if let Some(c) = self.career.as_mut() {
                if let Some(z) = zones {
                    c.park(z);
                }
                c.finish(&order, &times, &clean, &mut self.fe_rng);
                tracing::info!(target: "open_unleashed::app::race_start", order = ?order, message = c.message, money = c.career.money, "career event recorded");
            }
        }
        // a race the layout front end started (`NFS_FE_PLAYGAME`) goes back to it as the last
        // results page does, so the log names the screen 0x4D3420 opens
        if self.fe.race_game_type.is_some()
            && let Some(mut menu) = self.fe.menu_template.take()
        {
            self.knockout_race_end(&mut menu);
            self.career_opponents = None;
            self.player_grid_override = None;
            self.stop_race_audio();
            self.fe.menu = Some(menu);
            self.fe_after_race(false);
        }
        if self.screenshot.is_none() {
            // The original saves the replay from the race de-init chain after *every* race,
            // finished or not (replay.md §6.2) — so a scripted run saves on its way out too.
            self.save_replay();
            // scripted runs never stay open
            event_loop.exit();
        }
        false
    }

    /// **The still of `--screenshot`** (a scripted run's last step): the mirror's texture, the renderer's per-frame
    /// state the scripted loop never sets, the lens-flare warm-up, the offscreen render to `path` and the
    /// `NFS_SHOT_SEQ` extra; then the program ends. Moved out of `start_race` unchanged.
    pub(crate) fn take_screenshot(&mut self, path: String, event_loop: &ActiveEventLoop) {
        // a scripted run seldom redraws, so the mirror may not have a texture yet: render it,
        // and rebuild the scene once more so the HUD carries it
        if self.rear_mirror.is_some() {
            let first = self.mirror_tex.is_none();
            self.render_mirror();
            if first {
                self.rebuild_scene();
            }
        }
        // The scripted loop never calls `draw_frame`, which is otherwise the only place that
        // sets `set_trilinear` / `set_fakesort_mode` (both `SPECIALFX`-driven) each frame —
        // so a `--screenshot` run kept the renderer's construction-time defaults whatever
        // `Graphics.SpecialFX` said. Set them here too, or `NFS_CHOICE=Graphics.
        // SpecialFX=…` cannot show either one in a still.
        let fx_level = self.race.is_some().then(|| self.special_fx_level());
        let fakesort_mode = self.fakesort_mode(fx_level);
        if let Some(r) = self.renderer.as_mut() {
            r.set_trilinear(fx_level.is_some_and(|l| l >= 3));
            r.set_fakesort_mode(fakesort_mode);
        }
        // **TEST GLUE** (not in the original): `NFS_LENS_WARMUP=<n>` — the sun's lens flare runs over
        // frames and wall-clock (its stored block is taken on the first frame the sun is on
        // screen, its test every 200 ms, and the read-backs come back a frame later), and a
        // scripted run draws no frame; so the still is preceded by n frames drawn 210 ms apart
        // (`render_to_image`, discarded), the scene rebuilt for each as a frame does
        if let Some(n) = nfs_game::config::cfg().nfs_lens_warmup.as_str().and_then(|v| v.parse::<u32>().ok()) {
            for _ in 0..n {
                self.rebuild_scene();
                let _ = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1);
                std::thread::sleep(Duration::from_millis(210));
            }
            self.rebuild_scene();
        }
        // **TEST GLUE** (PORT-DEVIATION D46): `NFS_RACE_KEYS=<keys>` feeds keys through photo mode's and the pause menu's
        // handlers, `NFS_PHOTO=<ops>` enters photo mode and moves its camera before the still (and may save stills of
        // its own), see `photo.rs`
        if self.race_keys_script(event_loop) | self.photo_script() {
            self.rebuild_scene();
        }
        self.sync_gamma();
        let rgba = self
            .renderer
            .as_mut()
            .unwrap()
            .render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1)
            .expect("offscreen render");
        image::save_buffer(&path, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8)
            .expect("write png");
        tracing::info!(target: "open_unleashed::app::race_start", path, "screenshot written");
        // **TEST GLUE** (not in the original): `NFS_SHOT_SEQ=<n>` also writes the frames a windowed
        // run draws between this tick and the next — `<shot>.seq<k>.png` at the interpolation
        // fraction k / n (the mirror rebuilt with each, as a frame does), k = 0…n − 1
        if let Some(n) = nfs_game::config::cfg().nfs_shot_seq.as_str().and_then(|v| v.parse::<u32>().ok()) {
            for k in 0..n {
                self.test_alpha = Some(k as f32 / n as f32);
                self.rebuild_scene();
                self.render_mirror();
                let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1).expect("offscreen render");
                let sp = format!("{}.seq{k}.png", path.trim_end_matches(".png"));
                image::save_buffer(&sp, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8).expect("write png");
            }
            self.test_alpha = None;
        }
        event_loop.exit();
    }

    /// **TEST GLUE** `NFS_HUD_KEYS=tick:pause` / `pauserestart` / `pausequit`, the end of the held pause: the pause is
    /// let go as the pause menu's continue would, or — `pauserestart` — as its restart (command 3, 0x415721) does:
    /// the race made again ([`Self::restart_race`]). `true` when it restarted, which leaves the caller's own race
    /// behind. The windowed frame lets go after 64 pause ticks of its clock, a scripted run's loop after 64
    /// iterations.
    pub(crate) fn end_scripted_pause(&mut self, event_loop: &ActiveEventLoop) -> bool {
        self.scripted_hud.pause = false;
        self.paused = false;
        if std::mem::take(&mut self.scripted_hud.restart) {
            self.restart_race(event_loop);
            return true;
        }
        false
    }

    /// **TEST GLUE** `NFS_SHOT_TICKS=<prefix>@t1,t2,…` (with `NFS_FX_FRAMES`, whose drawn frames it
    /// takes): the frame drawn at each of those game ticks of the **last** race of a scripted run —
    /// not its replay — written as `<prefix>.t<tick>.png`, so a session's later race can be
    /// compared still by still with a first race in a fresh process.
    pub(crate) fn shot_at_tick(&mut self) {
        let Some(spec) = nfs_game::config::cfg().nfs_shot_ticks.as_str() else { return };
        let Some((path, ticks)) = spec.rsplit_once('@') else { return };
        let replaying = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        if self.session_races > 1 || replaying || !ticks.split(',').any(|t| t.trim().parse::<u64>().ok() == Some(self.tick)) {
            return;
        }
        let out = format!("{path}.t{}.png", self.tick);
        // **TEST GLUE**: a scripted run draws no frames, and the mirror's texture is made by the frame's
        // pass (`render_mirror`, before the main view) — without it the still carried the empty tray. The
        // first still makes the texture and rebuilds the scene so the HUD holds it; later ones refresh it
        // in place ([`Self::take_screenshot`] does the same).
        if self.rear_mirror.is_some() {
            let first = self.mirror_tex.is_none();
            self.render_mirror();
            if first {
                self.rebuild_scene();
            }
        }
        let (w, h, _) = shot_geometry();
        if let Some(r) = self.renderer.as_mut()
            && let Ok(rgba) = r.render_to_image(&self.scene, &self.camera, w, h)
        {
            image::save_buffer(&out, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
            tracing::info!(path = out, tick = self.tick, eye = ?self.camera.position, target = ?self.camera.target, view = self.camera_view(), "NFS_SHOT_TICKS: still written");
        }
    }
}

/// The `NFS_HUD_KEYS` script, `tick:key,…` (moved here from `main`'s constructor so that a
/// scripted session, `NFS_AUTOTEST_RACES`, can arm it again for every race: `App::rearm_hud_keys`).
pub(crate) fn parse_hud_keys(s: &str) -> Vec<(u64, ScriptedHudKey)> {
    s.split(',')
        .filter_map(|e| {
            let (t, k) = e.trim().split_once(':')?;
            let a = match k.trim() {
                "F5" | "f5" => ScriptedHudKey::Toggle(nfs_input::Action::ToggleHud),
                "F6" | "f6" => ScriptedHudKey::Toggle(nfs_input::Action::ToggleRearView),
                "F7" | "f7" => ScriptedHudKey::Toggle(nfs_input::Action::ToggleMap),
                "F8" | "f8" => ScriptedHudKey::Toggle(nfs_input::Action::HudTransparency),
                "2" => ScriptedHudKey::Toggle(nfs_input::Action::CycleMap),
                // port diagnostic: the pause key, the menu left through its continue
                // after 64 pause ticks (`ScriptedHud::pause`)
                "pause" | "P" => ScriptedHudKey::Pause,
                // port diagnostic: the pause key, the menu left through its restart
                // (command 3, 0x415721) instead — `replay.md` §18.12
                "pauserestart" => ScriptedHudKey::PauseRestart,
                // test glue: the pause key, the menu left through its quit (`leave_race`)
                "pausequit" => ScriptedHudKey::PauseQuit,
                // test glue: one press of the camera key (`I_CYCLECAMERA`)
                "cam" => ScriptedHudKey::Tap(nfs_input::Action::CameraNext),
                // network test glue: the horn button held from that tick / released
                "horn" => ScriptedHudKey::Pad(nfs_input::Action::Horn, true),
                "hornoff" => ScriptedHudKey::Pad(nfs_input::Action::Horn, false),
                "reset" => ScriptedHudKey::Pad(nfs_input::Action::Reset, true),
                _ => return None,
            };
            Some((t.trim().parse().ok()?, a))
        })
        .collect()
}
