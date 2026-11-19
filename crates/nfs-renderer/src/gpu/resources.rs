//! The renderer's resources: the command list's apply, the mesh arena and its uploads, the textures (plain, BC7, generated
//! mips, deferred level writes), the twins (ENHANCEMENT E41 / E42), release and the counts.

use crate::gfx;
use super::kit;
use super::*;

/// BC7's block: 4 x 4 texels in 16 bytes.
const BC_BLOCK: u32 = 4;
const BC_BLOCK_BYTES: u32 = 16;

impl Core {
    /// Execute a frame's command list (`front::CmdList`) in call order — the start of the frame's
    /// encode: every mutation the scene build made since the last one. A queued vertex / texture write
    /// lands in the next encoder (`flush_mesh_writes` / `flush_texture_writes`), as it did when the
    /// callers wrote to the renderer directly; an offscreen pass encodes and submits here, in its place
    /// in the order, so the writes before it are in its encoder and those after it are not.
    pub(crate) fn apply(&mut self, list: &mut CmdList) {
        let CmdList { cmds, bytes, .. } = list;
        // WIN-STARTUP: a new GPU error while enhancements are on = drop to the original path (queued last: it takes effect this frame)
        let errs = GPU_ERRORS.load(std::sync::atomic::Ordering::Relaxed);
        if errs != self.diag.gpu_errors_seen {
            self.diag.gpu_errors_seen = errs;
            if !self.diag.enh_forced_off {
                // (also when none is on yet: the set of the first frame arrives in this very list)
                self.diag.enh_forced_off = true;
                tracing::warn!(errors = errs, enhancements = %self.enhancements.describe(), "the GPU reported errors: every enhancement is turned off for this run (the original path)");
                cmds.push(Cmd::Enhancements(crate::Enhancements::ORIGINAL));
            }
        }
        for c in cmds.drain(..) {
            // PERF (picture identical): the mirror's and the environment faces' passes stay in the frame's open encoder, to be
            // submitted with the frame; the vertex / texture writes are queued for that encoder too. Any other command may write
            // through the queue (a `Queue::write_*` lands before the whole next submit) or change what the passes recorded so
            // far bind: those passes are submitted first, so every command sees the order it always saw
            if !matches!(c, Cmd::ReflFace(_) | Cmd::Offscreen(_) | Cmd::WriteMesh { .. } | Cmd::UpdateTextureLevel { .. }) {
                self.submit_frame();
            }
            match c {
                Cmd::WriteMesh { mesh, start, len } => self.defer_mesh_write(MeshId(mesh), &bytes[start..start + len]),
                Cmd::UpdateTextureLevel { texture, level, width, height, start, len } => self.queue_texture_level(TextureId(texture), level, width, height, &bytes[start..start + len]),
                Cmd::HeadlightMap(map) => self.set_headlight_map(&map),
                Cmd::Projector(t) => self.set_projector_texture(t),
                Cmd::HeadlightTexture(t) => self.set_headlight_texture(t),
                Cmd::MirrorView(on) => self.set_mirror_view(on),
                Cmd::Trilinear(on) => self.set_trilinear(on),
                Cmd::FakeSort(m) => self.set_fakesort_mode(m),
                Cmd::Gamma(g) => self.set_gamma(g),
                Cmd::TemporalCut => self.temporal_cut(),
                Cmd::PrewarmPipelines => self.prewarm_pipelines(),
                Cmd::ExactPipelines(on) => self.pipes.exact = on,
                Cmd::Enhancements(e) => {
                    // ENHANCEMENT ENH-UPSCALER: the stored set has both backends' levels; the GPU side works with the chosen one's
                    let e = if self.diag.enh_forced_off { crate::Enhancements::ORIGINAL } else { e.effective() };
                    if self.enhancements != e {
                        // ENHANCEMENT ENH-LIVETEX: the resource counts, to see that a flip allocates nothing
                        tracing::info!(enhancements = %e.describe(), textures = self.res.textures.len(), meshes = self.res.meshes.len(), materials = self.res.materials.len(), "enhancements on the GPU side");
                    }
                    let (old_e2, new_e2) = (self.enhancements.level(crate::E2), e.level(crate::E2));
                    let (old_hd, new_hd) = (self.enhancements.level(crate::E41) > 0, e.level(crate::E41) > 0);
                    self.enhancements = e;
                    if old_e2 != new_e2 {
                        self.apply_e2(new_e2);
                    } else if old_hd != new_hd && self.res.tex_twin.iter().any(|t| t.is_some()) {
                        // ENHANCEMENT ENH-LIVETEX: E41 went on / off: the materials sample the HD twins / the originals from this frame
                        self.rebuild_material_bind_groups(None);
                    }
                    self.apply_present_mode();
                    self.apply_edr();
                }
                Cmd::TextureTwin(b, t) => self.set_texture_twin(b, t),
                Cmd::MeshTwin(b, t) => self.set_mesh_twin(b, t),
                Cmd::ReleaseTexture(t) => self.release_texture(t),
                Cmd::ReleaseMesh(m) => self.release_mesh(m),
                Cmd::RecycleReleased => self.recycle_released(),
                Cmd::Resize(w, h) => self.resize(w, h),
                Cmd::LensProbe(on) => self.set_lens_enabled(on),
                Cmd::CreateMaterial(d) => {
                    self.create_material(d);
                }
                Cmd::DuplicateMaterial { id, texture } => {
                    self.duplicate_material_texture(id, texture);
                }
                Cmd::DynamicMesh(m) => {
                    self.upload_dynamic_mesh(&m);
                }
                Cmd::ReflUpload(size, faces) => self.upload_refl_faces(size, &faces),
                Cmd::ReflFace(j) => self.render_refl_face(&j),
                Cmd::Offscreen(j) => {
                    let id = self.render_offscreen(&j.scene, &j.camera, OffscreenSpec { width: j.width, height: j.height, reuse: j.reuse, clear_colour: j.clear, depth_far: j.depth_far });
                    debug_assert_eq!(id, j.id, "offscreen id allocated on the front differs from the core's");
                }
            }
        }
        bytes.clear();
    }

    pub(crate) fn white(&self) -> TextureId {
        self.res.white
    }

    pub(crate) fn texture_count(&self) -> u32 {
        self.res.textures.len() as u32
    }

    /// `(vertex bytes, writes)` per mesh, for the front's mirror.
    pub(crate) fn mesh_infos(&self) -> Vec<(u64, u32)> {
        self.res.meshes.iter().map(|m| (m.vertex_len, m.writes)).collect()
    }

    pub(crate) fn mesh_vertex_len(&self, id: MeshId) -> u64 {
        self.res.meshes[id.0 as usize].vertex_len
    }

    pub(crate) fn material_textures(&self) -> Vec<TextureId> {
        self.res.materials.iter().map(|m| m.texture).collect()
    }

    pub(crate) fn offscreen_sizes(&self) -> std::collections::HashMap<u32, (u32, u32)> {
        self.offscreen.clone()
    }

    /// A mesh whose vertices are rewritten every frame — [`Self::write_mesh_vertices`]. The index
    /// buffer and the index count are fixed at upload; only the vertex data moves.
    pub fn upload_dynamic_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dynamic mesh vb"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dynamic mesh ib"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.res.arena_bytes += vertex_buf.size() + index_buf.size();
        let serial = self.res.buffer_serial;
        self.res.buffer_serial += 1;
        let bounds = {
            let (c, r, _) = csm::mesh_bounds(&mesh.vertices);
            (c, r * DYNAMIC_BOUNDS_SLACK)
        };
        let alpha_one = crate::solid::vertex_bytes_alpha_one(bytemuck::cast_slice(&mesh.vertices));
        self.res.meshes.push(GpuMesh::dynamic(vertex_buf, index_buf, serial, mesh.indices.len() as u32, bounds, alpha_one));
        MeshId(self.res.meshes.len() as u32 - 1)
    }

    /// Queue a vertex write for the next frame's encoder (`flush_mesh_writes`); empty writes are
    /// dropped as `write_buffer` drops them.
    pub(crate) fn defer_mesh_write(&mut self, id: MeshId, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.res.meshes[id.0 as usize].writes = self.res.meshes[id.0 as usize].writes.wrapping_add(1);
        // ENHANCEMENT E1 (FOLIAGE-OVERDRAW): a written vertex below alpha 1 ends the mesh's solid answer
        self.res.meshes[id.0 as usize].alpha_one &= crate::solid::vertex_bytes_alpha_one(bytes);
        self.res.pending_writes.push(PendingWrite { mesh: id.0, start: self.res.pending_bytes.len(), len: bytes.len() });
        self.res.pending_bytes.extend_from_slice(bytes);
    }

    /// Record the queued vertex writes into `encoder`, from the staging belt, in call order.
    pub(super) fn flush_mesh_writes(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for w in &self.res.pending_writes {
            let size = wgpu::BufferSize::new(w.len as u64).expect("non-empty write");
            let m = &self.res.meshes[w.mesh as usize];
            self.belt.write_buffer(encoder, &m.vertex_buf, m.vertex_offset, size).copy_from_slice(&self.res.pending_bytes[w.start..w.start + w.len]);
        }
        self.res.pending_writes.clear();
        self.res.pending_bytes.clear();
    }

    /// The start of a frame, before its commands are applied: the previous frame's last
    /// submission joins the frames in flight, and once more than [`MAX_FRAMES_IN_FLIGHT`] are
    /// queued the oldest is waited for. Only the host's pacing: nothing drawn changes.
    pub(crate) fn frame_begin(&mut self) {
        if let Some(i) = self.last_submit.take() {
            self.frames_in_flight.push_back(i);
        }
        while self.frames_in_flight.len() > MAX_FRAMES_IN_FLIGHT {
            let oldest = self.frames_in_flight.pop_front().expect("non-empty");
            let _ = self.device.poll(wgpu::PollType::Wait { submission_index: Some(oldest), timeout: Some(std::time::Duration::from_secs(1)) });
        }
    }

    /// A frame that draws nothing (the window occluded, the surface lost) still carries out the
    /// buffer and texture writes queued for it, as `write_buffer` / `write_texture` did at once:
    /// left queued, the vertex writes of every skipped frame (a few hundred KB each, thousands of
    /// frames a second while hidden) piled up to gigabytes and were all copied by the next drawn
    /// frame (`RE/notes/PERF.md` PERF2).
    /// The passes the command list recorded for the frame (the mirror, the faces) are submitted with them.
    pub(super) fn flush_writes(&mut self) {
        if self.res.pending_writes.is_empty() && self.res.pending_textures.is_empty() && self.frame_cmds.is_empty() {
            return;
        }
        self.seal_arena();
        let mut encoder = self.frame_cmds.take(&self.device);
        self.flush_texture_writes(&mut encoder);
        self.flush_mesh_writes(&mut encoder);
        self.frame_cmds.park(encoder);
        self.submit_frame();
    }

    /// ENHANCEMENT ENH-LIVELEAK: a static mesh's table slot - a recycled one (a released twin's, see [`Self::recycle_released`]) or a new one.
    pub(super) fn add_mesh(&mut self, m: GpuMesh) -> MeshId {
        if let Some(i) = self.res.free_mesh.pop() {
            self.res.meshes[i as usize] = m;
            return MeshId(i);
        }
        self.res.meshes.push(m);
        MeshId(self.res.meshes.len() as u32 - 1)
    }

    /// ENHANCEMENT ENH-LIVELEAK: a texture's table slot, as [`Self::add_mesh`].
    pub(super) fn add_texture(&mut self, t: GpuTexture) -> TextureId {
        if let Some(i) = self.res.free_tex.pop() {
            self.res.textures[i as usize] = t;
            self.res.released_textures.remove(&i);
            return TextureId(i);
        }
        self.res.textures.push(t);
        TextureId(self.res.textures.len() as u32 - 1)
    }

    /// ENHANCEMENT ENH-LIVELEAK: the texture / mesh slots released so far may be handed out again. The app calls it when a live pass is
    /// complete: until then a model that has not adopted its new twins yet may still release an id the first one already released, and
    /// that must not hit a new owner of the slot.
    pub(crate) fn recycle_released(&mut self) {
        self.res.free_tex.append(&mut self.res.pending_tex);
        self.res.free_mesh.append(&mut self.res.pending_mesh);
    }

    pub fn upload_mesh(&mut self, mesh: &MeshData) -> MeshId {
        // a queued write below lands before the next submit: the passes recorded so far (which may draw a released mesh's
        // ranges this one reuses) go first
        self.submit_frame();
        let vbytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let ibytes: &[u8] = bytemuck::cast_slice(&mesh.indices);
        let (vlen, ilen) = (vbytes.len() as u64, ibytes.len() as u64);
        // every range is rounded to 8 bytes (a mapped range starts on a multiple of 8, a slice on 4;
        // and an empty one still reserves 8, so it can be sliced), a vertex range to whole vertices
        // too (48 bytes, a multiple of 8), so each mesh starts on a vertex: its `base_vertex`
        let (vreserve, ireserve) = (vlen.next_multiple_of(VERTEX_STRIDE).max(VERTEX_STRIDE), ilen.next_multiple_of(8).max(8));
        // ENHANCEMENT ENH-LIVELEAK: the ranges of a released mesh that this one fits (the smallest such slot) are used again, so a twin
        // replaced by the next level does not grow the arena. A queued write, not a mapped copy: the arena is sealed first.
        if let Some(k) = (0..self.res.free_slots.len()).filter(|&k| self.res.free_slots[k].vertex_cap >= vreserve && self.res.free_slots[k].index_cap >= ireserve).min_by_key(|&k| self.res.free_slots[k].vertex_cap + self.res.free_slots[k].index_cap) {
            self.seal_arena();
            let f = self.res.free_slots.swap_remove(k);
            if !vbytes.is_empty() {
                self.queue.write_buffer(&f.vertex_buf, f.vertex_offset, vbytes);
            }
            if !ibytes.is_empty() {
                self.queue.write_buffer(&f.index_buf, f.index_offset, ibytes);
            }
            let (c, r, shape) = csm::mesh_bounds(&mesh.vertices);
            let (vertex, index) = (BufRange { buf: f.vertex_buf, offset: f.vertex_offset, len: vlen, cap: f.vertex_cap }, BufRange { buf: f.index_buf, offset: f.index_offset, len: ilen, cap: f.index_cap });
            return self.add_mesh(GpuMesh::in_arena(vertex, index, f.serial, mesh.indices.len() as u32, (c, r), shape, crate::solid::vertex_bytes_alpha_one(vbytes)));
        }
        let fits = self.res.arena.as_ref().is_some_and(|a| a.vertex_used + vreserve <= a.vertex_buf.size() && a.index_used + ireserve <= a.index_buf.size());
        if !fits {
            // a full arena goes to the queue now, so the host copies of a whole track's meshes do not pile up
            if self.res.arena.as_ref().is_some_and(|a| a.mapped) {
                self.seal_arena();
                self.queue.submit(std::iter::empty());
            }
            let make = |label: &'static str, usage: wgpu::BufferUsages, need: u64| {
                self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: ARENA_CHUNK.max(need), usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true })
            };
            self.res.arena_bytes += 2 * ARENA_CHUNK.max(vreserve.max(ireserve));
            let serial = self.res.buffer_serial;
            self.res.buffer_serial += 1;
            self.res.arena = Some(MeshArena {
                serial,
                vertex_buf: make("mesh arena vb", wgpu::BufferUsages::VERTEX, vreserve),
                vertex_used: 0,
                index_buf: make("mesh arena ib", wgpu::BufferUsages::INDEX, ireserve),
                index_used: 0,
                mapped: true,
            });
        }
        let a = self.res.arena.as_mut().expect("arena made");
        let (vertex_offset, index_offset) = (a.vertex_used, a.index_used);
        if a.mapped {
            if !vbytes.is_empty() {
                a.vertex_buf.slice(vertex_offset..vertex_offset + vlen).get_mapped_range_mut().expect("mapped arena").copy_from_slice(vbytes);
            }
            if !ibytes.is_empty() {
                a.index_buf.slice(index_offset..index_offset + ilen).get_mapped_range_mut().expect("mapped arena").copy_from_slice(ibytes);
            }
        } else {
            if !vbytes.is_empty() {
                self.queue.write_buffer(&a.vertex_buf, vertex_offset, vbytes);
            }
            if !ibytes.is_empty() {
                self.queue.write_buffer(&a.index_buf, index_offset, ibytes);
            }
        }
        a.vertex_used += vreserve;
        a.index_used += ireserve;
        let (centre, radius, shape) = csm::mesh_bounds(&mesh.vertices);
        let (vertex, index) = (BufRange { buf: a.vertex_buf.clone(), offset: vertex_offset, len: vlen, cap: vreserve }, BufRange { buf: a.index_buf.clone(), offset: index_offset, len: ilen, cap: ireserve });
        let g = GpuMesh::in_arena(vertex, index, a.serial, mesh.indices.len() as u32, (centre, radius), shape, crate::solid::vertex_bytes_alpha_one(vbytes));
        self.add_mesh(g)
    }

    /// Unmap the mesh arena so the GPU can read it: what [`Self::upload_mesh`] copied into the mapped
    /// buffers becomes visible to the next frame. A later mesh that still fits goes in as a queued write.
    pub(super) fn seal_arena(&mut self) {
        if let Some(a) = self.res.arena.as_mut().filter(|a| a.mapped) {
            a.vertex_buf.unmap();
            a.index_buf.unmap();
            a.mapped = false;
        }
    }

    pub fn upload_texture(&mut self, tex: &TextureData) -> TextureId {
        self.upload_texture_mips(tex.width, tex.height, &tex.mips)
    }

    /// [`Self::upload_texture`] over borrowed levels, so a caller that keeps its pixels does not
    /// clone a whole mip chain just to hand it over.
    pub fn upload_texture_mips(&mut self, width: u32, height: u32, mips: &[Vec<u8>]) -> TextureId {
        let t = self.tex_maker().rgba(width, height, mips, None);
        self.add_texture(t)
    }

    /// Several textures at once, the ids in the order given (the next free ones, as the same
    /// number of [`Self::upload_texture_mips`] calls would hand them). The device and the queue
    /// are thread-safe, so the textures are built on worker threads and pushed in order.
    pub fn upload_textures(&mut self, items: &[(u32, u32, &[Vec<u8>])]) -> Vec<TextureId> {
        let items: Vec<_> = items.iter().map(|&(w, h, m)| (w, h, m, None)).collect();
        self.upload_textures_gen(&items)
    }

    /// [`Self::upload_textures`] where an item may name a `MipAlpha`: a texture the data ships
    /// with ONE level then gets a generated chain (ENHANCEMENT E2, the track's single-level
    /// textures; sampled at level 0 only while E2 is off). Items with levels of their own keep them.
    pub fn upload_textures_gen(&mut self, items: &[(u32, u32, &[Vec<u8>], Option<crate::mipgen::MipAlpha>)]) -> Vec<TextureId> {
        let mk = self.tex_maker();
        let built = kit::par_build(items, |&(w, h, m, g)| mk.rgba(w, h, m, g));
        built.into_iter().map(|t| self.add_texture(t)).collect()
    }

    /// ENHANCEMENT E2: a texture of one level, uploaded with a generated chain
    /// (`mipgen::chain`) beneath it — for the cars' pages and the sprites, which the original has
    /// no mips for. Sampled at level 0 only while E2 is off.
    pub fn upload_texture_generated(&mut self, width: u32, height: u32, rgba: &[u8], alpha: crate::mipgen::MipAlpha) -> TextureId {
        let one = vec![rgba.to_vec()];
        let t = self.tex_maker().rgba(width, height, &one, Some(alpha));
        self.add_texture(t)
    }

    /// ENHANCEMENT E41: whether the device takes BC (block-compressed) textures: the HD pack's BC7 files are then uploaded as they are.
    pub fn supports_bc7(&self) -> bool {
        self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    /// ENHANCEMENT E41: several BC7 textures (`width`, `height` of the base level, a multiple of 4 each; the blocks of every level,
    /// level 0 first, as `nfs_assets::ktx2` holds them), the ids in the order given. Built on worker threads like
    /// [`Self::upload_textures_gen`]. Sampled exactly like an `Rgba8Unorm` texture of the same size (UNORM, not sRGB: the game's surface
    /// is a byte surface). `generated` is `None`: the chain is the file's own.
    pub fn upload_textures_bc7(&mut self, items: &[(u32, u32, &[Vec<u8>])]) -> Vec<TextureId> {
        let mk = self.tex_maker();
        let built = kit::par_build(items, |&(w, h, m)| mk.bc7(w, h, m));
        built.into_iter().map(|t| self.add_texture(t)).collect()
    }

    /// What a texture is made with (every part `Sync`: the textures of a batch are made on worker threads).
    fn tex_maker(&self) -> TexMaker<'_> {
        TexMaker { device: &self.device, queue: &self.queue, sprite_layout: &self.layouts.sprite, sprite_sampler: &self.samplers.sprite }
    }
    /// One mip level of `Renderer::update_texture`: `data` holds `h` rows of `4 · w` bytes (or more; the
    /// rest is ignored; less and the level is skipped).
    pub(crate) fn queue_texture_level(&mut self, id: TextureId, level: u32, w: u32, h: u32, data: &[u8]) {
        let Some(gpu) = self.res.textures.get_mut(id.0 as usize) else {
            return;
        };
        // ENHANCEMENT E1 (FOLIAGE-OVERDRAW): a written level's alpha lowers the texture's minimum (never raises it)
        gpu.min_alpha = gpu.min_alpha.min(crate::solid::rgba_min_alpha(&data[..data.len().min(4 * w as usize * h as usize)]));
        let gpu = &self.res.textures[id.0 as usize];
        // ENHANCEMENT E2: a level-0 rewrite of a texture with a generated chain (the car page's damage
        // decals) regenerates the levels below it, queued the same way
        if level == 0 && let Some(mode) = gpu.generated && gpu.size == (w, h) && data.len() >= (4 * w * h) as usize {
            let (mut lw, mut lh) = (w, h);
            for (i, lv) in crate::mipgen::chain(&data[..(4 * w * h) as usize], w, h, mode).iter().enumerate() {
                lw = (lw / 2).max(1);
                lh = (lh / 2).max(1);
                self.queue_texture_level(id, i as u32 + 1, lw, lh, lv);
            }
        }
        // queued for the next frame's encoder (`flush_texture_writes`), where `write_texture` would
        // have executed it (before that submit's commands); the rows are padded to 256 bytes
        let row = 4 * w as usize;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        if data.len() >= row * h as usize {
            // a queued write of the same whole level is overwritten in place (the last wins, as the
            // later `write_texture` would), so frames that are not rendered do not pile writes up
            let same = self.res.pending_textures.iter().find(|q| q.texture == id.0 && q.level == level && q.width == w && q.height == h).map(|q| q.start);
            let start = same.unwrap_or(self.res.pending_texture_bytes.len());
            if same.is_none() {
                self.res.pending_texture_bytes.resize(start + padded * h as usize, 0);
                self.res.pending_textures.push(PendingTextureWrite { texture: id.0, level, width: w, height: h, bytes_per_row: padded as u32, start });
            }
            for y in 0..h as usize {
                self.res.pending_texture_bytes[start + y * padded..start + y * padded + row].copy_from_slice(&data[y * row..(y + 1) * row]);
            }
        }
    }

    /// Record the queued texture writes into `encoder`, from the staging belt, in call order.
    pub(super) fn flush_texture_writes(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for w in &self.res.pending_textures {
            let Some(gpu) = self.res.textures.get(w.texture as usize) else { continue };
            let len = w.bytes_per_row as usize * w.height as usize;
            let size = wgpu::BufferSize::new(len as u64).expect("non-empty write");
            let slice = self.belt.allocate(size, wgpu::BufferSize::new(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64).unwrap());
            slice.get_mapped_range_mut().expect("belt slice").copy_from_slice(&self.res.pending_texture_bytes[w.start..w.start + len]);
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo { buffer: slice.buffer(), layout: wgpu::TexelCopyBufferLayout { offset: slice.offset(), bytes_per_row: Some(w.bytes_per_row), rows_per_image: Some(w.height) } },
                kit::level_copy(&gpu._texture, w.level),
                kit::extent(w.width, w.height),
            );
        }
        self.res.pending_textures.clear();
        self.res.pending_texture_bytes.clear();
    }

    /// ENHANCEMENT ENH-LIVETEX: the texture a material shows for `t` - `t`'s HD twin while E41 is above 0 and it has one, else `t`.
    pub(super) fn shown_texture(&self, t: TextureId) -> TextureId {
        if self.enhancements.level(crate::E41) > 0
            && let Some(Some(w)) = self.res.tex_twin.get(t.0 as usize)
        {
            return *w;
        }
        t
    }

    /// ENHANCEMENT ENH-LIVETEX: register (or clear, `None`) `base`'s HD twin; the materials of `base` sample it at once when E41 is on.
    pub(crate) fn set_texture_twin(&mut self, base: TextureId, twin: Option<TextureId>) {
        let i = base.0 as usize;
        if self.res.tex_twin.len() <= i {
            self.res.tex_twin.resize(i + 1, None);
        }
        if self.res.tex_twin[i] == twin {
            return;
        }
        self.res.tex_twin[i] = twin;
        if self.enhancements.level(crate::E41) > 0 {
            self.rebuild_material_bind_groups(Some(base));
        }
    }

    /// ENHANCEMENT ENH-LIVETEX: register (or clear) `base`'s refined twin mesh, drawn while E42 is above 0.
    pub(crate) fn set_mesh_twin(&mut self, base: MeshId, twin: Option<MeshId>) {
        let i = base.0 as usize;
        if self.res.mesh_twin.len() <= i {
            self.res.mesh_twin.resize(i + 1, None);
        }
        self.res.mesh_twin[i] = twin;
    }

    /// ENHANCEMENT ENH-LIVETEX: give a texture's memory back (a replaced HD twin): its slot shares one 1 x 1 white image, the id stays valid.
    /// ENHANCEMENT ENH-LIVELEAK: the stub is one shared image (no allocation per release), the writes still queued for the texture are
    /// dropped, and a second release of the same id does nothing.
    pub(crate) fn release_texture(&mut self, id: TextureId) {
        if id == self.res.white || id.0 as usize >= self.res.textures.len() || self.offscreen.contains_key(&id.0) || !self.res.released_textures.insert(id.0) {
            return;
        }
        if self.res.stub_texture.is_none() {
            self.res.stub_texture = Some(self.tex_maker().rgba(1, 1, &[vec![255; 4]], None));
        }
        self.res.pending_textures.retain(|w| w.texture != id.0);
        self.res.textures[id.0 as usize] = self.res.stub_texture.clone().expect("stub made");
        self.res.pending_tex.push(id.0);
    }

    /// ENHANCEMENT ENH-LIVELEAK: give a mesh's memory back (a replaced refined twin): a static mesh's arena ranges go to the free list (the
    /// next upload that fits writes over them), a dynamic mesh's buffers are dropped; the id stays valid (an empty mesh). Never for a mesh
    /// still drawn: the caller clears the twin entry first. A second release of the same id does nothing.
    pub(crate) fn release_mesh(&mut self, id: MeshId) {
        let i = id.0 as usize;
        if i >= self.res.meshes.len() || self.res.meshes[i].released {
            return;
        }
        self.res.pending_writes.retain(|w| w.mesh != id.0);
        if self.res.mesh_stub.is_none() {
            let make = |usage: wgpu::BufferUsages| self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("released mesh"), size: 8, usage, mapped_at_creation: false });
            let (vb, ib) = (make(wgpu::BufferUsages::VERTEX), make(wgpu::BufferUsages::INDEX));
            self.res.mesh_stub = Some((vb, ib, self.res.buffer_serial));
            self.res.buffer_serial += 1;
        }
        let (svb, sib, stub_serial) = self.res.mesh_stub.clone().expect("stub made");
        self.res.pending_mesh.push(id.0);
        let m = &mut self.res.meshes[i];
        let old = std::mem::replace(m, GpuMesh::stub(svb, sib, stub_serial));
        if old.arena {
            self.res.free_slots.push(FreeMeshSlot { serial: old.vb_serial, vertex_buf: old.vertex_buf, vertex_offset: old.vertex_offset, vertex_cap: old.cap.0, index_buf: old.index_buf, index_offset: old.index_offset, index_cap: old.cap.1 });
        } else {
            self.res.arena_bytes = self.res.arena_bytes.saturating_sub(old.cap.0 + old.cap.1);
        }
    }

    /// ENHANCEMENT ENH-LIVELEAK: what is resident now (see [`ResourceStats`]).
    pub(crate) fn resource_stats(&self) -> ResourceStats {
        let mut st = ResourceStats { textures: self.res.textures.len(), meshes: self.res.meshes.len(), materials: self.res.materials.len(), arena_bytes: self.res.arena_bytes, ..Default::default() };
        for (i, t) in self.res.textures.iter().enumerate() {
            if self.res.released_textures.contains(&(i as u32)) {
                continue;
            }
            st.textures_live += 1;
            let tex = &t._texture;
            let f = tex.format();
            let (bw, bh) = f.block_dimensions();
            let bytes = f.block_copy_size(None).unwrap_or(4) as u64;
            let (mut w, mut h) = (tex.width() as u64, tex.height() as u64);
            for _ in 0..tex.mip_level_count() {
                st.texture_bytes += w.div_ceil(bw as u64) * h.div_ceil(bh as u64) * bytes;
                w = (w / 2).max(1);
                h = (h / 2).max(1);
            }
        }
        for m in self.res.meshes.iter().filter(|m| !m.released) {
            st.meshes_live += 1;
            st.mesh_bytes += m.vertex_len + m.index_len;
        }
        st
    }

    /// ENHANCEMENT ENH-LIVETEX: the mesh a draw of `id` uses - its refined twin while E42 is above 0.
    #[inline]
    pub(super) fn shown_mesh(&self, id: u32) -> u32 {
        if !self.res.mesh_twin.is_empty()
            && self.enhancements.level(crate::E42) > 0
            && let Some(Some(t)) = self.res.mesh_twin.get(id as usize)
        {
            return t.0;
        }
        id
    }
}

/// The device, queue, sprite layout and sampler a texture is made with ([`Core::tex_maker`]).
#[derive(Clone, Copy)]
struct TexMaker<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    sprite_layout: &'a wgpu::BindGroupLayout,
    sprite_sampler: &'a wgpu::Sampler,
}

impl TexMaker<'_> {
    /// An 8-bit RGBA texture of `mips` (level 0 first).
    fn rgba(self, width: u32, height: u32, mips: &[Vec<u8>], generate: Option<crate::mipgen::MipAlpha>) -> GpuTexture {
        let TexMaker { device, queue, sprite_layout, sprite_sampler } = self;
        let size = kit::extent(width, height);
        // ENHANCEMENT E2: one level in, `generate` set (and `width x height` really that long):
        // the chain follows; nothing else enters here with `generate` None
        let chained: Vec<Vec<u8>>;
        let (mips, generated) = match generate {
            Some(mode) if mips.len() == 1 && width.max(height) > 1 && mips[0].len() >= (4 * width * height) as usize => {
                chained = std::iter::once(mips[0].clone()).chain(crate::mipgen::chain(&mips[0], width, height, mode)).collect();
                (chained.as_slice(), Some(mode))
            }
            _ => (mips, None),
        };
        let mip_level_count = mips.len().max(1) as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        kit::write_levels(queue, &texture, width, height, mips, |w, h| (kit::extent(w, h), 4 * w, h));
        let view = texture.create_view(&Default::default());
        let sprite_bg = Some(gfx::tex_sampler_bg(device, "sprite tex", sprite_layout, &view, sprite_sampler));
        tracing::debug!(w = width, h = height, "texture uploaded");
        GpuTexture { _texture: texture, view, sprite_bg, generated, size: (width, height), min_alpha: crate::solid::rgba_levels_min_alpha(mips) }
    }

    /// A BC7 texture of `levels` (`Core::upload_textures_bc7`).
    fn bc7(self, width: u32, height: u32, levels: &[Vec<u8>]) -> GpuTexture {
        let TexMaker { device, queue, sprite_layout, sprite_sampler } = self;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture bc7"),
            size: kit::extent(width, height),
            mip_level_count: levels.len().max(1) as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc7RgbaUnorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // a level's copy covers its PHYSICAL size (whole 4x4 blocks of 16 bytes), also for the 2x2 and 1x1 levels at the end of the chain
        kit::write_levels(queue, &texture, width, height, levels, |w, h| {
            let (pw, ph) = (w.next_multiple_of(BC_BLOCK), h.next_multiple_of(BC_BLOCK));
            (kit::extent(pw, ph), pw / BC_BLOCK * BC_BLOCK_BYTES, ph / BC_BLOCK)
        });
        let view = texture.create_view(&Default::default());
        let sprite_bg = Some(gfx::tex_sampler_bg(device, "sprite tex bc7", sprite_layout, &view, sprite_sampler));
        GpuTexture { _texture: texture, view, sprite_bg, generated: None, size: (width, height), min_alpha: crate::solid::bc7_levels_min_alpha(levels) }
    }
}
