//! Load one original car model and view it.
//!
//! ```text
//! car-viewer                         # list car models
//! car-viewer 356_1 --info           # article list with bboxes and LODs
//! car-viewer 356_1 [--lod N] [--screenshot out.png]
//! ```
use anyhow::Result;
use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, GamePaths};
use nfs_renderer::{Camera, DrawItem, MaterialDesc, MeshData, Renderer, Scene};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct Viewer {
    car: CarAssets,
    lod: Option<u8>,
    screenshot: Option<String>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    scene: Scene,
    camera: Camera,
    t0: Instant,
}

fn palette(i: u16) -> [f32; 4] {
    let h = (i as f32 * 0.618_034) % 1.0;
    let (r, g, b) = hsv(h, 0.45, 0.9);
    [r, g, b, 1.0]
}
fn hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

impl Viewer {
    fn build_scene(&mut self) {
        let r = self.renderer.as_mut().unwrap();
        let arts = self.car.crp.articles(&self.car.crp_data).expect("articles");
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut mats = std::collections::HashMap::new();
        for a in &arts {
            let g = self.car.crp.article_geometry(&self.car.crp_data, a).expect("geometry");
            let Some(lod) = self.lod.or_else(|| g.primitives.iter().map(|p| p.lod).min()) else { continue };
            for mm in article_meshes(&g, lod, &mut skipped) {
                let mut mesh: MeshData = mm.mesh;
                let col = palette(mm.material);
                for v in &mut mesh.vertices {
                    v.color = col;
                    min = min.min(Vec3::from(v.position));
                    max = max.max(Vec3::from(v.position));
                }
                let mat = *mats.entry(mm.material).or_insert_with(|| r.create_material(MaterialDesc::flat()));
                let id = r.upload_mesh(&mesh);
                self.scene.items.push(DrawItem { mesh: id, material: mat, transform: Mat4::IDENTITY });
            }
        }
        tracing::info!(items = self.scene.items.len(), skipped, ?min, ?max, "car scene");
        let centre = (min + max) * 0.5;
        let ext = (max - min).length().max(1.0);
        self.camera.position = centre + Vec3::new(ext * 0.9, ext * 0.45, ext * 0.9);
        self.camera.target = centre;
        self.camera.near = 0.05;
        self.scene.clear_color = [0.18, 0.18, 0.2, 1.0];
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(event_loop.create_window(Window::default_attributes().with_title(format!("car-viewer — {}", self.car.model)).with_inner_size(winit::dpi::LogicalSize::new(1024, 768))).expect("window"));
        self.renderer = Some(Renderer::new(window.clone()).expect("renderer"));
        self.window = Some(window);
        self.build_scene();
        if let Some(path) = self.screenshot.take() {
            let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, 1024, 768).expect("render");
            image::save_buffer(&path, &rgba, 1024, 768, image::ColorType::Rgba8).expect("png");
            tracing::info!(path, "screenshot written");
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let t = self.t0.elapsed().as_secs_f32() * 0.4;
                let c = self.camera.target;
                let d = (self.camera.position - c).length();
                self.camera.position = c + Vec3::new(t.cos() * d * 0.8, d * 0.45, t.sin() * d * 0.8);
                if let Some(r) = self.renderer.as_mut() {
                    let _ = r.render(&self.scene, &self.camera);
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
    let Some(model) = args.first() else {
        let mut v: Vec<String> = std::fs::read_dir(paths.car_model_dir())?.flatten().filter_map(|e| {
            let p = e.path();
            (p.extension()?.eq_ignore_ascii_case("crp")).then(|| p.file_stem()?.to_str().map(String::from))?
        }).collect();
        v.sort();
        println!("car models: {v:?}");
        return Ok(());
    };
    let car = CarAssets::load(&paths, model)?;
    let arts = car.crp.articles(&car.crp_data)?;
    println!("car {}: {} articles, {} materials, {} embedded texture files, day textures {}, night textures {}", car.model, arts.len(), car.crp.materials(&car.crp_data)?.len(), car.crp.embedded_texture_files(&car.crp_data)?.len(), car.textures_day.len(), car.textures_night.len());
    if args.iter().any(|a| a == "--info") {
        for a in &arts {
            let g = car.crp.article_geometry(&car.crp_data, a)?;
            let lods: std::collections::BTreeSet<u8> = g.primitives.iter().map(|p| p.lod).collect();
            println!("  {:36} lods={:?} prims={:3} bbox={:?}..{:?} tag={:?}", g.name, lods, g.primitives.len(), g.base.as_ref().map(|b| b.bbox_min), g.base.as_ref().map(|b| b.bbox_max), g.base.as_ref().map(|b| String::from_utf8_lossy(&b.tag).to_string()));
        }
        return Ok(());
    }
    let lod = args.iter().position(|a| a == "--lod").and_then(|i| args.get(i + 1)?.parse().ok());
    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned());
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut v = Viewer { car, lod, screenshot, window: None, renderer: None, scene: Scene::default(), camera: Camera::default(), t0: Instant::now() };
    event_loop.run_app(&mut v)?;
    Ok(())
}
