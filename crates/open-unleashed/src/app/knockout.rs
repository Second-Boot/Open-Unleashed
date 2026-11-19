//! The knockout series' state: the series, the round flag, the quick knockout, the player's elimination, the opponents' cars and the grid override.

use super::*;

/// The knockout series' state: the series, the round flag, the quick knockout, the player's elimination, the opponents' cars and the grid override.
pub(crate) struct KnockoutState {
    pub(crate) knockout: Option<KnockoutSeries>,
    /// the race running is a knockout round (`Game.GameType` 2 at the launch, `[0x690AD4]` at its
    /// end) — set by `knockout_round`, taken by the race's end
    pub(crate) knockout_round: bool,
    /// Quick Knockout (`RACE_TYPE 3`): laps = opponents, the last car is out on every lap.
    pub(crate) quick_knockout: bool,
    pub(crate) player_eliminated: bool,
    /// `FEOpponentCarAccessor`'s seven embedded cars (`this + 0x24 + i·0x760`, `modes.md` §1.1a):
    /// what the last 0x4F4FA0 drew — each opponent's record and appearance. Refilled by every
    /// quick race's set-up (0x4EBC2D) and the knockout start (0x4DDF58), loaded with the profile
    /// (0x4F56F0); a knockout round takes entry `i − 1` for opponent `i` (0x4DE1E7). Empty until
    /// the first draw: the constructor's own draw (0x4F4EA8, in 0x4F4E50) is not made, and that is
    /// safe — the only readers of the seven cars are 0x4EBBA0 (0x4EBC8B, after its own draw at
    /// 0x4EBC2D), the knockout round 0x4DE150 (0x4DE1E7, after the series' draw 0x4DDF58) and the
    /// profile save 0x4F56C0 (which would write the constructor's clock-seeded random cars where
    /// the port writes none until a race has drawn them; xrefs read 2026-09-30).
    pub(crate) opp_cars: Vec<nfs_game::save::OpponentCarSave>,
    pub(crate) player_grid_override: Option<u32>,
}
