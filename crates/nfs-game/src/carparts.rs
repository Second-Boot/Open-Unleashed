//! The parts of a front-end `NFS5.3DCAR` that open and close — `FECar3DControl`'s click, slot 9
//! **0x4C0EB0**, and the per-frame step at **0x4C0696…0x4C07CF** (`RE/notes/controls.md` §4.153).
//!
//! The widget keeps, per zone 0…4, an **open state** 0…255 `[+0x3A4 + 4z]`, a **direction** ±1
//! `[+0x3B8 + 4z]`, a **start tick** `[+0x3CC + 4z]` and a **busy byte** `[+0x39C + z]`, plus the
//! two door bytes `[+0x1AE]` / `[+0x1AF]` and a zoom flag `[+0x1B0]`. Ticks are
//! `TIMER_getTicks()`, 0x53C2E0, **128 Hz**.
//!
//! What each zone moves:
//!
//! * **1, 2 — the doors.** 0x4C07ED…0x4C085A write `state·0.000588` (`[0x5B58E8]`, 0.15 turn at
//!   255) into the y of animation channel **6**, and `state·−0.000588` (`[0x5B58E4]`) into channel
//!   **7** — the channels `R3DCarLOD::draw` turns about the `.tpg` `dooraxis` (0x43CA5A).
//! * **3, 4 — the front and rear lids.** The click calls the car object's own open/close,
//!   0x439A00 / 0x439AA0 for the hood and 0x439B30 / 0x439BD0 for the engine lid: a 1000 ms easer
//!   toward `hoodextent` / `trunkextent` or 0, which slot 9 of the car writes into channels 8 / 9.
//! * **0 — the roof.** Its state feeds **0x4D1990(1, state)** after every step (0x4C07D9), the car
//!   model's animation 1; a click flips the `Car.Roof` accessor (0x4C0F8B…0x4C0FB5) and plays
//!   sound 0xD (0x4C106F), and the widget's tick starts the same travel when `Car.Roof` changes
//!   under it (0x4C05E7…0x4C063F).

/// What a click or a step asks of the car and the sound layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartEvent {
    /// front-end sound effect `n` (0x4AADB0)
    Sfx(u8),
    /// 0x439A00 (`true`) / 0x439AA0 (`false`): the hood's easer toward `hoodextent` or 0
    Hood(bool),
    /// 0x439B30 / 0x439BD0: the engine lid's, toward `trunkextent` or 0
    Trunk(bool),
    /// the click's `Car.Roof` write: 1 ("down") when the roof was up, else 0 (0x4C0F9E / 0x4C0FA7)
    Roof(bool),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarParts {
    pub state: [i32; 5],
    pub dir: [i32; 5],
    start: [u32; 5],
    pub busy: [bool; 5],
    /// `[+0x1AE]`, `[+0x1AF]` — a door that was clicked open; the next click on it zooms in
    pub door_flag: [bool; 2],
    /// `[+0x1B0]` — zoomed in on an open door; the widget holds focus meanwhile
    pub zoomed: bool,
    /// `[+0x1B4]` — the tick of the last zoom change, for the half-second debounce
    zoom_tick: u32,
    /// `[+0x140]` — the `Car.Roof` value the widget last saw; `None` before its bind
    roof_seen: Option<bool>,
}

impl CarParts {
    /// The click, slot 9 from 0x4C0F1A, for `zone` at tick `now`. `focused` is the widget's
    /// focus flag (0x4C7DD0); returns the new focus, and pushes what the car should do.
    pub fn click(&mut self, zone: u32, now: u32, focused: bool, out: &mut Vec<PartEvent>) -> bool {
        let z = zone as usize;
        if focused {
            // 0x4C0F28: a zoomed-in door lets go only after 64 ticks (0.5 s)
            if now.wrapping_sub(self.zoom_tick) <= 0x40 {
                return true;
            }
            self.zoomed = false;
            self.zoom_tick = now;
            self.door_flag[usize::from(zone != 1)] = false;
            return false;
        }
        if z > 4 || self.busy[z] {
            return focused;
        }
        let mut focus = focused;
        match z {
            // 0x4C0F87: the roof sets `Car.Roof` from its own state and remembers the value, so
            // the tick's change test (0x4C05F2) does not start it a second time
            0 => {
                let down = self.state[0] == 0;
                self.roof_seen = Some(down);
                out.push(PartEvent::Roof(down));
            }
            1 | 2 => {
                let p = usize::from(zone != 1);
                if self.door_flag[p] {
                    if now.wrapping_sub(self.zoom_tick) > 0x40 {
                        self.zoomed = true;
                        self.zoom_tick = now;
                        focus = true;
                    }
                } else if self.dir[z] != 1 {
                    self.door_flag[p] = true;
                }
            }
            _ => {}
        }
        if self.zoomed {
            return focus;
        }
        self.busy[z] = true;
        self.start[z] = now;
        self.dir[z] = if self.state[z] == 0 { 1 } else { -1 };
        let opening = self.dir[z] > 0;
        match z {
            0 => out.push(PartEvent::Sfx(0xD)),
            1 | 2 if opening => out.push(PartEvent::Sfx(0x30)),
            3 => {
                out.push(PartEvent::Hood(opening));
                if opening {
                    out.push(PartEvent::Sfx(0x32));
                }
            }
            4 => {
                out.push(PartEvent::Trunk(opening));
                if opening {
                    out.push(PartEvent::Sfx(0x34));
                }
            }
            _ => {}
        }
        focus
    }

    /// One frame of the step at 0x4C0696…0x4C07CF: each busy zone's state from the ticks since
    /// its start, and the closing sound when a part comes to rest shut.
    pub fn step(&mut self, now: u32, out: &mut Vec<PartEvent>) {
        for z in 0..5 {
            if !self.busy[z] {
                continue;
            }
            let e = now.wrapping_sub(self.start[z]) as i32;
            // `_ftol` truncates (0x5A0F98); the roof's `2e / 3` is an integer divide (0x4C06C3)
            let p = match z {
                0 => 2 * e / 3,
                1 | 2 => (e as f32 / if self.dir[z] > 0 { 0.302 } else { 0.389 }) as i32,
                3 => (e as f32 * 2.873_563_3) as i32,
                _ => (e as f32 / if self.dir[4] > 0 { 0.56 } else { 0.4 }) as i32,
            }
            .max(0);
            self.state[z] = if self.dir[z] > 0 { p } else { 0xFF - p };
            if self.state[z] <= 0 && self.dir[z] < 0 {
                self.state[z] = 0;
                self.busy[z] = false;
                // the closing sounds are zones 1…4's (the table at 0x4C08DC starts at zone 1)
                if z > 0 {
                    out.push(PartEvent::Sfx(match z {
                        1 | 2 => 0x31,
                        3 => 0x33,
                        _ => 0x35,
                    }));
                }
            }
            if self.state[z] >= 0xFF && self.dir[z] > 0 {
                self.state[z] = 0xFF;
                self.busy[z] = false;
            }
        }
    }

    /// `Car.Roof` as it stands, each tick: the widget's bind (0x4BFE71…0x4BFE86) takes a roof
    /// already down as fully travelled; afterwards a value that differs from the last seen starts
    /// the travel (`roof_changed`).
    pub fn sync_roof(&mut self, down: bool, now: u32, out: &mut Vec<PartEvent>) {
        match self.roof_seen {
            None => {
                self.state[0] = if down { 0xFF } else { 0 };
                self.roof_seen = Some(down);
            }
            Some(seen) if seen != down => {
                self.roof_seen = Some(down);
                self.roof_changed(now, out);
            }
            _ => {}
        }
    }

    /// The tick's arm for a `Car.Roof` changed from outside (0x4C05E7…0x4C063F): the roof starts
    /// travelling — up from 0, back from anything else — with sound 0xD.
    pub fn roof_changed(&mut self, now: u32, out: &mut Vec<PartEvent>) {
        self.busy[0] = true;
        self.start[0] = now;
        self.dir[0] = if self.state[0] == 0 { 1 } else { -1 };
        out.push(PartEvent::Sfx(0xD));
    }

    /// The room's zoom toward an open door, 0…1 — the widget's tick (0x4C0566…0x4C05C7):
    /// `(now − [+0x1B4]) / 64` clamped to 0…1, and `1 −` that while not zoomed, handed to the room's
    /// slot 0xB8 (0x4E7800 → `[room+0x2E8]`).
    pub fn zoom(&self, now: u32) -> f32 {
        let e = (now.wrapping_sub(self.zoom_tick) as i32 as f32 * 0.015625).clamp(0.0, 1.0);
        if self.zoomed { e } else { 1.0 - e }
    }

    /// Channels 6 and 7, in turns about `dooraxis` (0x4C07ED…0x4C085A).
    pub fn doors(&self) -> (f32, f32) {
        (self.state[1] as f32 * 0.000_588_235_3, self.state[2] as f32 * -0.000_588_235_3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The door zoom the room takes (0x4C0566…0x4C05C7): 0 at rest, rising over 64 ticks after a
    /// second click on an open door, falling back after the click that lets go.
    #[test]
    fn the_door_zoom_rises_and_falls_over_64_ticks() {
        let mut p = CarParts::default();
        let mut ev = Vec::new();
        assert_eq!(p.zoom(1000), 0.0);
        p.click(1, 1000, false, &mut ev); // opens the door, sets its byte
        p.step(1000 + 200, &mut ev);
        let focus = p.click(1, 1300, false, &mut ev); // zooms in
        assert!(focus && p.zoomed);
        assert_eq!(p.zoom(1300 + 32), 0.5);
        assert_eq!(p.zoom(1300 + 100), 1.0);
        p.click(1, 1500, true, &mut ev); // lets go
        assert!(!p.zoomed);
        assert_eq!(p.zoom(1500 + 16), 0.75);
        assert_eq!(p.zoom(1500 + 64), 0.0);
    }

    /// The roof, zone 0: a click flips `Car.Roof` with sound 0xD, travels `2e/3` a tick and rests
    /// without a closing sound; a changed accessor starts the same travel (0x4C05E7…).
    #[test]
    fn the_roof_travels_and_follows_car_roof() {
        let mut p = CarParts::default();
        let mut ev = Vec::new();
        p.sync_roof(false, 0, &mut ev);
        assert!(ev.is_empty() && p.state[0] == 0);
        p.click(0, 100, false, &mut ev);
        assert_eq!(ev, vec![PartEvent::Roof(true), PartEvent::Sfx(0xD)]);
        ev.clear();
        p.step(100 + 30, &mut ev);
        assert_eq!(p.state[0], 20);
        p.step(100 + 400, &mut ev);
        assert_eq!((p.state[0], p.busy[0]), (0xFF, false));
        // the accessor now reads "down", which the click already recorded: no second start
        p.sync_roof(true, 600, &mut ev);
        assert!(ev.is_empty());
        // flipped back from outside: the travel back starts with 0xD and rests silently
        p.sync_roof(false, 700, &mut ev);
        assert_eq!(ev, vec![PartEvent::Sfx(0xD)]);
        ev.clear();
        p.step(700 + 400, &mut ev);
        assert_eq!((p.state[0], p.busy[0]), (0, false));
        assert!(ev.is_empty());
    }

    /// A door opens in 0.302·255 ticks with its sound, and closes in 0.389·255 with the other;
    /// the second click on an open door zooms instead of closing it.
    #[test]
    fn a_door_opens_zooms_and_closes() {
        let mut c = CarParts::default();
        let mut ev = Vec::new();
        let f = c.click(1, 1000, false, &mut ev);
        assert!(!f);
        assert_eq!(ev, vec![PartEvent::Sfx(0x30)]);
        c.step(1000 + 77, &mut ev);
        assert_eq!(c.state[1], 254);
        c.step(1000 + 78, &mut ev);
        assert_eq!((c.state[1], c.busy[1]), (255, false));
        assert!((c.doors().0 - 0.15).abs() < 1e-4);
        // open door, flag set: the next click zooms (focus) rather than closing
        ev.clear();
        let f = c.click(1, 2000, false, &mut ev);
        assert!(f && c.zoomed && ev.is_empty());
        // a focused click within half a second is ignored, after it lets go and clears the flag
        assert!(c.click(1, 2030, true, &mut ev));
        assert!(!c.click(1, 2100, true, &mut ev));
        assert!(!c.zoomed && !c.door_flag[0]);
        // and now the click closes it
        c.click(1, 3000, false, &mut ev);
        assert_eq!(c.dir[1], -1);
        c.step(3000 + 100, &mut ev);
        assert_eq!((c.state[1], c.busy[1]), (0, false));
        assert_eq!(ev.last(), Some(&PartEvent::Sfx(0x31)));
    }

    #[test]
    fn the_lids_drive_the_car_easers() {
        let mut c = CarParts::default();
        let mut ev = Vec::new();
        c.click(3, 0, false, &mut ev);
        assert_eq!(ev, vec![PartEvent::Hood(true), PartEvent::Sfx(0x32)]);
        c.step(89, &mut ev);
        assert!(!c.busy[3]);
        ev.clear();
        c.click(3, 200, false, &mut ev);
        assert_eq!(ev, vec![PartEvent::Hood(false)]);
    }
}
