//! **The `NFS_*` environment switches, read once** (performance work F4, 2026-10-01).
//!
//! The port's diagnostics and scripted-test hooks are `NFS_*` environment variables, and many of
//! them were read with `std::env::var` **every frame / every tick** — each read a `getenv` (a
//! linear scan of the environment under a lock) and, for `var`, a `String` allocation. This
//! module snapshots every such variable the in-game code reads, **once, on first use**, into one
//! [`Config`] object; a call site asks `cfg().<name>.as_str()` / `.is_set()` / `.as_os_str()` and gets exactly what
//! `std::env::var("<NAME>").ok()` / `var_os("<NAME>").is_some()` / `var_os("<NAME>")` would have
//! answered at start-up, borrowed (no clone per call; `as_str().is_some()` vs `is_set()` differ
//! only for a non-Unicode value, so each switch keeps the test its call site always used).
//!
//! Semantics: the environment is **not re-read** later. The only writer in the program is
//! `main` (`NFS_FE_CURSOR` / `NFS_FE_CLOCK` for the scripted runs), which runs before anything
//! calls [`cfg`]; `main` calls [`init`] right after to pin the snapshot. Start-up-only reads
//! (`main`'s argument block, the save / game paths) stay direct `std::env` calls.
//!
//! `nfs-game` itself reads five of these (`NFS_FE_CLOCK` / `NFS_FE_CURSOR` / `NFS_FE_TIME` in the layout and
//! opponents, `NFS_SKY_LEVEL`, `NFS_DATA_SUN`); the rest are the application's. The file stays here (moving it would
//! have to thread those values through `LayoutScreen` and the sky for no gain); the save-directory resolution
//! (`NFS5_SAVE_DIR`) is [`save_root`] / [`save_root_override`].
//!
//! Every switch below is a TEST GLUE / diagnostic hook of the port, not a game rule; their
//! meaning is documented at the call sites.

use std::ffi::OsString;
use std::sync::OnceLock;

/// One environment variable as it stood when the snapshot was taken.
#[derive(Debug, Clone, Default)]
pub struct Switch(Option<OsString>);

impl Switch {
    fn read(name: &str) -> Self {
        Switch(std::env::var_os(name))
    }

    /// What `std::env::var(name)` answered at the snapshot, without the clone: `Some` only when
    /// the variable is set **and** valid Unicode (`var(..).is_ok()` / `.ok()` of the old API).
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref().and_then(std::ffi::OsStr::to_str)
    }

    /// What `std::env::var_os(name).is_some()` answered at the snapshot (set, Unicode or not).
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }

    /// Set and not `0` (the empty string counts as on): `var_os(name).is_some_and(|v| v != "0")`.
    pub fn is_on(&self) -> bool {
        self.0.as_deref().is_some_and(|v| v != "0")
    }

    /// Unset, empty or `0`: `var_os(name).is_none_or(|v| v.is_empty() || v == "0")` (the switch that is off unless named).
    pub fn is_off(&self) -> bool {
        self.0.as_deref().is_none_or(|v| v.is_empty() || v == "0")
    }

    /// What `std::env::var_os(name)` answered at the snapshot, borrowed.
    pub fn as_os_str(&self) -> Option<&std::ffi::OsStr> {
        self.0.as_deref()
    }
}

macro_rules! switches {
    ($($field:ident = $name:literal,)*) => {
        /// The snapshot: one [`Switch`] per variable, named after it in lower case.
        #[derive(Debug, Clone, Default)]
        pub struct Config {
            $(pub $field: Switch,)*
        }

        impl Config {
            /// Read every variable now.
            pub fn from_env() -> Self {
                Config { $($field: Switch::read($name),)* }
            }
        }
    };
}

switches! {
    // the save-data directory override (`save::save_dir`, `Career::legacy_json_path`, `diag`)
    nfs_ai_trace = "NFS_AI_TRACE",
    nfs_camera = "NFS_CAMERA",
    nfs_cam_at = "NFS_CAM_AT",
    // TEST GLUE (ENHANCEMENTS E9 / E51, comparison stills only): a hand-set data sun (`data_sun.rs`)
    nfs_data_sun = "NFS_DATA_SUN",
    nfs_choice = "NFS_CHOICE",
    nfs_fe_clicks = "NFS_FE_CLICKS",
    nfs_fe_clock = "NFS_FE_CLOCK",
    nfs_fe_cursor = "NFS_FE_CURSOR",
    nfs_fe_dz = "NFS_FE_DZ",
    nfs_fe_frames = "NFS_FE_FRAMES",
    // TEST GLUE: the rumble backend's by-hand check (`Gamepads::rumble_test_tick`), set to anything
    nfs_ff_test = "NFS_FF_TEST",
    nfs_fe_hints = "NFS_FE_HINTS",
    // TEST GLUE (PORT-DEVIATION D64): `xbox` / `ps` / `nintendo` shows the pad button icons as if such a pad were connected
    nfs_pad_kind = "NFS_PAD_KIND",
    nfs_fe_redraw_always = "NFS_FE_REDRAW_ALWAYS",
    nfs_detail = "NFS_DETAIL",
    nfs_fe_seq_frames = "NFS_FE_SEQ_FRAMES",
    nfs_fe_seq_script = "NFS_FE_SEQ_SCRIPT",
    nfs_fps_cap = "NFS_FPS_CAP",
    nfs_frame_stats = "NFS_FRAME_STATS",
    nfs_mission = "NFS_MISSION",
    nfs_shot_ticks = "NFS_SHOT_TICKS",
    nfs_render_thread = "NFS_RENDER_THREAD",
    nfs_fe_idle_verify = "NFS_FE_IDLE_VERIFY",
    nfs_fe_keys = "NFS_FE_KEYS",
    nfs_fe_loadscrn = "NFS_FE_LOADSCRN",
    nfs_fe_pad = "NFS_FE_PAD",
    nfs_fe_playgame = "NFS_FE_PLAYGAME",
    nfs_fe_tab = "NFS_FE_TAB",
    nfs_fe_time = "NFS_FE_TIME",
    nfs_fx_frames = "NFS_FX_FRAMES",
    nfs_hit_sweep = "NFS_HIT_SWEEP",
    nfs_hud_keys = "NFS_HUD_KEYS",
    nfs_lens_warmup = "NFS_LENS_WARMUP",
    nfs_loading_shot = "NFS_LOADING_SHOT",
    nfs_loading_wait = "NFS_LOADING_WAIT",
    // The three network dump switches are three different files (not one feature read twice):
    //  * `NFS_NET_DUMP=<file>`: the input-exchange race's state dump, one file per run (`app/net.rs` `net_exchange_begin`, rows of
    //    `simulate.rs` `net_dump_tick`, `<file>.cars`), and, read by `nfs-net`'s socket itself, every datagram (`S` lines);
    //  * `NFS_REMOTE_DUMP=<file>`: one CSV line per tick for every remote seat after the dead reckoning (`app/net.rs` `remote_dump`);
    //  * `NFS_NET_REMOTE_DUMP=<file>`: the dead reckoning in the line format of the original's gdb trace (`netplay/remote.rs`).
    nfs_net_dump = "NFS_NET_DUMP",
    nfs_net_pace = "NFS_NET_PACE",
    nfs_net_lockstep = "NFS_NET_LOCKSTEP",
    nfs_net_teleport = "NFS_NET_TELEPORT",
    // netplay test glue of the application (`netplay/{remote,keys,session,barrier,mod}.rs`)
    nfs_net_lag_ms = "NFS_NET_LAG_MS",
    nfs_net_port = "NFS_NET_PORT",
    nfs_net_remote_dump = "NFS_NET_REMOTE_DUMP",
    nfs_no_carcol = "NFS_NO_CARCOL",
    nfs_no_steer = "NFS_NO_STEER",
    nfs_autopilot = "NFS_AUTOPILOT",
    nfs_demo = "NFS_DEMO",
    nfs_race_place = "NFS_RACE_PLACE",
    nfs_remote_dump = "NFS_REMOTE_DUMP",
    nfs_replay_type = "NFS_REPLAY_TYPE",
    nfs_results_stop = "NFS_RESULTS_STOP",
    nfs_shot_in_race = "NFS_SHOT_IN_RACE",
    nfs_shot_scale = "NFS_SHOT_SCALE",
    nfs_shot_seq = "NFS_SHOT_SEQ",
    nfs_shot_size = "NFS_SHOT_SIZE",
    nfs_temporal_dt = "NFS_TEMPORAL_DT",
    nfs_temporal_seq = "NFS_TEMPORAL_SEQ",
    nfs_temporal_ticks = "NFS_TEMPORAL_TICKS",
    // TEST GLUE (PORT-DEVIATION D46, photo mode): a scripted still's photo-mode ops and the race-key script (`app/photo.rs`)
    nfs_photo = "NFS_PHOTO",
    nfs_race_keys = "NFS_RACE_KEYS",
    // TEST GLUE (PORT-DEVIATION D63, the race panel): the panel's actions at game ticks (`app/race_panel.rs`)
    nfs_race_panel = "NFS_RACE_PANEL",
    // TEST GLUE (PORT-DEVIATION D63): a windowed run draws each frame into an image instead of the surface (`app/frame.rs`)
    nfs_offscreen_frames = "NFS_OFFSCREEN_FRAMES",
    // TEST GLUE: a windowed front end's per-frame animation phases and wall-time stills (`app/frame.rs` `fe_anim_log`)
    nfs_fe_anim_log = "NFS_FE_ANIM_LOG",
    // TEST GLUE: the windowed loop's frame at given game ticks of a self-driving race (`app/frame.rs` `window_shot_due`)
    nfs_window_shot = "NFS_WINDOW_SHOT",
    // TEST GLUE: a scripted still taken as the window shows its tick (`app/glue/scripted.rs` `shot_window`)
    nfs_shot_window = "NFS_SHOT_WINDOW",
    // TEST GLUE: a scripted still taken as if that many milliseconds of frames had followed its tick (`app/scripted.rs`)
    nfs_shot_settle = "NFS_SHOT_SETTLE",
    nfs_pa_input = "NFS_PA_INPUT",
    nfs_pa_trace = "NFS_PA_TRACE",
    nfs_pa_finish = "NFS_PA_FINISH",
    nfs_pa_loops = "NFS_PA_LOOPS",
    nfs_sky_level = "NFS_SKY_LEVEL",
    nfs_smack_ram = "NFS_SMACK_RAM",
    nfs_traffic_seed = "NFS_TRAFFIC_SEED",
    nfs_view = "NFS_VIEW",
    // the application's remaining direct reads, moved here (REFACTOR-0119 P0): one mechanism for every fixed name
    // (`NFS_NO_FOCUS`, `NFS_FE_CURSOR` / `NFS_FE_CLOCK` stay raw reads in `main`, which writes them before the snapshot; `NFS_ENH` is the renderer's)
    nfs5_profile = "NFS5_PROFILE",
    nfs5_records_save = "NFS5_RECORDS_SAVE",
    nfs_all_cars = "NFS_ALL_CARS",
    nfs_all_tracks = "NFS_ALL_TRACKS",
    nfs_autotest_races = "NFS_AUTOTEST_RACES",
    nfs_cam_log = "NFS_CAM_LOG",
    nfs_car_id = "NFS_CAR_ID",
    nfs_cheat_bits = "NFS_CHEAT_BITS",
    nfs_cops = "NFS_COPS",
    nfs_display_live = "NFS_DISPLAY_LIVE",
    nfs_enh_live = "NFS_ENH_LIVE",
    nfs_fe_keys_after = "NFS_FE_KEYS_AFTER",
    nfs_fe_loading = "NFS_FE_LOADING",
    nfs_fe_seq = "NFS_FE_SEQ",
    nfs_haze_log = "NFS_HAZE_LOG",
    nfs_hud_keys_1 = "NFS_HUD_KEYS_1",
    nfs_lens_force = "NFS_LENS_FORCE",
    nfs_menu_mode = "NFS_MENU_MODE",
    nfs_oppcar = "NFS_OPPCAR",
    nfs_pa_fe0 = "NFS_PA_FE0",
    nfs_pa_rnglog = "NFS_PA_RNGLOG",
    nfs_play_intro = "NFS_PLAY_INTRO",
    nfs_race_loading = "NFS_RACE_LOADING",
    nfs_refl_live = "NFS_REFL_LIVE",
    nfs_replay_hover = "NFS_REPLAY_HOVER",
    nfs_replay_press = "NFS_REPLAY_PRESS",
    nfs_resim_gpu = "NFS_RESIM_GPU",
    nfs_results_shot = "NFS_RESULTS_SHOT",
    nfs_teleport_turn = "NFS_TELEPORT_TURN",
    nfs_test_cheat_parts = "NFS_TEST_CHEAT_PARTS",
    nfs_test_own_parts = "NFS_TEST_OWN_PARTS",
    nfs_test_own_setup = "NFS_TEST_OWN_SETUP",
    nfs_test_speedhack = "NFS_TEST_SPEEDHACK",
    nfs_traffic = "NFS_TRAFFIC",
}

/// A per-index variable name (`NFS_HUD_KEYS_<k>`, `NFS_SESSION_FE_KEYS_<k>`): not a fixed name, so not in the snapshot; the one
/// place that builds and reads such a name (`std::env::var(..).ok()`).
pub fn env_indexed(prefix: &str, k: impl std::fmt::Display) -> Option<String> {
    std::env::var(format!("{prefix}{k}")).ok()
}

/// `NFS5_SAVE_DIR` when it is set (a scripted run owns its directory), read directly from the environment and not through the
/// snapshot: `diag::init` / `guard::begin` ask before `main`'s `set_var`s, which the snapshot must see.
pub fn save_root_override() -> Option<std::path::PathBuf> {
    std::env::var_os("NFS5_SAVE_DIR").map(std::path::PathBuf::from)
}

/// The directory that holds the port's own files (`enhancements.json`, `gi/`, `career.json`): `NFS5_SAVE_DIR`, else the per-OS
/// data dir. (The profiles live one level down, in `SaveData`: [`crate::save::save_dir`].)
pub fn save_root() -> std::path::PathBuf {
    save_root_override().unwrap_or_else(nfs_assets::platform::data_dir)
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// The process-wide snapshot (taken on the first call).
pub fn cfg() -> &'static Config {
    CONFIG.get_or_init(Config::from_env)
}

/// Pin the snapshot now (idempotent) — `main` calls it once its own `set_var`s are done.
pub fn init() {
    let _ = cfg();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_accessors_match_the_std_env_answers() {
        let unset = Switch(None);
        assert!(!unset.is_set());
        assert_eq!(unset.as_str(), None);
        assert_eq!(unset.as_os_str(), None);

        assert!(unset.is_off() && !unset.is_on());
        for (v, on, off) in [("", true, true), ("0", false, true), ("1", true, false)] {
            let s = Switch(Some(OsString::from(v)));
            assert_eq!((s.is_on(), s.is_off()), (on, off), "{v:?}");
        }

        // set to the empty string and to "0": both are "set" (presence-only switches stay on)
        for v in ["", "0", "1", "a=b,c"] {
            let s = Switch(Some(OsString::from(v)));
            assert!(s.is_set(), "{v:?}");
            assert_eq!(s.as_str(), Some(v));
            assert_eq!(s.as_os_str(), Some(std::ffi::OsStr::new(v)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_value_is_set_but_has_no_str() {
        use std::os::unix::ffi::OsStringExt;
        let s = Switch(Some(OsString::from_vec(vec![0x66, 0xFF, 0x6F])));
        assert!(s.is_set(), "var_os().is_some()");
        assert_eq!(s.as_str(), None, "var().is_ok() was false for it");
    }
}
