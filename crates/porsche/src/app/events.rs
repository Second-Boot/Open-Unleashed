//! The winit event handlers: window events, the intro's event path and the application handler.

use crate::*;

impl App {
    /// The window's events while the start-up movies play: the player reads the keyboard ring for
    /// Esc (0x55FF60) and the mouse's buttons (0x5372A0(6)); nothing else is listening yet.
    fn intro_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(run) = self.intro.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => run.renderer.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                run.key(event.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let i = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => 3,
                };
                run.button(i, state == ElementState::Pressed);
            }
            WindowEvent::RedrawRequested => {
                // `NFS_PLAY_INTRO` on a scripted run: hold Esc down the way an impatient player
                // would, through the same key path (`run.key`), so the walk reaches the front end
                // without sitting through the whole movie.
                if NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) && PLAY_INTRO.load(std::sync::atomic::Ordering::Relaxed) {
                    run.key(true);
                }
                if run.frame(self.snd.audio.as_ref(), &self.camera) {
                    let run = self.intro.take().expect("intro");
                    if self.idle_movie_running {
                        // 0x4DC140 returned: back to the screen, `MusicPlay("Menu*", 0)`, the stamp
                        self.idle_movie_running = false;
                        run.window.request_redraw();
                        self.window = Some(run.window);
                        self.renderer = Some(run.renderer);
                        self.finish_idle_movie();
                        return;
                    }
                    self.intro_done = Some((run.window, run.renderer));
                    self.resume_inner(event_loop);
                    // the window outlives the movies, so winit sends no first redraw of its own
                    // for the front end — without this request the loading still stays forever
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `self.window` stays `None` for as long as the intro plays (`resume_inner`'s intro branch
        // returns before setting it) — a second `resumed` (winit's own contract allows more than
        // one on a suspend/resume platform) during the movies would otherwise fall through to
        // `resume_inner`'s `None` arm and build a second window under the first's nose. Tooling
        // hardening only, not a game rule: the original has no such lifecycle to diverge from.
        if self.window.is_some() || self.intro.is_some() {
            return;
        }
        self.resume_inner(event_loop);
    }

    /// The process ends inside a leave's wait: finish it here (the original's wait blocks, so nothing ends
    /// before it).
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        while !self.net.leaving.is_empty() {
            self.net_leaving_step();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.mouse_motion(delta.0, delta.1);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.net_leaving_step();
        if self.idle_movie_due && self.intro.is_none() {
            self.start_idle_movie();
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.sync_gamma();
        }
        if self.intro.is_some() {
            self.intro_event(event_loop, event);
            return;
        }
        // `FEApplication`'s event dispatch 0x4D2850 hands every front-end event to the callback list
        // `[fe+0xE0]` first (slot 3, 0x4D2868) — `FEMainScreen`'s is 0x510EA0, which stamps `[+0x290]`
        // with `TIMER_getTicks()` and lets the event go on (returns 0) — so the main screen's minute
        // (0x510210) counts from the last key, button or pointer move, not from the screen's entry
        // (REFERENCE-2026-10-02-SRVB.md §2)
        if matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::CursorMoved { .. })
            && self.fe.menu.is_some()
            && self.vehicle.is_none()
            && self.fe_list_now() == "ID_MAINSCREEN"
        {
            self.fe.main_music_stamp = self.fe_timer_ticks();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Focus lost = the original's WM_ACTIVATE deactivation (0x53B050, registered for message 6 at 0x53AD66):
            // LOWORD(wParam) == 0 or HIWORD != 0 -> ShowCursor(1) (0x53B113) and memset(0x5DE028, 0, 0x80)
            // (0x53B119..0x53B125) = the key-down table forgets every key. Focus regained: the original
            // clears nothing (0x53B07F..0x53B0CC only re-hooks the Windows key); the port clears again there
            // because macOS (Cmd+Shift+5 overlay) can hand back a synthetic press for a key whose release was
            // swallowed — PORT-DEVIATION D17 (remainder, FOCUS-INPUT).
            WindowEvent::Focused(_) => self.input.release_all(),
            WindowEvent::KeyboardInput { event, .. } => {
                self.input.key_event(&event);
                // 0x4D2850: the callback list sees the key, then a screen or the background in
                // transition, or a busy room, drops it (`fe_input_swallowed`)
                if self.fe_input_swallowed() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        if matches!(event.logical_key.as_ref(), Key::Named(NamedKey::Escape)) {
                            self.fe_callback_key(0x1B);
                        }
                    }
                    self.input.end_tick();
                    return;
                }
                // **A binding row waiting for a control takes every key** — its slot 4, 0x51C610,
                // answers true whatever it does, so nothing else on the screen sees the key (not
                // even Esc). It takes one only when exactly **one** key is down (the walk of the
                // key-down table 0x5DE028 at 0x51C623), and 0x51D0A0 then drops any key 0x51C050
                // cannot name — the row keeps waiting. A named key becomes `{3, 0, scancode, 0, 0,
                // code}`, is committed as `Bindings::assign` describes, and focus goes (0x51C65C).
                // `CameraView` and `LookBehind` are `I_*` bindings this port does not read, so
                // their rows only let focus go.
                if event.state == winit::event::ElementState::Pressed
                    && let Some((i, acc)) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_binding_row())
                {
                    if let winit::keyboard::PhysicalKey::Code(k) = event.physical_key
                        && let Some(sc) = nfs_input::scancode(k)
                        && self.input.keys_down_count() == 1
                    {
                        let code = nfs_input::bindings::display_code(sc);
                        let (name, arc) = nfs_game::keyname::key_name(code, &self.fe.strings);
                        if !name.is_empty() || arc.is_some() {
                            if let Some(id) = accessors::controller_action(&acc).and_then(nfs_input::binding_id) {
                                let b = nfs_input::bindings::Binding { kind: 3, device: 0, index: sc as u32, dir: 0, pov: 0, key: code };
                                self.input.bindings.assign(id, b);
                                tracing::info!(row = %acc, scancode = sc, "control rebound");
                            }
                            if let Some(l) = self.fe.layout_preview.as_mut() {
                                l.release_focus(i);
                            }
                        }
                    }
                    return;
                }
                // **`HReplaySave`'s field takes the typed characters** (0x42E375 → 0x435630, the
                // window's `WM_CHAR` queue) — only while it is the dialog up
                if event.state == winit::event::ElementState::Pressed
                    && self.replay_save.up
                    && !self.replay_save.overwrite
                    && !self.paused
                {
                    use winit::keyboard::{Key, NamedKey};
                    let took = match event.logical_key.as_ref() {
                        Key::Named(NamedKey::Backspace) => self.replay_save.type_char('\u{8}'),
                        Key::Named(NamedKey::Space) => self.replay_save.type_char(' '),
                        Key::Character(t) => t.chars().fold(false, |a, c| self.replay_save.type_char(c) || a),
                        _ => false,
                    };
                    if took {
                        return;
                    }
                }
                // **An input dialog's field takes the keyboard first** (its key handler 0x514C60)
                if event.state == winit::event::ElementState::Pressed
                    && let Some((d, _)) = self.fe.dialog.as_mut()
                    && d.up
                    && d.field.is_some()
                {
                    use winit::keyboard::{Key, NamedKey};
                    let sound = match event.logical_key.as_ref() {
                        Key::Named(NamedKey::Enter) => d.enter(),
                        Key::Named(NamedKey::Backspace) => d.backspace(),
                        Key::Named(NamedKey::Space) => d.type_char(' '),
                        Key::Character(t) => t.chars().next().and_then(|c| d.type_char(c)),
                        _ => None,
                    };
                    if let Some(s) = sound {
                        self.fe_sound(s as usize);
                        return;
                    }
                }
                // **A focused input box swallows the keyboard.** 0x4CCC20 is slot 4 of the three
                // input classes, and it returns *true* for every key it takes, which is what stops
                // the screen's own arrows and Enter from seeing it. Return commits, backspace
                // erases, and everything printable but `'%'` goes in — the filter itself lives in
                // `LayoutScreen::type_char`, beside the addresses it came from.
                if event.state == winit::event::ElementState::Pressed
                    && let Some(i) = self.fe.layout_preview.as_ref().and_then(|l| l.focused_input())
                {
                    use winit::keyboard::{Key, NamedKey};
                    let mut took = false;
                    match event.logical_key.as_ref() {
                        // the chat box's own Return, 0x4C3310 (`netplay::NetPlay::say`)
                        Key::Named(NamedKey::Enter)
                            if let Some(text) = self.fe.layout_preview.as_mut().and_then(|l| l.take_chat(i)) =>
                        {
                            let empty = self.fe.strings.get(287).unwrap_or_default().to_string();
                            if let Some(n) = self.net.session.as_mut() {
                                n.say(&text, &empty);
                            }
                            took = true;
                        }
                        // `NFS5.CHARVECTACCESSORINPUTBOX`'s own Return, slot 4 **0x4C2120** (in
                        // place of 0x4CCC20): a non-empty string goes into the list accessor at
                        // row 0 (0x4B8B70 with 0, the list's front) and the list selects row 0
                        // (slot 0x2C(0), 0x4C21B8); then `data.callback` when there is one
                        // (0x50D6D0 — empty on both shipped boxes), else the string's commit
                        // (slot 0x0C); focus off (0x4C7DA0), the update, and **no sound**. The
                        // port's `Comms.IPList` is the lobby's own list, whose row 0 is the typed
                        // address — so the address becomes `Comms.IP` and the selection 0.
                        Key::Named(NamedKey::Enter)
                            if self.fe.layout_preview.as_ref().and_then(|l| l.class_of(i)).as_deref() == Some("NFS5.CHARVECTACCESSORINPUTBOX") =>
                        {
                            self.fe_charvect_return(i);
                            return;
                        }
                        Key::Named(NamedKey::Enter) => {
                            if let Some(l) = self.fe.layout_preview.as_mut()
                                && let Some((name, text)) = l.commit_input(i)
                            {
                                tracing::info!(accessor = %name, value = %text, "input box committed");
                                took = true;
                            }
                        }
                        Key::Named(NamedKey::Backspace) => {
                            took = self.fe.layout_preview.as_mut().is_some_and(|l| l.backspace(i));
                        }
                        Key::Character(t) => {
                            if let Some(c) = t.chars().next() {
                                took = self.fe_input_key(i, c);
                            }
                        }
                        Key::Named(NamedKey::Space) => {
                            took = self.fe.layout_preview.as_mut().is_some_and(|l| l.type_char(i, ' '));
                        }
                        _ => {}
                    }
                    // Every accepted path in 0x4CCC20 ends at `0x4AADB0(9)`.
                    if took {
                        self.fe_sound(9);
                        // the name field's text gates `ID_CREATEPLAYEROKBUTTON`
                        self.bind_player_buttons();
                        return;
                    }
                }
                // **The front end's ESC and `b`.** The key code is the character (§4.166): the
                // callback list's slot 4 sees it first — `careerfirst`'s 0x506170 and the
                // end-of-era show's 0x500770 take 0x1B (the movie circle's tick 0x4CD410 then sees
                // ESC down and opens `tournament`; the show's tick goes back) — and what nothing
                // took reaches the manager's 0x4D2970 (`fe_manager_key`).
                if self.fe.layout_preview.is_some() && self.fe.dialog.is_none() && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{Key, NamedKey};
                    let code = match event.logical_key.as_ref() {
                        Key::Named(NamedKey::Escape) => Some(0x1Bu8),
                        Key::Character("b") => Some(0x62),
                        _ => None,
                    };
                    if let Some(code) = code {
                        self.fe_key_code(code, event_loop);
                        self.input.end_tick();
                        return;
                    }
                }
                // PORT-DEVIATION D10 (PORT-DEVIATIONS.md) [the FE key slot 0x4D2970 knows ESC, `b`, `=` alone]: arrows / Enter drive the menus
                if self.fe.menu.is_some() && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{KeyCode, PhysicalKey};
                    let key = match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowUp) => Some(MenuKey::Up),
                        PhysicalKey::Code(KeyCode::ArrowDown) => Some(MenuKey::Down),
                        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(MenuKey::Left),
                        PhysicalKey::Code(KeyCode::ArrowRight) => Some(MenuKey::Right),
                        PhysicalKey::Code(KeyCode::Enter) => Some(MenuKey::Enter),
                        // the text menu's own back; on a layout screen the original's backspace (8)
                        // is only an input box's (0x4CCC20) — the back key is `b`, above
                        PhysicalKey::Code(KeyCode::Backspace) if self.fe.layout_preview.is_none() => Some(MenuKey::Back),
                        _ => None,
                    };
                    if let Some(k) = key {
                        self.menu_key(k, event_loop);
                    }
                    self.input.end_tick();
                    return;
                }
                // Esc / P / Start: pause. The pause action (`0x4138D0(8, 1)`, 0x415170) opens the
                // pause menu (0x461740, 0x426800) only while the HUD's dialog queue is empty —
                // 0x41518C tests it with 0x426A60 first — and with a dialog up it goes to
                // 0x4151ED, which acts only in a network game (`[0x6573E8] == 3` → 0x41523A, the
                // resume). So in a single-player race the key does nothing while the pause menu,
                // its quit question or a post-race page is up: the menu is left through its own
                // buttons (0x42A6A0), and leaving the race through quit and its question, as
                // 0x426154 does. *(Until 2026-09-26 the port's pause key backed out of the
                // question or resumed — an approximation of a key the original does not have.)*
                // Once the pages have closed it opens the menu, whose quit leaves
                let dialog_up = self.paused || (self.results.is_some() && self.result_pages().get(self.results_page as usize).is_some());
                if self.vehicle.is_some() && self.fe.race_game_type == Some(game_type::NETWORK) {
                    // a network race (`[0x6573E8] == 3`): 0x415110's own rules, not the single-player pause
                    if self.input.take_pressed(Action::Pause) {
                        self.net_pause_action(event_loop);
                    }
                } else if dialog_up {
                    let _ = self.input.take_pressed(Action::Pause);
                } else if self.input.take_pressed(Action::Pause) {
                    if self.vehicle.is_some() {
                        self.paused = true;
                        // 0x4151E3 → 0x461740: the orbit's yaw and step start at 0
                        self.orbit = Orbit::default();
                        self.pause_sel = 0;
                        self.pause_confirm = None;
                    } else {
                        event_loop.exit();
                    }
                }
                // the pause orbit's step keys, read on the press only while a dialog is up outside
                // a replay (0x415855…0x41589D, in this order, so the last wins): `I_ROTCAMCCLOCK`
                // (F4, bit 0x0A) +3 (0x461790), `I_ROTCAMCLOCK` (F2, 0x0C) −3 (0x4617A0),
                // `I_ROTCAMSTOP` (F3, 0x0B) 0 (0x4617B0)
                if self.vehicle.is_some()
                    && self.hud_dialog_up()
                    && !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
                {
                    for (a, step) in [(Action::RotCamCounter, 3), (Action::RotCamClock, -3), (Action::RotCamStop, 0)] {
                        if self.input.take_pressed(a) {
                            self.orbit.step = step;
                        }
                    }
                }
                // `I_TOGGLEPAUSEMENU` (F1): 0x41596E reads bit 0x0E on the press whatever the pause,
                // and 0x425750 flips `[0x60CA00]` only with a dialog up
                if self.input.take_pressed(Action::TogglePauseMenu) && self.vehicle.is_some() && self.hud_dialog_up() {
                    self.hud_hidden = !self.hud_hidden;
                }
                // the HUD toggles, read on the press in this order while the pause is off
                // (`[0x606A90]`, 0x415982): F5 0x18, "2" 0x1C, F6 0x1A, F7 0x1B, F8 0x1F
                // (0x4159C9…0x415A8E)
                for a in [Action::ToggleHud, Action::CycleMap, Action::ToggleRearView, Action::ToggleMap, Action::HudTransparency] {
                    if self.input.take_pressed(a) && self.vehicle.is_some() && !self.paused {
                        self.hud_key(a);
                    }
                }
                // `I_SFX` (F10, bit 0x11) → 0x4AB4E0 and `I_MUSIC` (F9, bit 0x12) → 0x4AD790, read
                // on the press by `RaceFrame` whatever the pause (0x415A93…0x415AB6), both only with
                // the audio up (`[0x657C94]` clear, 0x4AB4E0; `[0x656AB4]`, 0x4AD790)
                if self.input.take_pressed(Action::Sfx)
                    && self.vehicle.is_some()
                    && let Some(a) = &self.snd.audio
                {
                    if self.snd.sfx_saved.is_none() {
                        a.silence_voices();
                    }
                    self.snd.audio_settings.toggle_effects(&mut self.snd.sfx_saved);
                    tracing::info!(on = self.snd.sfx_saved.is_none(), "I_SFX (0x4AB4E0)");
                }
                if self.input.take_pressed(Action::Music)
                    && self.vehicle.is_some()
                    && let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut())
                {
                    // the pick is `0x538040() + timeGetTime()` (0x4AD87E…0x4AD88B); the port's
                    // clock is the wall clock's milliseconds — APPROXIMATION for the time since
                    // Windows started
                    let rng = &mut self.fe_rng;
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u32);
                    m.toggle(a, &mut || rng.next_u32().wrapping_add(ms));
                    tracing::info!(playing = ?m.now_playing(), "I_MUSIC (0x4AD790)");
                }
                // The playback speeds (replay.md §7 commands 4…7), the replay's pause (3) and quit
                // (1 / 2) have no key: `RaceFrame`'s action switch (table 0x415D9C) takes its
                // action from 0x425EF0, the top HUD dialog's answer — the replay bar's cells, by
                // the pointer (`replay_bar_apply_press`). *(Corrected 2026-09-26: the port put the four
                // speeds on the digits 1…4; 1 and 2 are `I_CYCLEHUD` / `I_CYCLEMAP`.)*
                // `HPauseMenu` 0x42A440: continue (0x46BC) answers 2 and closes it, race again (0x46BD)
                // answers 3 — command 3, the in-place restart 0x415721 — and quit (0x46BE) answers
                // 0x1A, the "Are you sure you want to exit?" question (0x46FB); its yes sets
                // `[0x657E34] = 1` (aborted, 0x426182) and issues command 4, the exit
                // PORT-DEVIATION D10 (PORT-DEVIATIONS.md) [HButtonVisual picks by the cursor only, 0x41B3BA…0x41B3CE]: the arrows and Enter pick and answer the pause menu's buttons
                if self.paused && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{KeyCode, PhysicalKey};
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowUp | KeyCode::ArrowLeft) => match self.pause_confirm.as_mut() {
                            Some(c) => *c = 0,
                            None => self.pause_sel = self.pause_sel.saturating_sub(1),
                        },
                        PhysicalKey::Code(KeyCode::ArrowDown | KeyCode::ArrowRight) => match self.pause_confirm.as_mut() {
                            Some(c) => *c = 1,
                            None => self.pause_sel = (self.pause_sel + 1).min(2),
                        },
                        PhysicalKey::Code(KeyCode::Enter) => match (self.pause_confirm, self.pause_sel) {
                            (Some(0), _) => {
                                self.paused = false;
                                self.pause_confirm = None;
                                if !self.leave_race() {
                                    event_loop.exit();
                                }
                            }
                            (Some(_), _) => self.pause_confirm = None,
                            (None, 0) => self.paused = false,
                            (None, 1) => {
                                self.paused = false;
                                self.restart_race(event_loop);
                            }
                            _ => self.pause_confirm = Some(1),
                        },
                        _ => {}
                    }
                    self.input.end_tick();
                    return;
                }
                // `HMissionSummary`'s input slot 0x429390: a click on its "race again" button answers
                // event 3 — the pause menu's restart — but **only when the mission failed**
                // (`[0x657834] == 0`, 0x4293BB); otherwise it falls through to the billboard's Space.
                // The click is `hud_click`; Enter does the same here, a key of this port's
                // (PORT-DEVIATION D10 (PORT-DEVIATIONS.md) [0x429390, 0x4293BB], as the pause menu's keys are)
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter)
                    && self.results.is_some()
                    && self.result_pages().get(self.results_page as usize) == Some(&ResultPage::MissionSummary)
                    && self.mission_result.is_some_and(|(passed, _)| !passed)
                {
                    self.restart_race(event_loop);
                    self.input.end_tick();
                    return;
                }
                // Space on the pause action's `HInfo` 18190 (`[0x60C9FC]`, Spacebar byte 1): the billboard's
                // slot 9 answers 0x19 and the notice slides out (0x42A3D0, 0x4261ED)
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space)
                    && self.vehicle.is_some()
                    && self.net_notice.is_some()
                    && self.net_notice_space
                {
                    self.net_notice = None;
                    self.net_notice_space = false;
                    self.input.end_tick();
                    return;
                }
                // **Space** closes the page on top and the queue shows the next (0x4272FF): the
                // billboard's input slot 0x42A3D0 answers event 0x19 on the key-down edge of
                // `[0x5DE028 + DIK_SPACE]` while its enable byte is set, and 0x4261ED slides the
                // current page out. This port used Enter, which no billboard reads.
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space)
                    && self.results.is_some()
                    && (self.results_page as usize) + 1 < self.result_pages().len()
                {
                    self.close_results_page();
                    self.input.end_tick();
                    return;
                }
                // Space on the last page closes it and returns to the menu (window and renderer are
                // kept); with no page at all (a test drive) the same key leaves
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space)
                    && self.results.is_some()
                    && self.end_finished_race()
                {
                    self.input.end_tick();
                    return;
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
                // a new mode: the dll's SetMode cells (`camera::set_mode`)
                nfs_game::camera::set_mode(size.width, size.height);
            }
            // The pointer, mapped back through `place`: the design area keeps 4:3 and is centred
            // in a wider window, so the inverse is `(u - x0) / fw * 640` across and `v * 480` down.
            WindowEvent::CursorMoved { position, .. } => {
                // in a race the pointer is in the HUD's own pixels: the window's logical point minus the
                // design screen's origin ([`Hud::origin`]); the front end keeps its letterboxed design space
                let race_hud = self.hud.is_some() && self.race.is_some();
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                self.fe.cursor = if race_hud {
                    let (ox, oy) = Hud::origin();
                    Some(((position.x / scale) as f32 - ox, (position.y / scale) as f32 - oy))
                } else { self.window.as_ref().map(|w| {
                    let size = w.inner_size();
                    let (sw, sh) = (size.width.max(1) as f32, size.height.max(1) as f32);
                    let (u, v) = (position.x as f32 / sw, position.y as f32 / sh);
                    let design_aspect = 640.0 / 480.0;
                    let aspect = sw / sh;
                    let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
                    ((u - x0) / fw * 640.0, v * 480.0)
                }) };
                // **`FEControl::onMouseEnter` 0x4C7B90 plays sound 6** through slot 23 (0x4C7A30):
                // on entering a widget whose state is now `"hover"` (the string compare at
                // 0x4C7BD9), that was not hovered already (0x4C7BA5) and is not disabled (bit 0x10,
                // 0x4C7C18). Not carried: 0x4C7C1C's accessor-driven disable (0x4E08E0), which
                // this port does not model (§4.110).
                // the move event is 0x4D2850's to drop as well: while it does, only the cursor
                // sprite follows the device (0x4D32DB); the widgets keep the pointer they had
                if self.fe_input_swallowed() {
                    let at = self.fe.cursor;
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.set_pointer(at);
                    }
                    if let Some(b) = self.fe.layout_bottom.as_mut() {
                        b.set_pointer(at);
                    }
                    return;
                }
                if let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_mut(), self.fe.cursor) {
                    l.set_cursor(Some((cx, cy)));
                }
                // a dialog's buttons take the pointer too (slot 8, 0x515060)
                if let (Some((d, _)), Some((cx, cy)), Some(l)) = (self.fe.dialog.as_mut(), self.fe.cursor, self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref())) {
                    let th = l.dialog_text_height(&d.text);
                    let sounds = d.pointer(cx, cy, th);
                    for s in sounds {
                        self.fe_sound(s as usize);
                    }
                }
                let now = self.fe.layout_preview.as_ref().and_then(|l| l.hovered());
                if now != self.fe.last_hover {
                    self.fe.last_hover = now;
                    if let (Some(i), Some(l)) = (now, self.fe.layout_preview.as_ref()) {
                        let controls = l.controls();
                        // same gate as `fe_sprites`'s `sel`: the keyboard cursor is not the
                        // original's `selected` flag until Up/Down has actually moved it
                        let sel = controls
                            .get(self.fe.layout_sel.min(controls.len().saturating_sub(1)))
                            .map(|c| c.index)
                            .filter(|_| self.fe.layout_nav_used);
                        if l.state_name(i, sel).as_deref() == Some("hover") && !l.widget_disabled(i) {
                            self.fe_sound(6);
                        }
                    }
                }
                // Dragging the scrollbar's knob. `FeScrollBar`'s slot 8, 0x4D0F20: with the knob
                // grabbed (`[this+0x10C] == 3`) and the pointer still on the bar, it sets the
                // model's position to
                // `grabPosition + (y - grabY) · (positions - 1) / (trackHeight - knobHeight)`
                // — the exact inverse of the placement at 0x4D0BBE — and re-places the knob.
                if let (Some((index, gy, at)), Some((_, cy))) = (self.fe.list_drag, self.fe.cursor) {
                    let span = self.fe.layout_preview.as_ref().and_then(|l| l.list_scrollbar(index)).map(|b| (b.span, b.positions));
                    if let Some((span, positions)) = span
                        && span > 0.0
                        && positions > 1
                        && let Some(l) = self.fe.layout_preview.as_mut()
                    {
                        let moved = ((cy - gy) * (positions - 1) as f32 / span).round() as i32;
                        l.set_list_scroll(index, at as i32 + moved);
                    }
                }
                // The open combo's **own** scrollbar knob drag — same arithmetic, `combo_scroll_track`'s
                // numbers instead of `list_scrollbar`'s (§4.90's bar, not `NFS5.VerticalSlider`'s).
                // User report 2026-09-30 (b). Shared with the `NFS_FE_CLICKS=drag:…` diagnostic.
                self.fe_apply_combo_drag();
                // `NFS5.ColorBox`'s slot 8, 0x4C4830: while the editor is up the widget holds the
                // capture, so every move reaches it, and it drags the hue, the saturation or the
                // luminance whenever a button is held (§4.66)
                if let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_mut(), self.fe.cursor) {
                    l.picker_move(cx, cy);
                    l.deadzone_drag(cx, cy);
                }
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.open_picker().is_some()) {
                    self.store_picker_wells();
                    // slot 8 (0x4C4830) reaches the same tail as the press, only while a button
                    // is held (0x5299C0), and tells the accessor again (0x4C493B)
                    if self.fe.mouse_down {
                        self.picker_tell_accessor();
                    }
                }
                // the open drop-down's scrollbar tracks the pointer so its arrows can show their
                // hovered frame — `[this+0xEC]`, which 0x4DB2F0 writes on every move (§4.101)
                if let Some((cx, cy)) = self.fe.cursor {
                    let at = self.fe.layout_preview.as_ref().and_then(|l| l.combo_scroll_at(cx, cy));
                    let held = self.fe.mouse_down;
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.set_combo_scroll(at.map(|(i, r)| (i, r, held)));
                    }
                }
                if let Some((cx, cy)) = self.fe.cursor
                    && let Some(l) = self.fe.layout_preview.as_mut()
                {
                    l.picture_drag_to(cx, cy);
                }
                if self.fe.bar_grab.is_some()
                    && let Some((cx, cy)) = self.fe.cursor
                    && let Some((_, name, f)) = self.fe.layout_preview.as_ref().and_then(|l| l.bar_at(cx, cy))
                {
                    self.set_accessor_fraction(&name, f);
                    // the bar's draw reads its accessor every frame (0x4CA724); the port's fill is set
                    // at bind time, so set it again
                    self.bind_indicator_bars();
                }
            }
            WindowEvent::CursorLeft { .. } => self.fe.cursor = None,
            // `RE/notes/controls.md` §4.40: the pump stamps a `MouseButtonEvent` type **4** on the
            // press and **8** on the release (0x529806). The base's slot 5, 0x5258B0, fires the
            // widget's `"CLICKED"` message — the one reference to that string in the whole image —
            // and `NFS5.GOTOBUTTON` overrides only slot 9 (0x4C9470), where it plays sound 5 and
            // runs the screen transition: **a button navigates on the release, not the press**.
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.fe_left_button(event_loop, state == ElementState::Pressed);
            }
            WindowEvent::RedrawRequested if self.fe.menu.is_some() => {
                // gamepad: d-pad / left stick moves, South (A / cross) selects — PORT-DEVIATION D10
                // (PORT-DEVIATIONS.md) [the FE reads no pad, event pump 0x529700]
                self.gamepads.poll(&mut self.input);
                if nfs_game::config::cfg().nfs_ff_test.is_set() {
                    self.gamepads.rumble_test_tick();
                }
                let stick = self.input.pad.map(|p| p.steer).unwrap_or(0.0);
                let mut keys = Vec::new();
                if self.input.take_pressed(Action::SteerLeft) {
                    keys.push(MenuKey::Left);
                }
                if self.input.take_pressed(Action::SteerRight) {
                    keys.push(MenuKey::Right);
                }
                if self.input.take_pressed(Action::Throttle) {
                    keys.push(MenuKey::Up);
                }
                if self.input.take_pressed(Action::Brake) {
                    keys.push(MenuKey::Down);
                }
                if self.input.take_pressed(Action::ShiftUp) {
                    keys.push(MenuKey::Enter);
                }
                // the pad's back is the text menu's alone. **The original's front end reads no
                // pad** (`controls.md` §4.169.5): its event pump 0x529700 builds key events
                // (type 2) only from the window's keyboard ring — the two message handlers
                // 0x53B3D0 / 0x53B450 → 0x53A9E0 → 0x53AB00 → 0x55FF60 — plus the mouse and the
                // timers, so
                // the `b` of 0x4D2970 comes from the keyboard only and no pad button goes back on
                // a layout screen. (The pad's other keys above are the port's own on both.)
                if self.input.take_pressed(Action::Handbrake) && self.fe.layout_preview.is_none() {
                    keys.push(MenuKey::Back);
                }
                if stick.abs() > 0.6 && !self.pad_stick_latched {
                    keys.push(if stick < 0.0 {
                        MenuKey::Left
                    } else {
                        MenuKey::Right
                    });
                }
                self.pad_stick_latched = stick.abs() > 0.6;
                self.input.end_tick();
                // the pad's keys are the port's own; they go the way 0x4D2850 sends keys
                if self.fe_input_swallowed() {
                    keys.clear();
                }
                for k in keys {
                    self.menu_key(k, event_loop);
                    if self.fe.menu.is_none() {
                        return;
                    }
                }
                // the dialog's tick (0x512980 and the kind's own), its answer, and the covering
                // the manager keeps while it is up (0x4D2000 on the raise, 0x4D1E70 on the dismiss)
                if self.fe_dialog_answer(event_loop) {
                    return;
                }
                // the end-of-era show's tick (0x4FF180): back one screen when it is over
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.end_of_era_leave()) {
                    self.fe_back(0, event_loop);
                    return;
                }
                // the front end draws from `rebuild_scene` like everything else, so it keeps
                // animating between key presses
                self.pace_frame();
                let now = Instant::now();
                self.frame_dt = (now - self.last_frame).as_secs_f32();
                self.last_frame = now;
                self.draw_frame();
                if std::mem::take(&mut self.net.start_due) && self.net_playgame() {
                    self.net_launch_sync();
                    self.start_from_layout(event_loop);
                }
            }
            // the program is ending (a playback's end with no front end, `replay_exit_check`):
            // no more race frames
            WindowEvent::RedrawRequested if event_loop.exiting() => {}
            WindowEvent::RedrawRequested => {
                self.gamepads.poll(&mut self.input);
                if nfs_game::config::cfg().nfs_ff_test.is_set() {
                    self.gamepads.rumble_test_tick();
                }
                self.pace_frame();
                let now = Instant::now();
                self.frame_dt = (now - self.last_frame).as_secs_f32();
                // the replay pause freezes the frame as the pause menu does: `[0x606A98]` makes
                // `RaceFrame` render only (0x415C69) and pauses the audio (0x4AB150 / 0x4AB200,
                // 0x41581A…0x415849)
                // diagnostic `NFS_HUD_KEYS=tick:pause`: the pause menu's continue after 64 pause ticks
                // (or, `tick:pauserestart`, its restart instead — `replay.md` §18.12)
                if self.scripted_pause && self.pause_ticks >= 64.0 {
                    self.scripted_pause = false;
                    self.paused = false;
                    if std::mem::take(&mut self.scripted_restart) {
                        self.restart_race(event_loop);
                    }
                }
                let halted = self.paused || self.replay.frozen;
                if !self.paused {
                    self.pause_offset += self.pause_ticks;
                }
                self.pause_ticks = if self.paused { self.pause_ticks + self.frame_dt as f64 * TICKS_PER_SEC_F64 } else { 0.0 };
                // **The race pause 0x4AB150** (0x4151C8, 0x41554B, 0x415A2C: the pause menu; 0x415830:
                // the replay's freeze), skipped with `[0x657C94]`: the speech phrase's voices and the
                // voice table to volume 0 (0x4AF900(0, 0), 0x4AB16A…0x4AB18A) — they run on silent —
                // (0x4AF900's hard stop of `0x656AE8` / `0x656AEC` finds nothing: they are the radio
                // static under a phrase, `kStatic` 161…164 by 0x4AF7C0, started only for a speech
                // entry whose `+0x1F` is above 0 (0x4B0BEA) — and no request ever sets it, so the
                // port has nothing to stop there)
                // `MusicStop(0)` 0x4AE3C0, the tunnel reverb's buses to 0 (0x4AB193…0x4AB1B3), the
                // player's engine layers and start / stop samples hard-stopped (0x4AD510), and the live
                // levels from the registry (0x4AB1BF…0x4AB1F1). Nothing runs the audio update while
                // paused (`RaceFrame` renders only), but the library's service does: a released voice
                // ramps out. **The resume 0x4AB200** (0x415701, 0x41583E): the music at its level and
                // `MusicPlay(NULL, 0)` — a fresh pick — and the registry from the live levels; the
                // loops, the AI engines, the ambience and the player's engine come back with the next
                // audio update's `setVolume`s and starts; a one-shot, the phrase and a held horn stay
                // silent to their end. Before the pause menu's own sound 4 (0x41ADD0), as 0x4AB150
                // runs before the menu opens. *(Until 2026-09-27 the port held the whole mix at 0 —
                // the one-shots and the phrase then came back — and the pause key hard-stopped the
                // SFX loops.)*
                self.sync_audio_pause();
                self.billboard_key_frame();
                self.accumulator += now - self.last_frame;
                self.last_frame = now;
                if self.accumulator > Duration::from_millis(250) {
                    self.accumulator = Duration::from_millis(250);
                }
                if halted {
                    self.accumulator = Duration::ZERO;
                    self.ff_stop_all();
                }
                // **An input-exchange race's frame driver** (0x415455…0x415495, 0x415BD4): the ticks that
                // run are the ones the frames received and the host-synchronised clock allow, not the
                // local time's
                // the HUD tick's `HInfo` (0x425E0B): the network notices, once a frame
                self.net_race_notice();
                let exchange = self.net.session.as_ref().is_some_and(|n| n.exchanging());
                if exchange && halted {
                    let _ = self.net_exchange_gate();
                }
                let mut ran = 0;
                while if exchange { !halted && ran < 16 && self.net_exchange_gate() } else { self.accumulator >= GAME_TICK } {
                    ran += 1;
                    // a scripted pause (`NFS_HUD_KEYS`) taken inside a tick ends the frame's ticks,
                    // as the pause key between two frames does
                    if self.scripted_pause {
                        self.accumulator = Duration::ZERO;
                        break;
                    }
                    if exchange {
                        self.exchange_tick_at = Instant::now();
                    } else {
                        self.accumulator -= GAME_TICK;
                    }
                    // replay.md §5.1: playback speed is tick *decimation* inside the catch-up
                    // loop — never a `dt` scale. Every iteration still renders.
                    let n = match &self.replay.stream {
                        Some(r) if r.mode == nfs_game::replay::Mode::Playing => {
                            self.replay.speed.ticks_at(self.replay.loop_count)
                        }
                        _ => 1,
                    };
                    self.replay.loop_count += 1;
                    for _ in 0..n {
                        self.simulate();
                        // race over ends `RaceFrame` (0x415D17) and the session loop restarts
                        // the race as its replay
                        if self.auto_replay_due {
                            self.start_auto_replay(event_loop);
                            break;
                        }
                    }
                }
                if exchange {
                    // the render's interpolation runs on the time since the last tick
                    self.accumulator = now.saturating_duration_since(self.exchange_tick_at).min(GAME_TICK);
                }
                // 0x415CB3: the played-back stream's end, after the catch-up loop
                if self.replay_exit_check(event_loop) {
                    return;
                }
                let bar_save_was_up = self.replay_save.up;
                self.run_bar_script();
                self.replay_bar_frame(event_loop);
                // diagnostic: `NFS_REPLAY_TYPE`, as the scripted-run loop does
                if !bar_save_was_up
                    && self.replay_save.up
                    && let Some(name) = nfs_game::config::cfg().nfs_replay_type.as_str()
                {
                    for c in name.chars() {
                        self.replay_save.type_char(c);
                    }
                }
                self.draw_frame();
            }
            _ => {}
        }
    }
}
