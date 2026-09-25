//! The front end's screen manager — the vector of **layers** at `[fe+0xD8]` and the history each
//! layer keeps (`RE/notes/controls.md` §4.166).
//!
//! A layer is a 0x14-byte record (allocated at 0x4D21D1): `+0` the screen on show, `+4` the one
//! waiting to replace it, `+8` a `vector<string>` — the **history**. 0x4D3420 builds the vector
//! afresh on every entry with one empty layer (0x4D6520(0) is `&vec[0]`).
//!
//! ```text
//! 0x4D5DF0(layer, name, record)   show: nothing while +4 is set; +4 = the screen (0x4E0F90);
//!                                 record → push name onto the history (0x4D5E48…0x4D5EA8);
//!                                 +0 set → close it (slot 0x7C) and swap at the tick, else swap now
//! 0x4D5BE0(layer, name)           push name onto the history alone (0x4D5C41…0x4D5C60)
//! 0x4D2100(name, offset, record)  offset > 0: a new empty layer appended (0x4D21D1…0x4D2233),
//!                                            then 0x4D5DF0(new, name, record)
//!                                 offset < 0 and two layers or more (0x4D22DD): the top one's
//!                                            screen closed (slot 0x7C, 0x4D22FC), then
//!                                            0x4D5DF0(vec[-2], name, record)
//!                                 else:      0x4D5DF0(vec[-1], name, record)
//!                                 then 0x4D1E70, sound 0 (0x4D23D0), 0x4D1BD0
//! 0x4D23F0(arg)                   arg == −1: the top layer's history emptied (0x4D24F5…0x4D2529),
//!                                            sound 4, its screen closed (0x4D2539)
//!                                 else 0x4D5FB0(top), then 0x4D1BD0, 0x4D1E70
//! 0x4D5FB0(layer)                 history empty → nothing (0x4D5FD2);
//!                                 pop the last name (0x4D5FF4); one left → show it (0x4D6096,
//!                                 not recorded again) with sound 1, else sound 4 (0x4D610E);
//!                                 either way the screen on show closes (slot 0x7C)
//! 0x4D2570 (tick)                 a closed screen gives way to +4; a top layer with neither
//!                                 (+0 and +4 empty) is freed and popped (0x4D263C…0x4D26B1),
//!                                 then 0x4D1E70 and the bottom bar's 0x4D81C0
//! 0x4D1E70                        every layer but the top covered (0x4E08C0, `[+0x261]` = 1),
//!                                 the top one uncovered; the bottom bar covered when there is
//!                                 more than one layer (0x4D1F4C)
//! 0x4D6320 (BACK enabled)         !lock [+0x17C] and (one layer whose history is not exactly
//!                                 one name, or the race's options entry [+0x178] == 1)
//! 0x4D63A0                        exactly one layer (OPTIONS' second test)
//! ```
//!
//! A layer's closing is not immediate in the original — the screen fades out and the tick pops
//! the layer — but nothing reads the vector in between, so this model does it at once.

/// One entry of `[fe+0xD8]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layer {
    /// `+0` — the screen on show; `None` once it has closed with nothing to follow it
    pub screen: Option<String>,
    /// `+8` — the names 0x4D5DF0 recorded and 0x4D5BE0 pushed, oldest first
    pub history: Vec<String>,
}

/// What a navigation call leaves on show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// this screen is now on show (on the top layer)
    Screen(String),
    /// the call changed nothing (0x4D5FB0 on an empty history)
    Nothing,
    /// the last layer closed: the front end is over (the vector is empty)
    Ended,
}

/// The manager's vector and the two bytes its tests read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nav {
    pub layers: Vec<Layer>,
    /// `[fe+0x178]` == 1 — the front end entered from the race's options (0x4155B4). Never true
    /// in the original either: no HUD page answers the pump's event 1 that leads there
    /// (`controls.md` §4.169.1), so it stays false; the tests keep the arm 0x4D6320 reads.
    pub options_entry: bool,
    /// `[fe+0x17C]` — the back lock (0x4D5BA0 / 0x4D5BC0)
    pub back_lock: bool,
}

impl Default for Nav {
    fn default() -> Self {
        Self::new()
    }
}

impl Nav {
    /// A fresh manager: one empty layer (0x4D3420).
    pub fn new() -> Self {
        Self { layers: vec![Layer::default()], options_entry: false, back_lock: false }
    }

    /// A fresh manager with `screen` shown on its one layer and recorded — how 0x4D3420 opens
    /// the entry screen when nothing else is to be rebuilt (0x4D4035: `0x4D5DF0(vec[0], E, 1)`).
    pub fn with_screen(screen: &str) -> Self {
        let mut n = Self::new();
        n.show(0, screen, true);
        n
    }

    /// 0x4D5DF0 on layer `i`.
    fn show(&mut self, i: usize, name: &str, record: bool) {
        let l = &mut self.layers[i];
        if record {
            l.history.push(name.to_string());
        }
        l.screen = Some(name.to_string());
    }

    /// 0x4D5BE0 on `vec[0]` — the post-race rebuild's pushes.
    pub fn push_history(&mut self, name: &str) {
        if self.layers.is_empty() {
            self.layers.push(Layer::default());
        }
        self.layers[0].history.push(name.to_string());
    }

    /// 0x4D5DF0 on `vec[0]` with `record` 1 — the rebuild's closing open.
    pub fn show_first(&mut self, name: &str) -> Shown {
        if self.layers.is_empty() {
            self.layers.push(Layer::default());
        }
        self.show(0, name, true);
        self.shown()
    }

    /// **0x4D2100(`name`, `offset`, `record`)**.
    pub fn open(&mut self, name: &str, offset: i32, record: bool) -> Shown {
        if self.layers.is_empty() {
            self.layers.push(Layer::default());
        }
        if offset > 0 {
            self.layers.push(Layer::default());
            let top = self.layers.len() - 1;
            self.show(top, name, record);
        } else if offset < 0 && self.layers.len() >= 2 {
            self.layers.pop();
            let top = self.layers.len() - 1;
            self.show(top, name, record);
        } else {
            let top = self.layers.len() - 1;
            self.show(top, name, record);
        }
        self.shown()
    }

    /// **0x4D23F0(`arg`)** — the back.
    pub fn back(&mut self, arg: i32) -> Shown {
        let Some(top) = self.layers.last_mut() else { return Shown::Ended };
        if arg == -1 {
            top.history.clear();
            return self.close_top();
        }
        // 0x4D5FB0
        if top.history.is_empty() {
            return Shown::Nothing;
        }
        top.history.pop();
        match top.history.last().cloned() {
            Some(prev) => {
                top.screen = Some(prev.clone());
                Shown::Screen(prev)
            }
            None => self.close_top(),
        }
    }

    /// The top layer's screen closed with nothing to follow: the tick pops the layer, and the
    /// one under it (whose screen never went away) is on top again.
    fn close_top(&mut self) -> Shown {
        self.layers.pop();
        self.shown()
    }

    /// The screen on show on the top layer.
    pub fn shown(&self) -> Shown {
        match self.layers.last() {
            None => Shown::Ended,
            Some(l) => l.screen.clone().map_or(Shown::Nothing, Shown::Screen),
        }
    }

    /// The screen on show, by name.
    pub fn current(&self) -> Option<&str> {
        self.layers.last().and_then(|l| l.screen.as_deref())
    }

    /// **0x4D6320** — BACK's enable.
    pub fn back_enabled(&self) -> bool {
        if self.back_lock {
            return false;
        }
        match self.layers.as_slice() {
            [one] if one.history.len() != 1 => true,
            _ => self.options_entry,
        }
    }

    /// **0x4D63A0** — exactly one layer.
    pub fn one_layer(&self) -> bool {
        self.layers.len() == 1
    }

    /// 0x4D1E70 for the bottom bar: covered while more than one layer is open (0x4D1F4C).
    pub fn bottom_covered(&self) -> bool {
        self.layers.len() > 1
    }

    /// The whole state in one line, for logs: each layer's history, `*` before its screen.
    pub fn describe(&self) -> String {
        self.layers
            .iter()
            .map(|l| format!("[{}] *{}", l.history.join(" "), l.screen.as_deref().unwrap_or("-")))
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: &str) -> Shown {
        Shown::Screen(n.to_string())
    }

    #[test]
    fn offset_zero_keeps_history() {
        let mut n = Nav::with_screen("gamesetup");
        assert!(!n.back_enabled(), "one name on one layer");
        assert_eq!(n.open("singleplayer", 0, true), s("singleplayer"));
        assert!(n.back_enabled());
        assert_eq!(n.open("quickrace", 0, true), s("quickrace"));
        assert_eq!(n.layers[0].history, ["gamesetup", "singleplayer", "quickrace"]);
        assert_eq!(n.back(0), s("singleplayer"));
        assert_eq!(n.back(0), s("gamesetup"));
        assert!(!n.back_enabled());
        // one name left: 0x4D5FB0 pops it and closes the only layer
        assert_eq!(n.back(0), Shown::Ended);
    }

    #[test]
    fn offset_one_opens_a_layer_without_back() {
        let mut n = Nav::with_screen("gamesetup");
        n.open("tournament", 0, true);
        n.open("garage", 0, true);
        assert_eq!(n.open("carsetup", 1, true), s("carsetup"));
        assert!(!n.one_layer() && !n.back_enabled() && n.bottom_covered());
        n.open("carsetupdesc", 0, true);
        // the layer's own history: carsetupdesc → carsetup
        assert_eq!(n.back(0), s("carsetup"));
        // its last name: the layer closes and garage is on top again
        assert_eq!(n.back(0), s("garage"));
        assert!(n.one_layer() && n.back_enabled());
        assert_eq!(n.layers[0].history, ["gamesetup", "tournament", "garage"]);
    }

    #[test]
    fn offset_minus_one_closes_the_layer_and_opens_below() {
        let mut n = Nav::with_screen("prerace");
        n.open("repair", 1, true);
        assert_eq!(n.open("advancedrepair", -1, true), s("advancedrepair"));
        assert!(n.one_layer());
        assert_eq!(n.layers[0].history, ["prerace", "advancedrepair"]);
        // with one layer, offset −1 is a plain open (0x4D22DD falls to 0x4D235F)
        assert_eq!(n.open("x", -1, true), s("x"));
        assert_eq!(n.layers.len(), 1);
    }

    #[test]
    fn unrecorded_open_and_minus_one_back() {
        let mut n = Nav::with_screen("gamesetup");
        // SETTYPECAREERMODE's first time: careerfirst, not recorded
        n.open("careerfirst", 0, false);
        assert_eq!(n.current(), Some("careerfirst"));
        assert!(!n.back_enabled());
        n.open("tournament", 0, true);
        assert_eq!(n.back(0), s("gamesetup"));
        // 0x4D23F0(−1) empties the top layer's history and closes it
        let mut m = Nav::with_screen("gamesetup");
        m.open("selectname", 1, true);
        m.open("createplayer", 0, true);
        assert_eq!(m.back(-1), s("gamesetup"));
        assert!(m.one_layer());
        // an empty history: 0x4D5FB0 does nothing
        let mut e = Nav::new();
        e.open("a", 0, false);
        assert_eq!(e.back(0), Shown::Nothing);
    }

    #[test]
    fn options_entry_and_lock() {
        let mut n = Nav::with_screen("options");
        n.options_entry = true;
        assert!(n.back_enabled());
        n.back_lock = true;
        assert!(!n.back_enabled());
    }
}
