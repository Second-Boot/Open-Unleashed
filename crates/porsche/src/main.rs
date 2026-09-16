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
//! porsche [track] [car] [sim] [--screenshot out.png] [--autotest N [--follow]]   # direct race
//! keys: arrows = steer/throttle/brake, space = handbrake, C = camera, R = reset to track, Esc = quit
//! ```

use anyhow::Result;
use glam::{Mat4, Quat, Vec3};
use nfs_assets::{car::CarAssets, track::TrackAssets, GamePaths};
use nfs_game::{race::{grid_pose, RaceProgress, SliceTable}, sky::Sky, text::BitmapFont, world::{CarModel, TrackWorld}};
use nfs_input::{gamepad::Gamepads, Action, InputState};
use nfs_physics::{specs::CarSpecs, vehicle::{Controls, Vehicle}};
use nfs_renderer::{Camera, Renderer, Scene, Sprite, TextureData, TextureId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// HUD art from `GameData/DashHud` (speedometer dial + needle). The needle sweep was read off the dial
/// image (0 at ≈6:30, 300 at ≈4:45); the original's HUD placement code (`HSpeedometer`) has not been
/// analysed, so position/size are native choices.
struct Hud {
    dial: TextureId,
    needle: TextureId,
    dial_max_kmh: f32,
    /// countdown digits 3, 2, 1 (solid) from `321go.fsh` / `321go2.fsh`
    digits: [Option<TextureId>; 3],
    font: Option<BitmapFont>,
}

/// Race-start countdown (CONFIRMED values, `RE/notes/timing.md`): game ticks 256/320/384/448 =
/// 4/5/6/7 s; the race starts at tick 448 and brakes are forced until then.
const RACE_START_TICK: u64 = 448;
const COUNTDOWN_TICKS: [u64; 3] = [256, 320, 384];

impl Hud {
    fn load(r: &mut Renderer, paths: &GamePaths) -> Option<Self> {
        let dir = paths.game_data().join("DashHud");
        let load = |r: &mut Renderer, file: &str, entry: &str| -> Option<TextureId> {
            let p = GamePaths::find_file(&dir, file)?;
            let d = std::fs::read(p).ok()?;
            let f = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let i = f.entries.iter().position(|e| e.name.trim() == entry)?;
            let img = f.decode(&d, i).ok()?;
            Some(r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)))
        };
        let digits = [load(r, "321go.fsh", "3Sol"), load(r, "321go.fsh", "2Sol"), load(r, "321go2.fsh", "1Sol")];
        let font = GamePaths::find_file(&dir, "UniversC12b.ffn").and_then(|p| std::fs::read(p).ok()).and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()).map(|f| BitmapFont::upload(r, f));
        Some(Self { dial: load(r, "speedK.fsh", "300k")?, needle: load(r, "needle.fsh", "need")?, dial_max_kmh: 300.0, digits, font })
    }

    fn sprites(&self, speed_kmh: f32, tick: u64, gear: usize, race: Option<&RaceProgress>, num_laps: u32) -> Vec<Sprite> {
        let size = 0.28; // of screen height
        let pos = [0.02, 0.70];
        let dial = Sprite { texture: self.dial, pos, size: [size, size], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] };
        // needle: on the 300 km/h dial 0 sits at ≈6:30 (−0.46 turn) and 300 at ≈4:45 (+0.40 turn)
        let t = (speed_kmh / self.dial_max_kmh).clamp(0.0, 1.0);
        let rot = 0.46 - 0.86 * t; // positive = counter-clockwise on screen
        let nw = size * 0.06;
        let nh = size * 0.42;
        let needle = Sprite {
            texture: self.needle,
            pos: [pos[0] + size * 0.5 - nw * 0.5, pos[1] + size * 0.5 - nh],
            size: [nw, nh],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: rot,
            pivot: [0.5, 1.0],
        };
        let mut out = vec![dial, needle];
        if let Some(f) = &self.font {
            let gear_s = match gear { 0 => "R".to_string(), 1 => "N".to_string(), g => (g - 1).to_string() };
            out.extend(f.sprites(&format!("{} km/h  {gear_s}", speed_kmh as i32), [0.32, 0.93], 0.035, [1.0, 1.0, 1.0, 0.9]));
            if let Some(r) = race {
                let race_t = tick.saturating_sub(RACE_START_TICK) as f32 / 64.0;
                let lap_t = tick.saturating_sub(r.lap_start_tick.max(RACE_START_TICK)) as f32 / 64.0;
                let fmt = |t: f32| format!("{}:{:05.2}", (t / 60.0) as u32, t % 60.0);
                let line = if r.finished { format!("FINISHED  {}", fmt(race_t)) } else { format!("LAP {}/{}  {}", (r.laps + 1).min(num_laps), num_laps, fmt(lap_t)) };
                out.extend(f.sprites(&line, [0.02, 0.03], 0.035, [1.0, 1.0, 1.0, 0.9]));
                if r.wrong_way_ticks > 16 {
                    out.extend(f.sprites("WRONG WAY", [0.42, 0.12], 0.05, [1.0, 0.3, 0.2, 1.0]));
                }
            }
        }
        // countdown digit for the current second
        if tick < RACE_START_TICK {
            let idx = COUNTDOWN_TICKS.iter().rposition(|&t| tick >= t);
            if let Some(i) = idx && let Some(tex) = self.digits[i] {
                let h = 0.22;
                out.push(Sprite { texture: tex, pos: [0.5 - h * 0.4, 0.3], size: [h * 0.8, h], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5] });
            }
        }
        out
    }
}

/// Original game tick: 1/64 s (see module docs).
const GAME_TICK: Duration = Duration::from_nanos(15_625_000);

/// Minimal frontend (M7 first cut): choose track and car, Enter starts the race.
/// The original's `.lay`-driven screens are not reproduced yet; this uses the catalogue tables.
struct Menu {
    tracks: Vec<String>,
    cars: Vec<nfs_formats::fedata::CarCatalogEntry>,
    track_idx: usize,
    car_idx: usize,
    row: u8,
    font: Option<BitmapFont>,
}

impl Menu {
    fn sprites(&self) -> Vec<Sprite> {
        let Some(f) = &self.font else { return Vec::new() };
        let mut out = f.sprites("PORSCHE UNLEASHED  -  NATIVE", [0.05, 0.08], 0.05, [1.0, 1.0, 1.0, 1.0]);
        let sel = |r: u8| if self.row == r { [1.0, 0.75, 0.1, 1.0] } else { [0.85, 0.85, 0.85, 1.0] };
        out.extend(f.sprites(&format!("TRACK   < {} >", self.tracks[self.track_idx].to_uppercase()), [0.08, 0.30], 0.045, sel(0)));
        let c = &self.cars[self.car_idx];
        out.extend(f.sprites(&format!("CAR     < {} >", c.display_name.to_uppercase()), [0.08, 0.40], 0.045, sel(1)));
        out.extend(f.sprites(&format!("{}  {}  {} cc", c.family, c.body, c.displacement), [0.16, 0.47], 0.03, [0.7, 0.7, 0.7, 1.0]));
        out.extend(f.sprites("UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER START   ESC QUIT", [0.08, 0.85], 0.028, [0.6, 0.6, 0.6, 1.0]));
        out
    }
}

struct App {
    paths: GamePaths,
    menu: Option<Menu>,
    /// menu kept for returning after a race
    menu_template: Option<Menu>,
    track: TrackAssets,
    car: CarAssets,
    screenshot: Option<String>,
    /// scripted drive before the screenshot: seconds of full throttle (steer right for the last third)
    autotest: Option<f32>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<TrackWorld>,
    car_model: Option<CarModel>,
    sky: Option<Sky>,
    scene: Scene,
    camera: Camera,
    input: InputState,
    gamepads: Gamepads,
    car_pos: Vec3,
    car_rot: Quat,
    specs: CarSpecs,
    vehicle: Option<Vehicle>,
    camera_mode: u8,
    slices: Option<SliceTable>,
    race: Option<RaceProgress>,
    num_laps: u32,
    /// autotest: steer toward the centreline (pure pursuit) instead of scripted input
    follow_road: bool,
    car_half_width: f32,
    wall_hits: u32,
    rolled_ticks: u32,
    resets: u32,
    hud: Option<Hud>,
    audio: Option<nfs_audio::AudioOutput>,
    engine_sound: Option<nfs_audio::engine::EngineSound>,
    music: Option<nfs_audio::music::Music>,
    sound_set: String,
    last_frame: Instant,
    accumulator: Duration,
    tick: u64,
    frames: u32,
    fps_timer: Instant,
}

impl App {
    fn simulate(&mut self) {
        self.tick += 1;
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        if self.tick % 2 == 0 {
            // 32 Hz: latch controller, run the force step (Sc32)
            let mut steer = (self.input.steer() * 126.0) as i32; // right = +x in the body frame
            if self.follow_road && let (Some(t), Some(r)) = (&self.slices, &self.race) {
                // pure pursuit on the slice ahead (autotest only; not the original AI)
                let ahead = t.add(r.slice, 4);
                let target = t.slices[ahead].centre;
                let to = v.orientation * (target - v.position); // body-frame vector (rows = axes)
                let angle = to.x.atan2(to.z.max(0.1));
                steer = ((angle / 0.4).clamp(-1.0, 1.0) * 126.0) as i32;
                // crude speed limit for the scripted drive: ease off above 110 km/h or in sharp bends
                let limit = if angle.abs() > 0.15 { 60.0 } else { 110.0 };
                if v.speed() * 3.6 > limit {
                    self.input.release(Action::Throttle);
                    self.input.press(Action::Brake);
                } else {
                    self.input.release(Action::Brake);
                }
            }
            let mut throttle = self.input.throttle();
            if self.follow_road {
                // autotest: ease off above 130 km/h and when steering hard
                if v.speed() > 36.0 || steer.abs() > 80 { throttle = throttle.min(0.35); }
            }
            // before the start tick the original forces full brake and no throttle (0x49DD20)
            v.race_started = self.tick >= RACE_START_TICK;
            let controls = Controls {
                throttle: (throttle * 254.0) as u8,
                brake: (self.input.brake() * 254.0) as u8,
                steer,
                handbrake: self.input.held(Action::Handbrake),
                gear_request: None,
            };
            v.step32(&controls, &self.specs, &w.collision);
            if v.collide_walls(&w.walls, self.car_half_width, 0.2, 0.15) {
                self.wall_hits += 1;
            }
            // reset to the current slice when the car has left the world or rolled over for > 2 s
            // (original: reset mode 3, 0x40F540, also bound to a key) — APPROXIMATION of the placement rule
            let fallen = self.slices.as_ref().zip(self.race.as_ref()).map(|(t, r)| v.position.y < t.slices[r.slice].centre.y - 15.0).unwrap_or(false);
            let rolled = v.cos_tilt < 0.3 || v.orientation.row(1).y < 0.3;
            self.rolled_ticks = if rolled { self.rolled_ticks + 1 } else { 0 };
            if (fallen || self.rolled_ticks > 64 || self.input.take_pressed(Action::RearView)) && let (Some(t), Some(r)) = (&self.slices, &self.race) {
                self.rolled_ticks = 0;
                self.resets += 1;
                let sl = &t.slices[r.slice];
                let fwd = sl.forward.normalize();
                let up = sl.up.normalize();
                let right = up.cross(fwd).normalize();
                v.position = sl.centre + up * (0.5 + self.car_model.as_ref().map(|c| c.wheel_radius).unwrap_or(0.3));
                v.orientation = glam::Mat3::from_cols(right, up, fwd).transpose();
                v.velocity = Vec3::ZERO;
                v.angular_velocity = Vec3::ZERO;
                v.rpm = 0.0;
                v.gear = 2;
                tracing::info!(slice = r.slice, "car reset to track");
            }
            if let (Some(t), Some(r)) = (&self.slices, self.race.as_mut()) {
                let lap_done = r.update(t, v.position, v.forward(), self.tick, self.num_laps);
                if lap_done {
                    tracing::info!(lap = r.laps, time_s = r.lap_times.last().map(|t| *t as f32 / 64.0), "lap completed");
                }
                if r.finished {
                    tracing::info!("race finished");
                }
            }
            if self.input.take_pressed(Action::CameraNext) {
                self.camera_mode = (self.camera_mode + 1) % 2;
            }
            self.input.end_tick();
        }
        v.step64(&self.specs);
        if self.tick % 32 == 0 {
            v.renormalise();
        }
        self.car_pos = v.position;
        self.car_rot = Quat::from_mat3(&v.orientation.transpose()).normalize();
        if let (Some(a), Some(e)) = (&self.audio, &self.engine_sound) {
            e.update(a, v.rpm.max(600.0), v.throttle, 0.5);
        }
    }

    fn rebuild_scene(&mut self) {
        self.scene.items.clear();
        self.scene.sprites.clear();
        if let (Some(h), Some(v)) = (&self.hud, &self.vehicle) {
            self.scene.sprites.extend(h.sprites(v.speed() * 3.6, self.tick, v.gear, self.race.as_ref(), self.num_laps));
        }
        let fwd = self.car_rot * Vec3::Z;
        if self.camera_mode == 0 {
            // chase: behind and above, looking slightly ahead
            let vel = self.vehicle.as_ref().map(|v| v.velocity).unwrap_or(Vec3::ZERO);
            let dir = if vel.length() > 3.0 { vel.normalize() } else { fwd };
            self.camera.position = self.car_pos - dir * 7.5 + Vec3::Y * 2.3;
            self.camera.target = self.car_pos + Vec3::Y * 0.9 + dir * 2.0;
        } else {
            // bonnet camera
            self.camera.position = self.car_pos + Vec3::Y * 1.05 + fwd * 0.6;
            self.camera.target = self.car_pos + Vec3::Y * 0.9 + fwd * 20.0;
        }
        self.camera.far = 20000.0;
        if let Some(s) = &self.sky {
            self.scene.items.extend(s.draw_items(self.camera.position));
        }
        if let Some(w) = &self.world {
            self.scene.items.extend(w.items.iter().copied());
            self.scene.ambient = w.ambient;
        }
        if let Some(c) = &self.car_model {
            let m = Mat4::from_rotation_translation(self.car_rot, self.car_pos);
            let (spin, steer) = match &self.vehicle {
                Some(v) => ([v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation], v.wheels[0].steer / 256.0),
                None => ([0.0; 4], 0.0),
            };
            self.scene.items.extend(c.draw_items(m, spin, steer));
        }
    }
}

impl App {
    fn resume_inner(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("Porsche Unleashed (native)").with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut renderer = Renderer::new(window.clone()).expect("renderer");
        if let Some(m) = self.menu.as_mut() {
            let dir = self.paths.fe_data().join("Art");
            m.font = GamePaths::find_file(&dir, "Univers14b.ffn").and_then(|p| std::fs::read(p).ok()).and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()).map(|f| BitmapFont::upload(&mut renderer, f));
            self.renderer = Some(renderer);
            self.window = Some(window);
            self.last_frame = Instant::now();
            self.fps_timer = Instant::now();
            if let Some(path) = self.screenshot.take() {
                let mut scene = Scene { clear_color: [0.05, 0.05, 0.08, 1.0], ..Scene::default() };
                scene.sprites = m.sprites();
                let rgba = self.renderer.as_mut().unwrap().render_to_image(&scene, &self.camera, 1280, 800).expect("render");
                image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8).expect("png");
                event_loop.exit();
            }
            return;
        }
        self.start_race(renderer, window, event_loop);
    }

    fn start_race(&mut self, mut renderer: Renderer, window: Arc<Window>, event_loop: &ActiveEventLoop) {
        let world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
        let car = CarModel::upload(&mut renderer, &self.car, &self.paths, 1, [0xD4, 0x10, 0x10]).expect("car");
        self.sky = Sky::upload(&mut renderer, &self.track);
        self.hud = Hud::load(&mut renderer, &self.paths);
        if self.audio.is_none() {
            self.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        if let Some(a) = &self.audio {
            self.engine_sound = nfs_audio::engine::EngineSound::load(a, &self.paths.game_data().join("Sounds"), &self.sound_set).map_err(|e| tracing::warn!(error = %e, "engine sound unavailable")).ok();
            if self.music.is_none() && self.autotest.is_none() && self.screenshot.is_none() {
                // song choice: first entry of zzzymus.viv (the original's playlist logic is not recovered)
                self.music = nfs_audio::music::Music::play(a, &self.paths.game_data().join("Music"), 0, 0.35).map_err(|e| tracing::warn!(error = %e, "music unavailable")).ok();
                if let Some(m) = &self.music { tracing::info!(title = m.title, "music"); }
            }
        }
        // start pose from the slice-based grid rule (pole position, single racer)
        let table = SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok();
        let rot = match &table {
            Some(t) => {
                let (pos, rot, slice) = grid_pose(t, self.track.start_forward.as_ref(), 0, 1);
                self.car_pos = pos;
                self.race = Some(RaceProgress::new(slice, t));
                tracing::info!(slice, lap_slices = t.lap_len, "grid pose");
                rot
            }
            None => {
                self.car_pos = world.start;
                glam::Mat3::IDENTITY
            }
        };
        if let Some(h) = world.collision.ground(self.car_pos.x, self.car_pos.y + 5.0, self.car_pos.z, 50.0) {
            self.car_pos.y = h.point.y + car.wheel_radius;
        }
        self.car_rot = Quat::from_mat3(&rot.transpose()).normalize();
        self.slices = table;
        self.car_half_width = (car.bbox_max.x - car.bbox_min.x).abs() * 0.5;
        let mut vehicle = Vehicle::new(self.car_pos, rot, &self.specs, car.half_track, car.wheel_radius);
        vehicle.automatic = true;
        self.vehicle = Some(vehicle);
        tracing::info!(pos = ?self.car_pos, "car placed");
        self.world = Some(world);
        self.car_model = Some(car);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.rebuild_scene();
        self.last_frame = Instant::now();
        self.fps_timer = Instant::now();
        if let Some(secs) = self.autotest.take() {
            let ticks = (secs * 64.0) as u32;
            for t in 0..ticks {
                let phase = t as f32 / ticks as f32;
                if t % 2 == 0 {
                    self.input.press(Action::Throttle);
                    if !self.follow_road && phase > 0.66 { self.input.press(Action::SteerRight); }
                }
                self.simulate();
                if t % 64 == 0 && let Some(v) = &self.vehicle {
                    tracing::info!(t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), pos = ?v.position, "autotest");
                }
            }
            self.rebuild_scene();
        }
        if let Some(path) = self.screenshot.take() {
            let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, 1280, 800).expect("offscreen render");
            image::save_buffer(&path, &rgba, 1280, 800, image::ColorType::Rgba8).expect("write png");
            tracing::info!(path, "screenshot written");
            event_loop.exit();
        }
    }

}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.resume_inner(event_loop);
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.mouse_motion(delta.0, delta.1);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                self.input.key_event(&event);
                if self.menu.is_some() && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{KeyCode, PhysicalKey};
                    let m = self.menu.as_mut().unwrap();
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowUp) | PhysicalKey::Code(KeyCode::ArrowDown) => m.row ^= 1,
                        PhysicalKey::Code(KeyCode::ArrowLeft) => {
                            if m.row == 0 { m.track_idx = (m.track_idx + m.tracks.len() - 1) % m.tracks.len() } else { m.car_idx = (m.car_idx + m.cars.len() - 1) % m.cars.len() }
                        }
                        PhysicalKey::Code(KeyCode::ArrowRight) => {
                            if m.row == 0 { m.track_idx = (m.track_idx + 1) % m.tracks.len() } else { m.car_idx = (m.car_idx + 1) % m.cars.len() }
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            let (track_name, car) = (m.tracks[m.track_idx].clone(), m.cars[m.car_idx].clone());
                            let template = Menu { tracks: m.tracks.clone(), cars: m.cars.clone(), track_idx: m.track_idx, car_idx: m.car_idx, row: m.row, font: m.font.take() };
                            self.menu_template = Some(template);
                            tracing::info!(track = track_name, car = car.model, sim = car.sim, "menu: start race");
                            match (TrackAssets::load(&self.paths, &track_name), CarAssets::load(&self.paths, &car.model), CarAssets::load_sim(&self.paths, if car.sim.is_empty() { "default" } else { &car.sim })) {
                                (Ok(t), Ok(c), Ok(sim)) => {
                                    self.track = t;
                                    self.car = c;
                                    let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                                    self.specs = CarSpecs::from_sim(&sim, race_class);
                                    self.sound_set = car.sound_set.clone();
                                    self.menu = None;
                                    let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                                    self.tick = 0;
                                    self.start_race(renderer, window, event_loop);
                                }
                                (t, c, s) => tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed"),
                            }
                        }
                        _ => {}
                    }
                    self.input.end_tick();
                    return;
                }
                if self.input.take_pressed(Action::Pause) {
                    event_loop.exit();
                }
                // after the finish, Enter returns to the menu (window and renderer are kept)
                if event.state == winit::event::ElementState::Pressed
                    && event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Enter)
                    && self.race.as_ref().map(|r| r.finished).unwrap_or(false)
                    && let Some(menu) = self.menu_template.take()
                {
                    self.menu = Some(menu);
                    self.vehicle = None;
                    self.race = None;
                    self.world = None;
                    self.car_model = None;
                    self.sky = None;
                    self.scene = Scene::default();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested if self.menu.is_some() => {
                let sprites = self.menu.as_ref().unwrap().sprites();
                let mut scene = Scene { clear_color: [0.05, 0.05, 0.08, 1.0], ..Scene::default() };
                scene.sprites = sprites;
                if let Some(r) = self.renderer.as_mut() {
                    let _ = r.render(&scene, &self.camera);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.gamepads.poll(&mut self.input);
                let now = Instant::now();
                self.accumulator += now - self.last_frame;
                self.last_frame = now;
                if self.accumulator > Duration::from_millis(250) {
                    self.accumulator = Duration::from_millis(250);
                }
                while self.accumulator >= GAME_TICK {
                    self.simulate();
                    self.accumulator -= GAME_TICK;
                }
                self.rebuild_scene();
                if let Some(r) = self.renderer.as_mut()
                    && let Err(e) = r.render(&self.scene, &self.camera)
                {
                    tracing::error!(error = %e, "render failed");
                }
                self.frames += 1;
                if self.fps_timer.elapsed() >= Duration::from_secs(1) {
                    tracing::info!(fps = self.frames, tick = self.tick, "frame stats");
                    self.frames = 0;
                    self.fps_timer = Instant::now();
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let paths = GamePaths::locate()?;
    tracing::info!(root = %paths.root.display(), "located original game data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    // positional arguments = tokens that are neither flags nor flag values
    let flags_with_value = ["--screenshot", "--autotest"];
    let mut positional = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip { skip = false; continue; }
        if a.starts_with("--") { skip = flags_with_value.contains(&a.as_str()); continue; }
        positional.push(a.clone());
    }
    let use_menu = positional.is_empty() && !args.iter().any(|a| a == "--autotest");
    let track_name = positional.first().map(String::as_str).unwrap_or("alps");
    let car_name = positional.get(1).map(String::as_str).unwrap_or("993");
    let t0 = Instant::now();
    let track = TrackAssets::load(&paths, track_name)?;
    let car = CarAssets::load(&paths, car_name)?;
    // simulation variant: catalogue entry whose model matches, else "<car>coupe36"-style guess
    let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(paths.fe_data().join("Data").join("nfs5.car"))?)?;
    let sim_name = positional.get(2).cloned().or_else(|| catalog.iter().find(|c| c.model.eq_ignore_ascii_case(car_name) && !c.sim.is_empty()).map(|c| c.sim.clone())).unwrap_or_else(|| "default".into());
    let sim = CarAssets::load_sim(&paths, &sim_name)?;
    let sound_set = catalog.iter().find(|c| c.sim.eq_ignore_ascii_case(&sim_name)).map(|c| c.sound_set.clone()).unwrap_or_else(|| "sd7a".into());
    let race_class = sim_name.to_lowercase().starts_with("gt") || sim_name.starts_with("935");
    let specs = CarSpecs::from_sim(&sim, race_class);
    tracing::info!(sim = sim_name, name = specs.name, top_speed_mps = specs.top_speed, "car specs");
    tracing::info!(track = track_name, car = car_name, secs = t0.elapsed().as_secs_f32(), "assets loaded");
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned());
    let autotest = args.iter().position(|a| a == "--autotest").and_then(|i| args.get(i + 1)?.parse().ok());
    let follow_road = args.iter().any(|a| a == "--follow");
    let menu = if use_menu {
        let mut cars = catalog.iter().filter(|c| !c.sim.is_empty()).cloned().collect::<Vec<_>>();
        cars.sort_by_key(|c| c.year);
        let car_idx = cars.iter().position(|c| c.model.eq_ignore_ascii_case("993") && c.sim.contains("coupe")).unwrap_or(0);
        Some(Menu { tracks: TrackAssets::list(&paths), cars, track_idx: 0, car_idx, row: 0, font: None })
    } else {
        None
    };
    let mut app = App {
        paths,
        menu,
        menu_template: None,
        track,
        car,
        screenshot,
        autotest,
        window: None,
        renderer: None,
        world: None,
        car_model: None,
        sky: None,
        scene: Scene::default(),
        camera: Camera::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        car_pos: Vec3::ZERO,
        car_rot: Quat::IDENTITY,
        specs,
        vehicle: None,
        camera_mode: 0,
        slices: None,
        race: None,
        num_laps: 2,
        follow_road,
        car_half_width: 0.9,
        wall_hits: 0,
        rolled_ticks: 0,
        resets: 0,
        hud: None,
        audio: None,
        engine_sound: None,
        music: None,
        sound_set,
        last_frame: Instant::now(),
        accumulator: Duration::ZERO,
        tick: 0,
        frames: 0,
        fps_timer: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
