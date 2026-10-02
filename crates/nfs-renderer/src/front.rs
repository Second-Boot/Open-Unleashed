//! The renderer the game talks to: a front over the GPU core ([`crate::gpu::Core`]).
//!
//! **The command list.** Every change the game makes to the renderer while it runs — vertex and
//! texture rewrites, the state setters, an offscreen pass (the rear-view mirror, the front end's
//! 3-D widgets), a new material or dynamic mesh, a resize — is recorded here, in call order, into
//! one [`CmdList`], and the core executes the list at the start of the frame's encode
//! ([`Core::apply`](crate::gpu::Core::apply)). Nothing but the list and the frame's [`Scene`] reaches
//! the GPU side, so a frame is one self-contained hand-off: with the render thread on
//! ([`Renderer::enable_render_thread`]) the core lives on that thread, the list and a copy of the
//! scene go over to it, and the main thread simulates and builds the next frame while the thread
//! encodes, submits and presents this one (at most one frame in flight — the next hand-off waits for
//! the thread to finish the last). With it off the core runs the same list on the calling thread.
//!
//! What the callers read back — the ids they are handed, [`Renderer::mesh_writes`],
//! [`Renderer::material_texture`], the size — is answered from a small mirror of the core's tables
//! kept here, so no query waits for the thread. The bulk loaders (`upload_mesh`, `upload_texture*`)
//! and the read-backs (`render_to_image`, `read_texture`) are the one thing that does: they need
//! the core in hand, so they first take it back (waiting for the frame in flight) and run the list.

use crate::gpu::{Core, MaterialId, MeshId, TextureId};
use crate::{Camera, FakeSortMode, LensRead, MaterialDesc, MeshData, Scene, TextureData, Vertex};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use winit::window::Window;

/// One recorded mutation. Byte payloads live in [`CmdList::bytes`].
pub(crate) enum Cmd {
    /// `bytes[start..start + len]` into mesh `mesh`'s vertex range (validated on the front)
    WriteMesh { mesh: u32, start: usize, len: usize },
    /// one mip level of a texture, `height` tight rows of `4 · width` bytes at `bytes[start..]`
    UpdateTextureLevel { texture: u32, level: u32, width: u32, height: u32, start: usize, len: usize },
    HeadlightMap(Vec<u8>),
    Projector(Option<TextureId>),
    HeadlightTexture(Option<TextureId>),
    MirrorView(bool),
    Trilinear(bool),
    FakeSort(FakeSortMode),
    Gamma(f32),
    Resize(u32, u32),
    LensProbe(bool),
    CreateMaterial(MaterialDesc),
    DuplicateMaterial { id: MaterialId, texture: TextureId },
    DynamicMesh(Box<MeshData>),
    Offscreen(Box<OffscreenJob>),
}

pub(crate) struct OffscreenJob {
    pub scene: Scene,
    pub camera: Camera,
    pub width: u32,
    pub height: u32,
    pub reuse: Option<TextureId>,
    pub clear: Option<[f64; 4]>,
    pub depth_far: Option<f32>,
    /// the texture id the front handed out for it
    pub id: TextureId,
}

/// A frame's mutations, in call order.
#[derive(Default)]
pub(crate) struct CmdList {
    pub cmds: Vec<Cmd>,
    pub bytes: Vec<u8>,
    /// index in `cmds` after the last offscreen pass: a queued texture level is only overwritten in
    /// place by a later write of the same level past this point (the pass in between must see the
    /// first)
    barrier: usize,
}

struct Job {
    core: Box<Core>,
    list: CmdList,
    scene: Scene,
    camera: Camera,
}

struct Done {
    core: Box<Core>,
    list: CmdList,
    scene: Scene,
    result: Result<()>,
    /// the lens-flare read-backs the core had finished by the end of the frame
    lens: Vec<LensRead>,
}

/// The render thread: owns the core while a frame is in flight.
struct Worker {
    tx: Option<Sender<Job>>,
    rx: Receiver<Done>,
    handle: Option<std::thread::JoinHandle<()>>,
    in_flight: bool,
}

#[derive(Clone, Copy)]
struct MeshMirror {
    vertex_len: u64,
    writes: u32,
}

pub struct Renderer {
    /// `None` while a frame is in flight on the render thread
    core: Option<Box<Core>>,
    worker: Option<Worker>,
    list: CmdList,
    /// a list and a scene the thread handed back, reused for the next frame
    spare_list: Option<CmdList>,
    spare_scene: Option<Scene>,
    /// the error of the last frame the thread drew, reported by the next [`Self::render`]
    pending_err: Option<anyhow::Error>,
    /// lens-flare read-backs the render thread handed back, not yet taken
    lens_done: Vec<LensRead>,
    // the mirror of the core's tables
    size: (u32, u32),
    meshes: Vec<MeshMirror>,
    n_textures: u32,
    white: TextureId,
    material_tex: Vec<TextureId>,
    offscreen: HashMap<u32, (u32, u32)>,
    /// the last [`Self::set_gamma`] value, so an unchanged one records nothing
    gamma: f32,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let core = Core::new(window)?;
        let size = core.size();
        let meshes = core.mesh_infos().into_iter().map(|(vertex_len, writes)| MeshMirror { vertex_len, writes }).collect();
        Ok(Self {
            n_textures: core.texture_count(),
            white: core.white(),
            material_tex: core.material_textures(),
            offscreen: core.offscreen_sizes(),
            gamma: 1.0,
            meshes,
            size,
            core: Some(Box::new(core)),
            worker: None,
            list: CmdList::default(),
            spare_list: None,
            spare_scene: None,
            pending_err: None,
            lens_done: Vec::new(),
        })
    }

    /// Move the encode / submit / present of every frame to a render thread of its own. From here
    /// [`Self::render`] hands the frame over and returns; the pixels drawn for a scene are the same.
    pub fn enable_render_thread(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel::<Done>();
        let handle = std::thread::Builder::new()
            .name("nfs-render".into())
            .spawn(move || {
                while let Ok(Job { mut core, mut list, scene, camera }) = jobs.recv() {
                    core.apply(&mut list);
                    let result = core.render(&scene, &camera);
                    let lens = core.take_lens();
                    if done_tx.send(Done { core, list, scene, result, lens }).is_err() {
                        break;
                    }
                }
            })
            .expect("render thread");
        self.worker = Some(Worker { tx: Some(tx), rx, handle: Some(handle), in_flight: false });
    }

    pub fn render_thread(&self) -> bool {
        self.worker.is_some()
    }

    /// Wait for the frame in flight, if any, and take the core back.
    fn wait_home(&mut self) {
        let Some(w) = self.worker.as_mut().filter(|w| w.in_flight) else { return };
        let d = w.rx.recv().expect("render thread died");
        w.in_flight = false;
        self.core = Some(d.core);
        self.spare_list = Some(d.list);
        self.spare_scene = Some(d.scene);
        self.lens_done.extend(d.lens);
        if let Err(e) = d.result {
            self.pending_err = Some(e);
        }
    }

    /// The core in hand with everything recorded so far executed.
    fn home(&mut self) -> &mut Core {
        self.wait_home();
        let core = self.core.as_mut().expect("core home");
        core.apply(&mut self.list);
        self.list.barrier = 0;
        core
    }

    /// Draw a frame: the recorded commands, then the scene, into the window's next image, presented.
    /// With the render thread on, this hands the frame over (after the previous one is done) and
    /// returns; a failure of a frame is reported by the next call.
    pub fn render(&mut self, scene: &Scene, camera: &Camera) -> Result<()> {
        if self.worker.is_none() {
            let core = self.core.as_mut().expect("core home");
            core.apply(&mut self.list);
            self.list.barrier = 0;
            return core.render(scene, camera);
        }
        self.wait_home();
        if let Some(e) = self.pending_err.take() {
            return Err(e);
        }
        let core = self.core.take().expect("core home");
        let list = std::mem::replace(&mut self.list, self.spare_list.take().unwrap_or_default());
        self.list.barrier = 0;
        let mut copy = self.spare_scene.take().unwrap_or_default();
        scene.copy_into(&mut copy);
        let w = self.worker.as_mut().expect("worker");
        w.in_flight = true;
        w.tx.as_ref().expect("sender").send(Job { core, list, scene: copy, camera: *camera }).expect("render thread died");
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.size = (width, height);
        self.list.cmds.push(Cmd::Resize(width, height));
    }

    /// The surface's pixel size.
    pub fn size(&self) -> (u32, u32) {
        (self.size.0.max(1), self.size.1.max(1))
    }

    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    /// A mesh whose vertices are rewritten every frame — [`Self::write_mesh_vertices`]. The index
    /// buffer and the index count are fixed at upload; only the vertex data moves.
    pub fn upload_dynamic_mesh(&mut self, mesh: &MeshData) -> MeshId {
        // `create_buffer_init` rounds the size up to 4 bytes, at least 4
        let vertex_len = (std::mem::size_of_val(mesh.vertices.as_slice()) as u64).next_multiple_of(4).max(4);
        self.meshes.push(MeshMirror { vertex_len, writes: 0 });
        self.list.cmds.push(Cmd::DynamicMesh(Box::new(mesh.clone())));
        MeshId(self.meshes.len() as u32 - 1)
    }

    fn push_write(&mut self, id: MeshId, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let m = &mut self.meshes[id.0 as usize];
        m.writes = m.writes.wrapping_add(1);
        let start = self.list.bytes.len();
        self.list.bytes.extend_from_slice(bytes);
        self.list.cmds.push(Cmd::WriteMesh { mesh: id.0, start, len: bytes.len() });
    }

    /// Rewrite a dynamic mesh's vertices. The slice must be no longer than the one it was uploaded
    /// with; anything else is ignored.
    pub fn write_mesh_vertices(&mut self, id: MeshId, vertices: &[Vertex]) {
        let Some(m) = self.meshes.get(id.0 as usize) else { return };
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        if bytes.len() as u64 > m.vertex_len {
            return;
        }
        self.push_write(id, bytes);
    }

    /// New vertices for a mesh already uploaded, the same count — the morphing parts of a car
    /// (its roof) rewrite theirs when the animation moves.
    pub fn update_mesh_vertices(&mut self, id: MeshId, vertices: &[Vertex]) {
        if let Some(m) = self.meshes.get(id.0 as usize)
            && std::mem::size_of_val(vertices) as u64 == m.vertex_len
        {
            self.push_write(id, bytemuck::cast_slice(vertices));
        }
    }

    /// The number of vertex writes mesh `id` has had since its upload (0 for an unknown mesh): equal
    /// before and after a stretch of time means nobody wrote its vertices in between.
    pub fn mesh_writes(&self, id: MeshId) -> u32 {
        self.meshes.get(id.0 as usize).map_or(0, |m| m.writes)
    }

    pub fn upload_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let core = self.home();
        let id = core.upload_mesh(mesh);
        let vertex_len = core.mesh_vertex_len(id);
        debug_assert_eq!(id.0 as usize, self.meshes.len());
        self.meshes.push(MeshMirror { vertex_len, writes: 0 });
        id
    }

    pub fn upload_texture(&mut self, tex: &TextureData) -> TextureId {
        self.upload_texture_mips(tex.width, tex.height, &tex.mips)
    }

    /// [`Self::upload_texture`] over borrowed levels, so a caller that keeps its pixels does not
    /// clone a whole mip chain just to hand it over.
    pub fn upload_texture_mips(&mut self, width: u32, height: u32, mips: &[Vec<u8>]) -> TextureId {
        let core = self.home();
        let id = core.upload_texture_mips(width, height, mips);
        self.n_textures = core.texture_count();
        id
    }

    /// Several textures at once, the ids in the order given (the next free ones, as the same number
    /// of [`Self::upload_texture_mips`] calls would hand them).
    pub fn upload_textures(&mut self, items: &[(u32, u32, &[Vec<u8>])]) -> Vec<TextureId> {
        let core = self.home();
        let ids = core.upload_textures(items);
        self.n_textures = core.texture_count();
        ids
    }

    /// Overwrite an existing texture's pixels, keeping its id and every bind group that points at
    /// it. The new data must have the same dimensions and mip count as the original. Used for the
    /// car's texture pages, which are recomposed when the damage decals change
    /// (`RE/notes/car-model.md` §11).
    pub fn update_texture(&mut self, id: TextureId, tex: &TextureData) {
        if id.0 >= self.n_textures {
            return;
        }
        let (mut w, mut h) = (tex.width, tex.height);
        for (level, data) in tex.mips.iter().enumerate() {
            let row = 4 * w as usize;
            let len = row * h as usize;
            if data.len() >= len {
                // a queued write of the same whole level is overwritten in place (the last wins, as the
                // later `write_texture` would), so frames that are not rendered do not pile writes up
                let barrier = self.list.barrier;
                let same = self.list.cmds[barrier..].iter().find_map(|c| match c {
                    Cmd::UpdateTextureLevel { texture, level: l, width, height, start, .. } if *texture == id.0 && *l == level as u32 && *width == w && *height == h => Some(*start),
                    _ => None,
                });
                match same {
                    Some(start) => self.list.bytes[start..start + len].copy_from_slice(&data[..len]),
                    None => {
                        let start = self.list.bytes.len();
                        self.list.bytes.extend_from_slice(&data[..len]);
                        self.list.cmds.push(Cmd::UpdateTextureLevel { texture: id.0, level: level as u32, width: w, height: h, start, len });
                    }
                }
            }
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
    }

    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        self.material_tex.push(desc.texture.unwrap_or(self.white));
        self.list.cmds.push(Cmd::CreateMaterial(desc));
        MaterialId(self.material_tex.len() as u32 - 1)
    }

    /// The diffuse texture `id` samples, if it exists — used to find which materials of a shared
    /// model reference a damage-decal page when giving a traffic/cop car instance its own copy of
    /// it (`RE/notes/car-model.md` §13.1).
    pub fn material_texture(&self, id: MaterialId) -> Option<TextureId> {
        self.material_tex.get(id.0 as usize).copied()
    }

    /// A new material identical to `id` but sampling `texture` in its place — same blend/cull/lod
    /// state and specular map, a fresh bind group. Used to give one traffic/cop car instance its
    /// own decal-page material when `nfs_game::world::CarModel::instantiate` clones the shared
    /// model's damage resources so two differently-damaged instances in one frame no longer alias
    /// (`RE/notes/car-model.md` §13.1).
    pub fn duplicate_material_texture(&mut self, id: MaterialId, texture: TextureId) -> MaterialId {
        if self.material_tex.get(id.0 as usize).is_none() {
            return id;
        }
        self.material_tex.push(texture);
        self.list.cmds.push(Cmd::DuplicateMaterial { id, texture });
        MaterialId(self.material_tex.len() as u32 - 1)
    }

    /// `night.psh`'s `nght` bitmap, 128 × 128 bytes, for the headlight projector.
    pub fn set_headlight_map(&mut self, map: &[u8]) {
        if map.len() == 128 * 128 {
            self.list.cmds.push(Cmd::HeadlightMap(map.to_vec()));
        }
    }

    /// The car shadow projector's texture, `[0x6115CC]+0x18C` (`RE/notes/crp-geometry.md` §8);
    /// `None` for none.
    pub fn set_projector_texture(&mut self, tex: Option<TextureId>) {
        self.list.cmds.push(Cmd::Projector(tex));
    }

    /// `HEAD` of `Render/common.fsh`, what `Headlights::LayerUVLight` projects (0x452645 fetches it
    /// by that four-character code); `None` for none.
    pub fn set_headlight_texture(&mut self, tex: Option<TextureId>) {
        self.list.cmds.push(Cmd::HeadlightTexture(tex));
    }

    /// The view about to be drawn is the rear-view mirror's: `[0x6115CC]+0x190` is set from the
    /// view's type (`[[0x628130]+0x10]+0x2C == 1`, 0x43AD39…0x43AD4E; 0x4691D0 makes the mirror's
    /// view with type 1), and the specular and sky-map layers then bind no texture and skip their
    /// passes (0x445578, 0x445B69).
    /// The same flag keeps out the draws only the main view makes (`DrawItem::main_view_only`).
    pub fn set_mirror_view(&mut self, on: bool) {
        self.list.cmds.push(Cmd::MirrorView(on));
    }

    /// `[0x69DD1D]`, the mip filter's promotion (0x535B40 / 0x535A60, `RE/notes/thrash.md` §4):
    /// set, every texture with mips is sampled trilinear; clear, `POINT` between the levels.
    /// 0x44ED10 sets it only at `SPECIALFX` 3 on a card that took `setstate(0x0B, 3)`
    /// (`[0x6197A6]`, taken as accepted); the front end's 0x44F020 clears it.
    pub fn set_trilinear(&mut self, on: bool) {
        self.list.cmds.push(Cmd::Trilinear(on));
    }

    /// Let the window surface be copied from, which the lens flare's frame read-back needs
    /// ([`crate::LensProbe`]); off, the surface is a render attachment alone.
    pub fn set_lens_probe(&mut self, on: bool) {
        self.list.cmds.push(Cmd::LensProbe(on));
    }

    /// The lens-flare read-backs finished since the last call (a frame or two after the frame whose
    /// [`crate::Scene::lens_probe`] asked for them); never waits for the render thread.
    pub fn lens_results(&mut self) -> Vec<LensRead> {
        let mut out = std::mem::take(&mut self.lens_done);
        if let Some(core) = self.core.as_mut() {
            out.extend(core.take_lens());
        }
        out
    }

    /// `LayerFalseSort1`/`2`'s per-frame pick (`main.rs`'s `App::fakesort_mode`, [`FakeSortMode`]).
    pub fn set_fakesort_mode(&mut self, mode: FakeSortMode) {
        self.list.cmds.push(Cmd::FakeSort(mode));
    }

    /// State 0x2E, the display gamma (`Core::set_gamma`); only a change is recorded.
    pub fn set_gamma(&mut self, g: f32) {
        if self.gamma != g {
            self.gamma = g;
            self.list.cmds.push(Cmd::Gamma(g));
        }
    }

    /// Render a scene's meshes into an offscreen texture that the sprite pass can then draw, so a
    /// 3D view can sit **inside** the 2D screen in the right drawing order — what the front end's
    /// `NFS5.3DCAR` widgets need (`RE/notes/frontend-3d.md` §4). The clear is transparent, so only
    /// the geometry shows, unless `clear` gives an opaque colour (the in-race rear-view mirror).
    /// Passing the id of a texture this returned earlier reuses it.
    /// `depth_far` is the far plane of the pass's own screen z — the scale of the state 0x18 depth
    /// bias: `None` for the main view's 700, the rear view's 65000 (0x4691D0). The pass is recorded
    /// in the command list and runs, in its place in the order, when the frame is encoded.
    #[allow(clippy::too_many_arguments)]
    pub fn render_offscreen(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32, reuse: Option<TextureId>, clear_colour: Option<[f64; 4]>, depth_far: Option<f32>) -> TextureId {
        let (width, height) = (width.max(1), height.max(1));
        let id = match reuse.filter(|t| self.offscreen.get(&t.0).is_some_and(|(w, h)| *w == width && *h == height)) {
            Some(t) => t,
            None => {
                let id = TextureId(self.n_textures);
                self.n_textures += 1;
                self.offscreen.insert(id.0, (width, height));
                id
            }
        };
        self.list.cmds.push(Cmd::Offscreen(Box::new(OffscreenJob { scene: scene.clone_without_sprites(), camera: *camera, width, height, reuse, clear: clear_colour, depth_far, id })));
        self.list.barrier = self.list.cmds.len();
        id
    }

    pub fn render_to_image(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        self.home().render_to_image(scene, camera, width, height)
    }

    /// An offscreen texture's pixels as RGBA8 rows (diagnostics: `NFS_MIRROR_DUMP`).
    pub fn read_texture(&mut self, id: TextureId) -> Result<(u32, u32, Vec<u8>)> {
        self.home().read_texture(id)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.wait_home();
        if let Some(mut w) = self.worker.take() {
            drop(w.tx.take());
            if let Some(h) = w.handle.take() {
                let _ = h.join();
            }
        }
    }
}
