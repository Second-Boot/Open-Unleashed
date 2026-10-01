//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// Enable or disable a widget, as the by-name helper **0x50D880** does: 0x50D8DB reads the
    /// bit, and its argument is **enable**, not disable — 0x50D8EA takes 0x4C7DF0, which is
    /// `and al, 0xEF`, and 0x50D8F1 takes 0x4C7E10, which is `or al, 0x10` behind the template's
    /// own `[state.disabled]`. §4.113.
    pub fn set_disabled_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_disabled_flag(j, on);
            }
            return;
        }
        if on {
            self.disabled_flags.insert(index);
        } else {
            self.disabled_flags.remove(&index);
        }
    }

    /// Hide or show a widget, as the by-name helper **0x50D910** does: it finds the widget on the
    /// screen, clears the hidden bit with 0x4C7ED0 and then sets it with 0x4C7E80 or clears it
    /// with 0x4C7EB0 (0x50D96B…0x50D981). Fifty-one call sites use it — the repair screen's ten
    /// damage bars, the parts screen's install and uninstall buttons, the used-car screen's two
    /// panels. §4.112.
    pub fn set_hidden_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_hidden_flag(j, on);
            }
            return;
        }
        if on {
            self.hidden_flags.insert(index);
        } else {
            self.hidden_flags.remove(&index);
        }
    }

    /// Whether the screen has hidden this widget.
    pub fn is_hidden(&self, index: usize) -> bool {
        match self.route(index) {
            Some((k, j)) => self.groups.get(&k).is_some_and(|g| g.is_hidden(j)),
            None => self.hidden_flags.contains(&index),
        }
    }

    /// Set or clear a widget's **`selected`** flag, as 0x50D9A0 does by name: it looks the widget
    /// up on the screen, asks 0x4C7D50 whether the flag is already what is wanted, and calls
    /// 0x4C7D00 or 0x4C7D30 only when it is not (0x50D9FB…0x50DA1B). §4.111.
    pub fn set_selected_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_selected_flag(j, on);
            }
            return;
        }
        if on {
            self.selected_flags.insert(index);
            self.unselected.remove(&index);
        } else {
            self.selected_flags.remove(&index);
            self.unselected.insert(index);
        }
    }

    pub fn focus_input(&mut self, index: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.focus_input(j);
            }
            return;
        }
        // 0x4C7D77's guard: `[this+0x110] & 0x40`. That word is the **mask of the states the
        // widget's template declares**, built by slot 12 (0x4C7760) by walking the name table at
        // 0x5D81FC in 0x19-byte steps — bit 0 `normal`, 1 `stringnotused`, 2 `hover`, 3
        // `selected`, 4 `disabled`, 5 `hidden`, 6 `focused`, 7 `focusedhover`, 8 `selectedhover`,
        // 9 `wrenderoff`. So bit 6 means the `.ctr` has a `[state.focused]`, and a template
        // without one can never take focus. §4.109.
        if self.controls.get(&self.layout.elements[index].class).is_some_and(|c| c.has_state("focused")) {
            self.focused.insert(index);
        }
    }

    /// The longest string an input box's accessor will hold, from the **third argument** of
    /// `FEStringAccessor`'s constructor 0x4BC8B0 — the ctor allocates `maxLen + 1` bytes and the
    /// insert at `vt[0x30]` refuses to pass it. Read out of the three registrations that carry a
    /// name this port can see: `UserList.NameInput` at 0x4E5D9E pushes **8**, `ANOTHER` at
    /// 0x4E6016 pushes **15**, `COMMS.PASSWORD` at 0x50B9C1 pushes **7**, `Comms.ChatInput` at
    /// 0x50BA95 pushes **45** (0x2D).
    ///
    /// Every other box's limit lives in a registration this port has not read, so it is left
    /// **unlimited** rather than guessed — a cap invented here would silently truncate a name the
    /// original accepts, which is worse than one it would have refused.
    pub fn input_max_len(accessor: &str) -> Option<usize> {
        match accessor {
            "UserList.NameInput" => Some(8),
            "ANOTHER" => Some(15),
            "COMMS.PASSWORD" => Some(7),
            "Comms.ChatInput" => Some(45),
            _ => None,
        }
    }

    /// Which input box has focus, if any. A box with `data.alwaysfocused` counts from the moment
    /// it is built: its bind **0x4CC430** calls `setFocused` outright (0x4CC56E…0x4CC570, §4.41),
    /// so the keyboard reaches it with no click. Until 2026-10-01 the port only *drew* such a box
    /// focused, and `createplayer`'s name field took no keys at all.
    pub fn focused_input(&self) -> Option<usize> {
        let is_input = |i: usize| {
            self.layout.elements.get(i).is_some_and(|e| {
                matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX")
            })
        };
        self.focused.iter().copied().find(|&i| is_input(i)).or_else(|| {
            (0..self.layout.elements.len()).find(|&i| {
                is_input(i) && !self.is_hidden(i) && self.attr_num(&self.layout.elements[i], "alwaysfocused").unwrap_or(0.0) != 0.0
            })
        })
    }

    /// One typed character, filtered exactly as the shared key handler **0x4CCC20** filters it:
    ///
    /// ```text
    /// 0x4CCD95  c >= 0xFF  -> refuse
    /// 0x4CCD9D  c <  0x20  -> refuse
    /// 0x4CCDA2  c == 0x25  -> refuse          ; '%', and nothing else is singled out
    ///           otherwise accessor->vt[0x30](c)
    /// ```
    ///
    /// `'%'` is the only printable character a box turns down **at this shared layer**, and the
    /// reason is visible in what the strings are for: every front-end string reaching 0x5A0FBF is
    /// a `printf` format, so a typed `%` would be read as a conversion.
    ///
    /// **A second, per-instance filter, found this pass and not previously ported.** `vt[0x30]`
    /// (slot 12, `FEStringAccessor::append` **0x4BC950**, §4.139) does not insert the character
    /// itself — it calls slot 11, **0x4BCA90**, which insists on `[this+0x1C]` before it filters
    /// at all: a byte the ctor **0x4BC8B0** takes as its 5th argument, `false` for most of the
    /// class's registrations (e.g. `"ANOTHER"`, 0x4E6016) but **`true` for `UserList.NameInput`**
    /// (0x4E5D9E, confirmed by disassembling that one call site's own pushed args — the same site
    /// §4.139's `maxLen` table already cites for its `8`). When it is set, 0x4BCA90 rejects nine
    /// more characters no other box turns down — `\ / : * ? " < > |`, byte for byte the set
    /// Windows refuses in a file name — while still consuming the keystroke (a rejected character
    /// plays no different sound; it is simply never inserted). The profile name typed here is
    /// exactly what a save file is later named (`career_ui.rs`, `TEMPLATE_STEM`/`create_profile`),
    /// so the filter is not cosmetic: a name that reached this port's own save path unfiltered
    /// (`8Kate/Kate.sav`, say) would build an invalid or wrong path the original never lets a
    /// player type. Not read for `COMMS.PASSWORD` (0x50B9C1, unconfirmed — out of scope, network
    /// play carries no filesystem write this port makes).
    pub fn type_char(&mut self, index: usize, c: char) -> bool {
        let code = c as u32;
        if !(0x20..0xFF).contains(&code) || code == 0x25 {
            return false;
        }
        let accessor = self.layout.elements.get(index).and_then(|e| e.get("accessor"));
        // 0x4BCA90's own filename-illegal set, active only where the ctor's 5th argument was
        // `true` — checked directly against every one of `input_max_len`'s own four named
        // registrations by disassembling each one's own pushed args this pass: `true` only for
        // `UserList.NameInput` (0x4E5D9E); `"ANOTHER"` (0x4E6016), `COMMS.PASSWORD` (0x50B9C1)
        // and `Comms.ChatInput` (0x50BA95) all push `false`.
        if accessor == Some("UserList.NameInput") && filename_illegal_char(code) {
            return false;
        }
        let max = accessor.and_then(Self::input_max_len);
        let buf = self.edits.entry(index).or_default();
        if max.is_some_and(|m| buf.chars().count() >= m) {
            return false;
        }
        buf.push(c);
        true
    }

    /// Backspace — `accessor->vt[0x38]()`, reached from 0x4CCC20's `c == 8` arm. Returns whether
    /// anything was erased.
    pub fn backspace(&mut self, index: usize) -> bool {
        self.edits.get_mut(&index).is_some_and(|b| b.pop().is_some())
    }

    /// Return, `c == 0x0D`: 0x4CCDB5 drops the focus flag through `0x4C7DA0` and then calls
    /// `accessor->vt[0x0C]()` to commit. Hands back the accessor's name and what was typed so the
    /// caller can store it; the focus goes, the text stays — it is the accessor's own buffer,
    /// which `CREATEPLAYERDONE` (0x5107A8) reads through `vt[0x10]` after the box has let go.
    pub fn commit_input(&mut self, index: usize) -> Option<(String, String)> {
        let name = self.layout.elements.get(index)?.get("accessor")?.to_string();
        let text = self.edits.get(&index)?.clone();
        self.focused.remove(&index);
        Some((name, text))
    }

    /// `NFS5.CHATINPUTBOX`'s Return — its slot 4, **0x4C3310**, which the chat box has in place of
    /// the input boxes' 0x4CCC20:
    ///
    /// ```text
    /// 0x4C3325  focused? and 0x4C3332 the key is 0x0D?   otherwise 0x4CCC20, the shared handler
    /// 0x4C3364  text = accessor->vt[0x10]()              ; sent by 0x48D9B0, the caller's part
    /// 0x4C3376  accessor->vt[0x44]()                     ; clear (0x4BC9C0)
    /// 0x4C3388  this->vt[0x38]()                         ; slot 14, 0x4CC7B0, the box's re-measure
    /// ```
    ///
    /// so, unlike 0x4CCDB5's commit, the box **keeps its focus** and the line is taken whatever it
    /// holds, an empty one included. Answers the text when `index` is a chat input box.
    pub fn take_chat(&mut self, index: usize) -> Option<String> {
        (self.layout.elements.get(index)?.class == "NFS5.CHATINPUTBOX").then_some(())?;
        Some(self.edits.remove(&index).unwrap_or_default())
    }

    /// Type `text` into the input box bound to `accessor`, a character at a time through
    /// [`Self::type_char`] (diagnostic: `NFS_FE_KEYS`'s `type:`).
    pub fn type_into(&mut self, accessor: &str, text: &str) {
        if let Some(i) = self.layout.elements.iter().position(|e| e.get("accessor") == Some(accessor)) {
            for c in text.chars() {
                self.type_char(i, c);
            }
        }
    }

    /// Empty the input box bound to `accessor` — the string accessor's clear, its slot 0x44.
    pub fn clear_input(&mut self, accessor: &str) {
        if let Some(i) = self.layout.elements.iter().position(|e| e.get("accessor") == Some(accessor)) {
            self.edits.remove(&i);
        }
    }

    /// What the input box bound to `accessor` holds — `accessor->vt[0x10]()`, the string
    /// accessor's buffer (empty until something is typed; the buffers go with the screen).
    pub fn input_text(&self, accessor: &str) -> String {
        self.layout
            .elements
            .iter()
            .position(|e| e.get("accessor") == Some(accessor))
            .and_then(|i| self.edits.get(&i).cloned())
            .unwrap_or_default()
    }

    /// The `NFS5.CONTROLLERSETUP` row waiting for a control, if one has focus — its flattened
    /// index and its accessor (`Controller.SteerLeft` …). A row takes focus on activation
    /// (0x51C5F0) and, while it holds it, slot 4 0x51C610 takes every key message (§4.117).
    pub fn focused_binding_row(&self) -> Option<(usize, String)> {
        for &i in &self.focused {
            if let Some(e) = self.layout.elements.get(i)
                && e.class == "NFS5.CONTROLLERSETUP"
            {
                return Some((i, e.get("accessor").unwrap_or_default().to_string()));
            }
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if let Some((j, a)) = g.focused_binding_row() {
                return Some((base + j, a));
            }
            base += g.span();
        }
        None
    }

    /// Let focus go — 0x4C7DA0, which every path of a binding row's capture ends in (0x51C5DE,
    /// 0x51C65C).
    pub fn release_focus(&mut self, index: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.release_focus(j);
            }
            return;
        }
        self.focused.remove(&index);
    }

    /// What an input box should display: what has been typed into it, if anything, and otherwise
    /// whatever its accessor answers.
    pub(super) fn edited(&self, index: usize) -> Option<&str> {
        self.edits.get(&index).map(String::as_str)
    }

    /// A press on the open drop-down's scrollbar, as `FeListScrollBar::onPress` 0x4DB040 handles
    /// it: arm 1 steps the list back one row (`[this+0xDC] = -1` at 0x4DB080), arm 2 forward one
    /// (`+1` at 0x4DB096), both after the click 0x4AADB0(8). Arms 3 and 4 (the knob and the track)
    /// start a drag instead — `combo_scroll_grab` / `combo_scroll_track` / `set_combo_scroll_pos`,
    /// called from `App::fe_left_button` the way `list_drag` already does for the list box's own
    /// scrollbar. `index` is the combo's flattened index (`combo_scroll_at`'s own), routed into an
    /// embedded screen the same way every other combo method is — *(until this fix, a plain
    /// `self.open_combo` here meant a nested panel's combo scrollbar never stepped at all; user
    /// report's knob-drag gap was one symptom of the same missing routing)*.
    pub fn combo_scroll_press(&mut self, index: usize, region: u32) -> bool {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get_mut(&k).is_some_and(|g| g.combo_scroll_press(j, region));
        }
        let Some((_, _, _, positions, first)) = self.combo_scroll_geom(index) else { return false };
        let next = match region {
            1 => first.saturating_sub(1),
            2 => (first + 1).min(positions.saturating_sub(1)),
            _ => return false,
        };
        if next == first {
            return false;
        }
        self.scroll.insert(index, next);
        true
    }
}
