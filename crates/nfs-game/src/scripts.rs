//! The AI script runner (`RE/notes/traffic.md` §30): `0x40AAD0` init, `0x40AB30` request,
//! `0x40AB80` step, `0x40AC60` query.
//!
//! A car can have one script running at a time. A trigger *requests* a script id; the lowest id wins
//! and a request is refused outright while one is running. Each activation walks the row's four actions
//! from a **persistent cursor**, skipping the zero-duration ones (whose bits it still collects into the
//! mask) and stopping at the first timed one — so a four-action script is spread over up to four
//! separate trigger events. The cursor only rewinds after twenty seconds with no request at all, and an
//! `END` is a wall: once the cursor reaches the action before it, that action repeats for ever.
//!
//! Only six actions have a consumer in this build: `SPEED_BURST` multiplies the accel rate by 1.5,
//! `HONK_HORN` and `CRAZY_HORN` drive the horn, `SWERVE` and `SWIPE` push the lane scores, and
//! `WIPE_OUT` forces the out-of-control controller. `HARD_BRAKE`, `ATTACK`, `REAR_END` and `MOVE_AWAY`
//! are parsed and do nothing.

use nfs_formats::scripts::{action, Scripts};

/// `script+0x08 == 7` means idle, and 7 is also "no pending request".
pub const IDLE: u8 = 7;
/// `0x40AB30` — the cursors rewind after this many game ticks with no request (20 s at 64 Hz).
pub const CURSOR_RESET_TICKS: i64 = 0x500;

/// The seven triggers, in the order `scripts.bin` stores them (§12.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Collision = 0,
    OpponentWhizzedBy = 1,
    HumanBlocksOpponent = 2,
    OpponentBlocksHuman = 3,
    OpponentPassesHuman = 4,
    HumanPassesOpponent = 5,
    HumanHonks = 6,
}

/// `car+0xE3C` — the per-car runner.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptRunner {
    /// the personality row, `PERSONALITY % 5`
    pub row: usize,
    pending: u8,
    pending_target: i32,
    running: u8,
    running_target: i32,
    /// `script+0x10`, starts at −1
    action_idx: i32,
    /// `script+0x14`, the cumulative mask of the running activation, seeded to 1
    mask: u32,
    /// `script+0x18`, in 1/32 s units
    timer: i32,
    /// `script+0x20..0x38`, one persistent action cursor per script id
    cursor: [i32; 7],
    last_request_tick: i64,
}

impl ScriptRunner {
    pub fn new(row: usize) -> Self {
        Self { row: row % 5, pending: IDLE, pending_target: -1, running: IDLE, running_target: -1, action_idx: -1, mask: 1, timer: 0, cursor: [-1; 7], last_request_tick: i64::MIN / 4 }
    }

    pub fn is_idle(&self) -> bool {
        self.running == IDLE
    }

    /// `script+8`, the id of the running script (`None` when idle)
    pub fn running(&self) -> Option<u8> {
        (self.running != IDLE).then_some(self.running)
    }

    /// `0x40AB30(script, targetIdx, id, tick)` — the lowest id seen while idle wins, and any request at
    /// all refreshes the cursor-reset clock.
    pub fn request(&mut self, target: i32, trigger: Trigger, tick: i64) {
        if tick - CURSOR_RESET_TICKS > self.last_request_tick {
            self.cursor = [-1; 7];
        }
        self.last_request_tick = tick;
        if self.running != IDLE {
            return;
        }
        let id = trigger as u8;
        if id < self.pending {
            self.pending = id;
            self.pending_target = target;
        }
    }

    /// `0x40AB80(script, dt)` — one step, `dt` in 1/32 s units (the AI-high interval).
    pub fn step(&mut self, table: &Scripts, dt: i32) {
        if self.running == IDLE && self.pending != IDLE {
            let id = self.pending;
            self.running = id;
            self.pending = IDLE;
            self.running_target = self.pending_target;
            self.timer = 0;
            self.action_idx = self.cursor[id as usize];
            self.mask = 1;
            let s = id as usize;
            loop {
                let i = self.action_idx;
                if i + 1 < 4 && table.action_at(self.row, s, i + 1) != action::END {
                    self.action_idx = i + 1;
                    self.cursor[s] += 1;
                }
                let i = self.action_idx;
                let a = table.action_at(self.row, s, i);
                self.mask |= 1 << a;
                let d = table.duration_at(self.row, s, i);
                let timed = d != 0;
                if timed {
                    self.timer = (d as i32) << 4;
                }
                // a script whose first action is `END` collects nothing and goes straight back to idle
                if self.mask == 1 {
                    self.running = IDLE;
                    self.pending = IDLE;
                    break;
                }
                if timed {
                    break;
                }
            }
        }
        if self.running == IDLE {
            return;
        }
        if self.timer <= 0 {
            self.running = IDLE;
            self.pending = IDLE;
            return;
        }
        self.timer -= dt;
    }

    /// `0x40AC60(script, mask)` — the index of the car that triggered the running script, when its
    /// cumulative mask includes one of the bits asked for.
    pub fn query(&self, mask: u32) -> Option<i32> {
        if self.running == IDLE || (mask & self.mask) == 0 {
            return None;
        }
        Some(self.running_target)
    }

    /// `0x40AC80(script)` — the remaining time in 1/32 s units, 0 when idle. The SWERVE consumer uses
    /// its parity to weave at 1 Hz.
    pub fn timer(&self) -> i32 {
        if self.running == IDLE { 0 } else { self.timer }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_formats::scripts::{mask, ROWS, SCRIPTS};

    /// Row 0 script 0 as the shipped file has it: honk for a second, then end.
    fn nemesis() -> Scripts {
        let mut s = Scripts::default();
        let base = 0;
        s.table[base] = action::HONK_HORN;
        s.table[base + 4] = 2;
        s.table[base + 1] = action::END;
        // script 5: pause a second, then swipe for two
        let b5 = 5 * 8;
        s.table[b5] = action::PAUSE;
        s.table[b5 + 4] = 2;
        s.table[b5 + 1] = action::SWIPE;
        s.table[b5 + 5] = 4;
        s.table[b5 + 2] = action::END;
        let _ = (ROWS, SCRIPTS);
        s
    }

    #[test]
    fn a_request_runs_one_timed_action_and_expires() {
        let t = nemesis();
        let mut r = ScriptRunner::new(0);
        assert!(r.is_idle());
        r.request(3, Trigger::Collision, 1000);
        r.step(&t, 2);
        // the horn is running, and the query hands back who triggered it
        assert_eq!(r.query(mask::HONK_HORN), Some(3));
        assert_eq!(r.query(mask::SWIPE), None, "only the bits this activation collected");
        // one second at 1/32 s per unit, stepped two units at a time
        for _ in 0..16 {
            r.step(&t, 2);
        }
        assert!(r.is_idle(), "the script ends when its timer runs out");
        assert_eq!(r.query(mask::HONK_HORN), None);
    }

    #[test]
    fn a_script_continues_where_it_left_off() {
        let t = nemesis();
        let mut r = ScriptRunner::new(0);
        // script 5 is PAUSE 1 s then SWIPE 2 s, spread over two separate triggers
        r.request(1, Trigger::HumanPassesOpponent, 1000);
        r.step(&t, 2);
        assert_eq!(r.query(mask::SWIPE), None, "the first activation only pauses");
        for _ in 0..17 {
            r.step(&t, 2);
        }
        assert!(r.is_idle());
        // the second trigger picks up at the swipe
        r.request(1, Trigger::HumanPassesOpponent, 1100);
        r.step(&t, 2);
        assert_eq!(r.query(mask::SWIPE), Some(1));
    }

    #[test]
    fn nothing_interrupts_a_running_script_and_the_lowest_id_wins() {
        let t = nemesis();
        let mut r = ScriptRunner::new(0);
        // two requests while idle: the lower id is kept
        r.request(1, Trigger::HumanPassesOpponent, 1000);
        r.request(2, Trigger::Collision, 1000);
        r.step(&t, 2);
        assert_eq!(r.query(mask::HONK_HORN), Some(2), "the collision script won");
        // a request while busy is refused outright
        r.request(9, Trigger::HumanPassesOpponent, 1001);
        r.step(&t, 2);
        assert_eq!(r.query(mask::HONK_HORN), Some(2));
    }

    #[test]
    fn an_all_end_script_does_nothing() {
        let t = Scripts::default();
        let mut r = ScriptRunner::new(0);
        r.request(1, Trigger::Collision, 1000);
        r.step(&t, 2);
        assert!(r.is_idle());
        assert_eq!(r.query(0xFFFF_FFFF), None);
    }

    #[test]
    fn the_cursors_rewind_after_twenty_quiet_seconds() {
        let t = nemesis();
        let mut r = ScriptRunner::new(0);
        r.request(1, Trigger::HumanPassesOpponent, 1000);
        r.step(&t, 2);
        for _ in 0..20 {
            r.step(&t, 2);
        }
        // a request well after the quiet window starts the script from the top again
        r.request(1, Trigger::HumanPassesOpponent, 1000 + CURSOR_RESET_TICKS + 1);
        r.step(&t, 2);
        assert_eq!(r.query(mask::SWIPE), None, "back at the pause");
    }
}
