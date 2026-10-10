//! The race HUD's `FEStrings` ids (the HUD string table, 18100…18194), named after what
//! `RE/notes/hud.md` records for each; the values are the ones the original pushes at the
//! addresses given.

/// 18100: "Esc to exit", the bottom strip's left hint (0x41AC56, hud.md)
pub(crate) const HINT_ESC: u32 = 18100;
/// 18101: "Spacebar to proceed", the bottom strip's right hint
pub(crate) const HINT_SPACE: u32 = 18101;
/// 18108: the pause menu's first button (HPauseMenu 0x42A440 stacks 18108 / 18109 / 18110)
pub(crate) const PAUSE_BUTTON_1: u32 = 18108;
/// 18109: the pause menu's second button (also the mission summary page's button)
pub(crate) const PAUSE_BUTTON_2: u32 = 18109;
/// 18110: the pause menu's third button
pub(crate) const PAUSE_BUTTON_3: u32 = 18110;
/// 18111: "NAME" column header
pub(crate) const COL_NAME: u32 = 18111;
/// 18112: "CAR" column header
pub(crate) const COL_CAR: u32 = 18112;
/// 18113: "BEST LAP" column header
pub(crate) const COL_BEST_LAP: u32 = 18113;
/// 18114: "TIME" column header
pub(crate) const COL_TIME: u32 = 18114;
/// 18115: "POINTS" column header
pub(crate) const COL_POINTS: u32 = 18115;
/// 18127: "TOTAL POINTS" (HTournamentStandings 0x431860)
pub(crate) const COL_TOTAL_POINTS: u32 = 18127;
/// 18161: "WINNINGS" (HTournamentStandings)
pub(crate) const COL_WINNINGS: u32 = 18161;
/// 18162: "RALLY TIME" (HTournamentStandings, a timed event)
pub(crate) const COL_RALLY_TIME: u32 = 18162;
/// 18128: the pause billboard's title (0x42A440)
pub(crate) const TITLE_PAUSE: u32 = 18128;
/// 18129: "Single Race" (0x42BCDF, by game type)
pub(crate) const TITLE_SINGLE_RACE: u32 = 18129;
/// 18130: "Race Summary" (HRaceSummary 0x42BCD0)
pub(crate) const TITLE_RACE_SUMMARY: u32 = 18130;
/// 18132: "Tournament Standings" (0x431860)
pub(crate) const TITLE_TOURNAMENT_STANDINGS: u32 = 18132;
/// 18134: the mission summary billboard's left title
pub(crate) const TITLE_MISSION_LEFT: u32 = 18134;
/// 18135: the mission summary billboard's right title
pub(crate) const TITLE_MISSION_RIGHT: u32 = 18135;
/// 18136: HReplaySave's billboard title (0x42E040)
pub(crate) const TITLE_REPLAY_SAVE: u32 = 18136;
/// 18157: "Race Info" (HInfo)
pub(crate) const TITLE_RACE_INFO: u32 = 18157;
/// 18169: "Knockout" (0x42BCDF, by game type)
pub(crate) const TITLE_KNOCKOUT: u32 = 18169;
/// 18174: the yes / no overlay's billboard title
pub(crate) const TITLE_YES_NO: u32 = 18174;
/// 18175: "Quick Knockout" (0x42BCDF, by game type)
pub(crate) const TITLE_QUICK_KNOCKOUT: u32 = 18175;
/// 18137: HReplaySave's first button
pub(crate) const REPLAY_SAVE_BUTTON_1: u32 = 18137;
/// 18138: HReplaySave's second button
pub(crate) const REPLAY_SAVE_BUTTON_2: u32 = 18138;
/// 18170: HReplaySave's name-field label
pub(crate) const REPLAY_SAVE_LABEL: u32 = 18170;
/// 18139: HYesNoOverlay text line 1 (the name exists)
pub(crate) const OVERWRITE_LINE_1: u32 = 18139;
/// 18140: HYesNoOverlay text line 2
pub(crate) const OVERWRITE_LINE_2: u32 = 18140;
/// 18141: HYesNoOverlay "yes" button
pub(crate) const YES_NO_YES: u32 = 18141;
/// 18142: HYesNoOverlay "no" button
pub(crate) const YES_NO_NO: u32 = 18142;
/// 18171: the quit confirmation's text line (0x4376B0 passes 18171)
pub(crate) const QUIT_CONFIRM_LINE: u32 = 18171;
/// 18144: "rpm", HTachDigital's label (0x42F862)
pub(crate) const TACH_RPM: u32 = 18144;
/// 18145: "R", HGear 0 (0x421CB0)
pub(crate) const GEAR_REVERSE: u32 = 18145;
/// 18146: "N", HGear 1
pub(crate) const GEAR_NEUTRAL: u32 = 18146;
/// 18159: "WRONG WAY" (HWrongWay 0x436F70)
pub(crate) const WRONG_WAY: u32 = 18159;
/// 18165: the winnings panel's first label string (read as a format, 0x5CD240)
pub(crate) const PRIZE_LABEL: u32 = 18165;
/// 18166: "Safe Bonus"
pub(crate) const PRIZE_SAFE_BONUS: u32 = 18166;
/// 18167: "Tournament Bonus"
pub(crate) const PRIZE_TOURNAMENT_BONUS: u32 = 18167;
/// 18168: "Total"
pub(crate) const PRIZE_TOTAL: u32 = 18168;
/// 18191: "DNF" (0x42B2B6)
pub(crate) const DNF: u32 = 18191;
/// 18192: "Leader" (HLeader 0x428635)
pub(crate) const LEADER: u32 = 18192;
