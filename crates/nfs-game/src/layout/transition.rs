//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// The black bars that keep the 4:3 design area square in a wider window. The 2D screen is
    /// letterboxed by [`place`], but a 3D room behind it fills the whole window, so the bars have to
    /// be drawn over it. PORT-DEVIATION D14 (PORT-DEVIATIONS.md) [the original draws into its own D3D mode, 0x44E720] — the original's front end is always 4:3.
    pub fn letterbox_bars(&self, aspect: f32) -> Vec<Sprite> {
        let design = DESIGN.0 / DESIGN.1;
        if aspect <= design + 1e-3 {
            return Vec::new();
        }
        let w = (1.0 - design / aspect) * 0.5;
        let bar = |x: f32| Sprite {
            texture: self.white,
            pos: [x, 0.0],
            size: [w * aspect, 1.0],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [0.0, 0.0, 0.0, 1.0],
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: None, corner_colors: None,
            depth: None,
        };
        vec![bar(0.0), bar(1.0 - w)]
    }

    /// Mark this screen as drawn over something else — the bottom bar the front end lays over
    /// every screen (0x4D39D3), or any screen with a 3D room behind it
    /// (`RE/notes/frontend-3d.md`). Such a screen draws its widgets but not the screen fill, which
    /// would otherwise paint over what is behind. The background screen is marked with it.
    pub fn set_overlay(&mut self, overlay: bool) {
        self.overlay = overlay;
        if let Some(b) = self.background.as_mut() {
            b.set_overlay(overlay);
        }
    }

    /// Start this screen sliding and fading in, the way the original does when a screen is pushed
    /// (state 1, 0x4E09A0). The background screen transitions with it.
    pub fn transition_in(&mut self) {
        self.transition = Transition::In { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_in();
        }
        // and every panel open on it: the strip's selected button runs its slot 27, 0x4D02E0,
        // whose show 0x4CEC60 starts the sub-screen in (its slot 0x78, 0x4E09A0) as the screen
        // opens (MEDIUM on the frame: the call that runs it at the open was not traced)
        let panels: Vec<usize> = self.panel_groups().filter(|i| !self.closed.contains(i)).collect();
        for i in panels {
            if let Some(g) = self.groups.get_mut(&i) {
                g.transition_in();
            }
        }
    }

    /// The groups that are `NFS5.PANEL`s (the rest are tab strips).
    fn panel_groups(&self) -> impl Iterator<Item = usize> + '_ {
        self.groups.keys().copied().filter(|i| self.layout.elements.get(*i).is_some_and(|e| e.class == "NFS5.PANEL"))
    }

    /// Start it going out (state 3, 0x4E09C0).
    pub fn transition_out(&mut self) {
        self.transition = Transition::Out { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_out();
        }
    }

    pub fn transition(&self) -> Transition {
        self.transition
    }

    /// Hand a top-level screen the **first layer's** screen transition, which its widgets fall
    /// back on while their own screen is up: 0x4C8760 / 0x4C8840 read the widget's screen state
    /// (0x4E0E00 on `[this+0x114]`) and, when it is 2, **0x4D63C0** — 0x4E0E00 of `[[[fe+0xD8]]]`,
    /// the screen on show of layer 0 (2 when there is none); the progress is 0x4E0D90(1), the
    /// larger (`jg`, signed) of the own `+0x130` and **0x4D6300** — `0x4E0D90(0)`, the bare
    /// `+0x130`, of that same screen (0 when there is none). So the bottom bar (whose own
    /// transition is up almost always) and the screens of the layers above the first slide and
    /// fade with the first layer's screen. `front` is that screen's own transition, not what it
    /// inherits in turn (0x4D6312 pushes 0).
    pub fn take_front_transition(&self, front: Transition) {
        self.inherited.set((matches!(front, Transition::Out { .. }), front.progress()));
    }

    /// Advance the transition clock by one frame; `true` once the screen has settled or gone.
    pub fn advance_transition(&mut self, dt_ms: u32) -> bool {
        self.clock_ms = self.clock_ms.wrapping_add(dt_ms);
        // `[widget+0x12C]` is stamped with the tick whenever the widget is *not* hovered
        // (0x4C824D), so the count restarts the moment the pointer moves on (§4.52)
        self.claim_pointer();
        let on = self.hovered();
        if on == self.hover_of {
            self.hover_ms = self.hover_ms.saturating_add(dt_ms);
        } else {
            self.hover_of = on;
            self.hover_ms = 0;
        }
        self.transition = self.transition.advance(dt_ms);
        if let Some(b) = self.background.as_mut() {
            b.advance_transition(dt_ms);
        }
        let own = self.transition;
        self.tick_panels(dt_ms, matches!(own, Transition::Out { .. }));
        self.transition.done()
    }

    /// The screen's children that are sub-screens, ticked in the widgets' order (0x4E0950 walks
    /// the child vector from 0):
    ///
    /// * a panel, **0x4CEBC0**: when the screen is going out and the sub-screen is in or up
    ///   (0x4E0DE0, state 1 or 2) it starts it out (+0x7C); then the sub-screen's advance (+0x80)
    ///   and draw; and once the sub-screen is down (0x4E0E10, state 0 — an `Out` run to its end)
    ///   the panel closes (+0x70, 0x4CEB00);
    /// * a tab strip, **0x4CFEB0**: its buttons' ticks first — **0x4D0320** clears the selected
    ///   flag of a button whose panel is hidden (0x4CEC70: none, or its sub-screen down) — and
    ///   then, with a pending button, the current one no longer selected and the screen neither
    ///   going out (0x4E0E00 ≠ 3) nor down, the pending one becomes current: slot 27 0x4D02E0,
    ///   the selected flag (0x4C7D00), the panel's load (0x4CEA90) and show (0x4CEC60).
    ///
    /// So a tab switch is **the old panel out, then the new one in**, each three frames and 64
    /// ticks, and whether the new one starts on the frame the old one closes or the next depends
    /// on whether the panel comes before the strip in the file.
    fn tick_panels(&mut self, dt_ms: u32, screen_out: bool) {
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        for i in keys {
            let is_panel = self.layout.elements.get(i).is_some_and(|e| e.class == "NFS5.PANEL");
            if is_panel {
                if self.closed.contains(&i) {
                    continue;
                }
                let Some(g) = self.groups.get_mut(&i) else { continue };
                if screen_out && !matches!(g.transition, Transition::Out { .. }) {
                    g.transition_out();
                }
                g.advance_transition(dt_ms);
                if matches!(g.transition, Transition::Out { .. }) && g.transition.done() {
                    self.closed.insert(i);
                }
                continue;
            }
            // a strip: its clock and its own children first
            if let Some(g) = self.groups.get_mut(&i) {
                g.advance_transition(dt_ms);
            }
            self.tick_strip(i, screen_out);
        }
    }

    /// The panel of this screen a tab button names (0x4D01F0 resolves `data.panel` on the screen
    /// that holds the strip).
    pub(super) fn panel_index(&self, name: &str) -> Option<usize> {
        self.layout.elements.iter().position(|e| e.class == "NFS5.PANEL" && e.get("name").is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }

    /// 0x4CEC70: the panel shows nothing — none there, never loaded, or its sub-screen down.
    fn panel_hidden(&self, name: Option<&str>) -> bool {
        match name.and_then(|n| self.panel_index(n)) {
            Some(i) => self.closed.contains(&i) || !self.groups.contains_key(&i),
            None => true,
        }
    }

    /// The strip at `k`'s tick, 0x4CFEB0 (see `tick_panels`).
    fn tick_strip(&mut self, k: usize, screen_out: bool) {
        let is_tab = |c: &str| c == "NFS5.TABBUTTON" || c == "NFS5.GRAPHICTABBUTTON";
        let Some(g) = self.groups.get(&k) else { return };
        // the buttons' tick 0x4D0320
        let mut unselect = Vec::new();
        let mut current = None;
        for (n, e) in g.layout.elements.iter().enumerate() {
            if !is_tab(&e.class) {
                continue;
            }
            let selected = g.selected_flags.contains(&n) || (e.get("state") == Some("selected") && !g.unselected.contains(&n));
            if selected {
                if self.panel_hidden(e.get("panel").filter(|p| !p.is_empty())) {
                    unselect.push(n);
                } else {
                    current = Some(n);
                }
            }
        }
        let g = self.groups.get_mut(&k).expect("strip");
        for n in unselect {
            g.selected_flags.remove(&n);
            g.unselected.insert(n);
        }
        let Some((s, j)) = self.pending_tab.filter(|(s, _)| *s == k) else { return };
        let _ = s;
        if current.is_some() || screen_out {
            return;
        }
        // slot 27, 0x4D02E0: selected, the panel's load and show
        g.unselected.remove(&j);
        g.selected_flags.insert(j);
        let panel = g.layout.elements.get(j).and_then(|e| e.get("panel")).filter(|p| !p.is_empty()).map(str::to_string);
        self.pending_tab = None;
        if let Some(i) = panel.and_then(|p| self.panel_index(&p))
            && let Some(p) = self.groups.get_mut(&i)
        {
            self.closed.remove(&i);
            p.reopen();
            p.transition_in();
        }
    }

    /// The panel's load, 0x4CEA90: `0x4E1200` builds the sub-screen afresh from its file every
    /// time, so its widgets are new — their timers, typing and glints start from the open. The
    /// port keeps the loaded layout and restarts those.
    fn reopen(&mut self) {
        self.clock_ms = 0;
        self.preload_ticks = 0;
        self.typing.borrow_mut().clear();
        self.wrap_watch.borrow_mut().clear();
        self.glints.borrow_mut().clear();
        self.activate_glints();
    }

    /// The offset and the alpha a widget draws with at this point in the transition
    /// (`FeWidget::transitionOffset` 0x4C8760 and `::transitionAlpha` 0x4C8840): the slide is the
    /// screen size scaled by the progress, the fade is the progress itself, and the bits come from
    /// `transon` while the screen comes in and from `transoff` while it goes out.
    pub(super) fn transition_of(&self, e: &nfs_formats::lay::Element) -> ((f32, f32), f32) {
        let (out, p) = self.widget_state();
        if p == 0 {
            return ((0.0, 0.0), 1.0);
        }
        let (key, other) = if out { ("transoff", "transon") } else { ("transon", "transoff") };
        // `SAME` means "use the other one", and both `SAME` means no transition at all
        // (0x4C75D4…0x4C7601)
        let (mut bits, ob) = (transition_bits(self.attr(e, key).unwrap_or("")), transition_bits(self.attr(e, other).unwrap_or("")));
        if bits & trans::SAME != 0 {
            bits = if ob & trans::SAME != 0 { 0 } else { ob };
        }
        // **0x4C8760's four arms**, and the multiplier they use: `[0x5DEAC8]` for x and
        // `[0x5DEACC]` for y, over `[0x5B6B2C]` = 512. Those two are the **live screen extent**,
        // written at 0x556C8A from the selected video mode (0x5DE654 / 0x5DE658) and clipped
        // against by the platform layer at 0x53550E — so the widget travels exactly one screen,
        // and multiplying by the design size in design space is that same distance. Pinned
        // 2026-09-21; the addresses were cited for `TRANS_FULL` but never for the multiplier.
        let f = p as f32 / TRANS_FULL as f32;
        let mut d = (0.0f32, 0.0f32);
        if bits & trans::LEFT != 0 {
            d.0 -= DESIGN.0 * f;
        }
        if bits & trans::RIGHT != 0 {
            d.0 += DESIGN.0 * f;
        }
        if bits & trans::TOP != 0 {
            d.1 -= DESIGN.1 * f;
        }
        if bits & trans::BOTTOM != 0 {
            d.1 += DESIGN.1 * f;
        }
        // **0x4C8820**: `p << 8` less one copy of `p` — the ×255 idiom — over the same 512, an
        // integer divide, and the draw takes that off the full alpha.
        let alpha = if bits & trans::FADE != 0 { 1.0 - (p * 255 / TRANS_FULL) as f32 / 255.0 } else { 1.0 };
        (d, alpha)
    }

    /// The x of the widget named `name` on this screen — the column anchor an accessor's row draw
    /// looks up (`FE_widgetPos` 0x4D6410 searches the current screen's widget list from the end).
    pub(super) fn anchor_x(&self, name: &str) -> Option<f32> {
        let e = self.layout.elements.iter().rev().find(|e| e.name.eq_ignore_ascii_case(name))?;
        Some(self.position_of(e).0)
    }
}
