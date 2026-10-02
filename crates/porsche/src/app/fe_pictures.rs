//! The front end's picture and indicator binders: showcase, end of era, track maps, player pictures, bars.

use crate::*;

impl App {
    /// Fill the showcase screens' picture mosaic. `NFS5.ShowcasePicture` is only a place — its
    /// template has no drawable part — and the `number` its `.lay` block carries is the entry of
    /// the selected car's own pack, `FEData/Showcase/<key>.fsh`, where `key` is the catalogue
    /// record's +0xE3 string (86 of the 87 racing records name a pack that is on disk). The match
    /// is exact: `general.lay` and `glancestats.lay` give `number` 0…7, and each widget's `size`
    /// is entry `number`'s own width and height. CONFIRMED.
    ///
    /// **Which car, fixed 2026-09-29 (pass 10 CONFIRMED PORT DEFECT, same root cause as
    /// [`App::showcase_text`]).** `0x508B70` — the routine every `SHOWCASE.*`-family click handler
    /// calls in the same breath as the `[0x699FE0]` setter `fn compare_cars`/`CarCompare1` already
    /// reads (`0x50E6A0`) — builds this same `<car+0xE3>.fsh` pack path (falling back to
    /// `993tCo96.fsh` if the car's own pack is missing on disk, `0x59DC30`) for the **same** car
    /// `Showcase.Text`'s own `.loc` load uses. So the pack is the compare-column car's, not
    /// `zMergedList.Car` unconditionally — see `showcase_text`'s own doc comment for the address
    /// trail (`0x508B70`/`0x50E5C0`/`0x50DC40`).
    ///
    /// **`glancestats.lay`'s empty photo box, fixed 2026-09-29.** Not a data
    /// or car-routing bug: `356-14.fsh`'s eight entries all decode (dumped with `asset-inspector`
    /// — `356-14_000_Laye.png`…`356-14_007_Laye.png` are all valid photo quadrants). The bug is
    /// that this function was never called again after the widget existed. `showcase.lay` embeds
    /// a `NFS5.RADIOGROUP` (`showcasetabs.lay`) and four `NFS5.PANEL`s (`ID_general_panel`
    /// `filename=general`, `ID_glancestats_panel` `filename=glancestats`, plus `chassis`/
    /// `performance`); only `showcasetabs.lay`'s `ID_general_button` ships `state=selected`
    /// (§7), so `LayoutScreen::load_children` (this file's own doc on it) loads only
    /// `general.lay`'s group up front — `general.lay`'s `NFS5.ShowcasePicture`s (`number` 4…7)
    /// get their `set_pack_picture` from the screen-open call below (`fe_bind_screen`).
    /// `glancestats.lay`'s group (`number` 0…3) is loaded lazily, by `LayoutScreen::set_panel`,
    /// the first time `ID_glancestats_button` is clicked (`tab_click`) — and the click handler
    /// re-ran `bind_player_pictures` for exactly this reason (a panel reached only by a tab
    /// switch needs its picture-from-code widgets re-bound) but not this function, so
    /// `glancestats`' widgets never got a `set_pack_picture` call at all. Added the call beside
    /// `bind_player_pictures` at both re-bind sites — the real `NFS5.TABBUTTON`/
    /// `NFS5.GRAPHICTABBUTTON` click handler and the `NFS_FE_TAB` screenshot diagnostic that
    /// mirrors it.
    pub(crate) fn bind_showcase_pictures(&mut self) {
        // `NFS5.CAMERAPICTURE`'s `shp` part is in no state list, so nothing preloads it (§4.129)
        let cams: Vec<i64> = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.numbered_widgets("NFS5.CAMERAPICTURE").into_iter().filter_map(|(i, _)| l.layout.elements.get(i).and_then(|e| e.int("picturenum"))).collect())
            .unwrap_or_default();
        if !cams.is_empty()
            && let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut())
        {
            for n in cams {
                l.load_pack_entry(r, &self.paths, "Art", nfs_game::layout::CAMERA_PICTURES, n.max(0) as usize);
            }
        }
        // `NFS5.ANIMSHOWCASEPICTURE` needs all eight entries of `show.fsh` — four pictures and
        // four labels — which its own bind 0x4BDB90 loads up front (§4.127)
        if self.fe.layout_preview.as_ref().is_some_and(|l| !l.numbered_widgets("NFS5.ANIMSHOWCASEPICTURE").is_empty())
            && let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut())
        {
            for n in 0..8 {
                l.load_pack_entry(r, &self.paths, "Art", nfs_game::layout::SHOWCASE_ANIM, n);
            }
        }
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let want = l.numbered_widgets("NFS5.ShowcasePicture");
        if want.is_empty() {
            return;
        }
        let pack = self
            .logo_car(Some("CarCompare1"), None)
            .map(|c| format!("{}.fsh", c.showcase))
            .unwrap_or_default();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, number) in want {
            l.set_pack_picture(r, &self.paths, "Showcase", index, &pack, number.max(0) as usize);
        }
    }

    /// Fill the three map widgets.
    ///
    /// `NFS5.TrackSmall2dSpline` (`tournament.lay`) shows the tournament's events one after the
    /// other; the art is each track's own `nfs5.trk` `+0x58` name in `FEData/TrackArt`, and the
    /// marker is entry 1 or entry 2 by the event's `backwards` byte — see [`TrackMapEvent`].
    ///
    /// `NFS5.PRIZERACEMAP` (`VictoryMessage.lay`) and `NFS5.FACTORYMAP` (`missionmap.lay`) are one
    /// picture each. The prize map's pack is the era's (0x506430: `mclass`, `mgold`, `mmod` for the
    /// `+4` values 0x209, 0x20A, 0x20B of 0x504800 — the highest **completed** era,
    /// `Career::completed_era`; *until 2026-09-27 this said "the first era that still has an
    /// unfinished tournament" and took the current era*). The factory map's is the
    /// current mission's code at `fac+0x3F` — `1m01.fsh` and the rest, in `FEData/Factory` — from
    /// the first mission whose result is still 0 (0x51AE80).
    /// `ID_ENDOFERASCREEN` (`endofera.lay`): build `FEEndOfEraCallBackList`'s show from the career
    /// the way its constructor 0x4FD4F0 reads it (`RE/notes/controls.md` §4.150a). The tables are
    /// the original's globals, so without a career the screen still has them and a fresh career's
    /// state.
    pub(crate) fn bind_end_of_era(&mut self) {
        use nfs_formats::career::{Era, TournamentKind};
        use nfs_game::end_of_era::{EndOfEra, EraInput, TITLE_TEXT};
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let is_show = l.layout.elements.first().and_then(|e| e.get("callbacklist")).is_some_and(|c| c.eq_ignore_ascii_case("ID_ENDOFERASCREEN"));
        if !is_show {
            self.fe.era_music = false;
            return;
        }
        // The show's constructor 0x4FD4F0 starts the victory music before anything else:
        // 0x4FD531 / 0x4FD538 push **2** and `"vic*"` into `MusicPlay` 0x4AE4D0, which is mode 2 —
        // the playlist becomes that one pick (`engine-audio.md` §8.3). Once per opening.
        if !self.fe.era_music {
            self.fe.era_music = true;
            self.fe_music(|m, a| m.play_pattern(a, "vic*", 2));
        }
        let loaded;
        let fresh;
        let (tables, state) = match self.career.as_ref() {
            Some(c) => (&c.tables, &c.career),
            None => {
                let Ok(t) = nfs_game::career::CareerTables::load(&self.paths) else { return };
                loaded = t;
                fresh = nfs_game::career::Career::default();
                (&loaded, &fresh)
            }
        };
        let idx = |e: Era| match e {
            Era::Classic => 0usize,
            Era::Golden => 1,
            Era::Modern => 2,
        };
        // 0x503ED0: every era record not hidden (`+0xE4 != 2`) completed
        let done = |e: Era| tables.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(e) && !t.is_hidden()).all(|t| state.state(t.id).completed);
        let current = state.era;
        // 0x5032E0: Classic and Golden answer Classic, Modern answers Golden
        let previous = if current == Era::Modern { Era::Golden } else { Era::Classic };
        // 0x504800: the highest era found done; with none, a stack word the movie table maps to 0
        // (MEDIUM — `+0x30C` stays 0 unless that word happens to be an era id)
        let completed = state.completed_era(tables).map(idx).unwrap_or(0);
        // 0x4E06D0 walks the loaded tables in order and the front end's own is first, so an id the
        // file also gives the HUD reads the front end's row
        let text = |id: u32| self.fe.strings.get(id).unwrap_or_default().to_string();
        let tournaments = [Era::Classic, Era::Golden, Era::Modern].map(|e| {
            tables.tournaments.iter().filter(|t| t.kind == TournamentKind::Era(e) && t.visibility == 0).map(|t| text(t.title_string_id)).collect::<Vec<_>>()
        });
        let input = EraInput {
            previous: idx(previous),
            current: idx(current),
            completed,
            current_done: done(current),
            era_names: [text(0x209), text(0x20A), text(0x20B)],
            cars: tables.catalog.iter().map(|c| (c.year as i32, c.display_name.clone())).collect(),
            tournaments,
            title: text(TITLE_TEXT),
            centre: (320, 240),
        };
        // the front end's generator is seeded from the clock (0x4E3182); a fixed seed keeps a
        // still reproducible, as the movie circle's overlay does (a presentation choice, not a
        // rule: the clock seed makes the original's draws unobservable)
        let show = EndOfEra::build(&input, &mut nfs_game::opponents::FeRng::new(0));
        let (Some(r), Some(l)) = (self.renderer.as_mut(), self.fe.layout_preview.as_mut()) else { return };
        l.set_end_of_era(r, &self.paths, show);
    }

    pub(crate) fn bind_track_maps(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let splines = l.numbered_widgets("NFS5.TrackSmall2dSpline");
        let prize = l.numbered_widgets("NFS5.PRIZERACEMAP");
        let factory = l.numbered_widgets("NFS5.FACTORYMAP");
        if splines.is_empty() && prize.is_empty() && factory.is_empty() {
            return;
        }
        let events: Vec<TrackMapEvent> = self
            .career
            .as_ref()
            .and_then(|c| {
                let t = c.current()?;
                Some(
                    t.events
                        .iter()
                        .filter_map(|ev| {
                            let trk = c.tables.track(ev.track_id)?;
                            let name = self
                                .fe.strings
                                .get(trk.name_string_id + 20)
                                .map(str::to_string)
                                .unwrap_or_else(|| c.tables.track_name(ev.track_id).to_string());
                            Some(TrackMapEvent { art: trk.art_small.clone(), backwards: ev.backwards, mirror: ev.direction_flag != 0, name })
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        // 0x506439: `0x504800()`, the highest completed era; an id outside the three (none
        // completed) leaves the pack name unwritten (0x506449 `jne` past the sprintf; the buffer
        // `[this+0x144]` is not cleared by the ctor 0x5061E0, which sets `[+0x140]` only) — the port
        // keeps `mclass` there. UNREACHABLE in the game's own flow (2026-09-30): the widget
        // is only on `VictoryMessage.lay`, which only `VictoryRoom`'s creator opens (0x506BD4), and
        // `VictoryRoom` is only opened by the era step's race end (0x4D4809, gated on 0x505800 =
        // `[0x691E81]`), a flag 0x503AD0 sets **only when every visible tournament of the era is
        // completed** (`career.md` §10.2) — at which point 0x504800 answers that era or a later one.
        // Reachable only by the port's direct `--fe=VictoryMessage` on a career with no era done;
        // the original's pack name there is what the unwritten heap buffer held (indeterminate).
        let era_pack = self
            .career
            .as_ref()
            .and_then(|c| c.career.completed_era(&c.tables))
            .map(|e| match e {
                nfs_formats::career::Era::Classic => "mclass.fsh",
                nfs_formats::career::Era::Golden => "mgold.fsh",
                nfs_formats::career::Era::Modern => "mmod.fsh",
            })
            .unwrap_or("mclass.fsh")
            .to_string();
        let mission_pack = self
            .career
            .as_ref()
            .and_then(|c| Some(format!("{}.fsh", c.tables.missions[c.career.current_mission(&c.tables)?].code)))
            .unwrap_or_default();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, _) in splines {
            l.set_track_map(r, &self.paths, index, &events);
        }
        for (index, _) in prize {
            l.set_pack_picture(r, &self.paths, "TrackArt", index, &era_pack, 0);
        }
        for (index, _) in factory {
            l.set_pack_picture(r, &self.paths, "Factory", index, &mission_pack, 0);
        }
    }

    /// The licence stage both badge classes draw, `None` where the original answers −1.
    pub(crate) fn badge_stage(&self) -> Option<usize> {
        // 0x51AE00: the first mission whose `+0xE8` is still 0 (0x51AE1C); **none left — or no
        // missions at all — answers 3** (0x51AE2E, 0x51AE51), the fourth licence; mission 0 itself
        // unpassed answers −1 (0x51AE65…0x51AE6C) and both badge draws stop (0x4CF3ED, 0x4CF61D);
        // otherwise that mission's `stage` minus one (0x51AE71). *(Until 2026-09-26 a finished
        // career drew no badge at all.)*
        self.career.as_ref().and_then(|c| match c.career.current_mission(&c.tables) {
            None => Some(3usize),
            Some(0) => None,
            Some(i) => Some((c.tables.missions[i].stage.max(1) - 1).clamp(0, 3) as usize),
        })
    }

    /// Fill the player's portrait and the Factory Driver licence card.
    ///
    /// **The portrait index is the leading digit of the profile's file name.**
    /// `FEPlayerPicture::update` (0x4CFB70) asks its `UserList.UserList` accessor (0x4CFAE6) for
    /// the selected option's text and takes `text[0] - '0'`, clamped to 0..8 (0x4CFBA0…0x4CFBCC) —
    /// and that text is `sprintf("%d%s", picture, name)`, the same `.sav` stem 0x4DEDA0 builds. The
    /// picture is entry `n` of `FEData/Art/ALLdrivr.fsh` (0x4DBCEC), whose nine 54×64 entries are
    /// `M01`…`M04`, `F01`…`F04` and `driv`. This closes the UNKNOWN `RE/notes/layouts.md` left on
    /// where the chosen index lives.
    ///
    /// The licence card is `license`, `license2`, `license3` or `license4` (the table at 0x5D8E50)
    /// by **the stage of the mission the career is on, minus one** — 0x51AE00 finds the first
    /// mission whose `+0xE8` result is still 0, answers −1 when that is mission 0 (nothing passed
    /// yet, so no licence), reads `stage` at `+8` and subtracts 1, and answers 3 when every mission
    /// is done. The name under it is printed in the colour at 0x5D8E60 for that stage — white,
    /// 0xD20000, black, black — and the rank line is `FEStrings[3496 + stage]`, or `3485 + stage`
    /// when the portrait is one of the four female ones (0x4CF56E: `-11` when 0x5024E0 answers
    /// 4…7). 3496 is `IDS_JUNIORTESTDRIVER` and 3485 `IDS_FEMALEJUNIORTESTDRIVER`. CONFIRMED.
    pub(crate) fn bind_player_pictures(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let pictures: Vec<usize> = ["NFS5.PLAYERPICTURE", "NFS5.MINIPLAYERPICTURE"].iter().flat_map(|c| l.numbered_widgets(c)).map(|(i, _)| i).collect();
        let badges: Vec<usize> = ["NFS5.BADGE", "NFS5.MINIBADGE"].iter().flat_map(|c| l.numbered_widgets(c)).map(|(i, _)| i).collect();
        if pictures.is_empty()
            && badges.is_empty()
            && ["NFS5.FACTORYPIC", "NFS5.MOVIEBOX", "NFS5.MOVIECIRCLE", "NFS5.HUD"].iter().all(|c| l.numbered_widgets(c).is_empty())
        {
            return;
        }
        let profile = self.career.as_ref().map(|c| c.profile.clone());
        let picture = profile.as_ref().map_or(0, |p| p.picture.min(8) as usize);
        let name = profile.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let stage = self.badge_stage();
        let rank = stage
            .and_then(|st| {
                let base = if (4..=7).contains(&picture) { 3485 } else { 3496 };
                self.fe.strings.get(base + st as u32).map(str::to_string)
            })
            .unwrap_or_default();
        let colour = match stage.unwrap_or(0) {
            0 => [1.0, 1.0, 1.0, 1.0],
            1 => [0xD2 as f32 / 255.0, 0.0, 0.0, 1.0],
            _ => [0.0, 0.0, 0.0, 1.0],
        };
        let pack = stage.map(|st| if st == 0 { "license.fsh".to_string() } else { format!("license{}.fsh", st + 1) });
        // `NFS5.FACTORYPIC` (0x51A080): one of the six 54×64 faces of `FactoryPeople.fsh` —
        // `bill`, `step`, `diet`, `fran`, `klau`, `rolp`, in the narrator order the missions use.
        // With `source=profile` (`teamprofile.lay`) the entry is `Factory.ProfileNum` through the
        // jump table at 0x51A1A8, which permutes 0…5 to **2, 0, 3, 1, 4, 5** — profile 0 is Dieter,
        // which is what `FEStrings[3501]` says too. With anything else (`missionbrief.lay` says
        // `source=nothingmuch`) it is the current mission's `+0x1E`, the narrator (0x51A184).
        let faces: Vec<(usize, usize)> = l
            .numbered_widgets("NFS5.FACTORYPIC")
            .into_iter()
            .map(|(i, _)| {
                const PROFILE_FACE: [usize; 6] = [2, 0, 3, 1, 4, 5];
                let by_profile = l.layout.elements[i].get("source").is_some_and(|s| s.eq_ignore_ascii_case("profile"));
                let entry = if by_profile {
                    PROFILE_FACE[(self.fe.factory_profile as usize).min(5)]
                } else {
                    self.career
                        .as_ref()
                        .and_then(|c| Some(c.tables.missions[c.career.current_mission(&c.tables)?].narrator))
                        .unwrap_or(0)
                        .clamp(0, 5) as usize
                };
                (i, entry)
            })
            .collect();
        // `NFS5.MOVIEBOX`. 0x4CD190 reads the widget's `data.othersource`: `chroniclemain` plays
        // `<Movies>/<0x507390()>.mad`, and 0x507390 maps the chronicle's model group — the same
        // `[0x5DBA44]` the picture names use — to `ch5`, `Ch3`, `Ch1`, `Ch2`, `Ch4` for groups 0…4
        // and `Ch0` past them. `chronicle` instead plays the per-entry clip 0x507662 builds with
        // `"%s%c%c%02d%d%c.mad"`, the picture's own name with the other extension; this
        // installation ships none of those, so that widget finds no file, as the original would.
        //
        // `NFS5.MOVIECIRCLE` plays one as well, and its activate picks the file by the same
        // `othersource` key (0x4CD1C1, 0x4CD20B): **`demoloop`** goes through 0x4CD090, which
        // builds `"%s%s.mad"` from a rotation — `[this+0x1B8]` picks `er1pc-av`, `er2pc-av`,
        // `er3pc-av` or `er4pc-av` for 1…4 through the jump table at 0x4CD17C and **`ch6`** for
        // anything else (0x4CD142) — and the activate sets that index to the `strcmp` result,
        // **0**, so the first clip is `ch6`. **`careerfirst`** takes `er1pc-av` outright
        // (0x4CD247). All five ship in `FEData/Movies`.
        //
        // The port plays the first clip and does not rotate: what advances `[this+0x1B8]` is the
        // player's own end-of-clip, which this port's loop does not signal.
        // `data.audio` (default 1, `MovieBox.ctr` / `MovieCircle.ctr`) is `FEMovie`'s own
        // `[+0x75]` — whether the shared tick 0x4E25A0 (`frontend-3d.md` §11) may skip frames to
        // catch up after a stall (`MoviePlayer::catchup`).
        let movies: Vec<(usize, String, bool)> = l
            .numbered_widgets("NFS5.MOVIEBOX")
            .into_iter()
            .filter_map(|(i, _)| {
                const CHRONICLE_MOVIE: [&str; 5] = ["ch5", "Ch3", "Ch1", "Ch2", "Ch4"];
                let src = l.layout.elements[i].get("othersource")?;
                let audio = l.layout.elements[i].get("audio") != Some("0");
                src.eq_ignore_ascii_case("chroniclemain").then(|| (i, format!("{}.mad", CHRONICLE_MOVIE[self.fe.chronicle_group.min(4)]), audio))
            })
            .chain(l.numbered_widgets("NFS5.MOVIECIRCLE").into_iter().filter_map(|(i, _)| {
                let file = match l.layout.elements[i].get("othersource")? {
                    s if s.eq_ignore_ascii_case("demoloop") => "ch6.mad",
                    s if s.eq_ignore_ascii_case("careerfirst") => "er1pc-av.mad",
                    _ => return None,
                };
                let audio = l.layout.elements[i].get("audio") != Some("0");
                Some((i, file.to_string(), audio))
            }))
            .collect();
        let huds = l.numbered_widgets("NFS5.HUD");
        // the preview's pieces for the options as they stand (the frame hook keeps them current)
        let hud_preview = if huds.is_empty() { None } else { Some(self.hud_preview_pieces()) };
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for index in pictures {
            l.set_pack_picture(r, &self.paths, "Art", index, "ALLdrivr.fsh", picture);
        }
        for (index, entry) in faces {
            l.set_pack_picture(r, &self.paths, "Art", index, "FactoryPeople.fsh", entry);
        }
        for (index, file, audio) in movies {
            l.set_movie(r, &self.paths, index, "Movies", &file, audio);
        }
        // `NFS5.HUD`'s own art: 0x4CB65D builds `<Art>hud.fsh` and 0x4CB685 keeps all **fifteen**
        // of its entries at `widget + 0x178 + 4i`. Entry 0 is `disp`, 240×180 — the size
        // `display.lay` gives the widget — and the draw blits it first (0x4CBB93).
        let any_hud = !huds.is_empty();
        for (index, _) in huds {
            l.set_pack_picture(r, &self.paths, "Art", index, "hud.fsh", 0);
        }
        // …and the fourteen pieces the draw lays over it (OPTS, `hud_preview_pieces`)
        if any_hud {
            l.load_hud_preview_art(r, &self.paths);
        }
        if let Some((pieces, kx, ky)) = hud_preview {
            l.set_hud_preview(pieces, kx, ky);
        }
        if let Some(pack) = pack {
            for index in badges {
                l.set_badge(r, &self.paths, index, &pack, &name, &rank, colour);
            }
        }
    }

    /// Fill the `NFS5.GouraudIndicatorBar`s of whatever screen is up. Only the car-compare screen
    /// has any, and its bars read the same `<n>CarCompareCar<m>` sources as its numbers.
    pub(crate) fn bind_indicator_bars(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let wanted: Vec<(usize, Option<i32>)> = l
            .indicator_bars()
            .into_iter()
            .map(|(i, src, is_other)| {
                let v = if is_other {
                    self.compare_rating(&src)
                } else {
                    // an accessor-driven bar: a range setting scaled onto the bar's own 0..100
                    // (the widget's stops are `c0`, `c50` and `c100`, and its draw compares against
                    // the literal 0x64 at 0x4CA724), otherwise whatever number the accessor's text
                    // carries — the damage accessors already answer in per cent
                    let name = src.split_once('(').map_or(src.as_str(), |(b, _)| b.trim());
                    // a bar asks slot **6**, the integer, not slot 5's text — for the damage
                    // family that is `_ftol(100 − condition)`, truncated (§4.134)
                    match self.accessor_number(&src) {
                        Some(v) => Some(v),
                        None => match self.range_value(name) {
                            Some((v, lo, hi)) if hi > lo => Some((v - lo) * 100 / (hi - lo)),
                            _ => self
                                .accessor_value(&src)
                                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                                .map(|v| v.round() as i32),
                        },
                    }
                };
                (i, v)
            })
            .collect();
        // `NFS5.KNOCKOUTTRACKLIST` is the strip beside those bars: the opponent count and the row
        // `Knockout.TrackNumber` is on (`LayoutScreen`'s own arm has the geometry)
        let strip_widgets = self.fe.layout_preview.as_ref().map(|l| l.numbered_widgets("NFS5.KNOCKOUTTRACKLIST")).unwrap_or_default();
        // `FEKnockoutTrackList::draw` 0x4FA27B…0x4FA28C: a `Knockout.TrackNumber` at or past the
        // count (a smaller `Game.NumKOOpponents` than the row it was on — nothing else moves it)
        // is put back on row **0** through the accessor's slot 11, before the amber row is drawn
        if !strip_widgets.is_empty()
            && let Some(m) = self.fe.menu.as_mut()
            && m.ko_sel >= m.ko_opponents as usize
        {
            m.ko_sel = 0;
        }
        let strips: Vec<(usize, usize, usize)> = strip_widgets
            .into_iter()
            .map(|(i, _)| (i, self.fe.menu.as_ref().map_or(0, |m| m.ko_opponents as usize), self.fe.menu.as_ref().map_or(0, |m| m.ko_sel)))
            .collect();
        // the car-setting sliders: the knob's place along its track is the accessor's own index
        // in its own list (`RE/notes/layouts.md`, the car-setup section)
        let knobs: Vec<(usize, Option<(i32, i32)>)> = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_setting_sliders())
            .unwrap_or_default()
            .into_iter()
            .map(|(i, a)| {
                let base = a.split_once('(').map_or(a.as_str(), |(b, _)| b.trim());
                let v = nfs_game::career::CAR_SETUP_NAMES
                    .iter()
                    .position(|n| *n == base)
                    .and_then(|k| {
                        let c = self.career.as_ref()?;
                        let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                        Some(nfs_game::career::car_setup_index(k, c.career.garage.get(idx)?.setup[k]))
                    });
                (i, v)
            })
            .collect();
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        for (index, value) in wanted {
            l.set_bar(index, value);
        }
        for (index, value) in knobs {
            l.set_knob(index, value);
        }
        for (index, rows, current) in strips {
            l.set_ko_strip(index, rows, current);
        }
    }
}
