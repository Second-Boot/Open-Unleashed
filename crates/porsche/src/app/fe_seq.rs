//! **TEST GLUE** — scripted front-end frame sequences (`NFS_FE_SEQ`), performance work A8 proof.
//!
//! The one-still screenshot path proves a single frame; a cache that is wrong shows up between
//! frames. `NFS_FE_SEQ=<dir>` renders `NFS_FE_SEQ_FRAMES` (default 60) **consecutive real frames**
//! through [`App::draw_frame`] at a fixed 1/60 s step and saves each as `<dir>/fNNN.png`, applying
//! the events of `NFS_FE_SEQ_SCRIPT` (`<frame>@<token>|…`) before the frame they name. A token is
//! `move:x,y` (the scripted pointer), `down` / `up` (the left button) or any `NFS_FE_KEYS` token
//! (`goto:`, `type:`, `keys:`, `click:`, `pick:`, `b`, `esc`, …).

use crate::*;

impl App {
    /// One `NFS_FE_KEYS` token — what the start-up key script does for each of its comma-separated
    /// entries (moved out of `resume_inner` unchanged so `NFS_FE_SEQ_SCRIPT` can run the same).
    pub(crate) fn fe_scripted_key(&mut self, k: &str, event_loop: &ActiveEventLoop) {
        match k {
            "b" => self.fe_key_code(0x62, event_loop),
            "esc" => self.fe_key_code(0x1B, event_loop),
            // `playgame` — the bottom bar's `ID_PLAYGAMEBUTTON` released (`fe_bottom_click`'s
            // `PLAYGAME` arm): the only `PLAYGAME` button the shipped layouts carry
            // (`bottombar.lay`), shown by the top screen's `canplaygame`
            "playgame" => {
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.can_play()) {
                    self.fe_start_game(event_loop);
                } else {
                    tracing::info!(screen = ?self.fe.layout_name, "front end: NFS_FE_KEYS playgame — the screen cannot start a game");
                }
            }
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

    /// `NFS_FE_SEQ`: run the scripted sequence and write its frames. `true` when it ran.
    pub(crate) fn fe_sequence(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Ok(dir) = std::env::var("NFS_FE_SEQ") else { return false };
        let frames: usize = std::env::var("NFS_FE_SEQ_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        let script = std::env::var("NFS_FE_SEQ_SCRIPT").unwrap_or_default();
        let events: Vec<(usize, String)> = script
            .split('|')
            .filter_map(|e| e.split_once('@'))
            .filter_map(|(f, t)| Some((f.trim().parse().ok()?, t.trim().to_string())))
            .collect();
        let _ = std::fs::create_dir_all(&dir);
        let (w, h) = (shot_geometry().0, shot_geometry().1);
        for f in 0..frames {
            for (_, tok) in events.iter().filter(|(ef, _)| *ef == f) {
                if let Some((x, y)) = tok.strip_prefix("move:").and_then(|r| r.split_once(',')).and_then(|(a, b)| Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))) {
                    self.fe.cursor = Some((x, y));
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.script_cursor(Some((x, y)));
                    }
                } else if tok == "down" || tok == "up" {
                    self.fe_left_button(event_loop, tok == "down");
                } else {
                    self.fe_scripted_key(tok, event_loop);
                }
            }
            self.frame_dt = 1.0 / 60.0;
            self.draw_frame();
            let scene = self.scene.clone();
            let Some(r) = self.renderer.as_mut() else { break };
            let rgba = r.render_to_image(&scene, &self.camera, w, h).expect("render");
            image::save_buffer(format!("{dir}/f{f:03}.png"), &rgba, w, h, image::ColorType::Rgba8).expect("png");
        }
        event_loop.exit();
        true
    }
}
