//! One frame: `render` (the window's frame), `encode_frame` (the items' stages, the globals and object uniforms, the
//! shadow / spot / lamp / contact pre-passes, the main pass variants), the bucket / run draws, the 2-D sprites and the
//! shafts / haze / lighthouse-beam draws. Moved out of gpu.rs as it was.

use crate::wgsl::DepthKind;
use super::pipelines::{Cull, Group, PipelineKey};
use super::*;
use crate::prof::Stage;

/// Build the two triangles of a sprite in clip space (y up), rotating about its pivot in screen space.
fn sprite_quad(sp: &Sprite, aspect: f32) -> [SpriteVertex; 6] {
    let (z, alpha_ref) = sp.depth.map_or((0.0, 0.0), |d| (d.z, d.alpha_test.map_or(-1.0, f32::from)));
    if let Some((t, col)) = sp.tri {
        let v = |p: [f32; 2], c: [f32; 4]| SpriteVertex { pos: [p[0] * 2.0 - 1.0, 1.0 - p[1] * 2.0, z], uv: sp.uv_min, color: c, alpha_ref };
        let (a, b, c) = (v(t[0], col[0]), v(t[1], col[1]), v(t[2], col[2]));
        return [a, b, c, c, c, c];
    }
    let (sx, sy) = (sp.size[0] / aspect, sp.size[1]);
    let (px, py) = (sp.pos[0] + sp.pivot[0] * sx, sp.pos[1] + sp.pivot[1] * sy);
    let (sn, cs) = (sp.rotation * std::f32::consts::TAU).sin_cos();
    let corner = |u: f32, v: f32| -> SpriteVertex {
        // local offset from pivot in screen units (aspect-corrected so rotation is not sheared)
        let lx = (u - sp.pivot[0]) * sx * aspect;
        let ly = (v - sp.pivot[1]) * sy;
        let rx = lx * cs - ly * sn;
        let ry = lx * sn + ly * cs;
        let x = px + rx / aspect;
        let y = py + ry;
        SpriteVertex {
            pos: [x * 2.0 - 1.0, 1.0 - y * 2.0, z],
            uv: [sp.uv_min[0] + (sp.uv_max[0] - sp.uv_min[0]) * u, sp.uv_min[1] + (sp.uv_max[1] - sp.uv_min[1]) * v],
            color: sp.corner_colors.map_or(sp.color, |k| match (u > 0.5, v > 0.5) {
                (false, false) => k[0],
                (true, false) => k[1],
                (true, true) => k[2],
                (false, true) => k[3],
            }),
            alpha_ref,
        }
    };
    let (a, b, c, d) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 1.0), corner(0.0, 1.0));
    [a, b, c, a, c, d]
}

/// The two pipelines of an item's draw: `plain` (no depth writes) and `group` (the one it draws with; a `FakeSort` item's double
/// draw takes `plain` first). `layered`: the material's passes are drawn over its own blend (`fs_layers`).
fn item_pipelines(mat: &GpuMaterial, item: &crate::DrawItem, mirror: bool, layered: bool) -> (PipelineKey, PipelineKey) {
    // mirrored world: the winding flips, so a material whose cull is the view's culls
    // the other one (0x468C45); any other draws both faces rather than the wrong one
    let cull = match (mat.cull_back, mirror != item.mirrored, mat.view_cull) {
        (false, _, _) | (true, true, false) => Cull::None,
        (true, false, _) => Cull::Back,
        (true, true, true) => Cull::Front,
    };
    let key = |group| PipelineKey { group, cull, depth_test: mat.depth_test };
    if layered {
        return (key(Group::Layered { zwrite: false }), key(Group::Layered { zwrite: mat.device_states }));
    }
    if !mat.alpha_blend {
        // ENHANCEMENT E1: an alpha-TESTED opaque draw takes the alpha-to-coverage pipeline (on a multisampled target: a 1-sample
        // set has no such group, `PipelineKey::index`); blended draws never do
        let group = if mat.device_states || item.alpha_test { Group::AlphaToCoverage } else { Group::Opaque };
        return (key(Group::Opaque), key(group));
    }
    // the blend group without depth writes, and with them (`device_states`)
    let pair = mat.blend_pair;
    (key(Group::Blend { pair, zwrite: false }), key(Group::Blend { pair, zwrite: mat.device_states }))
}

impl Core {
    pub fn render(&mut self, scene: &Scene, camera: &Camera) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.configure_surface();
                self.flush_writes();
                return Ok(());
            }
            // a timeout or an occluded window: skip the frame (wgpu 30)
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // ENHANCEMENT E33 diagnostic: a locked / asleep screen gives no surface; with `NFS_EDR_PROBE` set the
                // EDR chain still runs into an image of its own so the probe can measure it
                #[cfg(target_os = "macos")]
                if self.edr.on && crate::dbg::flags().edr_probe_set {
                    let (w, h) = self.size();
                    let img = crate::gfx::texture_2d(&self.device, "edr diagnostic surface", w, h, HDR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
                    let view = img.create_view(&Default::default());
                    let (aspect, depth) = (self.aspect(), self.depth_view.clone());
                    self.render_edr(scene, camera, &view, (w, h), aspect, &depth);
                }
                self.flush_writes();
                return Ok(());
            }
            other => anyhow::bail!("surface texture: {other:?}"),
        };
        let view = frame.texture.create_view(&Default::default());
        let aspect = self.aspect();
        let depth = self.depth_view.clone();
        let (w, h) = (frame.texture.width(), frame.texture.height());
        let can_copy = self.lens_enabled;
        // ENHANCEMENT E33: the EDR output draws the byte path into an 8-bit frame image of its own and presents it
        // (gamma ramp, sRGB decode, the highlight gain) into the extended-range surface (macOS only: elsewhere `edr.on` is never
        // set, so the branch is compiled out instead of calling a no-op twin)
        #[cfg(target_os = "macos")]
        if self.edr.on {
            self.render_edr(scene, camera, &view, (w, h), aspect, &depth);
            self.queue.present(frame);
            self.collect_lens();
            return Ok(());
        }
        self.encode_with_gamma(scene, camera, FrameOut { view: &view, tex: &frame.texture, depth: &depth, can_copy, width: w, height: h, aspect });
        self.queue.present(frame);
        self.collect_lens();
        Ok(())
    }

    /// Submit what the frame recorded so far ([`FrameCmds`]) in one `Queue::submit` (up to three a frame: ahead of the main view,
    /// ahead of its post chain on the enhanced path, at the frame's end): the staging belt's chunks closed first and
    /// recalled after, the profiler's passes resolved in the last encoder, the lens blocks copied in the frame mapped after it.
    /// Nothing recorded = nothing submitted.
    pub(crate) fn submit_frame(&mut self) {
        if self.frame_cmds.is_empty() {
            return;
        }
        self.belt.finish();
        let last = self.frame_cmds.open.take().map(|mut e| {
            crate::prof::resolve(&mut e);
            crate::prof::finish(e)
        });
        self.last_submit = Some(crate::prof::submit(&self.device, &self.queue, self.frame_cmds.done.drain(..).chain(last)));
        self.belt.recall();
        self.lens_map_staged();
    }

    /// PERF (picture identical): the frame's first submit. What the command list recorded ahead of the main view - the
    /// rear-view mirror, the environment faces with their copies, E49's mask and E16's probe - and the coat prefilter of the
    /// faces drawn (which the main view's `encode_frame` would record first, [`Self::refl_prefilter`]) go to the GPU before
    /// the main view is encoded: the GPU runs them while the CPU records the main view (one submit for the whole frame
    /// made the GPU wait for all of it: +1.3..1.9 ms of a frame's latency, RE/notes/RENDER-AUDIT-2026-11-15.md §12). The queue runs
    /// the submits in order, so every pass still reads what the passes before it wrote. Nothing recorded = no submit.
    pub(super) fn submit_ahead_of_main(&mut self) {
        if self.frame_cmds.is_empty() {
            return;
        }
        let mut encoder = self.frame_cmds.take(&self.device);
        self.refl_prefilter(&mut encoder);
        self.frame_cmds.park(encoder);
        self.submit_frame();
    }

    /// Draw one stage's items into `pass`, in order: consecutive items of one mesh under one state
    /// are one instanced draw. `draws`: the frame's per-item records ([`Self::bucket_items`]); `st` is what the pass already
    /// has bound.
    pub(super) fn draw_bucket(&self, pass: &mut wgpu::RenderPass<'_>, draws: &[ItemDraw], bucket: &[u32], st: &mut DrawState) {
        let ms = self.multisampled_set;
        // consecutive items of one mesh under one state are one instanced draw
        let mut run: Option<Run> = None;
        for &idx in bucket {
            let d = &draws[idx as usize];
            // PERF (PERF-B): an item that can take a per-item part draws with a set that has it (`ensure_pipelines`)
            let var = (self.var_set[self.item_var.get(idx as usize).copied().unwrap_or(fill::ItemVariant::Both) as usize] as usize) << SET_SHIFT;
            let next = Run { first: idx, count: 1, pipeline: d.group.index(ms) | var, material: d.material, sky: d.sky, mesh: d.mesh };
            if d.twice {
                // two pipelines in turn on one item: never merged, and the run before it
                // is drawn first
                if let Some(r) = run.take() {
                    self.draw_run(pass, st, &r);
                }
                self.draw_run(pass, st, &Run { pipeline: d.plain.index(ms) | var, ..next });
                self.draw_run(pass, st, &next);
                continue;
            }
            match &mut run {
                Some(r) if r.first + r.count == idx && r.pipeline == next.pipeline && r.material == next.material && r.sky == next.sky && r.mesh == next.mesh => r.count += 1,
                _ => {
                    if let Some(r) = run.replace(next) {
                        self.draw_run(pass, st, &r);
                    }
                }
            }
        }
        if let Some(r) = run.take() {
            self.draw_run(pass, st, &r);
        }
    }

    /// Set what a run needs that the pass does not already hold, then draw it: one instanced draw
    /// of `r.count` consecutive objects (`first_instance` = the first object's index).
    pub(super) fn draw_run(&self, pass: &mut wgpu::RenderPass<'_>, st: &mut DrawState, r: &Run) {
        if st.pipeline != r.pipeline {
            let set = match r.pipeline >> SET_SHIFT {
                0 => &self.pipelines,
                k => &self.pipeline_vars[k - 1],
            };
            pass.set_pipeline(&set[r.pipeline & ((1 << SET_SHIFT) - 1)]);
            st.pipeline = r.pipeline;
        }
        let m = &self.materials[(r.material >> 1) as usize];
        let point = r.material & 1 == 1 && m.bind_group_point.is_some();
        let key = bind_key(m, point);
        if st.material != key {
            let bg = if point { m.bind_group_point.as_ref().unwrap_or(&m.bind_group) } else { &m.bind_group };
            pass.set_bind_group(2, bg, &[]);
            st.material = key;
        }
        if st.sky != r.sky {
            pass.set_bind_group(3, self.sky_bgs[r.sky as usize].as_ref().expect("sky bind group made"), &[]);
            st.sky = r.sky;
        }
        let mesh = &self.meshes[r.mesh as usize];
        if self.base_vertex {
            // the whole buffer bound once per arena; the mesh is its range of indices and its first
            // vertex — the same vertices and indices as a bind of its own slices
            if st.vb != mesh.vb_serial {
                pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                st.vb = mesh.vb_serial;
            }
            if st.ib != mesh.ib_serial {
                pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                st.ib = mesh.ib_serial;
            }
            let first_index = (mesh.index_offset / 4) as u32;
            crate::prof::count_draw(u64::from(mesh.index_count / 3) * u64::from(r.count));
            pass.draw_indexed(first_index..first_index + mesh.index_count, (mesh.vertex_offset / VERTEX_STRIDE) as i32, r.first..r.first + r.count);
            return;
        }
        if st.mesh != r.mesh {
            pass.set_vertex_buffer(0, mesh.vertex_buf.slice(mesh.vertex_offset..mesh.vertex_offset + mesh.vertex_len.max(4)));
            pass.set_index_buffer(mesh.index_buf.slice(mesh.index_offset..mesh.index_offset + mesh.index_len.max(4)), wgpu::IndexFormat::Uint32);
            st.mesh = r.mesh;
        }
        crate::prof::count_draw(u64::from(mesh.index_count / 3) * u64::from(r.count));
        pass.draw_indexed(0..mesh.index_count, 0, r.first..r.first + r.count);
    }

    /// The 2D pass's vertices into the belt (before any pass opens: a copy cannot be recorded
    /// inside one); how many sprites are drawn.
    /// `verts`: the frame's scratch ([`FrameScratch::sprites`]), refilled.
    pub(super) fn upload_sprites(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, with_sprites: bool, aspect: f32, verts: &mut Vec<SpriteVertex>) -> usize {
        // the 2D pass's vertices go into the belt before any pass opens (a copy cannot be
        // recorded inside one)
        let n_sprites = if with_sprites { scene.sprites.len().min(MAX_SPRITES as usize) } else { 0 };
        if with_sprites && scene.sprites.len() > MAX_SPRITES as usize {
            tracing::warn!(wanted = scene.sprites.len(), drawn = n_sprites, "sprite budget exceeded — the rest of the frame is dropped");
        }
        if n_sprites > 0 {
            let size = wgpu::BufferSize::new((n_sprites * 6 * std::mem::size_of::<SpriteVertex>()) as u64).expect("sprites size");
            verts.clear();
            verts.resize(n_sprites * 6, bytemuck::Zeroable::zeroed());
            for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
                verts[i * 6..i * 6 + 6].copy_from_slice(&sprite_quad(sp, aspect));
            }
            self.belt.write_buffer(encoder, &self.sprite_buf, 0, size).copy_from_slice(bytemuck::cast_slice(verts));
        }
        n_sprites
    }

    /// The 2D sprite pass over `view` (drawn into a 1-sample target with its own `depth_view`).
    pub(super) fn sprite_pass(&self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, n_sprites: usize, view: &wgpu::TextureView, depth_view: &wgpu::TextureView) {
        // The 2D pass, over the 3-D one. Its depth starts at 1.0: the front end clears the
        // depth window before its 2D draws (0x4DA890 at 0x4D30AC / 0x4D3182: `THRASH_window(3)`
        // then `THRASH_clearwindow`, which on `dx7z.dll` clears the depth buffer alone —
        // 0x60001230 → 0x60008380, `D3DCLEAR_ZBUFFER` without the target when the depth window
        // is current — to the view's `+0x34`, 1.0 (0x537741, Thrash 0x43, 0x60004C90)). Only a
        // sprite with a depth state (`Sprite::depth`) tests or writes it; every other draws
        // as before. With no sprite the pass would draw nothing, so it is not opened.
        // PERF (picture identical): a frame with no depth-state sprite (the race's HUD) attaches no depth image (its sprites
        // test `Always` and write none)
        if n_sprites > 0 {
            let depth = scene.sprites.iter().take(n_sprites).any(|s| s.depth.is_some());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: depth.then_some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: crate::prof::rp(Stage::Sprites.label()),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_sprites(&mut pass, scene, n_sprites, depth);
        }
    }

    /// The frame's first `n_sprites` sprites (uploaded by `upload_sprites`) into `pass`; `depth`: the pass has the sprites' depth
    /// image (a sprite with a depth state is among them), else every sprite takes the depth-less pipeline.
    pub(super) fn draw_sprites(&self, pass: &mut wgpu::RenderPass<'_>, scene: &Scene, n_sprites: usize, depth: bool) {
        // HUD sprites; consecutive ones on one texture and one pipeline are one draw (their
        // vertices are contiguous in the buffer, in order)
        pass.set_vertex_buffer(0, self.sprite_buf.slice(..));
        let mut bound_pipeline: Option<Option<SpriteCompare>> = None;
        let mut bound_tex = u32::MAX;
        let mut run: Option<(usize, usize, u32, Option<SpriteCompare>)> = None;
        let mut flush = |pass: &mut wgpu::RenderPass<'_>, r: (usize, usize, u32, Option<SpriteCompare>)| {
            let (first, end, tex, pipeline) = r;
            let Some(bg) = self.textures[tex as usize].sprite_bg.as_ref() else { return };
            if bound_pipeline != Some(pipeline) {
                pass.set_pipeline(match pipeline {
                    None if !depth => &self.sprite_flat_pipeline,
                    None => &self.sprite_pipeline,
                    Some(SpriteCompare::LessEqual) => &self.sprite_depth_pipelines[0],
                    Some(SpriteCompare::GreaterEqual) => &self.sprite_depth_pipelines[1],
                });
                bound_pipeline = Some(pipeline);
            }
            if bound_tex != tex {
                pass.set_bind_group(0, bg, &[]);
                bound_tex = tex;
            }
            crate::prof::count_draw(((end - first) * 2) as u64);
            pass.draw((first * 6) as u32..(end * 6) as u32, 0..1);
        };
        for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
            if self.textures[sp.texture.0 as usize].sprite_bg.is_none() {
                continue;
            }
            let pipeline = sp.depth.map(|d| d.compare);
            match &mut run {
                Some((_, end, tex, pl)) if *end == i && *tex == sp.texture.0 && *pl == pipeline => *end += 1,
                _ => {
                    if let Some(r) = run.replace((i, i + 1, sp.texture.0, pipeline)) {
                        flush(pass, r);
                    }
                }
            }
        }
        if let Some(r) = run.take() {
            flush(pass, r);
        }
    }

    /// The items' stages and draw records, in one walk: each goes into its stage's list, in submission order — no-depth (sky)
    /// first, then opaque, then blended; an `early` blended item goes with the opaque ones (`DrawItem::early`) — its draw
    /// record ([`ItemDraw`]: pipelines, bind groups, mesh) into `scratch.draws`, and the section maps its sky-map pass samples
    /// get their bind groups. For a temporal pass (`temporal`) also each item's sky flag (the camera's rotation moves it) and
    /// its role for the masks (1 a translucent effect), in `scratch.sky_flags` / `scratch.roles`.
    fn bucket_items(&mut self, scene: &Scene, n: usize, temporal: bool, scratch: &mut FrameScratch) -> Buckets {
        let mut lists = std::mem::take(&mut self.buckets);
        for b in &mut lists {
            b.clear();
        }
        scratch.draws.clear();
        let (mut soft_blended, mut blended_zwrite) = (false, false);
        for (i, item) in scene.items.iter().take(n).enumerate() {
            let mat = &self.materials[item.material.0 as usize];
            let stage = if item.overlay {
                DrawStage::Overlay
            } else if !mat.depth_test {
                DrawStage::Sky
            } else if mat.alpha_blend && !item.early {
                DrawStage::Blended
            } else {
                DrawStage::Opaque
            };
            let skip = (item.main_view_only && self.no_reflections) || item.shadow_only;
            let sky_texture = sky_tex_of(&self.materials, self.no_reflections, item);
            // a blended material with the specular, sky-map or projector pass draws them over its
            // own blend, not mixed into its colour (`fs_layers`); the plain alpha pair only
            let layered = mat.alpha_blend && mat.blend_pair == 0 && (sky_texture.is_some() || item.shadow.is_some() || (mat.env_mix > 0.0 && !self.no_reflections));
            let (plain, group) = item_pipelines(mat, item, scene.mirror, layered);
            // `LayerFalseSort1` / `2`: at `SPECIALFX` 3, always, and at 2 by the runtime
            // pick (`FakeSortMode`, `RE/notes/thrash.md` §6d.4), the same draw first with
            // depth writes off, then on (0x449ADB / 0x449B2A). Otherwise (below 2, or 2's
            // `Single` pick) the other arm never draws twice: one draw, `MAG`/`MINFILTER`
            // forced `POINT` for it (0x449ABE), depth writes untouched.
            let fakesort_single = match self.fakesort_mode {
                FakeSortMode::Single => true,
                FakeSortMode::Double => false,
                // `LayerFalseSort1`/`2`'s own fold, per drawn item (0x449AB2, `[0x61975C]
                // < 10000` unsigned)
                FakeSortMode::PerItem => !item.fakesort_near,
            };
            let twice = mat.false_sort && mat.alpha_blend && !fakesort_single;
            let point = mat.false_sort && mat.alpha_blend && mat.bind_group_point.is_some() && !twice;
            let alpha_blend = mat.alpha_blend;
            let device_states = mat.device_states;
            scratch.draws.push(ItemDraw {
                plain,
                group,
                twice,
                material: (item.material.0 * 2) | point as u32,
                sky: sky_texture.unwrap_or(self.white).0,
                sky_on: sky_texture.is_some(),
                mesh: self.shown_mesh(item.mesh.0),
            });
            if let Some(t) = sky_texture {
                self.sky_bind_group(t);
            }
            if !skip {
                lists[stage as usize].push(i as u32);
                if stage == DrawStage::Blended {
                    soft_blended |= item.soft > 0.0;
                    // the only blended pipelines that write depth (`pipelines.rs`)
                    blended_zwrite |= device_states;
                }
            }
            if temporal {
                scratch.sky_flags.push(stage == DrawStage::Sky);
                // `DrawItem` stages: 0 sky (the vector, no mask), 3 the lens flare / 2 blended / a blended `early` one: reactive
                scratch.roles.push(match stage {
                    DrawStage::Sky => crate::classes::ROLE_SKY,
                    DrawStage::Overlay | DrawStage::Blended => crate::classes::ROLE_EFFECT,
                    DrawStage::Opaque if alpha_blend => crate::classes::ROLE_EFFECT,
                    DrawStage::Opaque => crate::classes::ROLE_OPAQUE,
                });
            }
        }
        Buckets { lists, soft_blended, blended_zwrite }
    }

    /// The frame's globals into their buffer (through the belt, into `encoder`).
    fn write_globals(&mut self, encoder: &mut wgpu::CommandEncoder, f: &FrameIn, camera: &Camera) {
        let globals = fill::globals_uniform(&fill::GlobalsFrame {
            scene: f.scene,
            camera,
            vp_j: f.vp_j,
            enh: &self.enhancements,
            hdr: self.cur_hdr,
            no_reflections: self.no_reflections,
            refl_faces_done: self.refl.as_ref().is_some_and(|c| c.faces_done == ALL_FACES),
            probe_strength: self.probe_strength(f.scene),
            soft_split: f.soft_split,
            env: f.env,
            temporal: f.tcur.as_ref().map(|t| (t.jitter_ndc, t.size)),
            near: f.near,
        });
        let size = wgpu::BufferSize::new(std::mem::size_of::<GlobalsUniform>() as u64).expect("globals size");
        self.belt.write_buffer(encoder, &self.globals_buf, 0, size).copy_from_slice(bytemuck::bytes_of(&globals));
    }

    /// The first `f.n` items' object uniforms into their buffer (nothing for none), built in `scratch.objects` (refilled) from
    /// the walk's records (`scratch.draws`) and, for a temporal pass, the items' roles (`scratch.roles`, empty otherwise).
    fn write_objects(&mut self, encoder: &mut wgpu::CommandEncoder, f: &FrameIn, camera: &Camera, lights: &Lights, scratch: &mut FrameScratch) {
        let FrameScratch { objects, roles, draws, .. } = scratch;
        let n = f.n;
        self.item_var.clear();
        if n == 0 {
            return;
        }
        let size = wgpu::BufferSize::new((n * std::mem::size_of::<ObjectUniform>()) as u64).expect("objects size");
        // built on the CPU and copied in once: wgpu 30's mapped views are write-only
        // (`WriteOnly`), not a `&mut [u8]` to cast in place
        objects.clear();
        {
            let of = fill::ObjectFrame {
                camera,
                depth_far: f.depth_far,
                csm: lights.csm.as_ref().map(|c| fill::CsmFill { car_patch_gate: c.car_patch_gate(), decal_fades: c.decal_fades() }),
                soft_split: f.soft_split,
                spots: lights.spot.is_some() || lights.lamps_on,
                contact_on: lights.contact_on,
                env: f.env,
                roles: roles.as_slice(),
                motion_index: &self.motion.index,
                vp_j: glam::DMat4::from_cols_array(&f.vp_j.to_cols_array().map(f64::from)),
                lod_shift: self.cur_lod_shift,
                near_own: f.near.is_some_and(|n| n.own),
            };
            let (csm, contact) = (|it: &crate::DrawItem| self.csm_receive(it), |it: &crate::DrawItem| self.contact_receive(it));
            let rx = fill::Receivers { csm: &csm, contact: &contact };
            objects.extend(f.scene.items.iter().take(n).enumerate().map(|(i, item)| {
                let mat = self.materials.get(item.material.0 as usize).map(GpuMaterial::fill);
                fill::object_uniform(&of, i, item, mat, draws[i].sky_on, &rx)
            }));
        }
        self.belt.write_buffer(encoder, &self.object_buf, 0, size).copy_from_slice(bytemuck::cast_slice(objects));
        // PERF (PERF-B): which items draw with the set that has the per-item parts
        self.item_var.extend(objects.iter().map(fill::ItemVariant::of));
    }

    /// Which passes over the finished scene this frame has (E13 shafts, E36 haze, E44 lighthouse beam) and so which images
    /// the main pass keeps for them (its depth, its samples), and whether the depth is kept to the end of the frame (E27 /
    /// the depth of field (photo mode, E31) / E40 / ENH-TEMPORAL). `overlay`: the frame has an overlay stage (the lens flare).
    /// `spot_lit`: E15 lights spots this frame (its uniform's count is above 0; without them `spot_pass` zeroes it).
    fn post_flags(&self, f: &FrameIn, camera: &Camera, overlay: bool, spot_lit: bool) -> PostFlags {
        let (scene, with_sprites, depth_far, vp_j) = (f.scene, f.with_sprites, f.depth_far, f.vp_j);
        // the inverse clip transform E36 and E44 rebuild positions with: taken once, by the first that runs
        let inv = std::cell::OnceCell::new();
        let inv_vp = || *inv.get_or_init(|| vp_j.inverse());
        // ENHANCEMENT E13: light shafts of the main race view in the float image (E7 on, a sun point
        // in the scene): the depth is kept for the pass that reads it
        let shafts = scene.shafts.filter(|_| self.enhancements.is_on(crate::E13) && self.cur_hdr.is_some() && !with_sprites && depth_far == VIEW_FAR).map(|mut s| {
            // level 1 (subtle) / 2 (medium)
            s.strength *= crate::shaft_strength(self.enhancements.level(crate::E13));
            s
        });
        // ENHANCEMENT E36: the haze and the visible headlight beams of the main race view in the float image (E7 on, some
        // air in the scene: the pass is not run on a clear dry open road); the depth is kept for it like for the shafts
        let haze = scene.haze.filter(|_| self.enhancements.is_on(crate::E36) && self.cur_hdr.is_some() && !with_sprites && depth_far == VIEW_FAR && !self.no_reflections).and_then(|h| {
            let (beam, lamp) = h.air(self.enhancements.level(crate::E36));
            // PERF (picture identical): the march adds light only from E15's lit spots (`n_spots = spots.params.x`, the beams' term
            // is skipped without them) and from the tunnel lamps (`n_lamps = haze.n`, the glow's loop runs over them); with neither
            // every half-size texel is (0, 0, 0, distance), the composite adds exactly 0 (One + One blend) and the pass is not
            // run - nor the depth it would keep, its second depth conversion or the multisampled colour it would store
            ((beam > 1e-5 || lamp > 1e-5) && ((beam > 0.0 && spot_lit) || (lamp > 0.0 && h.n > 0))).then(|| {
                // ENHANCEMENT ENH-TEMPORAL: the haze rebuilds positions from the depth the jittered pass drew
                let fwd = (camera.target - camera.position).normalize_or_zero();
                crate::haze::Frame { inv_vp: inv_vp(), eye: camera.position, fwd, planes: [camera.near, camera.far], beam, lamp, gain: crate::haze::level_gain(self.enhancements.level(crate::E36)), haze: h }
            })
        });
        // PERF: the shafts go over the multisampled colour only when the overlay pass (the lens flare) draws after them
        // ENHANCEMENT E44: the lighthouse beam of the main race view in the float image (E7 on), after the blended pass; the depth is
        // kept for it like for the haze
        let lbeam = scene.lbeam.filter(|b| b.n > 0 && self.enhancements.is_on(crate::E44) && self.cur_hdr.is_some() && !with_sprites && depth_far == VIEW_FAR && !self.no_reflections).map(|b| {
            let fwd = (camera.target - camera.position).normalize_or_zero();
            crate::lbeam::Frame { inv_vp: inv_vp(), eye: camera.position, fwd, planes: [camera.near, camera.far], beams: b }
        });
        let post_depth = shafts.is_some() || haze.is_some() || lbeam.is_some();
        let shafts_ms = post_depth && overlay;
        // ENHANCEMENT E27: the depth is kept to the end of the frame for the motion blur (the main view's float scene)
        // ENHANCEMENT ENH-TEMPORAL: ... and for the temporal pass (and the upscaler after it)
        let keep_depth = f.tcur.is_some()
            || (((scene.motion.is_some() && self.enhancements.is_on(crate::E27))
                // ENHANCEMENTS E7 / E31: ... and for the depth of field (the photo mode's or a replay's lens)
                || scene.dof.is_some()
                // ENHANCEMENT E40: ... and for the windscreen drops
                || (scene.drops.is_some() && self.enhancements.is_on(crate::E40)))
                && self.cur_hdr.is_some()
                && !with_sprites
                && depth_far == VIEW_FAR);
        PostFlags { shafts, haze, lbeam, post_depth, shafts_ms, keep_depth, blended_depth: true }
    }

    /// One pass of the 3-D stages over the frame's targets, `role` (the lens reference, the main pass, the split pass's blended
    /// half, the overlay): its buckets drawn in turn, with the globals bind group `globals` and the object one `cx.object_bg`;
    /// what it loads, resolves and stores is the role's ([`StageRole::ops`]).
    /// ENHANCEMENT E1: a multisampled frame draws into the samples (`resolve`: resolved into `tg.view` at the end;
    /// `keep_samples`: stored for a later pass, else discarded); ENHANCEMENT ENH-TEMPORAL: a temporal main view also
    /// attaches its vector / mask targets (cleared with the colour, stored when `store_extra`).
    fn stage_pass(&self, encoder: &mut wgpu::CommandEncoder, cx: &StageCtx, role: StageRole, globals: &wgpu::BindGroup) {
        let (tg, ms) = (&cx.tg, cx.tg.ms);
        let op = role.ops(&cx.facts);
        let clear = op.clear.then_some(cx.clear);
        let colour = Some(wgpu::RenderPassColorAttachment {
            view: tg.color(),
            resolve_target: ms.filter(|_| op.resolve).map(|_| tg.view),
            ops: wgpu::Operations { load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear), store: if ms.is_some() && !op.keep_samples { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store } },
            depth_slice: None,
        });
        let [mv, reactive, tc] = temporal_attachments(cx.tcur, op.clear, op.store_extra);
        let atts = [colour, mv, reactive, tc];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(role.label()),
            color_attachments: &atts[..if cx.tcur.is_some() { atts.len() } else { 1 }],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: tg.depth(),
                depth_ops: Some(wgpu::Operations { load: if op.clear { wgpu::LoadOp::Clear(1.0) } else { wgpu::LoadOp::Load }, store: if op.store_depth { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                stencil_ops: None,
            }),
            timestamp_writes: crate::prof::rp(role.stage().label()),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, cx.object_bg, &[]);
        let mut st = DrawState::NONE;
        for bucket in &cx.buckets[role.buckets(cx.facts.split)] {
            self.draw_bucket(&mut pass, cx.draws, bucket, &mut st);
        }
    }

    /// ENHANCEMENT E12 / E14: the split pass's interlude, between the opaque stages and the blended one: the opaque part's
    /// depth into a readable image (returned), and the corner shading from it multiplied over the opaque colour (`ao_level`
    /// above 0).
    fn soft_and_ao(&mut self, encoder: &mut wgpu::CommandEncoder, tg: &FrameTargets, camera: &Camera, aspect: f32, ao_level: u8) -> wgpu::TextureView {
        let ms = tg.ms;
        let soft = self.soft.get_or_insert_with(|| crate::soft::SoftDepth::new(&self.device));
        let sv = soft.convert(&self.device, encoder, tg.depth(), ms.is_some(), tg.target.width(), tg.target.height());
        // ENHANCEMENT E14: the occlusion from that depth, multiplied over the opaque colour
        if ao_level > 0 {
            let ssao = self.ssao.get_or_insert_with(|| crate::ao::Ssao::new(&self.device));
            ssao.run(&self.device, &mut self.belt, encoder, ao_level, camera, aspect, tg.color(), tg.target.format(), ms.map_or(1, |m| m.samples), &sv, ms.map(|m| &m.depth), (tg.target.width(), tg.target.height()));
        }
        sv
    }

    /// ENHANCEMENT E12: the globals bind group with the split pass's depth image `soft` bound, kept while neither the image nor
    /// the globals bind group it was made from ([`Self::rebuild_globals_bg`]) changes.
    fn soft_globals_bg(&mut self, soft: &wgpu::TextureView) -> wgpu::BindGroup {
        match &self.soft_globals {
            Some((v, base, bg)) if v == soft && *base == self.globals_bg => bg.clone(),
            _ => {
                let bg = self.make_globals_bg(Some(soft));
                self.soft_globals = Some((soft.clone(), self.globals_bg.clone(), bg.clone()));
                bg
            }
        }
    }

    /// ENHANCEMENTS E13 / E44 / E36: the passes over the finished scene, in this order: the light shafts, the lighthouse
    /// beam, the haze (before the lens flare's test and elements, the sprites and the tone map). `soft_view`: the split
    /// pass's depth image (see below).
    fn scene_depth_passes(&mut self, encoder: &mut wgpu::CommandEncoder, post: PostFlags, tg: &FrameTargets, camera: &Camera, overlay: bool, mut soft_view: Option<wgpu::TextureView>) {
        // ENHANCEMENT ENH-HAZEDEPTH / ENH-SHAFTDEPTH: `soft_view` holds the OPAQUE-only depth (converted between the opaque and the
        // blended stage of a split pass, E12 / E14); the blended foliage writes depth too, so the shafts (E13: the sky mask) and the haze
        // (E36) read ONE fresh conversion of the FULL depth into the same image (E12's particles and E14 are done with it), as the lighthouse
        // beam reads the full depth (ENH-LBEAM2). No second pass: both effects share it; nothing runs without them.
        // PERF (picture identical): when no item of the blended half writes depth (`post.blended_depth`), the depth after it is
        // the opaque one, already in the image: no conversion
        if soft_view.is_some() && post.blended_depth && (post.shafts.is_some() || post.haze.is_some()) {
            let soft = self.soft.get_or_insert_with(|| crate::soft::SoftDepth::new(&self.device));
            soft_view = Some(soft.convert(&self.device, encoder, tg.depth(), tg.ms.is_some(), tg.target.width(), tg.target.height()));
            crate::prof::mark(encoder, Stage::DepthConversion2);
        }
        // ENHANCEMENT E13: the light shafts, added into the finished float scene (before the lens
        // flare's test and elements, the sprites and the tone map)
        if let Some(sh) = post.shafts {
            let pass = self.shafts.get_or_insert_with(|| crate::shafts::ShaftsPass::new(&self.device, HDR_FORMAT));
            pass.draw(&mut self.belt, encoder, &sh, [camera.near, camera.far], &over_scene(tg, overlay, soft_view.as_ref()));
            crate::prof::mark(encoder, Stage::Shafts);
        }
        // ENHANCEMENT E44: the lighthouse beam, over the finished scene before the shafts / haze (they add to it) and the lens flare.
        // ENHANCEMENT ENH-LBEAM2: it reads the FULL depth (opaque + alpha-tested + the blended foliage's own depth writes, i.e. the
        // image after the blended pass), never the soft pass's opaque-only copy (taken between the opaque and the blended
        // stage of a split pass, E12 / E14): the beam was drawn over every blended tree and bush in front of the lamp.
        if let Some(bf) = post.lbeam {
            let pass = self.lbeam.get_or_insert_with(|| crate::lbeam::LBeamPass::new(&self.device, HDR_FORMAT));
            pass.draw(&mut self.belt, encoder, &bf, &over_scene(tg, overlay, None));
            crate::prof::mark(encoder, Stage::LighthouseBeam);
        }
        // ENHANCEMENT E36: the haze, after the shafts and before the lens flare, the sprites and the tone map
        if let Some(hf) = post.haze {
            let pass = self.haze.get_or_insert_with(|| crate::haze::HazePass::new(&self.device, HDR_FORMAT));
            pass.draw(&mut self.belt, encoder, &hf, &over_scene(tg, overlay, soft_view.as_ref()), &self.spot.uniform.buf, self.spot.view());
            crate::prof::mark(encoder, Stage::Haze);
        }
    }

    /// Draw `d.scene` into `d.view` / `d.depth`: the 3-D items (sky, opaque, blended), then — when
    /// `with_sprites` — the 2D sprites. Every buffer write of the frame goes through the staging
    /// belt into this frame's encoder, so nothing allocates a staging buffer per call. Neither
    /// pass stores its depth: nothing reads it afterwards (the sprite pass clears it to 1.0).
    pub(super) fn encode_frame(&mut self, d: FrameDesc) {
        let FrameDesc { scene, clear: clear_color, with_sprites, camera, view, target, can_copy, depth: depth_view, aspect, depth_far, ms } = d;
        // ENHANCEMENT E1: the pipelines for the target's sample count (`None` = master's, 1)
        // ENHANCEMENT E7: ... and for the float set when the target is the float image
        // ENHANCEMENT ENH-TEMPORAL: ... and for the temporal set (vector / mask targets) in a temporal main view
        // ENHANCEMENT ENH-COAT: the environment cube's prefiltered mips are brought up to date before any frame that reads them
        let mut encoder = self.frame_cmds.take(&self.device);
        if !self.refl_building {
            self.refl_prefilter(&mut encoder);
        }
        let tcur = self.cur_temporal.clone();
        // the items' stage and, for the temporal pass, their role (sky: the camera's rotation moves it; 1 a translucent effect)
        let mut scratch = std::mem::take(&mut self.frame_scratch);
        scratch.sky_flags.clear();
        scratch.roles.clear();
        let n = scene.items.len().min(MAX_OBJECTS as usize);
        let white = self.white;
        self.sky_bind_group(white);
        let Buckets { lists: buckets, soft_blended, blended_zwrite } = self.bucket_items(scene, n, tcur.is_some(), &mut scratch);

        // ENHANCEMENT ENH-TEMPORAL: this frame's reprojection matrices (cleared in a frame without them: the object pass reads none)
        if let (Some(_), Some(tp)) = (tcur.as_ref(), self.temporal.as_mut()) {
            tp.motion(&scene.items[..n], &scratch.sky_flags, camera, aspect, scene.mirror, &mut self.motion);
            tp.commit();
        } else {
            self.motion.index.clear();
        }
        // ENHANCEMENT E12: with the switch on and a soft sprite among the blended items the main pass is
        // split after the opaque stages, so the depth they wrote can be read by the sprites
        let soft_split = self.enhancements.is_on(crate::E12) && soft_blended;
        // ENHANCEMENT E14: the corner shading (the race's main view, not the mirror) also splits the pass
        // after the opaque stages: it multiplies the opaque colour, the blended stage comes after it
        let ao_level = if scene.ao && !self.no_reflections && n > 0 { self.enhancements.level(crate::E14) } else { 0 };
        let split = soft_split || ao_level > 0;
        self.seal_arena();
        // ENHANCEMENT E49: an environment face's near-cars pass first keeps the face without them (`refl.rs`)
        if let (Some(layer), Some(c)) = (self.refl_pre_copy.take(), self.refl.as_ref()) {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: &c.face, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: &c.array, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: c.size, height: c.size, depth_or_array_layers: 1 },
            );
        }
        self.flush_texture_writes(&mut encoder);
        self.flush_mesh_writes(&mut encoder);
        // ENHANCEMENT ENH-TEMPORAL: the matrices into their buffer (the object pass's second binding)
        if let (Some(g), true) = (self.temporal_gpu.as_ref(), tcur.is_some()) {
            let size = wgpu::BufferSize::new((self.motion.mats.len() * 64) as u64).expect("motion size");
            self.belt.write_buffer(&mut encoder, &g.motion_buf, 0, size).copy_from_slice(bytemuck::cast_slice(&self.motion.mats));
        }
        let lights = Lights {
            // ENHANCEMENT E9: this frame's sun-shadow cascades (`None` = master's frame, no extra pass)
            csm: self.csm_frame(scene, camera, aspect),
            // ENHANCEMENT E15: this frame's headlight spots (`None` = master's frame)
            spot: self.spot_frame(scene),
            // ENHANCEMENT E23: this frame's street lamps
            lamps_on: self.lamps_on(scene),
            // ENHANCEMENT E43: this frame's contact shadows
            contact_on: self.contact_on(scene),
        };
        // PERF (PERF-B): the parts whose light is off this frame (their uniform is cleared: the code would run for nothing)
        let shut = {
            use crate::classes::feat;
            (if lights.csm.is_none() { feat::CSM } else { 0 }) | (if lights.spot.is_none() { feat::SPOT } else { 0 }) | (if lights.lamps_on { 0 } else { feat::LAMPS | feat::LSHAD }) | (if lights.contact_on { 0 } else { feat::CONTACT })
                // ENHANCEMENT E50 / E51: the moon's part only in a moon frame, the cars-only layers' only in a frame that has them
                | (if lights.csm.as_ref().is_some_and(|c| c.is_moon()) { 0 } else { feat::MOON })
                | (if lights.csm.as_ref().is_some_and(|c| c.has_car_layers()) { 0 } else { feat::CARSH })
                // (SHADOW-REFACTOR: the front end's focus shadow's and E25's kernels only in their own frames)
                | (if lights.csm.as_ref().is_some_and(|c| c.is_focus()) { 0 } else { feat::FOCUS })
                | (if lights.csm.as_ref().is_some_and(|c| c.is_overcast()) { 0 } else { feat::OVERCAST })
        };
        // PERF (PERF-D): a frame without a 3-D item (a loading screen, a 2-D front-end screen) draws no mesh: it needs no set (its
        // family, the 8-bit one of a sprite-only scene, was built in such a frame for nothing)
        if n > 0 {
            self.ensure_pipelines(ms.map_or(1, |m| m.samples), self.cur_hdr.is_some(), tcur.is_some(), shut);
        }
        let vp = camera.view_projection_mirrored(aspect, scene.mirror);
        // ENHANCEMENT ENH-TEMPORAL: the main view's clip transform carries the sub-pixel jitter (`vp` stays the un-jittered one:
        // the vectors, the shadow fits, the lens flare's position); the passes that rebuild a position from the depth
        // image take the jittered one: the haze does; the corner shading (E14) does not (its rays are the pixel centres', the
        // occlusion is off by at most half a pixel of the jitter, a smooth multiplier)
        let vp_j = tcur.as_ref().map_or(vp, |t| crate::temporal::jittered(vp, t.jitter_ndc));
        // ENHANCEMENT E17 / E26 / E18 / E11: the water's sky / sun and the wet road's sky and lamps (only with their switch on), the
        // particles' HDR / sun lighting and the HDR boost (on the float scene only)
        let env = fill::FrameEnv::new(scene, &self.enhancements, self.cur_hdr);
        let near = self.near_frame(scene);
        let f = FrameIn { scene, with_sprites, depth_far, n, tcur, vp_j, soft_split, env, near };
        self.write_globals(&mut encoder, &f, camera);
        self.write_objects(&mut encoder, &f, camera, &lights, &mut scratch);
        // TEST GLUE (`--perf-bench`): the stage marks of the frame (nothing is recorded without the bench)
        crate::prof::mark(&mut encoder, Stage::Uploads);

        // ENHANCEMENT E9 / E15 / E47: the frame's casters, collected once for every shadow map that draws this frame
        let mut casters = std::mem::take(&mut scratch.casters);
        casters.clear();
        if lights.csm.is_some() || lights.spot.as_ref().is_some_and(|s| s.casts()) || self.lamp_shadows_on(scene) {
            self.frame_casters(scene, n, &mut casters);
        }
        // ENHANCEMENT E9: the shadow cascades, before the passes that read them
        self.csm_pass(&mut encoder, scene, &casters, camera, lights.csm.as_ref());
        crate::prof::mark(&mut encoder, Stage::SunShadow);
        // ENHANCEMENT E15: the player's lamp pair's shadow map
        self.spot_pass(&mut encoder, scene, &casters, lights.spot.as_ref());
        crate::prof::mark(&mut encoder, Stage::LampPairShadow);
        // ENHANCEMENT E23 / E47: the street lamps and their car shadow maps
        self.lamp_pass(&mut encoder, scene, &casters, camera);
        crate::prof::mark(&mut encoder, Stage::StreetLamps);
        // ENHANCEMENT E43: the cars' contact shadow (SHADOW-REAL: its share against E9's sun shadow is split per pixel in the shader)
        self.contact_pass(&mut encoder, scene);
        crate::prof::mark(&mut encoder, Stage::ContactShadow);

        let n_sprites = self.upload_sprites(&mut encoder, scene, with_sprites, aspect, &mut scratch.sprites);

        let c = clear_color;
        let clear = wgpu::Color { r: c[0], g: c[1], b: c[2], a: c[3] };
        let probe = scene.lens_probe.filter(|_| can_copy);
        let overlay = !buckets[DrawStage::Overlay as usize].is_empty();
        let post = self.post_flags(&f, camera, overlay, lights.spot.is_some());
        let tg = FrameTargets { view, target, depth_view, ms };
        let tcur = f.tcur.as_ref();
        // ENHANCEMENT ENH-TEMPORAL: the object bind group that has the matrices; master's in master's frame
        let object_bg = tcur.map_or_else(|| self.object_bg.clone(), |t| t.objects_bg.clone());
        let facts = PassFacts { overlay, split, shafts_ms: post.shafts_ms, post_depth: post.post_depth, keep_depth: post.keep_depth, near: self.refl_near_pass };
        let cx = StageCtx { tg, tcur, object_bg: &object_bg, draws: &scratch.draws, buckets: &buckets, clear, facts };
        let tg = &cx.tg;
        // The lens flare's reference (0x460290 at 0x4603FF): the block as the sky pass leaves it with
        // the sun drawn — the stage with no depth test (the sky layers and the sun) alone, drawn
        // ahead of the frame proper into the same target, which the main pass then clears.
        if let Some(p) = probe.filter(|p| p.reference) {
            self.stage_pass(&mut encoder, &cx, StageRole::LensReference, &self.globals_bg);
            self.lens_copy(&mut encoder, target, p, true);
            crate::prof::mark(&mut encoder, Stage::LensReference);
        }
        self.stage_pass(&mut encoder, &cx, StageRole::Main, &self.globals_bg);
        crate::prof::mark(&mut encoder, Stage::MainPass);
        // ENHANCEMENT E12: the opaque part's depth into a readable image, then the blended stage in a
        // second pass over the same colour and depth (loaded), the soft sprites reading that image;
        // the draw order, blend states and depth tests of the stage are the original's
        let mut soft_view = None;
        if split {
            let sv = self.soft_and_ao(&mut encoder, tg, camera, aspect, ao_level);
            crate::prof::mark(&mut encoder, Stage::SoftDepthAo);
            soft_view = Some(sv.clone());
            // (only E12's soft sprites read the depth image: with AO alone the blended half binds the frame's own globals)
            let soft_bg = if soft_split { self.soft_globals_bg(&sv) } else { self.globals_bg.clone() };
            self.stage_pass(&mut encoder, &cx, StageRole::SplitBlended, &soft_bg);
            crate::prof::mark(&mut encoder, Stage::MainPassBlended);
        }
        // the split pass's blended half writes depth: one of its items has a blended material with depth writes (`device_states`,
        // the only blended pipelines that write it, `pipelines.rs`)
        let post = PostFlags { blended_depth: split && blended_zwrite, ..post };
        self.scene_depth_passes(&mut encoder, post, tg, camera, overlay, soft_view);
        // The lens flare's test (0x460525): the block of the finished 3-D pass, before the flare's
        // own elements are laid over it
        if let Some(p) = probe.filter(|p| p.test) {
            self.lens_copy(&mut encoder, target, p, false);
            crate::prof::mark(&mut encoder, Stage::LensTest);
        }
        // the lens flare's elements (0x460460 → 0x451710): flat on the screen over the finished
        // frame, under no depth test
        if overlay {
            self.stage_pass(&mut encoder, &cx, StageRole::Overlay, &self.globals_bg);
            crate::prof::mark(&mut encoder, Stage::Overlay);
        }
        self.sprite_pass(&mut encoder, scene, n_sprites, view, depth_view);
        if n_sprites > 0 {
            crate::prof::mark(&mut encoder, Stage::Sprites);
        }
        self.buckets = buckets;
        scratch.casters = casters;
        self.frame_scratch = scratch;
        // submitted with the rest of the frame (`submit_frame`)
        self.frame_cmds.park(encoder);
    }
}

/// One [`Core::encode_frame`]: what it draws (`scene` over `clear`, its 2D on top when `with_sprites`), from where (`camera`, the
/// view's `aspect`, `depth_far` the far plane of the pass's own screen z: the depth bias's scale) and into what (`view` of `target`,
/// which the lens flare's blocks may copy from when `can_copy`, the single-sample `depth`, ENHANCEMENT E1's samples `ms`).
pub(super) struct FrameDesc<'a> {
    pub scene: &'a Scene,
    pub clear: [f64; 4],
    pub with_sprites: bool,
    pub camera: &'a Camera,
    pub view: &'a wgpu::TextureView,
    pub target: &'a wgpu::Texture,
    pub can_copy: bool,
    pub depth: &'a wgpu::TextureView,
    pub aspect: f32,
    pub depth_far: f32,
    pub ms: Option<&'a Ms>,
}

/// What the 3-D passes of one `encode_frame` draw into: the colour `view` of the texture `target` (the resolve target of a
/// multisampled frame), the single-sample depth, and ENHANCEMENT E1's multisampled colour and depth.
pub(super) struct FrameTargets<'a> {
    pub view: &'a wgpu::TextureView,
    pub target: &'a wgpu::Texture,
    pub depth_view: &'a wgpu::TextureView,
    pub ms: Option<&'a Ms>,
}

impl<'a> FrameTargets<'a> {
    /// The colour the passes draw into: the samples of a multisampled frame, else `view`.
    fn color(&self) -> &'a wgpu::TextureView {
        self.ms.map_or(self.view, |m| &m.color)
    }

    /// The depth the passes test: the samples' of a multisampled frame, else `depth_view`.
    fn depth(&self) -> &'a wgpu::TextureView {
        self.ms.map_or(self.depth_view, |m| &m.depth)
    }
}

/// The stage an item draws in (its bucket), in drawing order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawStage {
    /// no depth test: the sky layers and the sun
    Sky,
    Opaque,
    Blended,
    /// the lens flare's elements, flat over the finished frame
    Overlay,
}

/// [`Core::bucket_items`]'s result: the item indices by [`DrawStage`], and what the blended stage holds.
struct Buckets {
    lists: [Vec<u32>; 4],
    /// ENHANCEMENT E12: a blended item has a soft fade
    soft_blended: bool,
    /// a blended item writes depth (its material's `device_states`)
    blended_zwrite: bool,
}

/// The inputs of one `encode_frame` the steps share, gathered once.
struct FrameIn<'a> {
    scene: &'a Scene,
    with_sprites: bool,
    /// the far plane the game's depth bias is scaled with
    depth_far: f32,
    /// the items drawn (at most `MAX_OBJECTS`)
    n: usize,
    /// ENHANCEMENT ENH-TEMPORAL: the temporal main view's targets (`None` = master's frame)
    tcur: Option<crate::temporal::Cur>,
    /// the main view's clip transform (ENHANCEMENT ENH-TEMPORAL: jittered in a temporal main view)
    vp_j: glam::Mat4,
    /// ENHANCEMENT E12: the main pass is split for the soft sprites
    soft_split: bool,
    env: fill::FrameEnv<'a>,
    /// ENHANCEMENT E49: the near lookup of this frame (`None` = off)
    near: Option<fill::NearFrame>,
}

/// ENHANCEMENTS E9 / E15 / E23 / E43: this frame's sun-shadow cascades, headlight spots, street lamps and contact shadows.
struct Lights {
    csm: Option<csm::CsmFrame>,
    spot: Option<spot::SpotFrame>,
    lamps_on: bool,
    contact_on: bool,
}

/// ENHANCEMENTS E13 / E36 / E44 and the depth they keep, from [`Core::post_flags`].
struct PostFlags {
    shafts: Option<crate::Shafts>,
    haze: Option<crate::haze::Frame>,
    lbeam: Option<crate::lbeam::Frame>,
    /// a pass over the finished scene reads the depth: the main pass stores it
    post_depth: bool,
    /// ... and the lens flare draws after them: they go over the multisampled colour (kept), not the resolved image
    shafts_ms: bool,
    /// the depth is kept to the end of the frame (E27 / the depth of field (photo mode, E31) / E40 / ENH-TEMPORAL)
    keep_depth: bool,
    /// a split pass's blended half writes depth (set after the split; `true` until then)
    blended_depth: bool,
}

/// The 3-D passes of one `encode_frame` ([`Core::stage_pass`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StageRole {
    /// the lens flare's reference: the sky stage alone, ahead of the frame proper (its target is then cleared again)
    LensReference,
    /// the sky, opaque and blended stages (ENHANCEMENT E12 / E14: the opaque ones only in a split frame)
    Main,
    /// ENHANCEMENT E12 / E14: the split pass's blended half, over the colour and depth the main pass kept
    SplitBlended,
    /// the lens flare's elements, flat over the finished frame
    Overlay,
}

/// What one `encode_frame`'s passes keep for each other, from which the roles' load / store ops follow.
struct PassFacts {
    /// the frame has an overlay stage (the lens flare), drawn over the multisampled colour
    overlay: bool,
    /// ENHANCEMENT E12 / E14: the main pass is split after the opaque stages
    split: bool,
    /// [`PostFlags::shafts_ms`], [`PostFlags::post_depth`], [`PostFlags::keep_depth`]
    shafts_ms: bool,
    post_depth: bool,
    keep_depth: bool,
    /// ENHANCEMENT E49: the environment face's near-cars pass this frame is (`Core::refl_near_pass`)
    near: Option<NearPass>,
}

/// A role's attachment ops ([`StageRole::ops`]).
struct PassOps {
    /// the colour clears to the frame's clear colour, the depth to 1.0, the temporal targets to 0; else all three load
    clear: bool,
    /// ENHANCEMENT E1: resolve the samples into the frame's view at the end of the pass
    resolve: bool,
    /// ENHANCEMENT E1: store the samples for a later pass (else discarded)
    keep_samples: bool,
    /// ENHANCEMENT ENH-TEMPORAL: store the vector / mask targets
    store_extra: bool,
    store_depth: bool,
}

impl StageRole {
    /// The pass's wgpu label.
    fn label(self) -> &'static str {
        match self {
            StageRole::LensReference => "lens reference pass",
            StageRole::Main => "main pass",
            StageRole::SplitBlended => "main pass (blended, soft particles)",
            StageRole::Overlay => "overlay pass",
        }
    }

    /// The profiler's name of the pass (TEST GLUE).
    fn stage(self) -> Stage {
        match self {
            StageRole::LensReference => Stage::LensReference,
            StageRole::Main => Stage::MainPass,
            StageRole::SplitBlended => Stage::MainPassBlended,
            StageRole::Overlay => Stage::Overlay,
        }
    }

    /// The buckets ([`DrawStage`]) the pass draws.
    fn buckets(self, split: bool) -> std::ops::Range<usize> {
        let (sky, blended, overlay) = (DrawStage::Sky as usize, DrawStage::Blended as usize, DrawStage::Overlay as usize);
        match self {
            StageRole::LensReference => sky..sky + 1,
            StageRole::Main => sky..if split { blended } else { blended + 1 },
            StageRole::SplitBlended => blended..blended + 1,
            StageRole::Overlay => overlay..overlay + 1,
        }
    }

    fn ops(self, f: &PassFacts) -> PassOps {
        match self {
            // ENHANCEMENT E1: multisampled, resolved into `view` (the probe reads the resolved image)
            StageRole::LensReference => PassOps { clear: true, resolve: true, keep_samples: false, store_extra: false, store_depth: false },
            // ENHANCEMENT E1: multisampled, resolved into `view`; the samples are kept only for the overlay pass that draws
            // over them. ENHANCEMENT E12: a split pass keeps its samples and resolves in the second half
            StageRole::Main => PassOps {
                // ENHANCEMENT E49: the near cars of an environment face are drawn over the face (its colour and depth loaded)
                clear: f.near != Some(NearPass::Over),
                resolve: !f.split && !f.shafts_ms,
                keep_samples: f.overlay || f.split || f.shafts_ms,
                store_extra: true,
                // ENHANCEMENT E49: ... and the face keeps its depth for them (and, NEAR-REFL2, both keep it for the geometry mask)
                store_depth: f.overlay || f.split || f.post_depth || f.keep_depth || f.near.is_some(),
            },
            StageRole::SplitBlended => PassOps {
                clear: false,
                resolve: !f.shafts_ms,
                keep_samples: f.overlay || f.shafts_ms,
                store_extra: true,
                store_depth: f.overlay || f.post_depth || f.keep_depth || f.near.is_some(),
            },
            StageRole::Overlay => PassOps { clear: false, resolve: true, keep_samples: false, store_extra: true, store_depth: f.keep_depth },
        }
    }
}

/// What every [`Core::stage_pass`] of one `encode_frame` shares.
struct StageCtx<'a> {
    tg: FrameTargets<'a>,
    tcur: Option<&'a crate::temporal::Cur>,
    object_bg: &'a wgpu::BindGroup,
    draws: &'a [ItemDraw],
    buckets: &'a [Vec<u32>; 4],
    /// the colour a clearing pass clears to
    clear: wgpu::Color,
    facts: PassFacts,
}

/// ENHANCEMENT ENH-TEMPORAL: the vector / mask targets every pass of a temporal main view also attaches (all `None` in master's
/// frame, which attaches the colour alone).
/// PERF (picture identical): a mask nobody reads this frame (`Cur::tc_read`) is cleared and discarded in every pass: it never
/// leaves tile memory.
fn temporal_attachments(tcur: Option<&crate::temporal::Cur>, clear: bool, store: bool) -> [Option<wgpu::RenderPassColorAttachment<'_>>; 3] {
    let Some(t) = tcur else { return [None, None, None] };
    [(&t.mv, true), (&t.reactive, true), (&t.tc, t.tc_read)].map(|(v, read)| Some(wgpu::RenderPassColorAttachment { view: v, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: if clear || !read { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load }, store: if store && read { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } } }))
}

/// Where a pass over the finished (or still multisampled) float image draws and which depth it reads (E13 / E36 / E44).
/// PERF: without an overlay pass after it the scene was resolved by the passes before, so the pass adds over the resolved
/// image (the multisampled colour is not stored and loaded again for it); and it reads the single-sample depth the
/// soft-particle pass made when there is one (`soft`, kind 2), else the multisampled depth.
fn over_scene<'a>(tg: &FrameTargets<'a>, overlay: bool, soft: Option<&'a wgpu::TextureView>) -> crate::gfx::OverScene<'a> {
    use crate::gfx::OverScene;
    match tg.ms {
        Some(_) if !overlay && soft.is_some() => OverScene { depth: soft.expect("soft"), kind: DepthKind::Soft, colour: tg.view, resolve: None, samples: 1, keep: false },
        Some(m) if !overlay => OverScene { depth: &m.depth, kind: DepthKind::Multi, colour: tg.view, resolve: None, samples: 1, keep: false },
        Some(m) => OverScene { depth: &m.depth, kind: DepthKind::Multi, colour: &m.color, resolve: Some(tg.view), samples: m.samples, keep: overlay },
        None => OverScene { depth: soft.unwrap_or(tg.depth_view), kind: if soft.is_some() { DepthKind::Soft } else { DepthKind::Single }, colour: tg.view, resolve: None, samples: 1, keep: false },
    }
}
