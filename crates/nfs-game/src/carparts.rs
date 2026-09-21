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
//! * **0 — the roof.** Its step feeds 0x4D1990(1, state), a model property this port has not read;
//!   it is left out (recorded in §4.153).

/// What a click or a step asks of the car and the sound layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartEvent {
    /// front-end sound effect `n` (0x4AADB0)
    Sfx(u8),
    /// 0x439A00 (`true`) / 0x439AA0 (`false`): the hood's easer toward `hoodextent` or 0
    Hood(bool),
    /// 0x439B30 / 0x439BD0: the engine lid's, toward `trunkextent` or 0
    Trunk(bool),
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
            // the roof — not ported (see the module comment)
            0 => return focus,
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
        for z in 1..5 {
            if !self.busy[z] {
                continue;
            }
            let e = now.wrapping_sub(self.start[z]) as i32;
            // `_ftol` truncates (0x5A0F98)
            let p = match z {
                1 | 2 => (e as f32 / if self.dir[z] > 0 { 0.302 } else { 0.389 }) as i32,
                3 => (e as f32 * 2.873_563_3) as i32,
                _ => (e as f32 / if self.dir[4] > 0 { 0.56 } else { 0.4 }) as i32,
            }
            .max(0);
            self.state[z] = if self.dir[z] > 0 { p } else { 0xFF - p };
            if self.state[z] <= 0 && self.dir[z] < 0 {
                self.state[z] = 0;
                self.busy[z] = false;
                out.push(PartEvent::Sfx(match z {
                    1 | 2 => 0x31,
                    3 => 0x33,
                    _ => 0x35,
                }));
            }
            if self.state[z] >= 0xFF && self.dir[z] > 0 {
                self.state[z] = 0xFF;
                self.busy[z] = false;
            }
        }
    }

    /// Channels 6 and 7, in turns about `dooraxis` (0x4C07ED…0x4C085A).
    pub fn doors(&self) -> (f32, f32) {
        (self.state[1] as f32 * 0.000_588_235_3, self.state[2] as f32 * -0.000_588_235_3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
