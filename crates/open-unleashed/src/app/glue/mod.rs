//! **TEST GLUE** (not in the original): the scripted runs' state - the headless flag, the autotest drive, the screenshot target, the layout sweep, the scripted session.

pub(crate) mod scripted;
pub(crate) mod pa_trace;
pub(crate) mod fe_seq;

/// **TEST GLUE** (not in the original): the scripted runs' state - the headless flag, the autotest drive, the screenshot target, the layout sweep, the scripted session.
pub(crate) struct Glue {
    /// headless run (`--autotest`): the bot may reset itself when stuck
    pub(crate) scripted: bool,
    pub(crate) stuck_ticks: u32,
    /// scripted drive before the screenshot: seconds of full throttle (steer right for the last third)
    pub(crate) autotest: Option<f32>,
    pub(crate) screenshot: Option<String>,
    /// **TEST GLUE** (not in the original): a still's render target, `(physical W, physical H,
    /// scale)` from `NFS_SHOT_SIZE=WxH` (default 1280x800) and `NFS_SHOT_SCALE=<s>` (default 1) —
    /// `Some` in a `--screenshot` / `NFS_RACE_LOADING` / `NFS_LOADING_SHOT` run, where the still is
    /// rendered at that size whatever the window is. The HUD and the loading picture are built for it.
    pub(crate) shot_target: Option<(u32, u32, f32)>,
    /// **TEST GLUE** (`NFS_SHOT_SETTLE`, a `--screenshot` still): milliseconds added to the lamps' clock, as if that much
    /// wall time of drawn frames had followed the still's tick; 0 otherwise
    pub(crate) shot_settle_ms: f64,
    /// `--layout-sweep`: load every `.lay` in the game, draw it once, and report every widget that
    /// produced no sprite at all. The port's own measure of what the front end does not draw.
    pub(crate) layout_sweep: bool,
    /// **TEST GLUE**: the interpolation fraction `NFS_SHOT_SEQ` sets for each of its sub-tick frames
    pub(crate) test_alpha: Option<f32>,
    /// diagnostic: `NFS_AUTOTEST_RACES=n` — a scripted session of `n` races, each `--autotest`
    /// seconds long: race → the automatic replay over it → leave → the next race (the session
    /// loop 0x4B6D88 and the front end's round trip), so the second race's `--record` rows can be
    /// compared with a fresh run's. What is left of the session, this race included.
    pub(crate) session_races: u32,
}
