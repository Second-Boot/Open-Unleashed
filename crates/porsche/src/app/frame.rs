//! One frame's drawing.

use crate::*;

impl App {
    /// Advance the screens, rebuild the scene and present it. **Both** redraw arms end here: the
    /// menu arm used to handle its keys and then ask for another redraw without ever drawing, which
    /// left the window blank for the whole front end.
    // PORT-DEVIATION D5 (RE/notes/PORT-DEVIATIONS.md): NFS_RENDER_THREAD=1 moves encode/submit/present to a thread, off by default; the original renders through dx7z.dll on its main thread.
    pub(crate) fn draw_frame(&mut self) {
        // the encode / submit / present of each frame on a render thread of its own (`nfs-renderer`'s
        // `front.rs`), from the first live frame on: `NFS_RENDER_THREAD=1` on (default off: on a
        // 120 Hz vsync-bound machine it adds frame-time jitter and gains nothing, `RE/notes/PERF.md` PH)
        if let Some(r) = self.renderer.as_mut()
            && !r.render_thread()
            && nfs_game::config::cfg().nfs_render_thread.var().is_ok_and(|v| v.trim() != "0")
        {
            r.enable_render_thread();
        }
        self.sync_save_side_state();
        // the front end's screens slide and fade in over half a second (`RE/notes/controls.md` §6)
        // whole milliseconds, the fraction carried to the next frame so the clock's rate does not
        // depend on the display's frame rate (the original reads a free-running timer, 0x53C2E0)
        let (dt_ms, carry) = fe_step_ms(self.frame_dt, self.fe_ms_carry);
        self.fe_ms_carry = carry;
        self.fe.ms += dt_ms as u64;
        // the garage screen's tick re-decides its buttons every frame (0x50F970), and the player
        // screens' tick theirs (0x510210)
        self.bind_garage();
        // carsetup's own tick, 0x4F1AC0, re-decides the gear-ratio panel every frame too
        self.bind_carsetup_gearratio();
        // the same 0x50CC30 helper decides `ID_LAPSSELECTOR`'s disabled state every frame too
        self.bind_laps_selector();
        self.bind_player_buttons();
        // and the bottom bar's tick (0x4D8250) with its refresh (0x4D81C0) — the button states
        self.bind_bottom_bar();
        self.fe_career_first_tick();
        // the dialog's tick (0x512980 and the kind's own), and the covering the manager keeps
        // while it is up (0x4D2000 on the raise, 0x4D1E70 on the dismiss)
        self.fe_dialog_step();
        self.fe_main_screen_music_tick();
        // the front end's music, when the CD player started some: the end-of-stream advance
        // (0x4ADD6D) the race's audio update runs
        if self.vehicle.is_none()
            && let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut())
        {
            m.update(a);
        }
        // the end-of-era show's sting, once, 128 ticks in (0x4FFA7C): `0x4A6270(k)` queues speech
        // id 4 + k — `Era-ClassictoGolden`, `Era-GoldentoModern`, `Era-CEvoModecomplet`
        // (`RE/notes/sfx.md` §7.1)
        if self.vehicle.is_none()
            && let Some(k) = self.fe.layout_preview.as_ref().and_then(|l| l.take_end_of_era_sting())
        {
            if self.snd.speech.is_none() {
                self.snd.speech = nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech"))
                    .map_err(|e| tracing::warn!(error = %e, "speech unavailable"))
                    .ok();
            }
            const ERA_STING: [&str; 3] = ["Era-ClassictoGolden.bnk", "Era-GoldentoModern.bnk", "Era-CEvoModecomplet.bnk"];
            if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                sp.say(a, ERA_STING[k.clamp(0, 2) as usize]);
            }
        }
        if self.vehicle.is_none()
            && let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech)
        {
            sp.set_level(self.snd.audio_settings.speech_level);
            sp.update(a, false);
        }
        self.fe_career_dialog();
        self.fe_offer_check();
        // the steering band is the front end's own `XAxis.Min` / `XAxis.Max`, not a constant of
        // this port's (`nfs-input/src/gamepad.rs`)
        if let Some(l) = self.fe.layout_preview.as_ref() {
            let bands = l.deadzone_values();
            self.gamepads.set_axis_band(bands[0][0], bands[0][1]);
            self.input.axis_bands = bands;
        }
        // **A binding row waiting for a control polls the pads** — slot 1, 0x51C4E0, every frame
        // while it has focus: 0x51F3B0 against the baseline taken when it got it, and on a find
        // the same commit as a key's (`Bindings::assign`) and focus goes (0x51C5DE).
        match self.fe.layout_preview.as_ref().and_then(|l| l.focused_binding_row()) {
            Some((i, acc)) => {
                if let Some(base) = self.fe.binding_base.as_ref()
                    && let Some(b) = nfs_input::bindings::capture(base, &self.input.pads)
                {
                    if let Some(id) = accessors::controller_action(&acc).and_then(nfs_input::binding_id) {
                        self.input.bindings.assign(id, b);
                        tracing::info!(row = %acc, kind = b.kind, index = b.index, "control rebound");
                    }
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.release_focus(i);
                    }
                    self.fe.binding_base = None;
                }
            }
            None => self.fe.binding_base = None,
        }
        let pads = self.fe_pads();
        // `Camera.Current` — the slot `NFS5.CAMERASELECT` highlights (§4.89)
        let cam_cur = self.fe.camera_current;
        let cam_list = self.fe.camera_list;
        self.fe_room_tick(dt_ms);
        // 0x4D2570 advances **every** layer's screen (+0x80, 0x4D25EB, bottom first), not only
        // the top one: a covered screen still finishes its fade — and the bar and the layers
        // above take the first layer's (0x4D63C0 / 0x4D6300), so a frozen one would hold them
        for (_, u) in self.fe.layout_under.iter_mut() {
            u.advance_transition(dt_ms);
        }
        // while 0x4D2850 drops the moves the widgets keep their pointer (`fe_input_swallowed`)
        let swallowed = self.fe_input_swallowed();
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.advance_transition(dt_ms);
            // hand the screen the pointer, so its widgets can pick their `hover` state (§4.39)
            if swallowed {
                l.set_pointer(self.fe.cursor);
            } else {
                l.set_cursor(self.fe.cursor);
            }
            l.set_pads(pads, self.fe.pads_sel);
            l.set_camera_current(cam_cur);
            l.set_camera_list(cam_list);
        }
        // the bottom bar is a screen of its own (0x4D39D3) and its four buttons hover and animate
        // like any other (§4.53), so it needs the clock and the pointer too. Its transition moves
        // **once** a frame: the front end's tick 0x4D2B80 runs 0x4D2570, which advances the
        // layers' screens (+0x80, 0x4D25EB) and then the bar's (0x4D260E) — the only call of the
        // bar's +0x80. (Until 2026-09-27 the port advanced it here and again before the scene
        // was rebuilt, so its clock ran double.)
        if let Some(b) = self.fe.layout_bottom.as_mut() {
            b.advance_transition(dt_ms);
            if swallowed {
                b.set_pointer(self.fe.cursor);
            } else {
                b.set_cursor(self.fe.cursor);
            }
        }
        // A held scrollbar arrow repeats. 0x4D0A97: the press starts `0x529220(this, 0x96, 1)` —
        // a **150 ms** repeating timer, the same one `NFS5.CARSETTINGBUTTON` uses (§4.40) — and
        // slot 10 (0x4D0B40) steps the position each time it fires.
        if let Some((index, delta, ref mut acc)) = self.fe.list_repeat {
            *acc += dt_ms;
            let steps = *acc / SCROLL_REPEAT_MS;
            *acc %= SCROLL_REPEAT_MS;
            if steps > 0 && let Some(l) = self.fe.layout_preview.as_mut() {
                l.scroll_list(index, delta * steps as i32);
            }
        }
        // and so does a held open combo's **own** scrollbar arrow — `FeListScrollBar` slot 19
        // fires again each time `0x4DB1D0`'s `[+0xEC] == [+0xF0]` (the same 150 ms timer), off
        // the direction the press already latched (`[+0xDC]`); the release (0x4DB180) is
        // `combo_arrow_repeat = None` below, at the button-up handler that already clears
        // `list_repeat`/`combo_drag`. `combo_scroll_press` itself is the one-step body (§4.101),
        // reused here exactly as the press already calls it once immediately.
        if let Some((index, region, ref mut acc)) = self.fe.combo_arrow_repeat {
            let (steps, new_acc) = nfs_game::layout::repeat_steps(*acc, dt_ms, SCROLL_REPEAT_MS);
            *acc = new_acc;
            if let Some(l) = self.fe.layout_preview.as_mut() {
                for _ in 0..steps {
                    l.combo_scroll_press(index, region);
                }
            }
        }
        // and so does a held car-setting arrow, off the very same timer (0x4C1F55, 0x4C1F80).
        // Same fix as the press (above): re-bind the full
        // `bind_after_accessor()` set, not a hand-picked subset of it — this used to be an even
        // smaller four than the press's six, so a held arrow refreshed even less than a tap did.
        if let Some((name, step, acc)) = self.fe.setting_repeat.take() {
            let acc = acc + dt_ms;
            let steps = acc / SCROLL_REPEAT_MS;
            self.fe.setting_repeat = Some((name.clone(), step, acc % SCROLL_REPEAT_MS));
            for _ in 0..steps {
                self.set_accessor(&name, step);
            }
            if steps > 0 {
                self.bind_after_accessor();
            }
        }
        // and the movies on it, which need the renderer to put each frame in its texture
        if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
            l.advance_movies(r, dt_ms, self.snd.audio.as_ref(), self.snd.audio_settings.music_level);
        }
        // the front end's own pointer (`controls.md` §4.152), and where the turntable stands,
        // which is what a hovered `NFS5.3DCAR` picks its cursor by (§4.153)
        let spin = self.fe_room_of().map(|r| r.spin_turns(self.fe.ms)).unwrap_or(0.0);
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.advance_cursor(dt_ms, self.fe.mouse_down);
            l.set_car_spin(spin);
        }
        // a session open on the front end's lobby screens is drained here, the race's
        // `net_step` being idle — the room's players and chat arrive this way
        if self.vehicle.is_none() && self.net.session.as_mut().is_some_and(|n| n.poll_lobby()) {
            self.bind_lists();
        }
        self.net_lobby_step();
        if self.vehicle.is_none() && self.race.is_none() {
            self.net_sync_options();
        }
        // the comm state machine's notices (0x509550), from the screens' ticks
        self.comm_tick();
        // `FEMainScreen`'s tick 0x5103F0: the update thread's state and dialog 78
        self.update_check_tick();
        // and each comms screen's own tick rules (0x50AB40, 0x50B760, 0x50B4E0, 0x50B270, 0x50AEF0)
        self.net_browse_step();
        // the host's `0xB3` (0x48F82F) came in on a lobby screen: 0x48F853 clears `[net+0xBC]`, and
        // the screen's tick then calls its slot 53 — PLAYGAME's own handler 0x50CE90 — when that
        // byte is 0 (0x50A8C3…0x50A8E1 on the join screen, the same tail on the others). The launch
        // itself needs the event loop, so it waits for the caller below.
        if self.vehicle.is_none() && self.race.is_none() && self.net.session.as_mut().is_some_and(|n| n.take_start()) {
            self.net.start_due = true;
        }
        // a network screen probes once a second and refills its lists from the answers
        if self.net.lobby.is_some() {
            let before = self.net.lobby.as_ref().map(|l| l.found.len());
            if let Some(l) = self.net.lobby.as_mut() {
                l.poll();
            }
            if before != self.net.lobby.as_ref().map(|l| l.found.len()) {
                self.bind_lists();
            self.bind_checks();
            }
        }
        self.rebuild_scene();
        self.update_damage_decals();
        // the mip filter's promotion `[0x69DD1D]`: the race's 0x44ED10 sets it at `SPECIALFX` 3
        // only (0x44EDD1 clears, 0x44EF04 sets), the front end's 0x44F020 clears it (0x44F080)
        let fx_level = self.race.is_some().then(|| self.special_fx_level());
        let trilinear = fx_level.is_some_and(|l| l >= 3);
        let fakesort_mode = self.fakesort_mode(fx_level);
        if let Some(r) = self.renderer.as_mut() {
            r.set_trilinear(trilinear);
            r.set_fakesort_mode(fakesort_mode);
        }
        self.render_mirror();
        // the front end's idle redraw (`idle.rs`): the frame the window already shows is not
        // rendered or presented again
        let idle = self.fe_frame_unchanged();
        if nfs_game::config::cfg().nfs_fe_idle_verify.var_os().is_some() {
            self.fe_idle_verify(idle);
        }
        if !idle
            && let Some(r) = self.renderer.as_mut()
            && let Err(e) = r.render(&self.scene, &self.camera)
        {
            tracing::error!(error = %e, "render failed");
        }
        self.mirror_sky_update();
        self.mirror_weather_step();
        self.frames += 1;
        if self.fps_timer.elapsed() >= Duration::from_secs(1) {
            tracing::info!(fps = self.frames, tick = self.tick, "frame stats");
            self.frames = 0;
            self.fps_timer = Instant::now();
        }
        if idle {
            self.idle_wait();
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// One frame's front-end clock step in whole milliseconds, with the fraction carried over:
/// `(step, new carry)`. The step is capped at 250 ms as before.
pub(crate) fn fe_step_ms(frame_dt: f32, carry: f32) -> (u32, f32) {
    let want = frame_dt * 1000.0 + carry;
    let ms = want.floor().clamp(0.0, 250.0) as u32;
    (ms, (want - ms as f32).clamp(0.0, 1.0))
}

#[cfg(test)]
mod fe_step_tests {
    use super::fe_step_ms;

    /// One second of wall time advances the front end's clock by 1000 ms at any frame rate
    /// (the original's clock is the free-running 128 Hz timer, 0x53C2E0).
    #[test]
    fn clock_rate_is_frame_rate_independent() {
        for fps in [30.0f32, 60.0, 75.0, 120.0, 144.0, 240.0] {
            let (mut total, mut carry) = (0u32, 0.0f32);
            for _ in 0..(fps as u32 * 10) {
                let (ms, c) = fe_step_ms(1.0 / fps, carry);
                total += ms;
                carry = c;
            }
            assert!((9_995..=10_005).contains(&total), "{fps} fps: {total} ms in 10 s");
        }
    }
}
