//! The HUD's state: the slide-in / key / score pages, the scripted key, bar and hover feeds, the pointer, the cones and records.

use super::*;

/// The HUD's state: the slide-in / key / score pages, the scripted key, bar and hover feeds, the pointer, the cones and records.
pub(crate) struct HudState {
    /// The race HUD's slide states of its element groups: `(state, [0x606AC8] stamp of the last transition)`
    pub(crate) slides: HudSlides,
    /// what the HUD toggle keys F5 / F6 / F7 / F8 / "2" change (`HudKeys`)
    pub(crate) hud_keys: HudKeys,
    /// diagnostic: `NFS_HUD_KEYS=tick:key,…` (key `F5` `F6` `F7` `F8` `2` `pause` or
    /// `pauserestart`) — the HUD toggle keys pressed on those game ticks (`hud_key`), for a
    /// `--screenshot` of their effect
    pub(crate) hud_key_script: std::collections::VecDeque<(u64, ScriptedHudKey)>,
    /// **TEST GLUE**: the state of the scripted `NFS_HUD_KEYS` / `NFS_AUTOTEST_RACES` runs
    pub(crate) scripted_hud: ScriptedHud,
    /// diagnostic: `NFS_REPLAY_PRESS=tick:cell,…` — the pointer put on a replay bar cell and pressed
    /// once the playback reaches that tick, for scripted runs (`App::run_bar_script`); cells 7 and
    /// 8 are `HReplaySave`'s two buttons, 9 and 10 the overwrite question's yes and no
    pub(crate) bar_script: Vec<(u64, usize)>,
    /// diagnostic: `NFS_REPLAY_HOVER=tick:cell,…` — the pointer put in a replay bar cell's middle
    /// (0…6) without a press, or off the bar (`off`: the screen's top-left corner), once the
    /// playback reaches that tick — for stills of the bar's hover, tooltip and auto-hide
    pub(crate) hover_script: Vec<(u64, Option<usize>)>,
    /// The in-race dialog button under the pointer as the last draw found it — `HButtonVisual`'s
    /// `+0x100` (0x41B3CE): `(dialog, button)`, dialog 0 the pause menu, 1 the quit question, 2 the
    /// mission summary. A release reads it (0x41B300), as the original's does.
    pub(crate) hud_hover: Option<(u8, usize)>,
    /// The mission HUD's cone count, `[0x60CA04]` and `HMissionSummary+0x140` / `HTime+0xE4`
    /// (`nfs_game::skill::HudCones`, 0x425950)
    pub(crate) hud_cones: nfs_game::skill::HudCones,
    /// `HTime +0xE0`: set to 16 on a cone hit (0x431830) and counted down once per HUD draw,
    /// i.e. per rendered frame (`HTime` draw 0x4316F0, 0x4317C9…0x4317E0 — reached whether or not
    /// the element is shown); the time shows in the alert colour while it is not 0 (0x431730)
    pub(crate) cone_flash: u32,
    /// The post-race records table (`HTrackRecords`, `nfs_game::records::hud_records`), built as
    /// the result pages are (0x433A20 at 0x426C4F); `None` when the race has none to show.
    pub(crate) hud_records: Option<nfs_game::records::HudRecords>,
    /// `car+0xB54` per racer (player first): metres covered, `fistp(min(car+0xAF8, NUM_LAPS) ·
    /// [[0x628BC0]+4] · 6.0)` — refreshed only when `gGameTick % 64 == 1` and there is more than
    /// one racer (0x417010…0x4170A2); `HPosition`'s distance / time gaps are taken from it
    pub(crate) hud_metres: Vec<i32>,
    /// What `hud_clock` adds so that `[0x606AC8]` is **kept across a pass**: the clock it is set
    /// from, `[0x5E99B8]`, is zeroed only when a session installs the timer (0x4A4700 → 0x4A4752;
    /// 0x4A4760 removes it at the session's end) and runs on through every pass, and both the pass
    /// setup (0x414F2B…0x414F3B) and `RaceFrame`'s entry (0x4152EB / 0x415312) set `[0x606AC8]` to
    /// it. So a restart or the automatic replay goes on from the clock the last pass reached
    /// (`+= tick` there), and a new session starts it at 0 (`= −pause_offset`). PORT-DEVIATION D13
    /// (PORT-DEVIATIONS.md) [`[0x5E99B8]` installed at 0x4A4752, before the load 0x468FB0]: the
    /// timer also counts the wall time the original spends loading the session's track (installed
    /// before 0x468FB0) and setting a pass up, which this port does not model — its own loading is
    /// not the original's.
    pub(crate) hud_clock_base: f64,
    /// the window's last seen scale factor ([`App::hud_geometry`]): `start_race` takes the renderer AND the window
    /// out of the `App` for the whole load, and the loading screen drawn meanwhile needs both (LOADSEAM)
    pub(crate) hud_scale: std::cell::Cell<f32>,
    /// The profile's race-HUD option tables (`<Name>1.hud` / `<Name>2.hud`, [`hud_cfg`]), loaded lazily: the
    /// 14 Display options live in the record of (`Display.ScreenLayout`, resolution).
    pub(crate) hud_cfg: std::cell::RefCell<HudCfgCache>,
    /// `HWrongWay`'s blink: `+0x14` ticks counted, `+0x18` the tick of the last draw, `+0x24`
    /// visible. Every draw adds the ticks since the last one, and the visible half flips off once
    /// the count passes 64, the hidden half back on once it passes 32, the count restarting
    /// (0x436F86…0x436FBD). Only the HUD reset 0x426A80 → 0x436F60 zeroes it (visible, 0, 0) —
    /// nothing does when the car turns round — so the first draw after a reset, at a tick past
    /// 448, flips straight to hidden. This port blinked on `tick % 96 < 64`.
    pub(crate) wrong_way_blink: (u64, u64, bool),
    /// The post-race page that is up and what its billboard animation counts
    pub(crate) pages: ResultPages,
    /// `HPauseMenu` 0x42A440: the highlighted button — 0 continue, 1 race again, 2 quit
    pub(crate) pause_sel: u8,
    /// the quit confirmation (`HYesNoOverlay`, 0x426154) when it is up: 0 yes, 1 no
    pub(crate) pause_confirm: Option<u8>,
    /// The single input mailbox `[0x610A6C]` (0x435410), as far as the bar is concerned: a
    /// `WM_LBUTTONDOWN` over the bar latches its position here and nothing more (the port used to
    /// answer it at once, `replay.md` §18.11's gap); `App::replay_bar_frame`, the pump's once-per-
    /// `RaceFrame` call (0x415520 → 0x4264CC), is the only reader — it takes the latch and clears
    /// it every frame, event or none, exactly as 0x435410 does
    pub(crate) bar_press: Option<(f32, f32)>,
    /// `HReplay`'s own state (`nfs_game::replay::BarState`): built once per session with the HUD
    /// (0x424F90 / 0x424FA6), kept by a restart and the automatic replay
    pub(crate) replay_bar: nfs_game::replay::BarState,
}
