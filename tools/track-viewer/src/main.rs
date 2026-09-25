//! Load one original track through `nfs-assets` and view it.
//!
//! ```text
//! track-viewer                # list tracks
//! track-viewer alps --info    # load and summarise only
//! track-viewer alps           # open a window (free camera: WASD/QE, PgUp/PgDn, Shift = fast, mouse drag = look)
//! track-viewer alps --screenshot out.png [--eye x,y,z --at x,y,z]   # offscreen render for checks
//! ```
use anyhow::Result;
use glam::{Mat4, Vec3};
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::scn::ElementKind;
use nfs_input::{Action, InputState};
use nfs_renderer::{Camera, DrawItem, MaterialDesc, MeshData, Renderer, Scene, TextureData};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn summarise(t: &TrackAssets) -> Result<()> {
    println!("track {}", t.name);
    println!("  crp: kind={:?} level entries={} articles={}", t.crp.kind, t.crp.level.len(), t.crp.articles(&t.crp_data)?.len());
    println!("  textures: {} (first: {:?}), env maps: {:?}", t.textures.len(), t.textures.first().map(|x| (&x.name, x.width, x.height, x.mips.len())), t.env_textures.iter().map(|x| x.name.as_str()).collect::<Vec<_>>());
    println!("  map sections: {}", t.map.sections.len());
    if let Some(s) = &t.start_forward {
        for e in s.of_kind(&ElementKind::Trigger).take(1) {
            println!("  forward start trigger at {:?}", e.position);
        }
    }
    if let Some(c) = &t.cameras {
        println!("  cameras: {} camera elements, {} triggers", c.of_kind(&ElementKind::Camera).count(), c.of_kind(&ElementKind::Trigger).count());
    }
    // collision sanity: ground under the start trigger
    let geo = t.build_geometry()?;
    let t0 = Instant::now();
    let grid = geo.collision_grid(20.0, &|_| 1);
    if let Some(p) = t.start_forward.as_ref().and_then(|s| s.of_kind(&ElementKind::Trigger).next()).map(|e| e.position) {
        let hit = grid.ground(p[0], p[1] + 5.0, p[2], 50.0);
        println!(
            "  collision grid: {} triangles in {:.2}s; ground under start {:?}: {:?}",
            grid.triangle_count(),
            t0.elapsed().as_secs_f32(),
            p,
            hit.map(|h| (h.point.y, h.normal, h.surface))
        );
    }
    Ok(())
}

struct Viewer {
    track: TrackAssets,
    screenshot: Option<(String, Option<Vec3>, Option<Vec3>)>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    scene: Scene,
    camera: Camera,
    yaw: f32,
    pitch: f32,
    input: InputState,
    dragging: bool,
    last: Instant,
}

impl Viewer {
    fn start_position(&self) -> Vec3 {
        self.track
            .start_forward
            .as_ref()
            .and_then(|s| s.of_kind(&ElementKind::Trigger).next())
            .map(|e| Vec3::from(e.position))
            .unwrap_or(Vec3::ZERO)
    }

    /// Builds the scene from the track's static geometry (lowest LOD), textured per material.
    fn build_scene(&mut self) {
        let t0 = Instant::now();
        let start = self.start_position();
        if let Some(h) = self.track.map.section("header")
            && let Some(a) = h.get_f32_list("ambient")
            && a.len() == 4
        {
            // `.map` ambient = (1.0, r, g, b) multipliers; use the mean as the scene ambient (approximation)
            self.scene.ambient = ((a[1] + a[2] + a[3]) / 3.0).clamp(0.2, 0.9);
        }
        let geometry = self.track.build_geometry().expect("track geometry");
        let materials = self.track.materials().expect("materials");
        let r = self.renderer.as_mut().unwrap();
        // textures by FourCC name
        let mut tex_ids = HashMap::new();
        for tex in &self.track.textures {
            let id = r.upload_texture(&TextureData { width: tex.width, height: tex.height, mips: tex.mips.clone() });
            tex_ids.insert(tex.name.clone(), (id, tex.has_alpha));
        }
        let mut mat_ids = HashMap::new();
        let mut missing = 0;
        let mut item_count = 0usize;
        let mut material_for = |r: &mut Renderer, id: u16| {
            *mat_ids.entry(id).or_insert_with(|| {
                let m = materials.get(id as usize);
                let tex = m.and_then(|m| tex_ids.get(&m.texture_name()));
                if tex.is_none() { missing += 1; }
                let alpha = tex.map(|t| t.1).unwrap_or(false) && m.map(|m| m.render_method != "Opaque").unwrap_or(true);
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false })
            })
        };
        for art in &geometry.world {
            for mm in &art.meshes {
                let mat_id = material_for(r, mm.material);
                let mesh = r.upload_mesh(&mm.mesh);
                self.scene.items.push(DrawItem::new(mesh, mat_id, art.transform));
                item_count += 1;
            }
        }
        // library props instanced by .scn GEOM elements (tag match on Base+68)
        let mut lib_gpu: HashMap<u32, Vec<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>> = HashMap::new();
        for art in &geometry.library {
            let entry = lib_gpu.entry(art.tag).or_default();
            if !entry.is_empty() { continue; } // first article per tag wins (duplicates like "cone SMACKABLE"/"default cone" differ by tag anyway)
            for mm in &art.meshes {
                let mat_id = material_for(r, mm.material);
                entry.push((r.upload_mesh(&mm.mesh), mat_id));
            }
        }
        let mut placed = 0usize;
        let mut unmatched = 0usize;
        for inst in &geometry.instances {
            match lib_gpu.get(&inst.tag) {
                Some(parts) => {
                    let lib_tr = geometry.library.iter().find(|a| a.tag == inst.tag).map(|a| a.transform).unwrap_or(Mat4::IDENTITY);
                    for (mesh, mat) in parts {
                        self.scene.items.push(DrawItem::new(*mesh, *mat, inst.transform * lib_tr));
                    }
                    placed += 1;
                }
                None => unmatched += 1,
            }
        }
        tracing::info!(placed_props = placed, unmatched_props = unmatched, "props");
        tracing::info!(
            articles = geometry.world.len(),
            library = geometry.library.len(),
            draw_items = item_count,
            materials_without_texture = missing,
            skipped_primitives = geometry.skipped_primitives,
            secs = t0.elapsed().as_secs_f32(),
            "scene built"
        );
        let grid = r.upload_mesh(&MeshData::grid(50.0, 5.0));
        let mat = r.create_material(MaterialDesc { texture: None, alpha_blend: false, cull_back: false, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false });
        self.scene.items.push(DrawItem::new(grid, mat, Mat4::from_translation(Vec3::new(start.x, start.y - 0.5, start.z))));
        self.camera.position = start + Vec3::new(0.0, 3.0, 12.0);
        self.camera.target = start;
        self.yaw = -std::f32::consts::FRAC_PI_2;
        self.pitch = -0.2;
        self.camera.far = 20000.0;
    }

    fn update_camera(&mut self, dt: f32) {
        let (dx, dy) = self.input.mouse_delta;
        if self.dragging {
            self.yaw += dx * 0.003;
            self.pitch = (self.pitch - dy * 0.003).clamp(-1.5, 1.5);
        }
        let dir = Vec3::new(self.yaw.cos() * self.pitch.cos(), self.pitch.sin(), self.yaw.sin() * self.pitch.cos());
        let right = dir.cross(Vec3::Y).normalize();
        let speed = if self.input.held(Action::FreeFast) { 120.0 } else { 25.0 } * dt;
        let mut p = self.camera.position;
        if self.input.held(Action::FreeForward) { p += dir * speed; }
        if self.input.held(Action::FreeBack) { p -= dir * speed; }
        if self.input.held(Action::FreeRight) { p += right * speed; }
        if self.input.held(Action::FreeLeft) { p -= right * speed; }
        if self.input.held(Action::FreeUp) { p += Vec3::Y * speed; }
        if self.input.held(Action::FreeDown) { p -= Vec3::Y * speed; }
        self.camera.position = p;
        self.camera.target = p + dir;
        self.input.end_tick();
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(format!("track-viewer — {}", self.track.name)).with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.renderer = Some(Renderer::new(window.clone()).expect("renderer"));
        self.window = Some(window);
        self.build_scene();
        self.last = Instant::now();
        if let Some((path, eye, at)) = self.screenshot.take() {
            let mut cam = self.camera;
            if let Some(e) = eye { cam.position = e; }
            if let Some(a) = at { cam.target = a; }
            let (w, h) = (1280u32, 800u32);
            let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &cam, w, h).expect("offscreen render");
            image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("write png");
            tracing::info!(path, "screenshot written");
            event_loop.exit();
        }
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
                if self.input.take_pressed(Action::Pause) {
                    event_loop.exit();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => self.dragging = state == ElementState::Pressed,
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f32().min(0.1);
                self.last = now;
                self.update_camera(dt);
                if let Some(r) = self.renderer.as_mut()
                    && let Err(e) = r.render(&self.scene, &self.camera)
                {
                    tracing::error!(error = %e, "render failed");
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
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let paths = GamePaths::locate()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        println!("tracks: {:?}", TrackAssets::list(&paths));
        return Ok(());
    };
    let t0 = Instant::now();
    let track = TrackAssets::load(&paths, name)?;
    tracing::info!(track = name, secs = t0.elapsed().as_secs_f32(), "loaded");
    summarise(&track)?;
    if args.iter().any(|a| a == "--info") {
        return Ok(());
    }
    let vec_arg = |flag: &str| -> Option<Vec3> {
        let i = args.iter().position(|a| a == flag)?;
        let v: Vec<f32> = args.get(i + 1)?.split(',').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
    };
    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned()).map(|p| (p, vec_arg("--eye"), vec_arg("--at")));
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut viewer = Viewer {
        track,
        screenshot,
        window: None,
        renderer: None,
        scene: Scene::default(),
        camera: Camera::default(),
        yaw: 0.0,
        pitch: 0.0,
        input: InputState::default(),
        dragging: false,
        last: Instant::now(),
    };
    event_loop.run_app(&mut viewer)?;
    Ok(())
}
