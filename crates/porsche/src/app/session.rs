//! The per-frame session driver: `resume_inner`, the session step and the camera / race restart glue.

use crate::*;

impl App {
    pub(crate) fn resume_inner(&mut self, event_loop: &ActiveEventLoop) {
        let scripted = NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed);
        let played = self.intro_done.is_some();
        let (window, mut renderer) = match self.intro_done.take() {
            Some(wr) => wr,
            None => {
                let attrs = Window::default_attributes()
                    .with_title("Porsche Unleashed (native)")
                    .with_inner_size({
                        // **TEST GLUE**: `NFS_SHOT_SIZE` also shapes the window (its aspect is the still's)
                        let (w, h, _) = shot_geometry();
                        winit::dpi::LogicalSize::new(w, h)
                    })
                    // a scripted run's window is never made the key window (see `main`)...
                    .with_active(!scripted)
                    // ...and, tooling only (not a game rule): never shown on screen at all. The
                    // window still exists and keeps its real 1280x800 inner size (the goldens
                    // depend on that resolution), so `Renderer::new` still creates a real Metal
                    // surface of the same size and every draw still presents to it exactly as
                    // before — an invisible NSWindow's CAMetalLayer still takes drawables and
                    // presents, it is just never composited to the display. `--screenshot`
                    // doesn't even read that surface: it renders into its own offscreen texture
                    // (`render_to_image`, `gpu.rs`), fully decoupled from what the window shows.
                    // `--record`'s CSV comes from simulation state, not pixels. So hiding the
                    // window changes nothing any golden or CSV can see; it only stops a scripted
                    // run's window from popping up in front of the user (a report of
                    // 2026-09-29: the `NFS_PLAY_INTRO=1` walk left one up ~10s).
                    .with_visible(!scripted);
                let window = Arc::new(event_loop.create_window(attrs).expect("window"));
                let renderer = Renderer::new(window.clone()).expect("renderer");
                (window, renderer)
            }
        };
        // **The start-up movies**, 0x4DD600 (main 0x4B6C42, once, on the first entry to the front
        // end): `NOINTROMOVIES` `[0x65743C]` — `fe.txt`'s `nointromovies`, 0 in the shipped file —
        // is the original's own skip, and it skips the loading mosaic with them. A scripted run
        // (`--autotest`, `--screenshot`, `--layout-sweep`, `--record`, `NFS_NO_FOCUS`) never plays
        // them, nor does the port's text menu, which has no front end to enter — unless
        // `NFS_PLAY_INTRO=1` asks to keep them anyway, so a scripted check can walk the intro →
        // front-end handoff a real interactive launch takes (`PLAY_INTRO`).
        // `RE/notes/mad.md` §12.
        if !played
            && (!scripted || PLAY_INTRO.load(std::sync::atomic::Ordering::Relaxed))
            && self.fe.menu.is_some()
            && self.fe.layout_name.is_some()
            && !self.registry.bool("nointromovies", false)
        {
            if self.snd.audio.is_none() {
                self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
            }
            self.intro = Some(intro_run::IntroRun::new(window, renderer, &self.paths));
            return;
        }
        if let Some(m) = self.fe.menu.as_mut() {
            let dir = self.paths.fe_data().join("Art");
            m.font = GamePaths::find_file(&dir, "Univers14b.ffn")
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
                .map(|f| BitmapFont::upload(&mut renderer, f));
        }
        if self.fe.menu.is_some() {
            // **0x4DD600** (main 0x4B6C42, once, on the first entry to the front end and unless
            // NOINTROMOVIES `[0x65743C]`): the intro movies, then `loadscrn.fsh`, flipped six
            // times — it stays on screen while 0x4D3420's first load runs, which draws no
            // 0x4DDA00 of its own (§4.165.3). `controls.md` §4.168.
            if self.fe.layout_name.is_some() && !self.registry.bool("nointromovies", false) {
                let l = nfs_game::layout::loading::LoadingScreen::startup(&mut renderer, &self.paths);
                let scene = Scene { sprites: l.mosaic_sprites(renderer.aspect()), clear_color: [0.0, 0.0, 0.0, 1.0], ..Scene::default() };
                if let Err(e) = renderer.render(&scene, &self.camera) {
                    tracing::error!(error = %e, "start-up loading screen render failed");
                }
                if std::env::var("NFS_FE_LOADSCRN").is_ok() {
                    self.fe.startup_still = Some(l);
                }
            }
            // `--layout <name>`: draw the original front-end screen instead of the text menu
            if let Some(name) = self.fe.layout_name.clone() {
                // the bar's construction (0x4D7E70) precedes the first layout's (0x4D67D0) —
                // `open_layout`'s own note — so it is built, and its glints bound, before `name`
                self.fe.layout_bottom = nfs_game::layout::LayoutScreen::load(
                    &mut renderer,
                    &self.paths,
                    "bottombar",
                )
                .ok();
                if let Some(b) = self.fe.layout_bottom.as_mut() {
                    b.set_overlay(true);
                    b.load_bar_oval(&mut renderer, &self.paths);
                    b.load_career_tabs(&mut renderer, &self.paths);
                }
                match nfs_game::layout::LayoutScreen::load(&mut renderer, &self.paths, &name) {
                    Ok(mut l) => {
                        l.cheat_bits = self.cheat_bits;
                        l.driver_picture = self.fe.driver_picture;
                        self.fe.layout_preview = Some(l);
                        // the entry screen on the fresh manager's one layer, recorded
                        // (0x4D4035: `0x4D5DF0(vec[0], E, 1)`) — so BACK starts disabled
                        self.fe.nav = nfs_game::layout::nav::Nav::with_screen(&name);
                        // **`selectname` is never its own base layer in the original — it is
                        // always `gamesetup` (the entry screen) with `selectname` pushed as a
                        // *second* layer over it** (`0x4D3E1D`: "the entry screen recorded, then
                        // `selectname` on a second layer", the same shape `fe_select_name`
                        // (`main.rs:8452`) gives a real "no loadable profile" entry or
                        // `--fe=gamesetup`'s own start-up path). A bare `--fe=selectname` used to
                        // start with *nothing* under it, so `CREATEPLAYERDONE`'s `0x4D23F0(−1)`
                        // (`fe_back(-1, …)`, already correct — `Nav::back`'s own test proves an
                        // emptied layer reveals the one under it) had no layer to reveal and
                        // closed the front end instead — the only thing keeping a scripted
                        // `NFS_FE_KEYS=cb:CREATEPLAYERENTER,type:UserList.NameInput=…,
                        // cb:CREATEPLAYERDONE` run from chaining a further key onto the profile it
                        // just made. Loading `gamesetup` underneath and re-opening `selectname` as
                        // its own layer over it, the one shape `selectname` can ever have, fixes
                        // the scripted path without touching the manager itself.
                        if name.eq_ignore_ascii_case("selectname")
                            && let Ok(mut under) = nfs_game::layout::LayoutScreen::load(&mut renderer, &self.paths, FE_ENTRY)
                        {
                            under.cheat_bits = self.cheat_bits;
                            under.set_covered(true);
                            under.set_cursor(None);
                            self.fe.layout_under.push((FE_ENTRY.to_string(), under));
                            self.fe.nav = nfs_game::layout::nav::Nav::with_screen(FE_ENTRY);
                            self.fe.nav.open("selectname", 1, true);
                        }
                        // the same as `open_layout`: the device list before the first draw
                        let (pads, dz) = (self.fe_pads(), Self::fe_deadzone());
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.set_pads(pads, self.fe.pads_sel);
                            if let Some(dz) = dz {
                                l.set_deadzone_values(0, dz);
                            }
                        }
                    }
                    Err(e) => tracing::error!(error = %e, name, "layout unavailable"),
                }
            }
            self.renderer = Some(renderer);
            self.window = Some(window);
            self.last_frame = Instant::now();
            self.fps_timer = Instant::now();
            // `--layout serverlist` and friends start straight on a network screen, which needs a
            // browsing socket just as `open_layout` gives one
            if let Some(name) = self.fe.layout_name.as_deref()
                && is_network_layout(name)
            {
                self.net.lobby = netplay::Lobby::open(&self.player_name)
                    .map_err(|e| tracing::warn!(error = %e, "cannot open a browsing socket"))
                    .ok();
                // `Comms.Positional`'s default row is the games screen's (`fe_bind_screen`)
                if let Some(l) = self.net.lobby.as_mut() {
                    l.positional = !name.eq_ignore_ascii_case("local");
                }
            }
            // 0x505570 at 0x4D5267 (the start-up's own entry), before the lists are bound
            if self.fe.layout_preview.is_some() {
                self.fe_tournament_refresh();
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
            self.bind_end_of_era();
            // the front end's start-up ends in 0x4D1BD0 (0x4D5209): the first screen's music
            self.fe_apply_music();
            // and 0x4D6130 → 0x4D81C0 (0x4D5210): the bottom bar's buttons
            self.bind_bottom_bar();
            // the first screen was constructed like any other (0x4E0F90): its callback list's
            // constructor (`fe_screen_built`, §4.169.2)
            if self.fe.layout_preview.is_some()
                && let Some((name, offset)) = self.fe_screen_built()
            {
                self.fe_open(name, offset, true);
            }
            // **The first entry always asks for a profile** (§4.169.3): with `[0x5D9158]` set
            // 0x4D3B50 skips the profile load and 0x4D3B69 goes on — `[+0x178]` is never 1 —
            // through 0x4DE880 (`SaveInfo("XXDefXX", −1)`) and the lobby launch test to 0x4D3E1D:
            // the entry screen recorded, then `selectname` on a second layer. Main enters with
            // `"gamesetup"` (0x4B6C52), so a start on that screen is the original's; any other
            // `--fe` screen is a diagnostic start in the middle of the front end.
            if self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(FE_ENTRY)) {
                self.fe_select_name();
            }
            // diagnostic: `NFS_FE_AFTER_RACE=<Game.GameType>[:aborted]` runs the front end's way
            // back from a race of that type (`fe_after_race`) on start-up, so a still shows the
            // screen 0x4D3420 would open and the log the stack under it
            if self.fe.layout_preview.is_some()
                && let Some(spec) = std::env::var("NFS_FE_AFTER_RACE").ok()
                && let Some(t) = spec.split(':').next().and_then(|t| t.trim().parse::<u8>().ok())
            {
                self.fe.race_game_type = Some(t);
                self.fe_after_race(spec.ends_with(":aborted"));
            }
            // diagnostic: `NFS_FE_CALLBACK=<name>` presses the first control whose `callback` is
            // `name` on start-up, as ENTER on it would (`layout_key`)
            if let Some(cb) = std::env::var("NFS_FE_CALLBACK").ok()
                && let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.controls().iter().position(|c| c.callback.as_deref() == Some(cb.as_str())))
            {
                self.fe.layout_sel = i;
                self.layout_key(MenuKey::Enter, event_loop);
                tracing::info!(callback = cb, screen = ?self.fe.layout_name, nav = self.fe.nav.describe(), "front end: NFS_FE_CALLBACK");
            }
            // diagnostic: `NFS_FE_KEYS=<k>[,…]` presses keys on start-up, after `NFS_FE_CALLBACK`:
            // `b` and `esc` through the front end's key path (`fe_key_code`), `goto:<n>` presses
            // the n-th control as ENTER would, `open:<name>:<offset>` runs 0x4D2100 directly,
            // `pick:<widget>:<row>` commits a drop-down row as a release on it does,
            // `strip:<widget>` steps a combo as a release in its caption strip does, `cb:<name>`
            // presses the current screen's control with that callback, `type:<accessor>=<text>`
            // types into that input box
            if self.fe.layout_preview.is_some()
                && let Ok(spec) = std::env::var("NFS_FE_KEYS")
            {
                for k in spec.split(',').map(str::trim) {
                    match k {
                        "b" => self.fe_key_code(0x62, event_loop),
                        "esc" => self.fe_key_code(0x1B, event_loop),
                        _ => {
                            if let Some(n) = k.strip_prefix("goto:").and_then(|n| n.parse::<usize>().ok()) {
                                self.fe.layout_sel = n;
                                self.layout_key(MenuKey::Enter, event_loop);
                            } else if let Some((name, off)) = k.strip_prefix("open:").and_then(|r| r.rsplit_once(':')) {
                                self.fe_open(name, off.parse().unwrap_or(0), true);
                            } else if let Some((i, row)) = k.strip_prefix("pick:").and_then(|r| r.split_once(':')).and_then(|(i, r)| Some((i.parse::<usize>().ok()?, r.parse::<usize>().ok()?))) {
                                // `pick:<widget>:<row>` — the release on row `row` of that
                                // combo's open drop-down (`combo_commit`)
                                self.fe.layout_open_combo = Some(i);
                                if let Some(l) = self.fe.layout_preview.as_mut() {
                                    l.set_open_combo(Some(i));
                                }
                                self.combo_commit(i, row);
                            } else if let Some((acc, row)) = k.strip_prefix("combo:").and_then(|r| r.rsplit_once(':')).and_then(|(a, r)| {
                                // the row, or for a track combo a track's file name (`alps`)
                                let row = r.parse::<usize>().ok().or_else(|| {
                                    self.fe.menu.as_ref()?.tracks.iter().position(|t| t.eq_ignore_ascii_case(r))
                                })?;
                                Some((a.to_string(), row))
                            }) {
                                // `combo:<accessor>:<row>` — the same selection a drop-down commit
                                // makes (`combo_select`), by the accessor's name (`Track1.Track(SHORT)`)
                                self.combo_select(&acc, row);
                                self.bind_after_accessor();
                            } else if let Some(i) = k.strip_prefix("strip:").and_then(|i| i.parse::<usize>().ok()) {
                                // `strip:<widget>` — a release in that closed combo's caption strip
                                self.combo_strip_release(i);
                            } else if let Some(cb) = k.strip_prefix("cb:") {
                                // `cb:<callback>` — ENTER on the current screen's first control
                                // with that callback, as `NFS_FE_CALLBACK` does at the start
                                if let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.controls().iter().position(|c| c.callback.as_deref() == Some(cb))) {
                                    self.fe.layout_sel = i;
                                    self.layout_key(MenuKey::Enter, event_loop);
                                }
                            } else if let Some((x, y)) = k.strip_prefix("click:").and_then(|r| r.split_once(':')).and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))) {
                                // `click:<x>:<y>` — a left click at that design point, in order
                                // with the `keys:` tokens around it
                                self.fe_scripted_click(x, y, event_loop);
                            } else if let Some(n) = k.strip_prefix("waitgames:").and_then(|n| n.parse::<usize>().ok()) {
                                // `waitgames:<n>` (tooling) — run the browsing socket as the frame
                                // tick does until `n` sessions have answered, 20 s at most
                                let until = Instant::now() + Duration::from_secs(20);
                                while Instant::now() < until && self.net.lobby.as_ref().is_some_and(|l| l.found.len() < n) {
                                    if let Some(l) = self.net.lobby.as_mut() {
                                        l.poll();
                                    }
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                                self.bind_lists();
                            } else if let Some(ms) = k.strip_prefix("wait:").and_then(|n| n.parse::<u64>().ok()) {
                                // `wait:<ms>` (tooling) — the lobby's tick for that long: the
                                // session drains and the option records go out / come in
                                let until = Instant::now() + Duration::from_millis(ms);
                                while Instant::now() < until {
                                    if let Some(s) = self.net.session.as_mut() {
                                        s.poll_lobby();
                                    }
                                    self.net_sync_options();
                                    self.net_lobby_step();
                                    self.comm_tick();
                                    self.update_check_tick();
                                    self.net_browse_step();
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                                self.bind_lists();
                            } else if let Some((acc, d)) = k.strip_prefix("step:").and_then(|r| r.rsplit_once(':')).and_then(|(a, d)| Some((a.to_string(), d.parse::<i32>().ok()?))) {
                                // `step:<accessor>:<delta>` (tooling) — a combo's arrow (`set_accessor`),
                                // in order with the keys around it (`Comms.PlayerList`'s selection)
                                self.set_accessor(&acc, d);
                                self.bind_lists();
                            } else if let Some(n) = k.strip_prefix("waitplayers:").and_then(|n| n.parse::<usize>().ok()) {
                                // `waitplayers:<n>` (tooling) — run the session as the lobby's tick
                                // does until the room holds `n` players, 60 s at most
                                let until = Instant::now() + Duration::from_secs(60);
                                while Instant::now() < until && self.net.session.as_ref().is_some_and(|s| s.room_len() < n) {
                                    if let Some(s) = self.net.session.as_mut() {
                                        s.poll_lobby();
                                    }
                                    self.net_sync_options();
                                    self.net_lobby_step();
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                                self.net_sync_options();
                                self.bind_lists();
                            } else if let Some(n) = k.strip_prefix("waitready:").and_then(|n| n.parse::<usize>().ok()) {
                                // `waitready:<n>` (tooling) — the lobby's tick until the room holds
                                // `n` players and PLAYGAME's gate 0x5091B0 holds, 60 s at most
                                let until = Instant::now() + Duration::from_secs(60);
                                while Instant::now() < until && !self.net.session.as_ref().is_some_and(|s| s.room_len() >= n && s.everyone_ready()) {
                                    if let Some(s) = self.net.session.as_mut() {
                                        s.poll_lobby();
                                    }
                                    self.net_sync_options();
                                    self.net_lobby_step();
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                                self.bind_lists();
                            } else if let Some(text) = k.strip_prefix("dialog:") {
                                // `dialog:<text>` (tooling) - type `text` after what an input
                                // dialog holds and answer it "yes", as Return would once its tick
                                // has read the latch (the launch has no frame tick to wait for)
                                if let Some((mut d, purpose)) = self.fe.dialog.take() {
                                    text.chars().for_each(|c| {
                                        d.type_char(c);
                                    });
                                    self.fe.dialog_input = d.field.as_ref().map(|f| f.text.clone());
                                    self.fe.dialog_answer = Some((true, purpose));
                                    self.fe_dialog_answer(event_loop);
                                }
                            } else if k == "enter" {
                                // `enter` — Return on the focused input box, through the key
                                // path's own commit (the chat box's send, the list box's commit)
                                if let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_input()) {
                                    if let Some(text) = self.fe.layout_preview.as_mut().and_then(|l| l.take_chat(i)) {
                                        let empty = self.fe.strings.get(287).unwrap_or_default().to_string();
                                        if let Some(n) = self.net.session.as_mut() {
                                            n.say(&text, &empty);
                                        }
                                    } else if self.fe.layout_preview.as_ref().and_then(|l| l.class_of(i)).as_deref() == Some("NFS5.CHARVECTACCESSORINPUTBOX") {
                                        self.fe_charvect_return(i);
                                    } else if let Some(l) = self.fe.layout_preview.as_mut() {
                                        l.commit_input(i);
                                    }
                                }
                            } else if let Some(text) = k.strip_prefix("keys:") {
                                // `keys:<text>` — each char as a real key press reaches the
                                // focused input box (the keyboard path's own `focused_input` +
                                // `fe_input_key`, no accessor named — unlike `type:`)
                                for c in text.chars() {
                                    if let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_input()) {
                                        self.fe_input_key(i, c);
                                    }
                                }
                                self.bind_player_buttons();
                            } else if let Some((acc, text)) = k.strip_prefix("type:").and_then(|r| r.split_once('=')) {
                                // `type:<accessor>=<text>` — the text typed into the input box
                                // bound to that accessor, through the box's own filter (0x4CCC20)
                                if let Some(l) = self.fe.layout_preview.as_mut() {
                                    l.type_into(acc, text);
                                }
                                self.bind_player_buttons();
                            }
                        }
                    }
                    tracing::info!(key = k, screen = ?self.fe.layout_name, nav = self.fe.nav.describe(), dialog = self.fe.dialog.is_some(), "front end: NFS_FE_KEYS");
                }
            }
            // diagnostic: `NFS_FE_STEP=<accessor>:<delta>[,…]` steps accessors on start-up as a
            // combo slider's arrows do (`set_accessor`), e.g. `Tournament.Tournament:1`
            if self.fe.layout_preview.is_some()
                && let Ok(spec) = std::env::var("NFS_FE_STEP")
            {
                for (name, d) in spec.split(',').filter_map(|s| s.rsplit_once(':')) {
                    if let Ok(d) = d.trim().parse::<i32>() {
                        self.set_accessor(name.trim(), d);
                    }
                }
                // was `bind_lists()` alone; a combo slider's own arrows run the full
                // `bind_after_accessor()` (this diagnostic's own doc comment above says it steps
                // "as a combo slider's arrows do") — same audit as the CARSETTINGBUTTON/track-
                // arrow/dealer-arrow fixes
                self.bind_after_accessor();
            }
            // diagnostic: `NFS_FE_PLAYGAME=<Game.GameType>` sets the type and presses `PLAYGAME` on
            // start-up — with `--autotest N` the race runs headless and the log shows where the
            // front end comes back to (`fe_after_race`)
            if self.fe.layout_preview.is_some()
                && let Some(t) = std::env::var("NFS_FE_PLAYGAME").ok().and_then(|v| v.trim().parse::<u8>().ok())
            {
                self.fe.game_type = t;
                let started = if t == 4 && self.career.is_some() {
                    self.start_tournament_from_layout(event_loop)
                } else {
                    self.fe_start_game(event_loop);
                    self.race.is_some()
                };
                if started {
                    return;
                }
                // a start check that said no raised its dialog (`CareerUi::dialog`)
                if let Some((id, _)) = self.career.as_ref().and_then(|c| c.dialog) {
                    tracing::info!(dialog = id, "front end: PLAYGAME refused");
                }
                // a scripted run has nobody to answer it
                if self.autotest.is_some() {
                    event_loop.exit();
                    return;
                }
            }
            if let Some(l) = self.fe.layout_preview.as_ref() {
                for (c, n) in l.unrendered() {
                    tracing::info!(
                        class = c,
                        count = n,
                        "layout widget placed by code, not drawn"
                    );
                }
                for (c, src) in l.unrendered_sources() {
                    tracing::info!(class = c, source = src, "unbound widget source");
                }
            }
            // `--sprite-selftest`: draw three quads through the real sprite path — a grey ground,
            // a black square and a white square — and print what came back. The smallest case that
            // can show the fault in `RE/AUDIT.md` F2, using the pipeline the front end uses.
            if std::env::var("NFS_SPRITE_SELFTEST").is_ok() {
                let white = self.fe.layout_preview.as_ref().map(|l| l.white_texture());
                let atlas = self.fe.layout_preview.as_ref().and_then(|l| l.a_font_texture());
                if let (Some(tex), Some(r)) = (white, self.renderer.as_mut()) {
                    let q = |x: f32, c: [f32; 4]| nfs_renderer::Sprite {
                        texture: tex,
                        pos: [x, 0.25],
                        size: [0.2, 0.2],
                        uv_min: [0.0, 0.0],
                        uv_max: [1.0, 1.0],
                        color: c,
                        rotation: 0.0,
                        pivot: [0.0, 0.0],
                        tri: None, corner_colors: None,
                        depth: None,
                    };
                    let mut scene = nfs_renderer::Scene::default();
                    let _ = &atlas;
                    scene.sprites = vec![
                        nfs_renderer::Sprite { pos: [0.0, 0.0], size: [1.0, 1.0], ..q(0.0, [0.5, 0.5, 0.5, 1.0]) },
                        q(0.1, [0.0, 0.0, 0.0, 1.0]),
                        q(0.4, [1.0, 1.0, 1.0, 1.0]),
                        q(0.7, [1.0, 0.0, 0.0, 1.0]),
                        // the same three at the size a glyph is actually drawn
                        nfs_renderer::Sprite { pos: [0.15, 0.6], size: [0.01875, 0.01875], ..q(0.0, [0.0, 0.0, 0.0, 1.0]) },
                        nfs_renderer::Sprite { pos: [0.45, 0.6], size: [0.01875, 0.01875], ..q(0.0, [1.0, 1.0, 1.0, 1.0]) },
                        nfs_renderer::Sprite { pos: [0.75, 0.6], size: [0.01875, 0.01875], ..q(0.0, [1.0, 0.0, 0.0, 1.0]) },
                    ];
                    // and the same three with the font atlas, sampling one texel that the ASCII
                    // dump of `Univers8b` shows solid inside 'A' (atlas 240×61, glyph at 26,32)
                    if let Some(atlas) = atlas {
                        let uv = [28.5 / 240.0, 37.5 / 61.0];
                        let a = |x: f32, c: [f32; 4]| nfs_renderer::Sprite {
                            texture: atlas,
                            pos: [x, 0.8],
                            size: [0.01875, 0.01875],
                            uv_min: [uv[0] - 0.0005, uv[1] - 0.0005],
                            uv_max: [uv[0] + 0.0005, uv[1] + 0.0005],
                            color: c,
                            rotation: 0.0,
                            pivot: [0.0, 0.0],
                            tri: None, corner_colors: None,
                            depth: None,
                        };
                        scene.sprites.push(a(0.15, [0.0, 0.0, 0.0, 1.0]));
                        scene.sprites.push(a(0.45, [1.0, 1.0, 1.0, 1.0]));
                        scene.sprites.push(a(0.75, [1.0, 0.0, 0.0, 1.0]));
                    }
                    let rgba = r.render_to_image(&scene, &self.camera, 200, 100).expect("render");
                    let at = |x: usize, y: usize| {
                        let i = (y * 200 + x) * 4;
                        (rgba[i], rgba[i + 1], rgba[i + 2])
                    };
                    println!("big:   ground {:?}  black {:?}  white {:?}  red {:?}", at(5, 50), at(30, 30), at(90, 30), at(150, 30));
                    println!("glyph: black {:?}  white {:?}  red {:?}", at(31, 61), at(91, 61), at(151, 61));
                    println!("atlas: black {:?}  white {:?}  red {:?}", at(31, 81), at(91, 81), at(151, 81));
                }
                event_loop.exit();
                return;
            }
            if self.layout_sweep {
                self.layout_sweep = false;
                self.run_layout_sweep();
                event_loop.exit();
                return;
            }
            // (tooling: `NFS_SHOT_IN_RACE=1` leaves the still for the race that follows, so a
            // network joiner — which the host's `0xB3` starts — can be photographed in its race)
            if std::env::var_os("NFS_SHOT_IN_RACE").is_none()
                && let Some(path) = self.screenshot.take()
            {
                // a screen fades in over half a second (`RE/notes/controls.md` §6), so a shot taken
                // on the first frame catches every faded widget at alpha 0 and looks empty. Let the
                // transition finish first.
                // the frames run, for the room object's own transition (`fe_room_tick`), which
                // runs alongside the screen's
                let mut ran = 0;
                let mut tab_clicked = false;
                if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                    // a movie starts on a black frame and runs at fifteen a second, so a screen
                    // with one gets the same half second the fade does, whether or not the fade
                    // has already settled. A screenshot of a playing movie catches whatever frame
                    // its clock is on; this picks a consistent one.
                    let least = if l.has_movies() { 32 } else { 0 };
                    // diagnostic: `NFS_FE_FRAMES=<n>` stops after n frames, for a still part way
                    // through the screen's transition
                    let frames: Option<usize> = std::env::var("NFS_FE_FRAMES").ok().and_then(|v| v.parse().ok());
                    for i in 0..frames.unwrap_or(120) {
                        l.advance_movies(r, 16, self.snd.audio.as_ref(), self.snd.audio_settings.music_level);
                        ran += 1;
                        // every layer's screen, as 0x4D2570 does (0x4D25EB)
                        for (_, u) in self.fe.layout_under.iter_mut() {
                            u.advance_transition(16);
                        }
                        if l.advance_transition(16) && i >= least && frames.is_none() {
                            break;
                        }
                    }
                    // diagnostic: `NFS_FE_TAB=<widget name>[@nth]:<n>[,<widget name>[@nth]:<n>…]` clicks that
                    // tab button once the screen has settled and takes the still n frames later. `@nth`
                    // (0-based) picks which widget of that name when a tab strip reuses one name
                    // for every button (`controllertabs.lay`, `graphicstabs.lay` — see
                    // `flat_index_of`); omitted, it is the first (`@0`), which is enough for the
                    // strips that name each button uniquely (`showcasetabs.lay`,
                    // `optiontabs.lay`).
                    // A chain `A:60,B:60` clicks tab A, runs 60 frames, then clicks tab B (looked
                    // up in the layout as it is *then*, so a tab strip that only exists inside the
                    // panel A loaded — Options' "controllers" panel embeds `Controllers.lay` with its
                    // own strip — is reached through its real parent), and so on; the single form
                    // `A:60` is a chain of one. A step whose widget is not found ends the chain.
                    if let Ok(chain) = std::env::var("NFS_FE_TAB") {
                        for step in chain.split(',') {
                            let Some((spec, n)) = step.split_once(':').map(|(a, b)| (a, b.parse::<usize>().unwrap_or(0))) else {
                                break;
                            };
                            let (name, nth) = spec.split_once('@').map_or((spec, 0), |(a, b)| (a, b.parse::<usize>().unwrap_or(0)));
                            let Some(hit) = l.flat_index_of(name, nth) else {
                                break;
                            };
                            l.tab_click(r, &self.paths, hit);
                            tab_clicked = true;
                            for _ in 0..n {
                                l.advance_transition(16);
                                ran += 1;
                            }
                        }
                    }
                }
                for _ in 0..ran {
                    self.fe_room_tick(16);
                }
                // same re-bind `tab_click`'s real-click handler does above — the diagnostic path
                // loads a new panel too, and every picture-from-code / accessor-bound widget
                // class that can sit in one (the HUD preview, `showcase.lay`'s
                // `glancestats`/`chassis`/`performance` panels' own `NFS5.ShowcasePicture`s, a
                // `Controllers.lay`/`graphics.lay` panel's `NFS5.GouraudIndicatorBar`…) needs
                // `bind_screen_widgets()` run again now that the panel exists
                if tab_clicked {
                    self.bind_screen_widgets();
                }
                // diagnostic (tooling): `NFS_FE_CLICKS=x,y;x,y;…` presses and releases the left
                // button at each design point in turn, through the same handler a real click takes
                // (`fe_left_button`), letting the screen run four frames after each. A token of the
                // shape `drag:x1,y1>x2,y2` instead presses at the first point, steps the pointer to
                // the second over several moves (through `fe_apply_combo_drag` and
                // `picture_drag_to`, the same calls a real `CursorMoved` drag runs — main.rs's
                // own `WindowEvent::CursorMoved` arm), and releases there — for exercising the
                // open combo's own scrollbar knob, or `NFS5.ChroniclePicture`'s drag (0x5082A0/
                // 0x5082F0, `Layout::picture_drag_to`), without a live mouse. Added for user
                // report 2026-09-30 (b); there is no `wheel:` token, because the original drops
                // the wheel entirely (`RE/notes/controls.md` §4.40, NOT A BUG — nothing to script).
                if let Ok(v) = std::env::var("NFS_FE_CLICKS") {
                    for p in v.split(';') {
                        if let Some(rest) = p.strip_prefix("drag:")
                            && let Some((p1, p2)) = rest.split_once('>')
                            && let Some((x1, y1)) = p1.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?)))
                            && let Some((x2, y2)) = p2.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?)))
                        {
                            self.fe.cursor = Some((x1, y1));
                            if let Some(l) = self.fe.layout_preview.as_mut() {
                                l.script_cursor(Some((x1, y1)));
                                for _ in 0..60 {
                                    l.advance_transition(16);
                                }
                            }
                            self.fe_left_button(event_loop, true);
                            const STEPS: i32 = 12;
                            for step in 1..=STEPS {
                                let t = step as f32 / STEPS as f32;
                                let (x, y) = (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
                                self.fe.cursor = Some((x, y));
                                if let Some(l) = self.fe.layout_preview.as_mut() {
                                    l.script_cursor(Some((x, y)));
                                    l.picture_drag_to(x, y);
                                }
                                self.fe_apply_combo_drag();
                            }
                            let dest = self.fe.layout_preview.as_ref().and_then(|l| l.combo_scroll_at(x2, y2));
                            let dragging = self.fe.layout_preview.as_ref().and_then(|l| l.dragging_picture());
                            self.fe_left_button(event_loop, false);
                            tracing::info!(x1, y1, x2, y2, ?dest, ?dragging, "NFS_FE_CLICKS drag");
                            for _ in 0..4 {
                                if let Some(l) = self.fe.layout_preview.as_mut() {
                                    l.advance_transition(16);
                                }
                            }
                            continue;
                        }
                        let Some((x, y)) = p.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))) else { continue };
                        self.fe_scripted_click(x, y, event_loop);
                    }
                }
                // and a room that is still changing — the last screen's going out, then this one's
                // coming in (`fe_room_tick`) — is let settle too, the screen's clock running with it
                let settle = std::env::var("NFS_FE_FRAMES").is_err() && std::env::var("NFS_FE_TAB").is_err();
                for _ in 0..240 {
                    if !settle || self.fe.room_obj.is_none_or(|(_, t)| matches!(t, nfs_game::layout::Transition::Idle)) {
                        break;
                    }
                    self.fe_room_tick(16);
                    for (_, u) in self.fe.layout_under.iter_mut() {
                        u.advance_transition(16);
                    }
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.advance_transition(16);
                    }
                }
                // a network screen is taken on its first frame, before any `'d'` has had time to
                // come back, so give the probe half a second — otherwise the shot shows an empty
                // list that says nothing about the screen
                if self.net.lobby.is_some() {
                    let until = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < until {
                        if let Some(l) = self.net.lobby.as_mut() {
                            l.poll();
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    self.bind_lists();
                    self.bind_checks();
                }
                // `NFS_FE_DIALOG` raises its dialog in the dialog step, which a still otherwise
                // never reaches
                // (and a dialog a scripted network run raised: `comm_tick`'s notices)
                if std::env::var("NFS_FE_DIALOG").is_ok() || self.fe.dialog.is_some() {
                    self.fe_dialog_step();
                }
                // the same scene the front end draws every frame, so a screenshot shows the 3D room too
                self.rebuild_scene();
                let mut scene = self.scene.clone();
                // `NFS_FE_KEEP=<n>` renders only the first n sprites of the front end's own list —
                // the bisect for `RE/AUDIT.md` F2
                if let Ok(n) = std::env::var("NFS_FE_KEEP").map(|v| v.parse::<usize>().unwrap_or(usize::MAX)) {
                    println!("fe sprites: {}, keeping {}", scene.sprites.len(), n.min(scene.sprites.len()));
                    scene.sprites.truncate(n);
                }
                let scene = scene;
                let rgba = self
                    .renderer
                    .as_mut()
                    .unwrap()
                    .render_to_image(&scene, &self.camera, shot_geometry().0, shot_geometry().1)
                    .expect("render");
                image::save_buffer(&path, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8).expect("png");
                event_loop.exit();
            }
            return;
        }
        self.start_race(renderer, window, event_loop);
    }

    /// "race again" — command 3 at 0x415721: the race audio is reset (0x4AB360), the simulation is
    /// set up again (0x412090) with the same options, `REPLAY [0x657424]` and `[0x5E99F4]` are
    /// cleared so the new run records afresh, and the frame loop carries on — no front-end round
    /// trip. Here that is `start_race` again on the loaded track and car.
    /// diagnostic: the next step of a scripted session (`NFS_AUTOTEST_RACES`), run where an
    /// `--autotest` race's ticks are over. First the race's automatic replay (the session loop's
    /// arm 0x4B6EF4…0x4B6F18, here without waiting for race over), `secs` of it; then the race is
    /// left and the next one started: through the front end when one launched it
    /// ([`App::end_finished_race`], then `PLAYGAME` again), else as the session loop's next pass
    /// from the same arguments would (0x4B6F2F → 0x4B68F0, 0x4B6C0C … 0x4B67B0). `true` when it
    /// took over (the next run is under way and has returned).
    pub(crate) fn session_step(&mut self, event_loop: &ActiveEventLoop, secs: f32) -> bool {
        if self.session_races <= 1 {
            return false;
        }
        // the script's own keys are let go, as a player's would be between the runs
        self.input.release(Action::Throttle);
        self.input.release(Action::SteerRight);
        let recorded = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Recording | nfs_game::replay::Mode::Full));
        if !self.session_replayed && recorded {
            self.session_replayed = true;
            tracing::info!(left = self.session_races, "session: the automatic replay");
            self.autotest = Some(secs);
            self.replay_again(event_loop, true);
            return true;
        }
        self.session_replayed = false;
        self.session_races -= 1;
        tracing::info!(left = self.session_races, "session: leave, next race");
        self.autotest = Some(secs);
        if self.fe.menu_template.is_some() {
            let game_type = self.fe.race_game_type.unwrap_or(self.fe.game_type);
            self.end_finished_race();
            self.fe.game_type = game_type;
            self.start_from_layout(event_loop);
        } else {
            self.save_replay();
            self.stop_race_audio();
            self.replay_session_end();
            self.sim_shutdown();
            let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return true };
            self.tick = 0;
            self.start_race(renderer, window, event_loop);
        }
        true
    }
}
