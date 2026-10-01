//! **The `NFS_*` environment switches, read once** (performance work F4, 2026-10-01).
//!
//! The port's diagnostics and scripted-test hooks are `NFS_*` environment variables, and many of
//! them were read with `std::env::var` **every frame / every tick** — each read a `getenv` (a
//! linear scan of the environment under a lock) and, for `var`, a `String` allocation. This
//! module snapshots every such variable the in-game code reads, **once, on first use**, into one
//! [`Config`] object; a call site asks `cfg().<name>.var()` / `.var_os()` and gets exactly what
//! `std::env::var("<NAME>")` / `std::env::var_os("<NAME>")` would have answered at start-up
//! (`Result<String, VarError>` / `Option<OsString>`, so `is_ok` vs `var_os().is_some()` vs a
//! parse-with-default stays each switch's own — the call sites keep their original expression).
//!
//! Semantics: the environment is **not re-read** later. The only writer in the program is
//! `main` (`NFS_FE_CURSOR` / `NFS_FE_CLOCK` for the scripted runs), which runs before anything
//! calls [`cfg`]; `main` calls [`init`] right after to pin the snapshot. Start-up-only reads
//! (`main`'s argument block, the save / game paths) stay direct `std::env` calls.
//!
//! Every switch below is a TEST GLUE / diagnostic hook of the port, not a game rule; their
//! meaning is documented at the call sites.

use std::env::VarError;
use std::ffi::OsString;
use std::sync::OnceLock;

/// One environment variable as it stood when the snapshot was taken.
#[derive(Debug, Clone, Default)]
pub struct Switch(Option<OsString>);

impl Switch {
    fn read(name: &str) -> Self {
        Switch(std::env::var_os(name))
    }

    /// What `std::env::var(name)` answered at the snapshot.
    pub fn var(&self) -> Result<String, VarError> {
        match &self.0 {
            None => Err(VarError::NotPresent),
            Some(v) => v.clone().into_string().map_err(VarError::NotUnicode),
        }
    }

    /// What `std::env::var_os(name)` answered at the snapshot.
    pub fn var_os(&self) -> Option<OsString> {
        self.0.clone()
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
    nfs_ai_flip = "NFS_AI_FLIP",
    nfs_ai_noavoid = "NFS_AI_NOAVOID",
    nfs_ai_trace = "NFS_AI_TRACE",
    nfs_alpha = "NFS_ALPHA",
    nfs_anim_article = "NFS_ANIM_ARTICLE",
    nfs_anim_view = "NFS_ANIM_VIEW",
    nfs_beacon_view = "NFS_BEACON_VIEW",
    nfs_billboard = "NFS_BILLBOARD",
    nfs_box_sweep = "NFS_BOX_SWEEP",
    nfs_camera = "NFS_CAMERA",
    nfs_cam_at = "NFS_CAM_AT",
    nfs_choice = "NFS_CHOICE",
    nfs_cone_drive = "NFS_CONE_DRIVE",
    nfs_cone_limit = "NFS_CONE_LIMIT",
    nfs_congrats = "NFS_CONGRATS",
    nfs_cop_view = "NFS_COP_VIEW",
    nfs_damage = "NFS_DAMAGE",
    nfs_derby_smash = "NFS_DERBY_SMASH",
    nfs_emitter_view = "NFS_EMITTER_VIEW",
    nfs_eoe_steps = "NFS_EOE_STEPS",
    nfs_escape_log = "NFS_ESCAPE_LOG",
    nfs_expr_sweep = "NFS_EXPR_SWEEP",
    nfs_fe_after_race = "NFS_FE_AFTER_RACE",
    nfs_fe_bg = "NFS_FE_BG",
    nfs_fe_callback = "NFS_FE_CALLBACK",
    nfs_fe_clicks = "NFS_FE_CLICKS",
    nfs_fe_clock = "NFS_FE_CLOCK",
    nfs_fe_cursor = "NFS_FE_CURSOR",
    nfs_fe_dialog = "NFS_FE_DIALOG",
    nfs_fe_dz = "NFS_FE_DZ",
    nfs_fe_frames = "NFS_FE_FRAMES",
    nfs_fe_hints = "NFS_FE_HINTS",
    nfs_fe_keep = "NFS_FE_KEEP",
    nfs_eval_verify = "NFS_EVAL_VERIFY",
    nfs_map_verify = "NFS_MAP_VERIFY",
    nfs_fe_redraw_always = "NFS_FE_REDRAW_ALWAYS",
    nfs_fps_cap = "NFS_FPS_CAP",
    nfs_render_thread = "NFS_RENDER_THREAD",
    nfs_fe_idle_verify = "NFS_FE_IDLE_VERIFY",
    nfs_fe_keys = "NFS_FE_KEYS",
    nfs_fe_ko_series = "NFS_FE_KO_SERIES",
    nfs_fe_loadscrn = "NFS_FE_LOADSCRN",
    nfs_fe_open = "NFS_FE_OPEN",
    nfs_fe_pad = "NFS_FE_PAD",
    nfs_fe_parts = "NFS_FE_PARTS",
    nfs_fe_picker = "NFS_FE_PICKER",
    nfs_fe_playgame = "NFS_FE_PLAYGAME",
    nfs_fe_press = "NFS_FE_PRESS",
    nfs_fe_step = "NFS_FE_STEP",
    nfs_fe_tab = "NFS_FE_TAB",
    nfs_fe_time = "NFS_FE_TIME",
    nfs_flare_view = "NFS_FLARE_VIEW",
    nfs_flip_view = "NFS_FLIP_VIEW",
    nfs_follow_viewed = "NFS_FOLLOW_VIEWED",
    nfs_fx_frames = "NFS_FX_FRAMES",
    nfs_hud_boxes = "NFS_HUD_BOXES",
    nfs_ko_place = "NFS_KO_PLACE",
    nfs_layer_dump = "NFS_LAYER_DUMP",
    nfs_lightning = "NFS_LIGHTNING",
    nfs_loading_shot = "NFS_LOADING_SHOT",
    nfs_loading_wait = "NFS_LOADING_WAIT",
    nfs_lookback = "NFS_LOOKBACK",
    nfs_mirror_dump = "NFS_MIRROR_DUMP",
    nfs_net_dump = "NFS_NET_DUMP",
    nfs_net_pace = "NFS_NET_PACE",
    nfs_net_ram = "NFS_NET_RAM",
    nfs_net_ram_prop = "NFS_NET_RAM_PROP",
    nfs_no_aireset = "NFS_NO_AIRESET",
    nfs_no_carcol = "NFS_NO_CARCOL",
    nfs_no_gouraud = "NFS_NO_GOURAUD",
    nfs_no_shadow = "NFS_NO_SHADOW",
    nfs_offroad_log = "NFS_OFFROAD_LOG",
    nfs_phantom_drive = "NFS_PHANTOM_DRIVE",
    nfs_projector_view = "NFS_PROJECTOR_VIEW",
    nfs_race_place = "NFS_RACE_PLACE",
    nfs_remote_dump = "NFS_REMOTE_DUMP",
    nfs_replay_stop_exhausted = "NFS_REPLAY_STOP_EXHAUSTED",
    nfs_replay_type = "NFS_REPLAY_TYPE",
    nfs_results_cursor = "NFS_RESULTS_CURSOR",
    nfs_results_stop = "NFS_RESULTS_STOP",
    nfs_shot_in_race = "NFS_SHOT_IN_RACE",
    nfs_shot_scale = "NFS_SHOT_SCALE",
    nfs_shot_seq = "NFS_SHOT_SEQ",
    nfs_shot_size = "NFS_SHOT_SIZE",
    nfs_skid_nowalk = "NFS_SKID_NOWALK",
    nfs_skid_script = "NFS_SKID_SCRIPT",
    nfs_skid_stats = "NFS_SKID_STATS",
    nfs_sky_level = "NFS_SKY_LEVEL",
    nfs_slow_log = "NFS_SLOW_LOG",
    nfs_smack_drive = "NFS_SMACK_DRIVE",
    nfs_smack_view = "NFS_SMACK_VIEW",
    nfs_sprite_selftest = "NFS_SPRITE_SELFTEST",
    nfs_sweep_dump = "NFS_SWEEP_DUMP",
    nfs_sweep_state = "NFS_SWEEP_STATE",
    nfs_sweep_states = "NFS_SWEEP_STATES",
    nfs_sweep_where = "NFS_SWEEP_WHERE",
    nfs_text_ab = "NFS_TEXT_AB",
    nfs_text_big = "NFS_TEXT_BIG",
    nfs_text_fulluv = "NFS_TEXT_FULLUV",
    nfs_text_probe = "NFS_TEXT_PROBE",
    nfs_text_solid = "NFS_TEXT_SOLID",
    nfs_traffic_seed = "NFS_TRAFFIC_SEED",
    nfs_trays_in = "NFS_TRAYS_IN",
    nfs_update_check_ticks = "NFS_UPDATE_CHECK_TICKS",
    nfs_view = "NFS_VIEW",
    nfs_weather_steps = "NFS_WEATHER_STEPS",
    nfs_weather_view = "NFS_WEATHER_VIEW",
    nfs_widget_dump = "NFS_WIDGET_DUMP",
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
