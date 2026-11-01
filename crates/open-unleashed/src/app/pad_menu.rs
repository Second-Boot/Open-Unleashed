//! The front end with a pad (or the arrow keys) alone — PORT-DEVIATION D54, extending D10.
//!
//! The original's front end is pointer-only (`RE/notes/controls.md` §4.38, §4.169.5: its event pump
//! builds key events from the keyboard ring alone, and its key slot 0x4D2970 knows ESC, `b` and `=`).
//! The port's keys therefore drive a **virtual pointer**: a direction key moves it to the next place
//! a click would land ([`nfs_game::layout::pad::pad_pick`]), and A presses and releases the left button
//! there through the very code the real mouse runs ([`App::fe_left_button`]) — so a combo opens, a
//! drop-down row commits, a tab switches, a list row selects, an accessor updates, exactly as for a
//! click. B is the keyboard's `b`, Start the bottom bar's "race", Y its "options", View the Escape
//! path, and the shoulders turn the tabs. Nothing of the original's front end is changed; no key
//! pressed means no change.

use crate::*;
use nfs_game::layout::pad::{DEADZONE_PAD_STEP, PICKER_LUM_STEP, PICKER_PAD_STEP, PadDir, PadKind, PadTarget, bar_fraction, bar_step_index, pad_pick};

impl App {
    /// A front-end key the pad scheme takes. `true` when it did; `false` leaves the key to the
    /// layout screen's older handling (a keyboard Enter with no pointer placed, `layout_sel`).
    pub(crate) fn pad_menu_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) -> bool {
        if self.fe.layout_preview.is_none() {
            return false;
        }
        // a screen that slides, a busy room: the front end drops the keys (0x4D2850)
        if self.fe_input_swallowed() {
            return true;
        }
        // the colour editor is up (PAD-SLIDERS): the d-pad moves the picked point over the wheel — Left / Right the hue,
        // Up / Down the saturation, a thirty-second of the wheel a press —, the shoulders the brightness (a twentieth of
        // the ramp), as the pointer's drag does it (`picker_press`'s tail: the well is written, the accessor told);
        // A takes the colour as it stands ("ok") and B leaves ("cancel"), through the editor's own two buttons
        if let Some((ok, cancel)) = self.fe.layout_preview.as_ref().and_then(|l| l.picker_button_centres()) {
            let step = match key {
                MenuKey::Left => Some((-PICKER_PAD_STEP, 0.0, 0.0)),
                MenuKey::Right => Some((PICKER_PAD_STEP, 0.0, 0.0)),
                MenuKey::Up => Some((0.0, PICKER_PAD_STEP, 0.0)),
                MenuKey::Down => Some((0.0, -PICKER_PAD_STEP, 0.0)),
                MenuKey::TabPrev => Some((0.0, 0.0, -PICKER_LUM_STEP)),
                MenuKey::TabNext => Some((0.0, 0.0, PICKER_LUM_STEP)),
                _ => None,
            };
            if let Some((dh, ds, dl)) = step {
                if self.fe.layout_preview.as_mut().is_some_and(|l| l.picker_step(dh, ds, dl)) {
                    self.store_picker_wells();
                    self.picker_tell_accessor();
                }
                return true;
            }
            let at = match key {
                MenuKey::Enter => Some(ok),
                MenuKey::Back => Some(cancel),
                _ => None,
            };
            if let Some(p) = at {
                self.fe.cursor = Some(p);
                self.fe_pointer_moved();
                self.fe_left_button(event_loop, true);
                self.fe_left_button(event_loop, false);
            }
            return true;
        }
        // the older Enter (the mouse's release on a button, `goto:`) is not the pad's
        if key == MenuKey::Enter && self.fe.pad_target.is_none() && !self.fe.dialog.as_ref().is_some_and(|(d, _)| !d.gone()) {
            return false;
        }
        // a dialog is up: its buttons are all there is
        if self.fe.dialog.as_ref().is_some_and(|(d, _)| !d.gone()) {
            self.pad_dialog_key(key);
            return true;
        }
        // PAD-SLIDERS: on a widget a drag sets, Left / Right adjust its value
        if matches!(key, MenuKey::Left | MenuKey::Right)
            && let Some(t) = self.pad_current().filter(|t| t.kind.adjustable())
        {
            self.pad_adjust(&t, if key == MenuKey::Right { 1 } else { -1 });
            return true;
        }
        match key {
            MenuKey::Up => self.pad_move(PadDir::Up),
            MenuKey::Down => self.pad_move(PadDir::Down),
            MenuKey::Left => self.pad_move(PadDir::Left),
            MenuKey::Right => self.pad_move(PadDir::Right),
            MenuKey::Enter => {
                let Some((index, row)) = self.fe.pad_target else { return false };
                match self.pad_find(index, row) {
                    // a dead-zone bar has no click: A takes the other handle pair
                    Some(t) if t.kind == PadKind::DeadZone => {
                        if let Some(l) = self.fe.layout_preview.as_mut() {
                            l.deadzone_pad_toggle();
                        }
                        self.pad_reaim(&t);
                    }
                    // the other sliders and bars have nothing to press
                    Some(t) if t.kind.adjustable() => {}
                    Some(t) => self.pad_click(&t, event_loop),
                    None => self.fe.pad_target = None,
                }
            }
            MenuKey::Back => self.pad_back(event_loop),
            MenuKey::Play => self.pad_bottom("ID_PLAYGAMEBUTTON", event_loop),
            MenuKey::Options => self.pad_bottom("ID_OPTIONSBUTTON", event_loop),
            // View: the Escape path, the original's exit question (`fe_manager_key` 0x1B)
            MenuKey::Quit => self.fe_key_code(0x1B, event_loop),
            MenuKey::TabPrev => self.pad_tab(-1, event_loop),
            MenuKey::TabNext => self.pad_tab(1, event_loop),
        }
        true
    }

    /// Whether a held key's repeat (`MenuHold`) means something now: the colour editor takes all six keys, a widget a
    /// drag sets takes Left / Right. Anything else is moved one press at a time.
    pub(crate) fn pad_repeat_ok(&self, key: MenuKey) -> bool {
        if self.fe.layout_preview.is_none() || self.fe.dialog.as_ref().is_some_and(|(d, _)| !d.gone()) {
            return false;
        }
        if self.fe.layout_preview.as_ref().is_some_and(|l| l.picker_button_centres().is_some()) {
            return matches!(key, MenuKey::Up | MenuKey::Down | MenuKey::Left | MenuKey::Right | MenuKey::TabPrev | MenuKey::TabNext);
        }
        matches!(key, MenuKey::Left | MenuKey::Right) && self.pad_current().is_some_and(|t| t.kind.adjustable())
    }

    /// The target the pad stands on (or, for a mouse user, the one under the pointer).
    fn pad_current(&self) -> Option<PadTarget> {
        let l = self.fe.layout_preview.as_ref()?;
        let targets = l.pad_targets();
        let i = self.pad_resolve(&targets).or_else(|| l.hovered().and_then(|h| targets.iter().position(|t| t.index == h)))?;
        targets.get(i).cloned()
    }

    /// Left (-1) / Right (+1) on a widget a drag sets (PORT-DEVIATION D54, PAD-SLIDERS): the value moves through the
    /// code the mouse takes to the same place — the arrow buttons' `set_accessor` for a car set-up slider (one index a
    /// press, as the `CARSETTINGBUTTON` beside it), `set_accessor_fraction` for an indicator bar (a twentieth of its
    /// values, [`bar_step_index`]), the dead-zone setter's clamp for a handle, the chronicle slider's year index.
    fn pad_adjust(&mut self, t: &PadTarget, dir: i32) {
        match t.kind {
            PadKind::CarSlider => {
                if let Some(name) = self.fe.layout_preview.as_ref().and_then(|l| l.widget_accessor(t.index)) {
                    self.set_accessor(&name, dir);
                    self.bind_after_accessor();
                }
            }
            PadKind::Bar => {
                if let Some(name) = self.fe.layout_preview.as_ref().and_then(|l| l.widget_accessor(t.index)) {
                    let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(&name).to_string();
                    if let Some((v, lo, hi)) = self.range_value(&base) {
                        let n = (hi - lo + 1).max(1);
                        let to = bar_step_index(n, v - lo, dir);
                        self.set_accessor_fraction(&name, bar_fraction(n, to));
                        self.bind_indicator_bars();
                    }
                }
            }
            PadKind::DeadZone => {
                if self.fe.layout_preview.as_mut().is_some_and(|l| l.deadzone_pad_step(t.index, dir * DEADZONE_PAD_STEP)) {
                    self.pad_reaim(t);
                }
            }
            PadKind::ChronicleSlider => {
                let (cur, count) = self.chronicle_year_index();
                let want = (cur + dir).clamp(0, (count - 1).max(0));
                if want != cur {
                    self.fe.chronicle_year_idx = Some(want);
                    self.bind_chronicle_group();
                }
            }
            _ => {}
        }
    }

    /// The target again, from the current screen (its click point moved with a handle), and the pointer on it.
    fn pad_reaim(&mut self, t: &PadTarget) {
        let now = self.fe.layout_preview.as_ref().map(|l| l.pad_targets()).unwrap_or_default();
        if let Some(n) = now.iter().find(|x| x.index == t.index && x.kind == t.kind && (x.centre().0 - t.centre().0).abs() < 1.5 && (x.centre().1 - t.centre().1).abs() < 1.5).cloned() {
            self.pad_stand(&n);
            self.pad_aim(&n);
        }
    }

    /// The pad stands on `t`.
    fn pad_stand(&mut self, t: &PadTarget) {
        self.fe.pad_target = Some((t.index, t.row));
        self.fe.pad_centre = t.centre();
    }

    fn pad_find(&self, index: usize, row: usize) -> Option<PadTarget> {
        let targets = self.fe.layout_preview.as_ref()?.pad_targets();
        let i = self.pad_resolve(&targets)?;
        let _ = (index, row);
        targets.get(i).cloned()
    }

    /// Where the pad stands among `targets`: the same index and row **at the same place** — a tab click that
    /// opens a panel shifts the flattened indices of the widgets after it, so the place is what is kept — else
    /// the nearest target of the same kind within a couple of pixels.
    fn pad_resolve(&self, targets: &[PadTarget]) -> Option<usize> {
        let (index, row) = self.fe.pad_target?;
        let (px, py) = self.fe.pad_centre;
        let near = |t: &PadTarget| {
            let (x, y) = t.centre();
            (x - px).abs() < 1.5 && (y - py).abs() < 1.5
        };
        targets
            .iter()
            .position(|t| t.index == index && t.row == row && near(t))
            .or_else(|| targets.iter().position(|t| t.row == row && near(t)))
    }

    /// Put the virtual pointer on a target: the first of a few points inside it that the hit test
    /// really gives to the target (a widget drawn over part of it takes the others), else its click point.
    fn pad_aim(&mut self, t: &PadTarget) {
        let inset = |v: f32, a: f32| v + a.min(t.w.min(t.h) * 0.25);
        let points = [
            t.click,
            t.centre(),
            (inset(t.x, 4.0), inset(t.y, 4.0)),
            (t.x + t.w - 4.0f32.min(t.w * 0.25), inset(t.y, 4.0)),
            (inset(t.x, 4.0), t.y + t.h - 4.0f32.min(t.h * 0.25)),
            (t.x + t.w - 4.0f32.min(t.w * 0.25), t.y + t.h - 4.0f32.min(t.h * 0.25)),
        ];
        // rows of a drop-down lie outside the combo's own box: the row walk finds them, not the hit scan
        let by_hit = !matches!(t.kind, PadKind::ComboRow);
        let mut at = points[0];
        for p in points {
            self.fe.cursor = Some(p);
            self.fe_pointer_moved();
            if !by_hit || self.fe.layout_preview.as_ref().and_then(|l| l.hovered()) == Some(t.index) {
                at = p;
                break;
            }
        }
        if self.fe.cursor != Some(at) {
            self.fe.cursor = Some(at);
            self.fe_pointer_moved();
        }
        // and the old keyboard cursor follows, so a plain Enter after the pad lands on the same control
        if let Some(l) = self.fe.layout_preview.as_ref()
            && let Some(n) = l.controls().iter().position(|c| c.index == t.index)
        {
            self.fe.layout_sel = n;
        }
    }

    /// A click on a target: aim, press, release — the mouse's own path.
    fn pad_click(&mut self, t: &PadTarget, event_loop: &ActiveEventLoop) {
        let serial = self.fe.pad_serial;
        let name = self.fe.layout_name.clone();
        self.pad_aim(t);
        // the target is taken away while the click runs, so the release's own `layout_key(Enter)` takes the
        // button's legacy path instead of clicking again
        self.fe.pad_target = None;
        self.fe_left_button(event_loop, true);
        self.fe_left_button(event_loop, false);
        // the screen is still the same one: the pointer stays where it was, and so does the pad
        if self.fe.pad_serial == serial && self.fe.layout_name == name && self.fe.layout_preview.is_some() {
            self.pad_stand(t);
            // a click that opened a panel shifted the indices after it: stand where the target now is
            let now = self.fe.layout_preview.as_ref().map(|l| l.pad_targets()).unwrap_or_default();
            match self.pad_resolve(&now) {
                Some(i) => self.pad_stand(&now[i]),
                // a drop-down's row was taken and the list closed: back on the combo itself
                None if t.kind == PadKind::ComboRow => match now.iter().find(|x| x.index == t.index && x.kind == PadKind::Combo).cloned() {
                    Some(c) => {
                        self.pad_stand(&c);
                        self.pad_aim(&c);
                    }
                    None => self.fe.pad_target = None,
                },
                None => self.fe.pad_target = None,
            }
            // a combo the click opened has its rows now: stand on the one it shows selected, else the first
            if self.fe.layout_preview.as_ref().is_some_and(|l| l.pad_targets().iter().any(|x| x.kind == PadKind::ComboRow)) {
                self.pad_enter_rows();
            }
        } else {
            self.fe.pad_target = None;
        }
    }

    /// An open drop-down just opened: the pad stands on its first visible row.
    fn pad_enter_rows(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let rows: Vec<PadTarget> = l.pad_targets().into_iter().filter(|t| t.kind == PadKind::ComboRow).collect();
        // the row the accessor has selected, when it is among the visible ones
        let current = rows.first().and_then(|t| l.list_current(t.index));
        let pick = rows.iter().find(|t| Some(t.row) == current).or(rows.first()).cloned();
        if let Some(t) = pick {
            self.pad_stand(&t);
            self.pad_aim(&t);
        }
    }

    /// A direction key: the neighbour in that direction; inside a list or an open drop-down the rows
    /// first, scrolling at the ends.
    fn pad_move(&mut self, dir: PadDir) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let targets = l.pad_targets();
        if targets.is_empty() {
            return;
        }
        let hovered = l.hovered();
        let cur = self
            .pad_resolve(&targets)
            // a mouse user's pointer counts as where the pad stands
            .or_else(|| hovered.and_then(|h| targets.iter().position(|t| t.index == h)));
        let Some(cur) = cur else {
            // nowhere yet: the first place in reading order
            let mut order: Vec<usize> = (0..targets.len()).collect();
            order.sort_by(|&a, &b| {
                let (ca, cb) = (targets[a].centre(), targets[b].centre());
                ca.1.total_cmp(&cb.1).then(ca.0.total_cmp(&cb.0))
            });
            let t = targets[order[0]].clone();
            self.pad_stand(&t);
            self.pad_aim(&t);
            return;
        };
        let from = targets[cur].clone();
        let vertical = matches!(dir, PadDir::Up | PadDir::Down);
        // a tab walks its strip along the strip's own line — the graphic tabs side by side with Left / Right, the
        // staircase of text tabs with Up / Down — and any other key leaves the strip, past its (overlapping) boxes
        let strip: Option<Vec<usize>> = (from.kind == PadKind::Tab)
            .then(|| self.fe.layout_preview.as_ref().and_then(|l| l.tab_strips().into_iter().find(|s| s.tabs.contains(&from.index))))
            .flatten()
            .map(|s| s.tabs);
        if let Some(tabs) = strip {
            let ys: Vec<f32> = targets.iter().filter(|t| tabs.contains(&t.index)).map(|t| t.y).collect();
            let staircase = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min) > 20.0;
            let along = if staircase { vertical } else { !vertical };
            let at = tabs.iter().position(|t| *t == from.index).unwrap_or(0);
            let step: i32 = if matches!(dir, PadDir::Down | PadDir::Right) { 1 } else { -1 };
            let to = at as i32 + step;
            if along && to >= 0 && (to as usize) < tabs.len() {
                if let Some(t) = targets.iter().find(|t| t.index == tabs[to as usize]).cloned() {
                    self.pad_stand(&t);
                    self.pad_aim(&t);
                    return;
                }
            }
            let rest: Vec<PadTarget> = targets.iter().filter(|t| !tabs.contains(&t.index)).cloned().collect();
            if let Some(t) = self.pad_pick_outside(&from, &rest, dir) {
                self.pad_stand(&t);
                self.pad_aim(&t);
            }
            return;
        }
        let next: Option<PadTarget> = if matches!(from.kind, PadKind::ComboRow | PadKind::ListRow) && vertical {
            // the rows of this list, top to bottom
            let mut rows: Vec<&PadTarget> = targets.iter().filter(|t| t.index == from.index && t.kind == from.kind).collect();
            rows.sort_by_key(|t| t.row);
            let at = rows.iter().position(|t| t.row == from.row).unwrap_or(0);
            let step: i32 = if dir == PadDir::Down { 1 } else { -1 };
            let to = at as i32 + step;
            if to >= 0 && (to as usize) < rows.len() {
                Some(rows[to as usize].clone())
            } else {
                // an end: scroll one row, and stand on the row that came in
                let first = rows.first().map_or(0, |t| t.row) as i32;
                let before = (rows.first().map(|t| t.row), rows.last().map(|t| t.row));
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    if from.kind == PadKind::ComboRow {
                        l.set_combo_scroll_pos(from.index, (first + step).max(0));
                    } else {
                        l.scroll_list(from.index, step);
                    }
                }
                let after: Vec<PadTarget> = self
                    .fe
                    .layout_preview
                    .as_ref()
                    .map(|l| l.pad_targets().into_iter().filter(|t| t.index == from.index && t.kind == from.kind).collect())
                    .unwrap_or_default();
                let mut moved = !after.is_empty() && (after.first().map(|t| t.row), after.last().map(|t| t.row)) != before;
                if !moved && after.is_empty() {
                    // scrolled past the last row (the scrollbar's range runs one over): back
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        if from.kind == PadKind::ComboRow {
                            l.set_combo_scroll_pos(from.index, first);
                        } else {
                            l.scroll_list(from.index, -step);
                        }
                    }
                    moved = false;
                }
                if moved {
                    if dir == PadDir::Down { after.last().cloned() } else { after.first().cloned() }
                } else if from.kind == PadKind::ListRow {
                    // the list's end: on to the next control that way, outside the list
                    let rest: Vec<PadTarget> = targets.iter().filter(|t| t.index != from.index).cloned().collect();
                    self.pad_pick_outside(&from, &rest, dir)
                } else {
                    None
                }
            }
        } else if from.kind == PadKind::ComboRow {
            // an open drop-down answers to Up and Down alone
            None
        } else if from.kind == PadKind::ListRow {
            let rest: Vec<PadTarget> = targets.iter().filter(|t| t.index != from.index).cloned().collect();
            self.pad_pick_outside(&from, &rest, dir)
        } else {
            // a list's rows count once for the pick: the row level with the pad's centre is what matters
            pad_pick(&targets, cur, dir).map(|i| targets[i].clone())
        };
        if let Some(t) = next {
            self.pad_stand(&t);
            self.pad_aim(&t);
        }
    }

    /// The pick from `from` among targets other than its own list's rows.
    fn pad_pick_outside(&self, from: &PadTarget, rest: &[PadTarget], dir: PadDir) -> Option<PadTarget> {
        let mut v = vec![from.clone()];
        v.extend(rest.iter().cloned());
        pad_pick(&v, 0, dir).map(|i| v[i].clone())
    }

    /// B: the keyboard's `b` — the bottom bar's back where it is enabled; a dialog's "no" and an open
    /// drop-down's closing come first, and a screen whose bottom bar has no back falls to its own
    /// leaving button (one with the `GOBACK` callback, or a destination with a negative `offset`: the overlay's close).
    fn pad_back(&mut self, event_loop: &ActiveEventLoop) {
        // an open drop-down closes unchanged, as a click away from it does (0x4C6E2E: sound 3)
        let open = self.fe.layout_open_combo.or_else(|| self.fe.layout_preview.as_ref().and_then(|l| l.open_combo()));
        if let Some(i) = open {
            self.fe_sound(3);
            self.fe.layout_open_combo = None;
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_open_combo(None);
            }
            if let Some(t) = self.fe.layout_preview.as_ref().and_then(|l| l.pad_targets().into_iter().find(|t| t.index == i && t.kind == PadKind::Combo)) {
                self.pad_stand(&t);
                self.pad_aim(&t);
            }
            return;
        }
        if self.fe_callback_key(0x62) || self.fe_manager_key(0x62, event_loop) {
            return;
        }
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let controls = l.controls();
        let usable = |c: &&nfs_game::layout::Control| !l.widget_disabled(c.index);
        // the screen's own back button first, then a button that closes its layer
        let leave = controls
            .iter()
            .filter(usable)
            .find(|c| c.callback.as_deref() == Some("GOBACK"))
            .or_else(|| controls.iter().filter(usable).find(|c| c.destination.is_some() && c.offset < 0))
            .cloned();
        if let Some(c) = leave
            && let Some(t) = l.pad_targets().into_iter().find(|t| t.index == c.index)
        {
            self.pad_click(&t, event_loop);
        }
    }

    /// A bottom-bar key (Start, Y): the button by its name, when the bar has it enabled — and, for the
    /// race, when the screen can start one.
    fn pad_bottom(&mut self, name: &str, event_loop: &ActiveEventLoop) {
        let Some(b) = self.fe.layout_bottom.as_ref() else { return };
        let Some((index, _)) = b.widgets_named(&[name]).first().copied() else { return };
        if b.widget_disabled(index) {
            return;
        }
        let Some(c) = b.controls().into_iter().find(|c| c.index == index) else { return };
        if name == "ID_PLAYGAMEBUTTON" && !self.fe.layout_preview.as_ref().is_some_and(|l| l.can_play()) {
            return;
        }
        self.fe_bottom_callback(c.callback.as_deref(), event_loop);
    }

    /// LB / RB: the previous / next tab of the strip the pad stands in (else the outermost strip).
    fn pad_tab(&mut self, step: i32, event_loop: &ActiveEventLoop) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let strips = l.tab_strips();
        let stand = self.fe.layout_preview.as_ref().and_then(|l| self.pad_resolve(&l.pad_targets())).map(|i| l.pad_targets()[i].index);
        // the strip the pad stands on; off the tabs, the innermost strip on show (the graphics panel's sub-tabs
        // while the pad is in the panel); the outer strip again once the pad is back on one of its tabs
        let by_tab = strips.iter().find(|s| stand.is_some_and(|x| s.tabs.contains(&x)));
        let Some(strip) = by_tab.or_else(|| strips.iter().max_by_key(|s| s.depth)).cloned() else { return };
        let tabs = strip.tabs;
        let at = strip.current.or_else(|| tabs.iter().position(|t| Some(*t) == stand)).unwrap_or(0) as i32;
        let to = at + step;
        if to < 0 || to as usize >= tabs.len() {
            return;
        }
        let want = tabs[to as usize];
        if let Some(t) = l.pad_targets().into_iter().find(|t| t.index == want) {
            self.pad_click(&t, event_loop);
        }
    }

    /// A key while a dialog is up: Up / Down (and Left / Right) walk its buttons, A presses the one the pointer
    /// is on — the only button of a one-button dialog needs no walk —, B presses the last (the "no" /
    /// "cancel"). Through the pointer, like the mouse: the dialog's slot 8 hovers, its slot 9 answers.
    fn pad_dialog_key(&mut self, key: MenuKey) {
        let Some(l) = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()) else { return };
        let Some((d, _)) = self.fe.dialog.as_ref() else { return };
        let th = l.dialog_text_height(&d.text);
        let n = d.buttons.len();
        if n == 0 {
            return;
        }
        let hovered = d.buttons.iter().position(|b| b.hovered);
        let centres: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let (x, y, w, h) = d.button_rect(i, th);
                (x + w * 0.5, y + h * 0.5)
            })
            .collect();
        let aim = |app: &mut Self, i: usize| {
            app.fe.cursor = Some(centres[i]);
            app.fe_pointer_moved();
        };
        match key {
            MenuKey::Up | MenuKey::Left | MenuKey::Down | MenuKey::Right => {
                let back = matches!(key, MenuKey::Up | MenuKey::Left);
                let to = match hovered {
                    // the first key lands on the safe answer: the last button ("no" / "cancel")
                    None => n - 1,
                    Some(h) if back => (h + n - 1) % n,
                    Some(h) => (h + 1) % n,
                };
                aim(self, to);
            }
            MenuKey::Enter => {
                if let Some(i) = hovered.or((n == 1).then_some(0)) {
                    aim(self, i);
                    self.pad_press_release();
                }
            }
            MenuKey::Back => {
                aim(self, n - 1);
                self.pad_press_release();
            }
            _ => {}
        }
    }

    /// A press and a release of the left button where the virtual pointer stands (a dialog's, which has
    /// no event loop of its own to hand on).
    fn pad_press_release(&mut self) {
        // the dialog's release (slot 9, 0x515130) is all that runs: `fe_left_button` hands it the event first
        if let Some((d, _)) = self.fe.dialog.as_mut()
            && let Some(s) = d.release()
        {
            self.fe_sound(s as usize);
        }
    }
}

impl App {
    /// The widgets' side of a pointer move, from `self.fe.cursor`: the hover (and its sound), the dialog's
    /// buttons, the scrollbar drags. The real `CursorMoved` runs it, and so does the pad's virtual pointer.
    pub(crate) fn fe_pointer_moved(&mut self) {
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
}
