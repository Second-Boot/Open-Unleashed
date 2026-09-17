//! `Simulation/AI/scripts.bin` — the AI action scripts (`RE/notes/traffic.md` §30).
//!
//! A `u32` header the loader discards, then `9 × 7 × 4` pairs of `{u32 action, u32 duration}` truncated
//! to bytes. In memory the original keeps them as `u8 table[9][7][8]` at `0x5E6B08` with the four
//! actions at `+0..3` and the four durations at `+4..7`, and this parser keeps the same flat 504-byte
//! block so the runner's signed cursor can reproduce the original's off-by-one read.
//!
//! Nine personality rows (Nemesis, Blurrr, Zippy, JetBoy, Menace, Cop1..3, Traffic) × seven scripts,
//! one per trigger: collision, opponent whizzed by, human blocks opponent, opponent blocks human,
//! opponent passes human, human passes opponent, human honks.

use crate::{u32_le, FormatError};

/// Action ids, from the shipped `SCRIPTS.CSV`.
pub mod action {
    pub const INIT: u8 = 0;
    pub const END: u8 = 1;
    pub const SPEED_BURST: u8 = 2;
    pub const HONK_HORN: u8 = 3;
    pub const HARD_BRAKE: u8 = 4;
    pub const SWIPE: u8 = 5;
    pub const ATTACK: u8 = 6;
    pub const PAUSE: u8 = 7;
    pub const REAR_END: u8 = 8;
    pub const SWERVE: u8 = 9;
    pub const MOVE_AWAY: u8 = 10;
    pub const WIPE_OUT: u8 = 11;
    pub const CRAZY_HORN: u8 = 12;
}

/// The mask bits the six live consumers test (`1 << action`).
pub mod mask {
    pub const SPEED_BURST: u32 = 1 << 2;
    pub const HONK_HORN: u32 = 1 << 3;
    pub const SWIPE: u32 = 1 << 5;
    pub const SWERVE: u32 = 1 << 9;
    pub const WIPE_OUT: u32 = 1 << 11;
    pub const CRAZY_HORN: u32 = 1 << 12;
}

pub const ROWS: usize = 9;
pub const SCRIPTS: usize = 7;
/// 9 rows × 7 scripts × 8 bytes.
pub const TABLE_BYTES: usize = ROWS * SCRIPTS * 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scripts {
    /// the flat `u8[9][7][8]` block, actions at `+0..3` and durations at `+4..7`
    pub table: Vec<u8>,
}

impl Default for Scripts {
    /// Every script `END`, i.e. no reactions at all.
    fn default() -> Self {
        let mut table = vec![0u8; TABLE_BYTES];
        for r in 0..ROWS {
            for s in 0..SCRIPTS {
                for a in 0..4 {
                    table[r * 56 + s * 8 + a] = action::END;
                }
            }
        }
        Self { table }
    }
}

impl Scripts {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let what = "scripts.bin";
        let mut table = vec![0u8; TABLE_BYTES];
        // the first dword is read and thrown away (0x407F80)
        let mut p = 1usize;
        for r in 0..ROWS {
            for s in 0..SCRIPTS {
                for a in 0..4 {
                    let base = r * 56 + s * 8;
                    table[base + a] = u32_le(data, p * 4, what)? as u8;
                    table[base + 4 + a] = u32_le(data, (p + 1) * 4, what)? as u8;
                    p += 2;
                }
            }
        }
        Ok(Self { table })
    }

    /// The action byte at a **signed** index inside a script, reproducing the original's read one byte
    /// before the record when the cursor is still −1. Off the front of the block it reads 0 (`INIT`),
    /// which is what the original's neighbour normally supplies.
    pub fn action_at(&self, row: usize, script: usize, i: i32) -> u8 {
        let idx = (row * 56 + script * 8) as i32 + i;
        if idx < 0 { action::INIT } else { self.table.get(idx as usize).copied().unwrap_or(action::END) }
    }

    /// The duration byte for the same signed index (`+4` from the action).
    pub fn duration_at(&self, row: usize, script: usize, i: i32) -> i8 {
        let idx = (row * 56 + script * 8) as i32 + 4 + i;
        if idx < 0 { 0 } else { self.table.get(idx as usize).copied().unwrap_or(0) as i8 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(rows: &[[[u8; 8]; SCRIPTS]]) -> Vec<u8> {
        let mut d = vec![0u8; 4];
        for r in rows {
            for s in r {
                for a in 0..4 {
                    d.extend_from_slice(&(s[a] as u32).to_le_bytes());
                    d.extend_from_slice(&(s[4 + a] as u32).to_le_bytes());
                }
            }
        }
        d
    }

    #[test]
    fn parses_the_flat_table() {
        let mut rows = [[[action::END, action::END, action::END, action::END, 0, 0, 0, 0]; SCRIPTS]; ROWS];
        // row 0 script 0 = honk 1 s then end, as the shipped Nemesis row has it
        rows[0][0] = [action::HONK_HORN, action::END, action::HONK_HORN, action::END, 2, 0, 2, 0];
        let s = Scripts::parse(&build(&rows)).unwrap();
        assert_eq!(s.action_at(0, 0, 0), action::HONK_HORN);
        assert_eq!(s.duration_at(0, 0, 0), 2, "two halves of a second");
        assert_eq!(s.action_at(0, 0, 1), action::END);
        // the signed read one before the record is the previous record's last duration
        assert_eq!(s.action_at(0, 0, -1), action::INIT, "off the front of the block");
        assert_eq!(s.action_at(1, 0, -1), s.table[55], "the previous record's duration[3]");
        // a short file is an error, not a panic
        assert!(Scripts::parse(&[0u8; 16]).is_err());
    }
}
