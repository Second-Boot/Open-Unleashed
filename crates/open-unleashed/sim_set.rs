// The simulation set of the build fingerprint (**PORT-DEVIATION D56**, `RE/notes/lobby.md` section 14.1). Included by `build.rs` (which
// hashes these files into `OU_SIM_FINGERPRINT`) and by the test in `src/brand.rs` (which fails when a file of `src/` is in neither list).
// Over-inclusion is harmless (the fingerprint changes without the physics changing); under-inclusion is not. Paths are relative to the
// repository root (`SIM_CRATES`: every file under `crates/<name>/src`) or to `crates/open-unleashed/src` (a trailing `/` = the directory).

/// Library crates whose whole `src/` is hashed: everything a race's tick, the launch and the network replay can run.
pub const SIM_CRATES: &[&str] = &["nfs-math", "nfs-physics", "nfs-formats", "nfs-assets", "nfs-game", "nfs-net"];

/// Files / directories of `open-unleashed/src` that are hashed: the race build, the tick, the scripted and `--resim` runs, the network
/// replay, the lobby / launch glue and the shared state they touch.
pub const SIM_SRC: &[&str] = &[
    "main.rs", "loopctl.rs", "app.rs", "accessors.rs", "career_ui.rs", "cli.rs", "netplay/", "parts_shop.rs", "power.rs",
    "app/carsetup.rs", "app/init.rs", "app/net/", "app/opponents.rs", "app/profile.rs", "app/quickfix.rs",
    "app/race_end.rs", "app/race_setup.rs", "app/race_stages.rs", "app/race_start.rs", "app/replay.rs", "app/resim.rs", "app/results.rs",
    "app/glue/mod.rs", "app/glue/scripted.rs", "app/knockout.rs", "app/scratch.rs", "app/session.rs", "app/simulate.rs", "app/race_audio.rs","app/loading.rs", "app/camera.rs", "app/camera/", "app/input.rs",
];

/// Files / directories of `open-unleashed/src` that cannot change what a race computes: drawing, sound, the front end's screens, the
/// HUD, diagnostics, branding.
pub const NOT_SIM: &[&str] = &[
    "brand.rs", "diag.rs", "guard.rs", "intro_run.rs", "hud.rs", "hud_cfg.rs", "hud_str.rs", "hud/",
    "app/ambient.rs", "app/audio.rs", "app/hud_state.rs", "app/car_lights.rs", "app/chronicle.rs", "app/debug_menu.rs", "app/effects.rs", "app/events.rs",
    "app/fe_callbacks.rs", "app/fe_cars.rs", "app/fe_choices.rs", "app/fe_colour.rs", "app/fe_flow.rs", "app/fe_lists.rs", "app/fe_pictures.rs",
    "app/fe_room.rs", "app/fe_rows.rs", "app/fe_screens.rs", "app/force_feedback.rs", "app/frame.rs", "app/hud_pass.rs",
    "app/idle.rs", "app/lens_flare.rs", "app/menu.rs", "app/mirror.rs", "app/glue/fe_seq.rs", "app/glue/pa_trace.rs", "app/pad/", "app/photo.rs", "app/race_panel.rs", "app/race_shot.rs", "app/reflections.rs", "app/cover.rs",
    "app/render_scene.rs", "app/lights.rs", "app/car_draw.rs",
    // the enhanced game's render / front-end modules (their switches never reach the simulation)
    "app/camfx.rs", "app/cover.rs", "app/drops.rs", "app/enhancements.rs", "app/fe_enh.rs", "app/haze.rs", "app/rain.rs",
    "app/reflections.rs", "app/street_lamps.rs",
];
