//! Native macOS application entry point.
//!
//! Timing model (CONFIRMED by disassembly, `RE/notes/timing.md`): the original runs a fixed-step
//! game tick at **64 Hz** (15.625 ms, `GameTick` = 0x414FF0), paced by a 128 Hz multimedia timer;
//! controller input is latched at 32 Hz. No delta-time scaling exists in the original.
//!
//! Current state (M5 in progress): loads a real track and a real car, places the car at the
//! forward start pose and runs the recovered vehicle model (`nfs-physics::vehicle`) from
//! keyboard/gamepad input, with a chase camera.
//!
//! ```text
//! porsche                                  # menu: choose track and car, Enter to race
//! porsche [track] [car] [sim] [--fe[=layout]] [--debug-menu] [--screenshot out.png] [--autotest N [--follow]] [--opponents N] [--record run.csv]
//!        [--net host[:PORT] | --net join:HOST[:PORT] [--net-name NAME]]
//! keys: arrows = steer/throttle/brake, space = handbrake, C = camera, R = reset to track, Esc = quit
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// Layout of the crate: `app` (the `App` state and its `impl App` blocks, one module per subsystem),
// `hud` (the race HUD), and the front-end / network modules below. Every module starts with
// `use crate::*;`, so the imports below and the items re-exported here are visible crate-wide.
mod accessors;
mod app;
mod career_ui;
mod diag;
mod hud;
mod hud_cfg;
mod intro_run;
mod netlobby;
mod netplay;
mod parts_shop;
use app::*;
use hud::*;

use anyhow::Result;
use career_ui::{CareerOpponent, CareerUi};
use glam::{Mat4, Quat, Vec3};
use nfs_assets::{GamePaths, car::{CarAssets, Paint}, track::TrackAssets};
use nfs_game::layout::{ListCell, ListRow, TrackMapEvent};
use nfs_game::{
    lanes::{LaneCar, lane_of, lane_span},
    race::{RaceProgress, SliceTable, grid_pose},
    racecar::{Controller, RaceCar},
    records::{Records, format_ticks, format_time_dword},
    skill::{SkillEvent, SkillTest},
    sky::Sky,
    text::BitmapFont,
    traffic::{Reference, TrafficSet, TrafficSpec},
    world::{CarModel, TrackWorld},
};
use nfs_input::{Action, InputState, gamepad::Gamepads};
use nfs_physics::{
    specs::CarSpecs,
    vehicle::{Controls, Vehicle},
};
use nfs_renderer::{Camera, Renderer, Scene, Sprite, TextureData, TextureId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    diag::init();
    if let Err(e) = run() {
        diag::fatal(&e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let paths = GamePaths::locate()?;
    tracing::info!(root = %paths.root.display(), "located original game data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    // positional arguments = tokens that are neither flags nor flag values
    let flags_with_value = [
        "--screenshot",
        "--autotest",
        "--opponents",
        "--record",
        "--laps",
        "--career",
        "--layout",
        "--replay",
        "--net",
        "--net-name",
        "--net-browse",
    ];
    // flags without a value: --follow, --mission
    let mut positional = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = flags_with_value.contains(&a.as_str());
            continue;
        }
        positional.push(a.clone());
    }
    // `--replay <file>`: play back a recording the port saved (`RE/notes/replay.md` §14.5 — the
    // original's 118 312-byte .rpl is the registry dump of §16, which `--rpl` writes)
    let replay_file = args
        .iter()
        .position(|a| a == "--replay")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| {
            let d = std::fs::read(p)
                .map_err(|e| tracing::error!(error = %e, path = p, "replay unreadable"))
                .ok()?;
            read_replay_file(&d)
                .or_else(|| {
                    tracing::error!(path = p, "not a replay file");
                    None
                })
        });
    let career_id: Option<i16> = args
        .iter()
        .position(|a| a == "--career")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let mission_flag = args.iter().any(|a| a == "--mission");
    // (`NFS_FE_PLAYGAME` keeps the front end for a scripted run: the race starts from it)
    let use_menu = positional.is_empty()
        && (!args.iter().any(|a| a == "--autotest") || std::env::var_os("NFS_FE_PLAYGAME").is_some() || std::env::var_os("NFS_FE_KEYS").is_some())
        && career_id.is_none()
        && !mission_flag
        && replay_file.is_none();
    // `--career <id>`: start the next event of tournament <id> from the saved career (buying the
    // cheapest eligible car when the garage has none); used for headless smoke runs
    let mut career = if use_menu || career_id.is_some() || mission_flag {
        CareerUi::load(&paths)
            .map_err(|e| tracing::warn!(error = %e, "career unavailable"))
            .ok()
    } else {
        None
    };
    let mut career_request = None;
    // `--mission`: start the current Factory Driver mission headlessly (with --autotest)
    if mission_flag && let Some(ui) = career.as_mut() {
        ui.set_mode(3);
        career_request = Some(
            ui.activate_mission()
                .ok_or_else(|| anyhow::anyhow!("cannot start a mission: {}", ui.message))?,
        );
    }
    if let (Some(id), Some(ui)) = (career_id, career.as_mut()) {
        let idx = ui
            .tournaments()
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow::anyhow!("tournament {id} not in the current era"))?;
        ui.t_idx = idx;
        ui.ensure_car();
        ui.car_sel = 0;
        career_request = Some(
            ui.activate()
                .ok_or_else(|| anyhow::anyhow!("cannot start tournament {id}: {}", ui.message))?,
        );
    }
    let mut replay_setup = replay_file.as_ref().map(|r| r.setup.clone());
    if let Some(rs) = replay_setup.as_mut() {
        resolve_replay_car(&paths, rs);
    }
    let track_name = replay_setup
        .as_ref()
        .map(|r| r.track.clone())
        .or(career_request.as_ref().map(|r| r.track_file.clone()))
        .or(positional.first().cloned())
        .unwrap_or_else(|| "alps".into());
    let car_name = replay_setup
        .as_ref()
        .map(|r| r.car.clone())
        .or(career_request.as_ref().map(|r| r.car.model.clone()))
        .or(positional.get(1).cloned())
        .unwrap_or_else(|| "993".into());
    let (track_name, car_name) = (track_name.as_str(), car_name.as_str());
    let t0 = Instant::now();
    let mut track = TrackAssets::load(&paths, track_name)?;
    let mut skill = None;
    if let Some(ms) = career_request.as_ref().and_then(|r| r.mission.as_ref())
        && let Some(suffix) = &ms.scene_suffix
    {
        if let Some(scene) = track.load_scene(suffix)? {
            skill = Some(SkillTest::from_scene(&scene, &track.name));
            track.extra_scenes.push(scene);
        }
    }
    // a recording's `SCENENUMBER` 0x1C: the skill test's scene the race ran in (0x4ECDF0)
    if let Some(n) = replay_setup.as_ref().and_then(|r| r.scene).filter(|n| *n > 0) {
        match track.load_scene(&format!("_st{n}.scn"))? {
            Some(scene) => {
                // the scene's triggers are the test (start pose 0x410430, arrows, cones, finish): `SkillTest`
                skill = Some(SkillTest::from_scene(&scene, &track.name));
                track.extra_scenes.push(scene);
            }
            None => tracing::error!(scene = n, "replay: the recorded skill-test scene is missing"),
        }
    }
    let car = CarAssets::load(&paths, car_name)?;
    // simulation variant: catalogue entry whose model matches, else "<car>coupe36"-style guess
    let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(
        paths.fe_data().join("Data").join("nfs5.car"),
    )?)?;
    // a replay's car is the catalogue record it names (`car_id`): its body style and roof — the
    // model's box, wheels and heights the physics builds the car from — as the live race's car had
    // them (`start_from_layout`'s `with_style(variant).with_cabrio(cabrio)`); found by the network
    // replay's bit-exact comparison (`replay.md` §18.15)
    let car = match replay_setup.as_ref().filter(|r| r.car_id != 0).and_then(|r| catalog.iter().find(|e| e.id == r.car_id)) {
        Some(e) => car.with_style(e.variant).with_cabrio(e.cabrio),
        None => car,
    };
    let sim_name = replay_setup
        .as_ref()
        .map(|r| r.sim.clone())
        .or_else(|| career_request.as_ref().map(|r| r.car.sim.clone()))
        .or_else(|| positional.get(2).cloned())
        .or_else(|| {
            catalog
                .iter()
                .find(|c| c.model.eq_ignore_ascii_case(car_name) && !c.sim.is_empty())
                .map(|c| c.sim.clone())
        })
        .unwrap_or_else(|| "default".into());
    let sim = CarAssets::load_sim(&paths, &sim_name)?;
    let sound_set = catalog
        .iter()
        .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
        .map(|c| c.sound_set.clone())
        .unwrap_or_else(|| "sd7a".into());
    let paint = replay_setup
        .as_ref()
        .map(|r| r.paint)
        .or_else(|| career_request.as_ref().map(|r| r.paint))
        .or_else(|| {
            catalog
                .iter()
                .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
                .and_then(|c| c.paint_sets.first())
                .map(|p| p.colours[0])
        })
        .unwrap_or([0xD4, 0x10, 0x10]);
    let race_class = sim_name.to_lowercase().starts_with("gt") || sim_name.starts_with("935");
    let mut specs = CarSpecs::from_sim(&sim, race_class);
    // `.TRANSMISSION` (`carinfo+0xC`) of the player's car: `Car.Transmission`'s index, 1 = automatic (0x4EEF9F), the
    // same word `App::player_automatic` reads — here the command line has no front end, so the option is the
    // `NFS_CHOICE` override or the registry's `car[0].transmission` (default 0, as `choice_index`'s); and 0 whatever
    // the option says in Factory Driver mission 0x1D (the player's 0x4EEDA0 call alone, 0x4EB9C6…0x4EEF8D). A
    // recording's own word is read below. GB: the folds took `true` whatever the option said.
    let cli_automatic = {
        let over = nfs_game::config::cfg().nfs_choice.var().ok().and_then(|o| {
            o.split(',').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "Car.Transmission").and_then(|(_, v)| v.trim().parse::<usize>().ok())
        });
        let option = over.unwrap_or_else(|| nfs_formats::fetxt::Registry::load(&paths.root).u32("car[0].transmission", 0) as usize);
        option >= 1 && !career_request.as_ref().and_then(|r| r.mission.as_ref()).is_some_and(|m| m.index == 0x1D)
    };
    if let Some(r) = &career_request {
        let family = catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| c.sim_family.clone())
            .unwrap_or_default();
        let owned = career.as_ref().and_then(|cu| cu.pending.as_ref().and_then(|p| p.garage_idx).and_then(|i| cu.career.garage.get(i)));
        if r.parts.is_empty() && owned.is_none() {
            // no garage record and no parts of its own: a fresh `FECar` (0x4EE9A0 stock parts, 0x4EED2D
            // neutral sliders) — same as `load_career_race`'s
            let stock_ids: Vec<i16> = catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).map(|c| c.stock_parts.clone()).unwrap_or_default();
            let table = nfs_formats::career::parse_parts(&std::fs::read(paths.fe_data().join("Data").join("nfs5.prt")).unwrap_or_default()).unwrap_or_default();
            nfs_game::career::fold_fresh_car(&mut specs, &stock_ids, &table, &family, cli_automatic);
        } else {
            specs.apply_parts(&r.parts.iter().collect::<Vec<_>>(), &family);
            // the installed-part list `car+0x768` with the garage car's carried states, as the
            // front end's start of the same event builds it (`start_career_event`)
            if owned.is_some() {
                specs.parts = nfs_game::career::installed_parts(&r.parts.iter().collect::<Vec<_>>(), owned);
            }
            // …then the sliders (0x49D290): the owned car's own, else the constructor's neutral ones
            specs.apply_setup(owned.map_or(&nfs_game::career::CAR_SETUP_DEFAULT, |o| &o.setup), cli_automatic);
        }
    } else {
        // no career request: this CLI/replay invocation's car races on its stock parts
        // (`specs.rs:49`, 0x4EE9A0's unconditional fit at `FECar::init`)
        let stock_ids: &[i16] = catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| c.stock_parts.as_slice())
            .unwrap_or_default();
        let parts_table = nfs_formats::career::parse_parts(&std::fs::read(paths.fe_data().join("Data").join("nfs5.prt")).unwrap_or_default()).unwrap_or_default();
        let family = catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).map(|c| c.sim_family.clone()).unwrap_or_default();
        // the stock parts (0x4EE9A0) and the constructor's neutral sliders (0x4EED2D) through 0x49CB50;
        // `.TRANSMISSION` is the gearbox option (`cli_automatic` above: `Car.Transmission` index >= 1 = automatic)
        let before = (specs.top_speed, specs.final_drive, specs.grip, specs.transfer_div, specs.roll_gain, specs.front_weight);
        // a recording's car-info record of slot 0 (`Replay_begin` 0x413CF0's restore): its parts, their wear and
        // its sliders, parts first (0x49CB50 / 0x49D290); a file with none races the fresh catalogue car
        let recorded = replay_setup.as_ref().and_then(|r| r.player.as_ref()).and_then(|c| c.parts.as_ref().map(|ids| (c, ids)));
        if let Some((c, ids)) = recorded {
            let parts: Vec<&nfs_formats::career::Part> = ids.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id)).collect();
            specs.apply_parts(&parts, &family);
            specs.parts = parts
                .iter()
                .enumerate()
                .map(|(i, p)| nfs_physics::specs::InstalledPart::new(p.kind, p.zone_mask.map(|b| b != 0), p.condition, c.part_damage.get(i).copied().unwrap_or(p.state as f32)))
                .collect();
            // `.TRANSMISSION` of the recorded block (`carinfo+0xC`); a file from before the field: the session's
            // option, `cli_automatic` (APPROXIMATION)
            let automatic = replay_setup.as_ref().and_then(|r| r.transmission).unwrap_or(cli_automatic);
            specs.apply_setup(&c.setup.unwrap_or(nfs_game::career::CAR_SETUP_DEFAULT), automatic);
        } else {
            if replay_setup.is_some() {
                tracing::info!("replay: no car record in the setup, the car is a fresh catalogue car (APPROXIMATION)");
            }
            nfs_game::career::fold_fresh_car(&mut specs, stock_ids, &parts_table, &family, cli_automatic);
        }
        tracing::info!(
            stock = ?stock_ids, top_speed_before = before.0, top_speed = specs.top_speed, final_drive_before = before.1, final_drive = specs.final_drive,
            grip_before = before.2, grip = specs.grip, transfer_div_before = before.3, transfer_div = specs.transfer_div,
            roll_gain_before = before.4, roll_gain = specs.roll_gain, front_weight_before = before.5, front_weight = specs.front_weight,
            "the catalogue car's stock parts and neutral set-up folded (0x4EE9A0, 0x4EED2D, 0x49CB50)"
        );
    }
    let specs = specs;
    tracing::info!(
        sim = sim_name,
        name = specs.name,
        top_speed_mps = specs.top_speed,
        grip = specs.grip,
        transfer_div = specs.transfer_div,
        roll_gain = specs.roll_gain,
        "car specs"
    );
    tracing::info!(
        track = track_name,
        car = car_name,
        secs = t0.elapsed().as_secs_f32(),
        "assets loaded"
    );
    // A scripted run (`--autotest`, `--screenshot`, `--layout-sweep`, `--record`, or `NFS_NO_FOCUS=1`)
    // must not take the keyboard focus away from whatever the user is doing: on macOS the process
    // stays an accessory (no Dock icon, never activated on launch) and its window is shown without
    // being made key. The window still appears, so the GPU path is the real one.
    let scripted = ["--autotest", "--screenshot", "--layout-sweep", "--record"].iter().any(|f| args.iter().any(|a| a == f))
        || std::env::var_os("NFS_NO_FOCUS").is_some();
    NO_FOCUS.store(scripted, std::sync::atomic::Ordering::Relaxed);
    // diagnostic: `NFS_PLAY_INTRO=1` keeps the start-up movies for a scripted run (see `PLAY_INTRO`)
    PLAY_INTRO.store(std::env::var_os("NFS_PLAY_INTRO").is_some(), std::sync::atomic::Ordering::Relaxed);
    // Tooling only, not a game rule: a scripted capture (`--screenshot`, or any other scripted
    // run, which is always `NFS_NO_FOCUS=1` too — the flag `scripted` is set from above) must not
    // let wherever the operator's real mouse happens to rest change a hover state — the golden
    // must come out the same regardless (a row/header hover state was seen to differ this way on
    // `parts.png`). `NFS_FE_CURSOR=x,y` already parks the pointer for a still taken by hand (to
    // look at one hover state) and `LayoutScreen::set_cursor`/`set_pointer`
    // (`crates/nfs-game/src/layout.rs`) already ignore every real `CursorMoved` event once it is
    // set. Default it — the same way `layout_sweep` below defaults `NFS_FE_CLOCK` — to a design
    // pixel outside the 640×480 layout, so a scripted run never reads a real pointer position
    // unless the operator explicitly asks for one with their own `NFS_FE_CURSOR`. The original's
    // own front end has no notion of "no pointer"; this is a capture-determinism device only,
    // the same class as `NFS_FE_DZ` standing in for a dragged bar.
    if scripted && std::env::var("NFS_FE_CURSOR").is_err() {
        // SAFETY: single-threaded start-up, before any screen is loaded
        unsafe { std::env::set_var("NFS_FE_CURSOR", "-1000,-1000") };
    }
    // Windows / Linux: no activation policy exists; the scripted window is created with
    // `with_active(false)` (see `resumed`), which is the nearest no-focus behaviour winit offers.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    if scripted {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory).with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    // `--fe` boots into the original front end instead of the text menu, on the quick-race screen
    // or on the layout named after it (`--fe carcompare`)
    let fe_start = args.iter().find(|a| a.starts_with("--fe")).map(|a| match a.split_once('=') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => "QuickRace".to_string(),
    });
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1).cloned());
    let layout_sweep = args.iter().any(|a| a == "--layout-sweep");
    // The sweep measures **gaps**, and a widget that is correctly blank because an animation has
    // not started yet is not one. `data.animating` keeps a wordwrap widget empty for its first
    // 0x97 = 151 ticks (§4.87), so a sweep taken at clock 0 counts five of them; preloading the
    // screen clock past that wait keeps the measurement comparable with every earlier round.
    // An explicit `NFS_FE_CLOCK` still wins, so a sweep can be taken at any moment on purpose.
    if layout_sweep && std::env::var("NFS_FE_CLOCK").is_err() {
        // SAFETY: single-threaded start-up, before any screen is loaded
        unsafe { std::env::set_var("NFS_FE_CLOCK", "5000") };
    }
    // the `NFS_*` switches are read once from here on (`nfs_game::config`, F4): pin the snapshot after the two `set_var`s above
    nfs_game::config::init();
    let fe_start = if layout_sweep { fe_start.or_else(|| Some("QuickRace".to_string())) } else { fe_start };
    // A plain launch is the original's: main 0x4B6A50 plays the intro movies (0x4DD600, unless
    // NOINTROMOVIES) and enters the front end on `"gamesetup"` (0x4B6C52, `FE_ENTRY`), with the
    // profile prompt of the first entry. The port's text menu is a developer tool and opens only
    // with `--debug-menu`; a `--layout <name>` start keeps its own screen.
    let debug_menu = args.iter().any(|a| a == "--debug-menu");
    let fe_start = if use_menu && fe_start.is_none() && !debug_menu && !args.iter().any(|a| a == "--layout") {
        Some(app::FE_ENTRY.to_string())
    } else {
        fe_start
    };
    // the views the player cycles through come from the registry, like everything else in `fe.txt`
    // a `--replay`'s own list when the file carries one (`Replay_begin` 0x413CF0 restores the block's `.CAMERA`)
    let camera_cycle = match replay_setup.as_ref().and_then(|r| r.camera.as_ref()).filter(|c| !c.is_empty()) {
        Some(c) => c.iter().map(|&v| v as u16).collect(),
        None => nfs_game::camera::cycle_from_registry(&nfs_formats::fetxt::Registry::load(&paths.root), 0),
    };
    tracing::info!(cycle = ?camera_cycle, "camera cycle");
    // diagnostic: `NFS_CAMERA=<index into the cycle>` starts on that view instead of the first
    let camera_mode = std::env::var("NFS_CAMERA")
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(0);
    let autotest = args
        .iter()
        .position(|a| a == "--autotest")
        .and_then(|i| args.get(i + 1)?.parse().ok());
    let follow_road = args.iter().any(|a| a == "--follow");
    // `--net` without `--opponents` still needs one seat for the other player
    let num_opponents_default = u32::from(args.iter().any(|a| a == "--net"));
    let num_opponents: u32 = replay_setup
        .as_ref()
        .map(|r| r.opponents)
        .or_else(|| career_request.as_ref().map(|r| r.opponents.len() as u32))
        .or_else(|| {
            args.iter()
                .position(|a| a == "--opponents")
                .and_then(|i| args.get(i + 1)?.parse().ok())
        })
        .unwrap_or(num_opponents_default);
    let num_laps: u32 = replay_setup
        .as_ref()
        .map(|r| r.laps)
        .or_else(|| career_request.as_ref().map(|r| r.laps))
        .or_else(|| {
            args.iter()
                .position(|a| a == "--laps")
                .and_then(|i| args.get(i + 1)?.parse().ok())
        })
        .unwrap_or(2)
        .max(1);
    // Quick Knockout: laps = opponents (modes.md §4)
    let num_laps = if args.iter().any(|a| a == "--quick-knockout") {
        num_opponents.max(1)
    } else {
        num_laps
    };
    // `RAND_SEED`: `fe.txt`'s `rand_seed` (see `start_race`)
    let fe_rand_seed = nfs_formats::fetxt::Registry::load(&paths.root).u32("rand_seed", 0);
    let record = args
        .iter()
        .position(|a| a == "--record")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| {
            let mut w = std::io::BufWriter::new(std::fs::File::create(p).ok()?);
            use std::io::Write;
            writeln!(
                w,
                "tick,throttle,brake,steer,handbrake,x,y,z,vx,vy,vz,rpm,gear,slice"
            )
            .ok()?;
            Some(w)
        });
    let backwards = match &replay_setup {
        Some(r) => r.backwards,
        None => {
            args.iter().any(|a| a == "--backwards")
                || career_request
                    .as_ref()
                    .map(|r| r.backwards)
                    .unwrap_or(false)
        }
    };
    let records_path = std::env::var_os("NFS5_RECORDS_SAVE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Records::default_save_path);
    let records = Records::load(&paths, &records_path);
    // race options from the front-end registry (shipped: skill 0, one lap, no traffic, no cops)
    let fe = nfs_formats::fetxt::RaceSettings::from_registry(&nfs_formats::fetxt::Registry::load(
        &paths.root,
    ));
    let track_tables = nfs_game::track_list::TrackListTables::load(&paths);
    let (track_all, track_looped) = track_tables.build(nfs_game::track_list::START_YEAR, 0);
    let menu = if use_menu {
        // `GenericCar.Car`'s own list, `[0x6919EC]`, is rebuilt by `0x4F3E40` (kind-0 records only
        // — `record+0x1F4 == 0`, 0x4F400E…0x4F4087, `nfs-formats/src/fedata.rs`'s own `kind` field,
        // "only kind 0 may be listed or raced" — plus the `+0x204` dealer-availability admit rule,
        // [`dealer_flag_admitted`]) and then sorted by **0x4F0F50** with the comparator **0x4F1D40**:
        // era `+0x1B0` first, then class `+0x1AC`, then first year `+0x1F8` (signed word), all
        // three ascending, swapping the instant a later record compares less — not a stable sort
        // (`era_class_year_sort`, above, is the same exchange walk instruction for instruction;
        // `career.md` §10.9's "Dealer / garage order" cites the same two addresses for the
        // dealer/garage list, `App::dealer_cars`, which now shares this same routine). *(Until this
        // pass: `!c.sim.is_empty()` for the filter — 6 of the 93 sim-bearing records are kind-2 cop
        // cars with a drivable model (`Cop 930`/`930 German`/`Cop 356B`/`356B German`/`Cop 993
        // Turbo`, `+/German` pairs plus one more), wrongly admitted; and `cars.sort_by_key(|c|
        // c.year)`, which is not the original's order past row 0. Then, until this pass: no
        // `+0x204` test at all, so every kind-0 record showed regardless of `Game.GameType` — see
        // [`dealer_flag_admitted`]'s doc for the two arms and the live original check that settled
        // them; `Game.GameType` is 0 (QuickRace) at start-up, `controls.md`:9519.)*
        let cars = generic_car_list(&catalog, 0);
        // `GenericCar2.Car`'s own list `[0x6919F4]` is a **separate** rebuild, not `cars` again —
        // see `Menu::cars2`'s field doc and `Menu::set_cars2`. Built here for the same chronicle
        // year `tracks_year` below starts at (`nfs_game::track_list::START_YEAR`).
        let mut cars2: Vec<_> = catalog.iter().filter(|c| c.kind == 0 && c.year as i32 <= nfs_game::track_list::START_YEAR).cloned().collect();
        era_class_year_sort(&mut cars2, era_class_year_key);
        // **`0x4FC730`** ("reset track list") zeroes `zMergedList.Car`'s catalogue-half index —
        // among the seven `Track<i>` rows, `Knockout.TrackNumber` and `LoopedTrack.Track` it also
        // resets — and `modes.md` §2 confirms it runs **both** at accessor registration (i.e. from
        // the very first frame, before any profile) and again on every `SETTYPEQUICKRACE`/
        // `SETTYPEKNOCKOUT`/`SETTYPEQUICKKNOCKOUT` (`Menu::reset_track_rows`, ported). So the
        // catalogue always starts on row 0 — the (flag-filtered, sorted) list's first entry —
        // unless the player has picked something else. `NFS_FE_CAR=<display name substring>` still
        // overrides it, for stills of the showroom (the same kind of diagnostic as `NFS_MENU_MODE`).
        // *(Until 2026-09-29 the no-override default hunted the list for a modern 993 coupe
        // instead — a leftover convenience from the menu's very first cut, never RE-grounded,
        // which left every screen reached before a `SETTYPE*` reset — including this port's own
        // `--fe=QuickRace`/`quickknockout`/`knockout` scripted goldens — opening on a `911
        // Carrera` where the original opens on the catalogue's first car, `REFERENCE-2026-09-28.md`
        // "Pass 10", `QuickRace`(a). That fix (`b5f1896e`) landed on the *unfiltered* kind-0 sort's
        // row 0, `'50 356 1100 Cabriolet` — this pass's `+0x204` filter (see
        // [`dealer_flag_admitted`]) moves QuickRace's row 0 to `'59 356 A 1600 Super Convertible D`
        // instead, matching the reference capture's own read of the original's default car, `356 A
        // Convertible`, which `'50 356 1100 Cabriolet` does not.)*
        let car_idx = std::env::var("NFS_FE_CAR")
            .ok()
            .and_then(|want| cars.iter().position(|c| c.display_name.contains(want.as_str())))
            .unwrap_or(0);
        // [`default_paint_colour`]/[`default_paint_row`]: `GenericCar.Colour` does not start on
        // row 0 of whichever car ends up default — its `FEColourAccessor` ctor (0x4B91F0) reads
        // `nfs5.car`'s own **record 0** once, at registration, and the row shown for the actual
        // default car is whichever of *its* rows carries that fixed colour.
        let default_paint_idx = cars.get(car_idx).map_or(0, |c| default_paint_row(default_paint_colour(&catalog), &c.paint_sets));
        let mut menu0 = Menu {
            mode: std::env::var("NFS_MENU_MODE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            // 0x4FBC10 as the `nfs5.trk` loader's tail 0x4FBA67 runs it: before any profile, on
            // the chronicle year's initial value `[0x5DB764]` = 0x79E and `Game.GameType` 0
            tracks: track_all,
            looped: track_looped,
            tracks_year: nfs_game::track_list::START_YEAR,
            cars,
            cars2,
            track_idx: 0,
            car_idx,
            car_garage: false,
            merged_at_zero: true,
            car2_idx: 0,
            ko: Vec::new(),
            ko_sel: 0,
            opponents: 4,
            ko_opponents: 4,
            damage: true,
            difficulty: 1,
            opp_car: std::env::var("NFS_OPPCAR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            paint_idx: default_paint_idx,
            paint_custom: None,
            laps: num_laps,
            traffic: std::env::var("NFS_TRAFFIC")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(fe.traffic_density as u32),
            cops: std::env::var("NFS_COPS")
                .map(|v| v != "0")
                .unwrap_or(fe.cops),
            backwards,
            mirror: false,
            circuits: records.circuits(),
            looped_idx: 0,
            knockout_line: "KNOCKOUT   ENTER STARTS A SERIES".into(),
            row: 0,
            font: None,
        };
        menu0.fit_ko();
        Some(menu0)
    } else {
        None
    };
    if let (Some(m), Some(c)) = (&menu, career.as_mut()) {
        c.set_mode(m.mode);
    }
    let mission_start = career_request.as_ref().and_then(|r| r.mission.clone()).map(cone_limit_diagnostic).or_else(|| {
        // a recorded mission's launch words (`Replay_begin` restores the block the HUD reads): the mission they describe
        let rs = replay_setup.as_ref()?;
        let m = career_ui::MissionStart::from_replay_words(&paths, rs.mission.as_ref()?, rs.scene);
        if m.is_none() {
            tracing::info!("replay: the recorded mission matches no nfs5.fac record, no mission HUD (APPROXIMATION)");
        }
        m
    });

    // `--net-browse N`: probe for N seconds and print the sessions that answer, then exit. The
    // same `'w'` / `'d'` exchange the network screens run, without a window.
    if let Some(secs) = args
        .iter()
        .position(|a| a == "--net-browse")
        .and_then(|i| args.get(i + 1)?.parse::<f32>().ok())
    {
        let mut lobby = netplay::Lobby::open("browser")?;
        let until = std::time::Instant::now() + Duration::from_secs_f32(secs);
        while std::time::Instant::now() < until {
            lobby.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        let games = lobby.rows();
        println!("{} session(s):", games.len());
        for g in &games {
            println!("  {}   {}/{}   {}   ping {}", g.name, g.players, g.limit, g.addr, g.ping.map_or("-".to_string(), |p| p.to_string()));
        }
        return Ok(());
    }

    // `--net host[:PORT]` / `--net join:HOST[:PORT]`: two copies of the port in one race.
    let net = match args
        .iter()
        .position(|a| a == "--net")
        .and_then(|i| args.get(i + 1))
    {
        Some(spec) => {
            let who = args
                .iter()
                .position(|a| a == "--net-name")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| {
                    if spec.starts_with("host") {
                        "host".into()
                    } else {
                        "racer".into()
                    }
                });
            Some(netplay::NetPlay::start(spec, &who)?)
        }
        None => None,
    };

    let mut audio_settings = nfs_audio::music::Settings::load(&paths.root);
    // **The front end's five level accessors start at their constructors' 0x7F, not at `fe.txt`'s
    // `MUSICLEVEL` … `AMBIENTLEVEL`**: 0x4E4910, the Audio data area's slot 0 that
    // 0x4E5D00 runs at every front-end entry (0x4D36D7), builds `Audio.MusicLevel` / `SfxLevel` /
    // `EngineLevel` / `SpeechLevel` / `AmbientLevel` with `0x4BA310(name, 0x37…0x3B, 0x7F, 0, 0x7F,
    // 0, cb)` (0x4E4928…0x4E4A5A); the third argument is the value 0x4BA362's `SetSelection` stores.
    // Nothing reads the registry words `[0x657CA0…0x657CB0]` into them (their readers are the race
    // audio's 0x4A8A80 / 0x4AB150 / 0x4AB200 / 0x4AB270 / 0x4AB360, the launch dump's 0x4B4DA0 that
    // *writes* them from the accessors, and the movie's 0x4E27BA) — only a profile's
    // `LoadAccessors` changes them. A race without the front end keeps the registry's.
    // `[0x657CA0]` as the registry load left it, before the accessors take their own
    let registry_music_level = audio_settings.music_level;
    if use_menu {
        audio_settings.front_end_defaults();
    }
    // `FEStrings.loc` / `hud.loc` in the current language (0x4E04F0, 0x41E170)
    let fe_strings = nfs_game::layout::load_strings(&paths);
    // `Replay_begin` 0x413CF0 copies the whole saved block back over `0x6573E8` (0x413DCF) and
    // restores only the machine-local options after it, so `SKILL`, `COPS` and `DAMAGE` come from
    // the replay. *(Until the audit of 2026-09-26 `--replay` took all three from the registry.)*
    let race_skill = replay_setup.as_ref().map_or(fe.skill, |r| r.skill);
    let music_names = nfs_audio::music::song_titles(&paths.game_data().join("Music")).unwrap_or_default();
    let music_lengths = nfs_audio::music::song_lengths_ms(&paths.game_data().join("Music")).unwrap_or_default();
    // The CD player's own list `[0x65B62C]`, not `fe.txt`'s `PLAYLIST=`/`NUMPLAYLISTSONGS=`
    // (`[0x657CB8]`/`[0x657CBC]`, the *race* pick `AudioLoadTrack` 0x4A88A0 reads directly and
    // `Settings::load` above already carries): `0x4BE3D0`, the CD player's song-list rescan, is
    // called exactly once — from the front-end manager's own init (`0x4D36D2`, no other caller,
    // `calls.py 0x4BE3D0`) — and it unconditionally replaces `[0x65B62C]` with a fresh vector,
    // then walks every record of the freshly rescanned `game*` table (`0x4ADE20("game*", 0)`) and
    // appends each one's stream index (disassembly at `0x4BE3D0`: the `mov dword ptr [0x65b62c],
    // eax`/`ebp` reset at `+0x1F`/`+0x26`, then the per-record `game` prefix compare at `+0xB7..
    // +0xD9` gating the same append call `0x46dae0` that `ADDSONG`'s `0x4BE670` uses). So every
    // launch starts the CD player's own selection at *every* `game*` track, independent of the
    // registry — which is exactly `native_TEST.sav`'s `CDPlayer` module on a brand-new profile
    // (14 tracks, `0..13`, `savegame.md` "Genuine save 2026-09-28"). A profile load overrides
    // this from its own `.sav` `CDPlayer` module (`apply_saved_profile`, `set_cd_playlist`); a
    // brand-new profile has none, so this start-up default is what a fresh save carries.
    audio_settings.playlist = music_names.iter().enumerate().filter(|(_, n)| nfs_audio::music::pattern_match(n, "game*")).map(|(i, _)| i).collect();
    // a `--career` race in a garage car wears the car's own appearance — its colours (a saved
    // custom one included, `OwnedCar::custom`) and race number — as the front end's start of the
    // same event does (`start_tournament_from_layout`)
    let cli_owned = career_request.as_ref().and_then(|r| {
        let ui = career.as_ref()?;
        let o = ui.career.garage.get(ui.pending.as_ref()?.garage_idx?)?;
        (!r.car.paint_sets.is_empty()).then(|| (App::paint_of(&r.car, o), o.trim.decal))
    });
    let mut app = App {
        fe: FrontEnd {
            era_music: false,
            bar_grab: None,
            chronicle_group: CHRONICLE_GROUP,
            chronicle_col: None,
            chronicle_sel: 0,
            chronicle_year_idx: None,
            chronicle_slider_drag: None,
            picker_prev: None,
            generic_colour_row: None,
            usedcars_showparts: false,
            usedcar_sel: 0,
            dealer: DealerSel::default(),
            compare_dealer: false,
            remaining_sel: 0,
            parts_sel: None,
            setting_repeat: None,
            mouse_down: false,
            repair_ticks: Default::default(),
            binding_base: None,
            controllers_sel: 0,
            pads_sel: 0,
            resolution_sel: 0,
            owned_seen: None,
            resolution_race: None,
            parts_forced: false,
            last_hover: None,
            dialog: None,
            dialog_forced: false,
            dialog_answer: None,
            dialog_input: None,
            replay_sel: 0,
            user_sel: None,
            driver_picture: std::env::var("NFS_FE_DRIVER_PICTURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(nfs_game::save::DEFAULT_PICTURE as i32),
            cd_available: None,
            cd_selected: None,
            cd_title: 0,
            cd_current: -1,
            after_replay: false,
            test_drive: None,
            game_type: 0,
            // **Diagnostic glue**: `--fe=local` skips the `SETMODELAN` button that leads to that
            // screen (0x510E00 writes `Comms.GameMode` 1), and the games list's row draw
            // (0x4BA030) is laid out by that mode, so the direct entry starts in it
            comms_game_mode: u32::from(fe_start.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("local"))),
            update_check: None,
            camera_current: 0,
            camera_list: [0, 1, 2, 3],
            camera_pic_click: Default::default(),
            race_game_type: None,
            xxdefxx: None,
            track_tables,
            cursor: None,
            // both of `Parts.AllParts`'s sections start closed (0x515190, 0x515193)
            parts_open: [false, false],
            shop: parts_shop::PartsShop::default(),
            factory_profile: 0,
            menu,
            menu_template: None,
            room: Vec::new(),
            cars: Vec::new(),
            cars_key: Vec::new(),
            cars_env: None,
            // `NFS_FE_CLOCK` preloads this the same way it preloads `LayoutScreen::clock_ms`
            // (layout.rs), so the turntable (`room.spin_turns`, `frontend-3d.md` §3) can be
            // pinned for a still — the room's clock was previously never seeded from it at all
            ms: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            dealer_row: None,
            dealer_prev: None,
            room_car_half: None,
            garage_shadow_prev: None,
            garage_shadow_tex: None,
            victory_movie: None,
            track_room: None,
            room_obj: None,
            track_art: None,
            trk_table: None,
            track_turn_back: false,
            layout_preview: None,
            layout_bottom: None,
            music_current: String::new(),
            list_current: String::new(),
            main_music_stamp: 0,
            timer_origin: std::time::Instant::now(),
            loading: None,
            loading_still: std::env::var("NFS_FE_LOADING").ok().and_then(|v| v.trim().parse().ok()),
            race_loading: None,
            race_loading_still: std::env::var("NFS_RACE_LOADING").ok().and_then(|v| v.trim().parse().ok()),
            startup_still: None,
            strings: fe_strings,
            layout_name: args
                .iter()
                .position(|a| a == "--layout")
                .and_then(|i| args.get(i + 1).cloned())
                .or(fe_start),
            layout_sel: 0,
            layout_nav_used: false,
            layout_open_combo: None,
            list_drag: None,
            combo_drag: None,
            list_repeat: None,
            combo_arrow_repeat: None,
            nav: nfs_game::layout::nav::Nav::new(),
            layout_under: Vec::new(),
        },
        layout_sweep,
        registry: nfs_formats::fetxt::Registry::load(&paths.root),
        language: paths.language(),
        choices: std::collections::HashMap::new(),
        hud_cfg: std::cell::RefCell::new(HudCfgCache::default()),
        paths,
        net: Network { session: net, leaving: Vec::new(), lobby: None, start_due: false, join_notified: false, lobby_sent: false, lobby_car: None, lobby_setsel: false, race: None, connect_pending: false, connect_opened: false },
        career,
        career_opponents: career_request.map(|r| r.opponents),
        cheat_bits_live: None,
        // diagnostic: `NFS_OPPCAR=0..4` picks `OpponentCar.Car` without going through the menu
        opp_choice: std::env::var("NFS_OPPCAR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        player_car_id: None,
        pad_stick_latched: false,
        skill,
        smackables: Default::default(),
        mission: mission_start,
        prev_car_pos: Vec3::ZERO,
        world_actions: Vec::new(),
        render_prev: None,
        local_pose: (Vec3::ZERO, Quat::IDENTITY),
        local_prev: None,
        frame_dt: 1.0 / 60.0,
        fe_ms_carry: 0.0,
        pause_ticks: 0.0,
        pause_offset: 0.0,
        hud_clock_base: 0.0,
        hud_scale: std::cell::Cell::new(1.0),
        tray_slide: (0, 0),
        time_trays: (0, 0),
        map_slide: (0, 0),
        mirror_slide: (0, 0),
        hud_keys: HudKeys::new(1, false, true),
        replay_bar: nfs_game::replay::BarState::new(),
        bar_press: None,
        hud_key_script: std::env::var("NFS_HUD_KEYS_1").or_else(|_| std::env::var("NFS_HUD_KEYS")).map(|s| parse_hud_keys(&s)).unwrap_or_default(),
        scripted_race_no: 0,
        race_order: Default::default(),
        audio_paused: false,
        scripted_pause: false,
        scripted_restart: false,
        scripted_net_leave: false,
        scripted_quit: false,
        results_billboard: None,
        billboard_key: 0,
        paused: false,
        knockout: None,
        opp_cars: Vec::new(),
        knockout_round: false,
        player_grid_override: None,
        net_fe_txt: Default::default(),
        net_nocollision: false,
        net_seat_grids: Vec::new(),
        net_seat_cars: Vec::new(),
        net_own_words: Vec::new(),
        exchange_in: None,
        replay_net: None,
        replay_view: 0,
        net_derby: false,
        net_flags: None,
        net_traffic: Vec::new(),
        crt_rand: nfs_game::movie_overlay::CRand::new(1),
        backwards,
        mirror: replay_setup
            .as_ref()
            .map(|r| r.mirror)
            .unwrap_or_else(|| args.iter().any(|a| a == "--mirror")),
        // a recording's `RACE_TYPE` 3 (`Replay_begin` 0x413CF0's restore); no such word: the command line's
        quick_knockout: match replay_setup.as_ref().and_then(|r| r.race_type) {
            Some(t) => t == 3,
            None => args.iter().any(|a| a == "--quick-knockout"),
        },
        player_eliminated: false,
        net_time_sent: false,
        net_pad_buttons: 0,
        records,
        replay: Playback {
            stream: Some(match &replay_file {
                Some(f) => nfs_game::replay::Replay::playing(f.stream.clone(), f.final_tick),
                None => nfs_game::replay::Replay::new(),
            }),
            speed: nfs_game::replay::Speed::Normal,
            race_over: false,
            setup: replay_setup
                .clone()
                .unwrap_or(nfs_game::replay::ReplaySetup {
                    track: track_name.to_string(),
                    car: car_name.to_string(),
                    sim: sim_name.clone(),
                    car_id: catalog
                        .iter()
                        .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
                        .map(|c| c.id)
                        .unwrap_or(0),
                    paint,
                    laps: num_laps,
                    opponents: num_opponents,
                    traffic: std::env::var("NFS_TRAFFIC")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(fe.traffic_density as u32),
                    cops: std::env::var("NFS_COPS")
                        .map(|v| v != "0")
                        .unwrap_or(fe.cops),
                    // `Game.Damage`'s accessor starts on (0x4E55DD's initial 1); `fe.txt` has no key
                    damage: true,
                    backwards,
                    mirror: args.iter().any(|a| a == "--mirror"),
                    seed: 0,
                    skill: fe.skill,
                    // overwritten by `start_race`'s own rebuild before this is read for a fresh
                    // (non-replay) race; this placeholder only shows if that never runs
                    driver_name: String::new(),
                    net: None,
                    race_type: None,
                    fegame_type: None,
                    camera: None,
                    ..Default::default()
                }),
            path: nfs_game::save::save_dir().join("replay.prpl"),
            saved: false,
            loop_count: 0,
            camera: 0,
            frozen: false,
        },
        // the original seeds the race RNG from the registry's RAND_SEED (replay.md §11); a replay
        // carries the seed so playback re-simulates the same traffic, police and AI
        // (**test glue**: `NFS_CHEAT_BITS` sets the cheat bits a name-box code would, for the scripted runs)
        cheat_bits: {
            let bits: u32 = std::env::var("NFS_CHEAT_BITS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
            // a recording's `DERBY` word (0x1B, the block `Replay_begin` restores)
            match replay_setup.as_ref().and_then(|r| r.derby) {
                Some(true) => bits | (1 << 7),
                Some(false) => bits & !(1 << 7),
                None => bits,
            }
        },
        traffic_seed: match (
            &replay_setup,
            std::env::var("NFS_TRAFFIC_SEED")
                .ok()
                .and_then(|v| v.parse().ok()),
        ) {
            (Some(r), _) => r.seed,
            (None, Some(s)) => s,
            // `fe.txt`'s `rand_seed`, as `start_race` reads it for every live race
            (None, None) => fe_rand_seed,
        },
        scripted: autotest.is_some(),
        stuck_ticks: 0,
        detail: std::env::var("NFS_DETAIL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        records_path,
        car_id: catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| c.id)
            .unwrap_or(0),
        player_name: nfs_game::save::read_player_lst(&nfs_game::save::save_dir())
            .and_then(|s| nfs_game::save::Profile::from_stem(&s))
            .map(|p| p.name)
            .unwrap_or_else(|| "DRIVER".to_string()),
        hud_records: None,
        records_spoken: false,
        results_page: 0,
        page_seen: None,
        page_frames: 0,
        page_open_tick: None,
        page_closing: None,
        pause_sel: 0,
        pause_confirm: None,
        hud_hover: None,
        replay_save: ReplaySave::default(),
        bar_script: std::env::var("NFS_REPLAY_PRESS")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|e| {
                        let (t, c) = e.split_once(':')?;
                        Some((t.trim().parse().ok()?, c.trim().parse().ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        hover_script: std::env::var("NFS_REPLAY_HOVER")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|e| {
                        let (t, c) = e.split_once(':')?;
                        let c = c.trim();
                        Some((t.trim().parse().ok()?, if c == "off" { None } else { Some(c.parse().ok()?) }))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        results: None,
        auto_replay_due: false,
        results_page_tick: 0,
        mission_result: None,
        cone_flash: 0,
        hud_cones: Default::default(),
        start_damage: [0; 8],
        track,
        car,
        shot_target: (screenshot.is_some() || std::env::var_os("NFS_RACE_LOADING").is_some() || std::env::var_os("NFS_LOADING_SHOT").is_some()).then(shot_geometry),
        screenshot,
        autotest,
        fe_rng: nfs_game::opponents::FeRng::new(0),
        race_pass: false,
        session_races: std::env::var("NFS_AUTOTEST_RACES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1),
        session_replayed: false,
        intro: None,
        intro_done: None,
        idle_movie_due: false,
        idle_movie_running: false,
        window: None,
        renderer: None,
        world: None,
        car_model: None,
        car_lod: Default::default(),
        driver_cam: None,
        // `NFS_CAMERA=0..3` picks the starting view (chase / bonnet / trackside / cockpit) —
        // the port's own switch, for screenshots and tests
        sky: None,
        scene: Scene::default(),
        camera: Camera::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        ff: None,
        ff_levels: nfs_physics::force_feedback::FfLevels { total: 50, road: 50, engine: 50, collision: 50, grip: 50 },
        car_pos: Vec3::ZERO,
        car_rot: Quat::IDENTITY,
        specs,
        vehicle: None,
        camera_mode,
        weather_kept: None,
        anim_cam: AnimCamSlot::Off,
        anim_cam_counter: 0,
        cam_anims: Vec::new(),
        camera_cycle,
        skid: nfs_game::skidmarks::SkidMarks::new(),
        skid_pool: Vec::new(),
        particle_draw: None,
        shadow_draw: None,
        shadow_meshes: Default::default(),
        glare_draw: None,
        glare_star: [None, None],
        lamp_pools: LampPools::default(),
        flare_defs: Vec::new(),
        flare_sprites: std::collections::HashMap::new(),
        sun_sprites: std::collections::HashMap::new(),
        lens_sprites: std::collections::HashMap::new(),
        lens: Default::default(),
        track_particles: None,
        walk_calls: 0,
        track_particle_defs: Vec::new(),
        track_particle_sprites: std::collections::HashMap::new(),
        car_effects: None,
        car_effect_defs: Vec::new(),
        weather: None,
        weather_draws: [None, None],
        splat_draw: None,
        streak_draw: None,
        article_draws: Vec::new(),
        draw_distance: 700.0,
        track_item_range: (0, 0),
        effect_item_range: (0, 0),
        blended_dyn_items: Vec::new(),
        scratch: Default::default(),
        order_cache: Default::default(),
        order_key: Default::default(),
        view_far: 700.0,
        pool_used: PoolUse::default(),
        dyn_pose: None,
        test_alpha: None,
        sky_item_range: (0, 0),
        particles_left_ref_one: false,
        track_cameras: None,
        slices: None,
        race: None,
        road: RoadCars {
            opponents: Vec::new(),
            opponent_models: Vec::new(),
            opp_prev: Vec::new(),
            opp_steer_prev: Vec::new(),
            player_steer_prev: None,
            traffic: None,
            traffic_models: Vec::new(),
            opponent_sets: Vec::new(),
            traffic_sets: Vec::new(),
            opponent_paints: Vec::new(),
            opponent_families: Vec::new(),
            traffic_paints: Vec::new(),
            police: None,
            police_models: Vec::new(),
            police_sets: Vec::new(),
            police_specs: Vec::new(),
            police_prev: Vec::new(),
            // the recording's `RACE_TYPE` 1 (the cops race) / `FEGAME_TYPE` 9 (a mission's two cops)
            cops_race: replay_setup.as_ref().is_some_and(|r| r.race_type == Some(1)),
            cop_count: if replay_setup.as_ref().is_some_and(|r| r.fegame_type == Some(9)) { 2 } else { 4 },
            cops_enabled: replay_setup.as_ref().map(|r| r.cops).unwrap_or_else(|| {
                std::env::var("NFS_COPS").map(|v| v != "0").unwrap_or(fe.cops)
            }),
            traffic_prev: Vec::new(),
            traffic_specs: None,
            traffic_spin: Vec::new(),
            opponent_lods: Vec::new(),
            police_lods: Vec::new(),
            traffic_lods: Vec::new(),
            // the registry's `traffic_density`, or `NFS_TRAFFIC=0..3` for the menu-less --autotest start
            traffic_ids: None,
            traffic_colours: Vec::new(),
            damage_on: replay_setup.as_ref().is_none_or(|r| r.damage),
            traffic_count: TRAFFIC_COUNT[replay_setup
                .as_ref()
                .map(|r| r.traffic as usize)
                .unwrap_or_else(|| {
                    std::env::var("NFS_TRAFFIC")
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(fe.traffic_density as usize)
                })
                .min(3)],
        },
        racing_line: None,
        num_opponents,
        player_grid: 0,
        hud_metres: Vec::new(),
        rear_mirror: None,
        mirror_tex: None,
        wrong_way_blink: (0, 0, true),
        record,
        net_dump: None,
        exchange_tick_at: Instant::now(),
        net_notice: None,
        net_notice_space: false,
        net_lag_box: false,
        num_laps,
        derby_hud_laps: None,
        follow_road,
        car_half_width: 0.9,
        chase_offset: None,
        chase_settle: 0,
        slot_view: None,
        slot_key_enters: 0,
        fe0: !use_menu && career_id.is_none() && !mission_flag && replay_file.is_none(),
        slot_seen: (0, 0),
        look_yaw: None,
        look_held: [false; 3],
        lamp_bits: 0,
        hud_hidden: false,
        orbit: Orbit::default(),
        head_ramp: HeadRamp::default(),
        camera_tick: 0,
        cam_hist: Default::default(),
        colour_idx: Default::default(),
        flip_side: None,
        slot_eye: None,
        replay_wander: Default::default(),
        circle: (nfs_game::camera::CircleCam::default(), 0xFFFF, 0),
        cam_slice: None,
        wall_hits: 0,
        resets: 0,
        hud: None,
        dials: catalog
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&sim_name))
            .map(|c| (c.dial_kmh, c.dial_mph, c.dial_tach)),
        paint,
        owned_paint: cli_owned.map(|(p, _)| p),
        owned_decal: cli_owned.map(|(_, d)| d),
        snd: AudioState {
            audio: None,
            sfx_saved: None,
            registry_music_level,
            engine_sound: None,
            sfx: None,
            ai_engines: None,
            slots: Default::default(),
            crowding: 128,
            ambience: None,
            audio_triggers: Vec::new(),
            reverb: nfs_audio::reverb::Reverb::default(),
            music: None,
            music_names,
            music_lengths,
            audio_settings,
            speech: None,
            countdown_said: 0,
            sound_set,
            last_gear: 1,
            crash_played: false,
            ai_sfx: std::collections::HashMap::new(),
            player_loops: Default::default(),
            fe_sfx: None,
        },
        sim_name: sim_name.clone(),
        race_skill,
        last_frame: Instant::now(),
        presented: None,
        idle_check: None,
        accumulator: Duration::ZERO,
        tick: 0,
        cop_blips: Vec::new(),
        frames: 0,
        fps_timer: Instant::now(),
    };
    // the player's key bindings are `fe.txt`'s `C1_*` lines (`nfs_input::bindings`)
    app.input.bindings = nfs_input::bindings::Bindings::from_registry(|k| app.registry.get(k));
    // …then the `Controller.*` accessors' factory values: once it has built them, 0x51E89B
    // counts the listed devices and, with more than one, runs 0x51D1D0 on row 1's (0x51F230(1))
    // and puts `Controller.AllControllers` there (slot 0x2C) — else on device 0, the keyboard.
    // The port's list has no keyboard row, so that is its first pad, row 0.
    app.input.pads = app.gamepads.records();
    let first_pad = (!app.input.pads.is_empty()).then_some(0);
    app.input.default_controls(first_pad, |n| accessors::controller_action(n).and_then(nfs_input::binding_id));
    // 0x4DE880 at 0x4D3B76: `XXDefXX` is the state before any profile is applied
    app.fe.xxdefxx = Some(app.startup_side());
    // …and the profile's own, when it has saved any — with the rest of what `LoadInfo` restores.
    // **N03a:** not on the first entry to the front end (`FE_ENTRY`, the plain launch): 0x4D3B50
    // jumps over `LoadInfo` there (`[0x5D9158]` set) and the accessors keep their start-up values
    // until the profile chooser's `SETNAMETHENBACK` / `CREATEPLAYERDONE` loads one
    // (`fe_callbacks.rs`, both call `apply_saved_profile`). `--fe=<screen>`, `--layout`,
    // `--career`, `--mission` and every race start never meet the chooser and keep the profile.
    if !(app.fe.menu.is_some() && app.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(app::FE_ENTRY))) {
        app.apply_saved_profile();
    }
    // a network race's replay file: its humans, their cars and the traffic (`replay.md` §18.15)
    app.apply_replay_net();
    // every `<Axis>.Min` / `.Max` starts at 0 / 0x80 (0x51E546, 0x51E550)
    app.input.axis_bands = [[0, 0x80]; 8];
    event_loop.run_app(&mut app)?;
    Ok(())
}


