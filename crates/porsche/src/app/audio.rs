//! Audio: the CD player / front-end music, race sounds, the pause sync and the audio state.

use crate::*;

impl App {
    /// The song names, from the live player in a race and from the files in the front end.
    pub(crate) fn cd_names(&self) -> Vec<String> {
        match self.snd.music.as_ref() {
            Some(m) => m.names.clone(),
            None => self.snd.music_names.clone(),
        }
    }

    /// The song table `[0x65B628]` (0x4ADE20 with `game*`): each matching stream's index and name,
    /// in stream order.
    pub(crate) fn cd_table(names: &[String]) -> Vec<(usize, String)> {
        names.iter().enumerate().filter(|(_, n)| nfs_audio::music::pattern_match(n, "game*")).map(|(i, n)| (i, n.clone())).collect()
    }

    /// The playlist `[0x65B62C]` as stream indices.
    ///
    /// **The CD player's list, not the music module's live one.** `[0x65B62C]` is read and
    /// written only inside the CD-player block 0x4BE187…0x4BED01 (every reference to it lies
    /// there), and `CDPlayer.SelectedList`'s row 0x4BEC30 indexes the song table with it. The
    /// port keeps it in `audio_settings.playlist` ([`App::set_cd_playlist`] writes that and the
    /// player's saved list together). This read the player's **live** list until 2026-09-26,
    /// which the front end's `menu*` pick replaces (`play_pattern` mode 0) — so once the menu
    /// music was chosen the play list read the menu streams, none of them in the `game*` table,
    /// and `cdplayer.lay`'s `ID_PLAYLIST` came up empty.
    pub(crate) fn cd_playlist(&self) -> Vec<usize> {
        self.snd.audio_settings.playlist.iter().map(|i| *i as usize).collect()
    }

    pub(crate) fn set_cd_playlist(&mut self, list: Vec<usize>) {
        self.snd.audio_settings.playlist = list.clone();
        if let Some(m) = self.snd.music.as_mut() {
            m.set_saved(list);
        }
    }

    /// The CD player's editing callbacks (`cdplayer.lay`, the screen at vtable 0x5B9F58):
    ///
    /// * `ADDSONG` 0x511FF0: with an `AvailableList` row selected, **815** when the playlist
    ///   already holds 40 (`cmp eax, 0x28`, raised at 0x51202C), else that record's stream index
    ///   appended (0x4BEBE0 → 0x4BE670);
    /// * `REMOVESONG` 0x512070: with a `SelectedList` row selected, that entry erased (0x4BE790);
    /// * `CLEARALL` (slot 0xEC → 0x4BE9E0): the playlist emptied;
    /// * `RESETLIST` (slot 0xF0 → 0x4BE7E0): emptied, then every table record whose name's first
    ///   four characters, lower-cased, are `game` appended in table order.
    pub(crate) fn cd_callback(&mut self, cb: &str) -> bool {
        let names = self.cd_names();
        let table = Self::cd_table(&names);
        let mut list = self.cd_playlist();
        match cb {
            "ADDSONG" => {
                let Some(r) = self.fe.cd_available.filter(|r| *r < table.len()) else { return true };
                if list.len() >= 0x28 {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 815, [0, 0], DialogFor::Notice);
                    return true;
                }
                list.push(table[r].0);
            }
            "REMOVESONG" => {
                let Some(r) = self.fe.cd_selected.filter(|r| *r < list.len()) else { return true };
                list.remove(r);
            }
            // slots 55 / 56, 0x511F90 / 0x511FC0: one of `Audio.lay`'s two panels at a time — the
            // one going away hidden (0x4CEC50), the other loaded (its slot 0x6C) and shown
            // (0x4CEC60); `[+0x290]` remembers which
            "SHOWPLAYLISTPANEL" | "HIDEPLAYLISTPANEL" => {
                let show = cb == "SHOWPLAYLISTPANEL";
                let (off, on) = if show { ("ID_MAINSCREENPANEL", "ID_PLAYLISTPANEL") } else { ("ID_PLAYLISTPANEL", "ID_MAINSCREENPANEL") };
                if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                    l.set_panel(r, &self.paths, off, false);
                    l.set_panel(r, &self.paths, on, true);
                }
                self.bind_lists();
                return true;
            }
            // the transport (`audioreal.lay`)
            "PLAYSONG" => {
                // 0x5120D0: `MusicStop(0)`, then `MusicPlay(0x4BEB40(), eax)` where `eax` is
                // 0x4BEB40()'s return: 0 (`xor eax, eax`, 0x4beb6d/0x4beb4e) when `[0x65B628]`
                // (the CD player's own entry list, a different structure from the music globals'
                // `[0x656AB4]`) is empty or names no entry equal to the sentinel `[0x5D7938]`, else
                // a found entry's `+8` field. `MusicPlay`'s pattern argument here is the literal 0
                // pushed at 0x5120e1 (`push eax` after `push 0` is not popped — the earlier `push 0`
                // for 0x4BEB40's own dummy argument becomes `MusicPlay`'s first argument), so the
                // pattern is always NULL and mode dispatches to 0x4AE709/0x4AE736 (mode 0's
                // "no-repeat" pick over `[0x656AB4]`'s candidates as they stand) unless the mode
                // equals 1 or 2 — which a "no current song" 0 cannot. So with no current song this
                // is exactly the resume's `MusicPlay(NULL, 0)` (0x4AB222, `play_next`): a fresh pick
                // from the list **as it stands**, nothing when the candidate pool is exhausted or
                // the volume is 0 — no `MusicSetPlaylist`, no reset to the saved list.
                // *(Until this port pass, `None` called `start_race`, which resets the list to the
                // saved one — recorded 2026-09-27 as a divergence, now matched.)*
                let name = table.iter().find(|(i, _)| *i as i32 == self.fe.cd_current).map(|(_, n)| n.clone());
                self.fe_music(|m, a, draw| {
                    m.stop(a);
                    match &name {
                        Some(n) => m.play_pattern(a, n, 0, draw),
                        None => {
                            m.play_next(a, draw);
                        }
                    }
                });
                return true;
            }
            "STOPSONG" => {
                // 0x5120F0: `MusicStop(0)`
                self.fe_music(|m, a, _| m.stop(a));
                return true;
            }
            "SKIPFWD" | "SKIPBK" => {
                // 0x512100 / 0x512110: `CDPlayer.Title` stepped forward / back, wrapping
                // (0x4BC690 / 0x4BC660), through its setter 0x4BEDC0
                let n = table.len();
                if n > 0 {
                    let i = self.fe.cd_title.min(n - 1);
                    let next = if cb == "SKIPFWD" { (i + 1) % n } else { (i + n - 1) % n };
                    self.cd_set_title(next);
                }
                return true;
            }
            "CLEARALL" => list.clear(),
            "RESETLIST" => {
                list = table.iter().filter(|(_, n)| n.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("game"))).map(|(i, _)| *i).collect();
            }
            _ => return false,
        }
        self.set_cd_playlist(list);
        self.bind_lists();
        true
    }

    /// `CDPlayer.Title`'s setter **0x4BEDC0**: the index clamped (0x4BC240), then the record's
    /// stream index made the current song `[0x5D7938]` when it differs (and is not −1), and played
    /// at once — `MusicStop(0)`, `MusicPlay(name, 0)` — when the current screen answers its slot
    /// 0xA8 true, which only the CD player's does (0x510E90; every other screen's is 0x50DD60,
    /// false). The screen object `[[0x65B928]+0xE0]` is the one the layout's `callbacklist=` id
    /// builds, and of the 41 screen classes (every vtable holding 0x4E0B30) only `FEScreenOptions`
    /// (0x5B9F58, registered as `ID_OPTIONSSCREEN` at 0x511540) answers slot 0xA8 true — and
    /// `Options.lay` alone names that id, with `Audio` / `audioreal` / `cdplayer` inside it. (Until
    /// 2026-09-24 the port asked for `audioreal` or `cdplayer` by name, which as panels of
    /// `Options` are never the screen.)
    pub(crate) fn cd_set_title(&mut self, index: usize) {
        let table = Self::cd_table(&self.cd_names());
        let Some((id, name)) = table.get(index.min(table.len().saturating_sub(1))).cloned() else { return };
        self.fe.cd_title = index.min(table.len() - 1);
        if id as i32 == self.fe.cd_current {
            return;
        }
        self.fe.cd_current = id as i32;
        // `[[0x65B928]+0xE0]`, the bottom layer's list
        let cd_screen = self.fe_list_now() == "ID_OPTIONSSCREEN";
        if cd_screen {
            self.fe_music(|m, a, draw| {
                m.stop(a);
                m.play_pattern(a, &name, 0, draw);
            });
        }
        self.bind_lists();
    }

    /// **0x4D1BD0**'s music half — run by the front end on every screen it opens (0x4D23DA, the
    /// open 0x4D2100; 0x4D2552, the way back) and at the end of its own start-up (0x4D5209):
    ///
    /// ```text
    /// 0x4D1CF7  pattern = screen slot 0x94 (0x4E0C30: data.musicfile); "" → "menu*" (0x4D1D5B)
    /// 0x4D1D7F  if strcmp(pattern, [this+0x10C]) != 0:
    /// 0x4D1DB8      MusicStop(1500)                                  ; 0x4AE3C0
    /// 0x4D1DF9      if strcmp(pattern, "NONE") != 0: MusicPlay(pattern, 0)   ; 0x4AE4D0
    /// 0x4D1E0E      [this+0x10C] = pattern                           ; sprintf
    /// 0x4D1E4C  if strcmp(pattern, "NONE") == 0: MusicStop(150)
    /// ```
    ///
    /// So a screen with the same pattern as the one before leaves the song playing (93 of the 98
    /// layouts carry `menu*`), `NONE` (`Options`, `careerfirst`, `chroniclemovie`,
    /// `chroniclemoviemain`) silences it and `dogface` (`endofera`) matches no stream even as
    /// `*-dogface`, so the old song stops and nothing starts (the era show has its own movie). `[this+0x10C]` is empty on
    /// every entry, so the first screen after start-up starts a `menu*` song — and so does the
    /// first one after a race, which the loading screen's last frame then stops again at once
    /// (`fe_loading_frame`, 0x4DDA00(9) after 0x4D1BD0): the original comes back from a race
    /// silent until a screen with another pattern plays.
    /// The front end plays with the shuffle forced on (`[0x65B928]`, `MusicPlayer::set_front_end`).
    /// The two stops fade: `MusicStop(1500)` takes the old song out over 1.5 s and the new one
    /// starts when it is silent (`MusicPlayer::stop_fade`, `request`; 0x4AE3C0, 0x4AE4D0's swap,
    /// 0x4ADC88), `MusicStop(150)` of a `NONE` screen over 150 ms. *(Until batch A6 both were
    /// immediate.)*
    /// The other half of 0x4D1BD0 (the screen / callback-list / room objects `[+0xE4]`, `[+0xE0]`,
    /// `[+0xE8]` swapped when the new screen names others, 0x4D1BED…0x4D1CF4) is the port's
    /// `open_layout` itself.
    ///
    /// **Both halves read the bottom layer's screen**: 0x4D1BDA…0x4D1BEB takes
    /// `[[[fe+0xD8]]]`'s pending-else-current screen once and asks it for the background (slot
    /// 0x84), the callback list (0x88), the room (0x8C) and the music pattern (0x94, 0x4D1CFC). A
    /// layer pushed over a screen never changes them — `settings` / `repair` / `selectname` /
    /// `missionbrief` over their parent leave the parent's song and list in place; only a screen
    /// that replaces the bottom one (offset 0) or comes back from another list does.
    ///
    /// Returns whether the callback-list object was **replaced** (a new class made, the old one
    /// destroyed: its destructor ran — [`Self::fe_list_gone`] — and the new one's constructor is
    /// due, [`Self::fe_screen_built`]), which the callers use to run the constructor once.
    pub(crate) fn fe_apply_music(&mut self) -> bool {
        let mut swapped = false;
        let Some(l) = self.fe_base_layout() else { return false };
        // 0x4D1BD0's first half — before its music half — swaps `[fe+0xE0]` for the class the new
        // layout's `callbacklist=` names, when it differs (the name compare 0x4D1C1D…0x4D1C3E, the
        // object's slot 0x7C): the new object's constructor runs. `ID_OPTIONSSCREEN`'s, 0x511600,
        // pushes `MusicStop(1500)` (0x511611, called at 0x511634) and sets `CDPlayer.Current` to −1 (0x51175D) — so entering the options stops
        // the song even when the screen before had the same music pattern; `ID_MAINSCREEN`'s
        // leaves its `+0x290` stamp unwritten (see `main_music_stamp`).
        let list = callback_list_of(l);
        if list != self.fe.list_current {
            swapped = true;
            // the old object's destructor (0x4D2570 0x4D2763…0x4D2794, `delete` slot 0)
            let old = std::mem::take(&mut self.fe.list_current);
            self.fe_list_gone(&old);
            match list.as_str() {
                "ID_OPTIONSSCREEN" => {
                    self.fe_music(|m, a, _| m.stop_fade(a, 1500));
                    // 0x51175D: `0x4BEAC0(-1)` — `CDPlayer.Current` `[0x5D7938]` back to −1
                    self.fe.cd_current = -1;
                }
                // the constructor stamps `[+0x290]` with `TIMER_getTicks()` (0x510175, found by a
                // hardware watch on the running original, REFERENCE-2026-10-02-SRVB.md §2 — the earlier
                // byte scan missed it and took the word for heap leftovers); every front-end event then
                // re-stamps it (0x510EA0, `window_event`)
                "ID_MAINSCREEN" => self.fe.main_music_stamp = self.fe_timer_ticks(),
                _ => {}
            }
            self.fe.list_current = list;
        }
        let Some(l) = self.fe_base_layout() else { return swapped };
        let pattern = l.music_pattern().unwrap_or("menu*").to_string();
        if pattern != self.fe.music_current {
            let play = pattern != "NONE";
            let p = pattern.clone();
            self.fe_music(|m, a, draw| {
                m.set_front_end(true);
                m.stop_fade(a, 1500);
                if play {
                    m.play_pattern(a, &p, 0, draw);
                }
            });
            tracing::info!(pattern, "front end: music (0x4D1BD0)");
            self.fe.music_current = pattern.clone();
        }
        if pattern == "NONE" {
            self.fe_music(|m, a, _| m.stop_fade(a, 150));
        }
        swapped
    }

    /// **`FEMainScreen`'s tick 0x510210** (vtable 0x5B9D64 slot 1, the class `ID_MAINSCREEN` builds),
    /// its first lines: when `TIMER_getTicks() − 0x1E00` (60 s of the 128 Hz clock) is above the
    /// object's `[+0x290]` (0x510221…0x510233), the music is re-rolled — `MusicStop(200)`
    /// (0x510235…0x51023A), **0x4DC140** — which is not a bare wait: it calls 0x4DD4A0, the start-up's
    /// second movie `inPC-av.mad` through the player 0x4DC850 (the same call 0x4DD600 makes, skippable
    /// by Esc or a mouse button), and then loops until the key ring is empty and none of the mouse's
    /// four buttons is down (`start_idle_movie`, `intro_run::IntroRun::new_idle`) — then
    /// `MusicPlay("Menu*", 0)` (0x510246…0x510251), and the stamp is set to now (0x510254…0x510259,
    /// read after the movie). So a main-screen stay longer than a minute plays the movie and swaps
    /// the song. `[+0x290]` is stamped with `TIMER_getTicks()` by the constructor (0x510175) and again
    /// by every front-end event (0x510EA0, the list's slot 3 from the dispatch 0x4D2868): the movie
    /// comes after a minute **without input** on the main screen (measured on the original,
    /// REFERENCE-2026-10-02-SRVB.md §2; until 2026-10-02 the port counted from the entry).
    pub(crate) fn fe_main_screen_music_tick(&mut self) {
        // a tick of the callback list object `[fe+0xE0]`, the bottom layer's
        if self.vehicle.is_some() || self.fe_list_now() != "ID_MAINSCREEN" {
            return;
        }
        if self.idle_movie_due || self.idle_movie_running {
            return;
        }
        let now = self.fe_timer_ticks();
        if now - 0x1E00 > self.fe.main_music_stamp {
            // 0x510235: MusicStop(200); 0x510244: 0x4DC140 — which plays `inPC-av.mad` (0x4DD4A0, the
            // movie the start-up shows second) and waits until no button is down, a modal wait the
            // window's event loop takes over (`start_idle_movie`, `finish_idle_movie`)
            self.fe_music(|m, a, _| m.stop_fade(a, 200));
            let scripted = NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) && !PLAY_INTRO.load(std::sync::atomic::Ordering::Relaxed);
            if scripted || self.screenshot.is_some() || self.window.is_none() {
                // a scripted run plays no movies (as at the start-up)
                self.finish_idle_movie();
            } else {
                self.idle_movie_due = true;
            }
        }
    }

    /// The front end's music player, made on first use (the port has no menu music of its own yet)
    /// — nothing in a screenshot run. `f` gets the picks' draw, `0x538040() + GetTickCount()`
    /// (0x4AE4D0): the shared generator `fe_rng` and [`tick_count_ms`].
    pub(crate) fn fe_music(&mut self, f: impl FnOnce(&mut nfs_audio::music::MusicPlayer, &nfs_audio::AudioOutput, &mut dyn FnMut() -> u32)) {
        if self.screenshot.is_some() {
            return;
        }
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        // the voice budget of the sound's open 0x4A66B0 for the current `AUDIOQUALITY` — re-given on
        // every use, as 0x4A6A00 reopens the sound when the setting changed
        if let Some(a) = &self.snd.audio {
            a.set_audio_quality(self.snd.audio_settings.audio_quality);
        }
        if self.snd.music.is_none() {
            self.snd.music = nfs_audio::music::MusicPlayer::new(&self.paths.game_data().join("Music"), &self.paths.game_data().join("Sounds"), &self.snd.audio_settings)
                .map_err(|e| tracing::warn!(error = %e, "music unavailable"))
                .ok();
        }
        let rng = &mut self.fe_rng;
        if let (Some(a), Some(m)) = (self.snd.audio.as_ref(), self.snd.music.as_mut()) {
            f(m, a, &mut || rng.next_u32().wrapping_add(tick_count_ms()));
        }
    }

    /// Silence the race audio (engine layers, squeal / horn loops) when leaving to the menu.
    pub(crate) fn stop_race_audio(&mut self) {
        // the effect levels come back with the race: the front end reads the registry variables
        // F10 kept them in, and the next race's audio init sets `[0x656840]` again (0x4A8841)
        if self.snd.sfx_saved.is_some() {
            self.snd.audio_settings.toggle_effects(&mut self.snd.sfx_saved);
        }
        self.snd.slots.clear();
        self.snd.ai_sfx.clear();
        self.snd.player_loops = Default::default();
        if let Some(a) = &self.snd.audio {
            // the speech flushed: the shutdown's 0x4A8A80 (0x4A8A8C) and the restart's 0x4AB360
            // (0x4AB39B) — the race start's own (`AudioLoadTrack` 0x4A88D0) is in `start_race`
            if let Some(sp) = self.snd.speech.as_ref() {
                sp.flush(a);
            }
            if let Some(mut e) = self.snd.engine_sound.take() {
                e.stop(a);
            }
            if let Some(s) = self.snd.sfx.as_mut() {
                s.stop_loops(a);
            }
            if let Some(ai) = self.snd.ai_engines.as_mut() {
                ai.stop_all(a);
            }
            if let Some(amb) = self.snd.ambience.as_mut() {
                amb.stop(a);
            }
        }
    }

    /// 0x426A60: the HUD's dialog queue `[0x60C9C4]` holds something — the pause menu or its
    /// question, a billboard, a results page, the replay's save dialog.
    /// The HUD toggle keys (`controls.md` §4.170, read 2026-09-27). Each handler returns at once
    /// while a billboard is queued (`[0x60C9C4]` non-empty: 0x4273EC, 0x426740, 0x427550,
    /// 0x427360, 0x4275D0); a slide is set through slot 5 0x434490, which takes the state
    /// whatever the element's and stamps `[0x606AC8]`.
    ///
    /// * **F5** `I_TOGGLEHUD` 0x4273B0. Live → hidden (0x41E720): with MAP on, `[0x60C9A8] = 1`,
    ///   the map suspended (0x41E6E0) and slid out (2), else `[0x60C9A8] = 0`; the same for the
    ///   mirror with `[0x60C9F8]` (REARVIEWMIRRROR set 0, 0x41E750); every `+4` element that is
    ///   not the map or the mirror (`+6` clear) and not already in state 2 slides out. Hidden →
    ///   live (0x41E710): the map restored (0x41E690) and slid in (1) if `[0x60C9A8]`, the mirror
    ///   set 1 (0x41E730) and slid in if `[0x60C9F8]`, and every `+4`, `+6`-clear element slid in
    ///   whatever its state.
    /// * **F6** `I_TOGGLEREARVIEW` 0x427550: mirror on → off and slid out; off → on and slid in
    ///   only while live.
    /// * **F7** `I_TOGGLEMAP` 0x427360: MAP on → suspended and slid out; off → restored and slid
    ///   in only while live.
    /// * **"2"** `I_CYCLEMAP` 0x426740: MAP 3 → 2 with cfg1`+0x44` = 20.0; 1 → 3; anything else
    ///   (0 or 2) → 1 with cfg1`+0x44` = 4.0 (0x41E7C0, 0x41E770, 0x41E820 / 0x41E800 / 0x41E7E0,
    ///   0x41E670). No slide: a map switched on this way opens through 0x425E20. cfg1`+0x44` has
    ///   no reader besides that writer (every `[0x60C818]` load in the image scanned), so the
    ///   scale changes nothing on screen.
    /// * **F8** `I_HUDTRANSPARENCY` 0x4275D0: the trays' box flipped (0x41E5C0 → 0x41E540 /
    ///   0x41E590).
    /// The race pause's audio side, run when `paused || frozen` changes (see the call site's note).
    pub(crate) fn sync_audio_pause(&mut self) {
        let halted = self.paused || self.replay.frozen;
        if halted != self.audio_paused {
            self.audio_paused = halted;
            if halted {
                self.snd.audio_settings.pause_levels(&self.snd.sfx_saved);
            } else {
                self.snd.audio_settings.resume_levels(&mut self.snd.sfx_saved);
            }
            if let Some(a) = &self.snd.audio {
                if halted {
                    a.silence_voices();
                    if let Some(e) = self.snd.engine_sound.as_mut() {
                        e.pause(a);
                    }
                    if let Some(p) = self.snd.reverb.pause() {
                        a.set_env(p.p20, p.p80);
                    }
                    tracing::info!("race pause: voices to volume 0, engine layers stopped (0x4AB150)");
                } else {
                    tracing::info!("race resume (0x4AB200)");
                }
                if let Some(m) = self.snd.music.as_mut() {
                    if halted {
                        m.stop(a);
                    } else if self.race.is_some() && self.vehicle.is_some() {
                        // not when the pause ended by leaving the race (`sim_shutdown`). The
                        // resume's `MusicPlay(NULL, 0)` (0x4AB222) tests neither `REPLAY` nor
                        // `[0x5E99F4]` nor the finish: a playback unpaused (0x415701, the menu's
                        // continue; 0x41583E, the bar's unfreeze) starts a song from the list as it
                        // stands, which its race start left alone (0x4A8A4A) — the live race's and
                        // its jingle for an automatic replay, none for a command-line one.
                        // *(Until 2026-09-27 the port kept a playback and a finished race silent
                        // here, and reset the list to the saved one as the race start does.)*
                        let rng = &mut self.fe_rng;
                        m.play_next(a, &mut || rng.next_u32().wrapping_add(tick_count_ms()));
                    }
                }
            }
        }
    }

    /// **0x4AADB0(`slot`)**, the front end's one-shot: `fesfx.bnk`'s slot at the level
    /// `nfs_audio::fesfx::level` gives, silent at sound detail 3. The output opens and the bank
    /// loads on the first call; a screenshot run plays nothing.
    pub(crate) fn fe_sound(&mut self, slot: usize) {
        if self.screenshot.is_some() {
            return;
        }
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        // the voice budget of the sound's open 0x4A66B0 for the current `AUDIOQUALITY` — re-given on
        // every use, as 0x4A6A00 reopens the sound when the setting changed
        if let Some(a) = &self.snd.audio {
            a.set_audio_quality(self.snd.audio_settings.audio_quality);
        }
        let q = self.snd.audio_settings.audio_quality;
        if self.snd.fe_sfx.is_none() {
            self.snd.fe_sfx = nfs_audio::fesfx::FeSfx::load(&self.paths.game_data().join("Sounds"), q)
                .map_err(|e| tracing::warn!(error = %e, "front-end sounds unavailable"))
                .ok();
        }
        if let (Some(out), Some(fx)) = (self.snd.audio.as_ref(), self.snd.fe_sfx.as_mut()) {
            fx.play(out, slot, self.snd.audio_settings.sfx_level, q);
        }
    }
}

/// The sound half of [`App`]: the output device and every voice that plays through it — the
/// player's engine, the AI and traffic engine loops, the collision and squeal sounds, the
/// ambience, the reverb, music and speech — with the settings they are mixed by. Split out of
/// `App` on 2026-09-21; every field kept its name, so `self.x` became `self.snd.x`.
/// A car the audio slots can hold — the non-player cars of a race.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AudioCar {
    Opponent(usize),
    Traffic(usize),
    Police(usize),
}

pub(crate) struct AudioState {
    pub(crate) audio: Option<nfs_audio::AudioOutput>,
    /// `I_SFX` (F10) has the four effect levels off: `[0x656840]` clear, holding the levels the
    /// registry variables `[0x657CA4…0x657CB0]` keep (`Settings::toggle_effects`)
    pub(crate) sfx_saved: Option<[u8; 4]>,
    pub(crate) engine_sound: Option<nfs_audio::engine::EngineSound>,
    pub(crate) sfx: Option<nfs_audio::sfx::CollisionSfx>,
    pub(crate) ai_engines: Option<nfs_audio::ai_engine::AiEngines>,
    /// the audio car slots `0x655D60` (`nfs_audio::slots`)
    pub(crate) slots: nfs_audio::slots::AudioSlots<AudioCar>,
    /// Every non-player car's sound queue `car+0x1110` (0x40F020) and loop bits `car+0xE00`,
    /// filled by the physics pass and drained for the slot cars (0x4A73DE) — see
    /// `nfs_audio::sfx::SfxQueue`.
    pub(crate) ai_sfx: std::collections::HashMap<AudioCar, nfs_audio::sfx::SfxQueue>,
    /// the player's loop queue and its `car+0xE00` bits (0x40FFF0 / 0x40FBC0)
    pub(crate) player_loops: nfs_audio::sfx::SfxQueue,
    /// `[0x655F34]`, the crowding factor 0x4A8199 works out over the slots' cars
    pub(crate) crowding: u8,
    pub(crate) ambience: Option<nfs_audio::ambience::Ambience>,
    /// The type-4 `CTriggerElem`s of the audio scene (`[0x62816C]`, 0x46C680), each with its
    /// `+0xD8` element as an index into `[0x628168]` — swept by 0x470BB0 on the odd tick. Loaded
    /// with or without an output device, so a scripted run still logs them.
    pub(crate) audio_triggers: Vec<nfs_game::cameras::CameraTrigger>,
    /// tunnel reverb level (`engine-audio.md` §4.7)
    pub(crate) reverb: nfs_audio::reverb::Reverb,
    pub(crate) music: Option<nfs_audio::music::MusicPlayer>,
    /// The song names in `GameData/Music`, read once at start-up. The CD-player screen needs them
    /// in the **front end**, where there is no `MusicPlayer` yet — the original plays menu music
    /// there (every screen block carries `musicfile=menu*`) and that screen edits the playlist, so
    /// the lists must fill without a race having been started.
    pub(crate) music_names: Vec<String>,
    /// each song's length in milliseconds — `track + 0x20` in the original's own music table
    pub(crate) music_lengths: Vec<u32>,
    /// `fe.txt` audio settings (playlist, shuffle, the five level sliders)
    pub(crate) audio_settings: nfs_audio::music::Settings,
    pub(crate) speech: Option<nfs_audio::speech::Speech>,
    pub(crate) countdown_said: u8,
    /// the catalogue sound set of the car being driven (`nfs5.car +0x179`)
    pub(crate) sound_set: String,
    /// player gear on the previous tick (shift click)
    pub(crate) last_gear: usize,
    /// the roll-over crash sound fires once per roll-over
    pub(crate) crash_played: bool,
    /// the front end's one-shots, `fesfx.bnk` (0x4AADB0) — loaded on the first one played
    pub(crate) fe_sfx: Option<nfs_audio::fesfx::FeSfx>,
}

/// The wall event `WallImpulse` 0x494000 leaves for 0x40F740, from the impact the wall pass just
/// left (`Vehicle::impact`, the largest `j` of the call): a hit (`0x40008 + class`) when
/// `j ≥ speedEst/8`, else a scrape (`0x40015 + class`) — the rule the player's one-shot takes.
pub(crate) fn wall_sfx(b: &nfs_physics::vehicle::Vehicle) -> nfs_audio::sfx::QueuedSfx {
    if b.impact >= b.speed() / 8.0 {
        nfs_audio::sfx::QueuedSfx::WallHit { j: b.impact, class: b.last_wall_class }
    } else {
        nfs_audio::sfx::QueuedSfx::WallScrape { speed: b.speed(), class: b.last_wall_class }
    }
}

/// The prop pass's event (`0x50000 | class` / the bush's `0x6000F`) as a queue entry.
pub(crate) fn prop_sfx(impact: f32, e: nfs_physics::vehicle::PropEvent) -> nfs_audio::sfx::QueuedSfx {
    match e {
        nfs_physics::vehicle::PropEvent::Prop(class) => nfs_audio::sfx::QueuedSfx::Prop { impact, class },
        nfs_physics::vehicle::PropEvent::Bush => nfs_audio::sfx::QueuedSfx::Bush,
    }
}

/// The listener's two axes of 0x4A6FE0 (`nfs_audio::mixer::pan_angle`): row `+0x24`, the view's
/// forward, and row `+0xC`, the `+side` hand whose sources the mixer's channel 0 takes. The port's
/// `forward × Y` (screen left through the renderer's x flip) is that side's opposite — its pan had
/// `+1` on channel 1 — so the side is `Y × forward`; `MIRROR` negates it (0x4A704D, 0x4A78C1,
/// 0x4A7C1A negate the side dot before the `atan2`). Returns the angle of an offset from the
/// listener, not unit: 0x4A6FE0 takes the raw offset, and no distance gate.
pub(crate) fn listener_pan_of(cam: Vec3, target: Vec3, mirror: bool) -> impl Fn(Vec3) -> u16 {
    let fwd = (target - cam).normalize_or_zero();
    let side = Vec3::Y.cross(fwd).normalize_or_zero() * if mirror { -1.0 } else { 1.0 };
    move |to: Vec3| nfs_audio::mixer::pan_angle(to.dot(side), to.dot(fwd))
}

/// The 16-bit pan angle of a world point as the listener hears it ([`listener_pan_of`]). The
/// engine and the queue's loops take it of `car+0x330`, the player's collision one-shots of
/// `car+0x440` (0x4A7BB1…0x4A7C3B).
pub(crate) fn listener_pan(cam: Vec3, target: Vec3, mirror: bool, p: Vec3) -> u16 {
    listener_pan_of(cam, target, mirror)(p - cam)
}

pub(crate) fn crowding_factor(distances: impl IntoIterator<Item = f32>) -> u8 {
    let mut x = 192.0f32;
    for d in distances {
        if d < 128.0 {
            x -= 128.0 - d;
        }
    }
    x.clamp(0.0, 128.0) as u8
}

/// The music picks' clock term: `GetTickCount()` (0x4AE4D0, 0x4ADDA0) / `timeGetTime()` (0x4AD790),
/// the milliseconds since Windows started. APPROXIMATION: the port has no such clock and takes the
/// wall clock's milliseconds since the epoch, truncated to 32 bits — like the original's, a value
/// that differs from launch to launch, which is all the picks use it for (the sum with
/// 0x538040's draw, `|sum| % n`).
pub(crate) fn tick_count_ms() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u32)
}
