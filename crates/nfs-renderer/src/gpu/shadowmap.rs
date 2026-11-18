//! The pieces every depth-only shadow map shares (SHADOW-REFACTOR): ENHANCEMENT E9's sun / moon cascades (`csm.rs`), E15's
//! headlight map (`spot.rs`) and E47's lamp maps (`lamps.rs`).
//!
//! * [`UniformSlot`]: a light's uniform, zeroed at creation (the shader's "off": every light's count / switch field is 0) and
//!   written back to zero ONCE when the light goes out, so a frame without the light uploads nothing;
//! * [`DepthArray`]: a `Depth32Float` array texture, its array view (the main shader's binding) and a view per layer (a pass's
//!   attachment), and the 1 x 1 placeholder bound until a light first needs its map;
//! * [`LightMats`]: one light matrix per map layer for the depth-only pipelines' group 0 (`shadow.wgsl` `light_vp`);
//! * [`Caster`] / [`Core::frame_casters`]: the frame's casting items with their world bounding spheres, collected ONCE a frame
//!   and filtered by each map (before: three walks over the items, each testing `csm_caster` and transforming the bounds);
//! * [`Core::draw_shadow_casters`] and [`depth_pass`]: one depth-only pass over a list of casters with E9's two pipelines.

use super::*;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use crate::gfx;
use std::marker::PhantomData;

/// A light matrix's size in a [`LightMats`] buffer (one `mat4x4<f32>`).
const LIGHT_MAT_BYTES: u64 = std::mem::size_of::<Mat4>() as u64;

/// A light's uniform buffer of `T` and whether it holds a frame's values (see the module doc).
pub(super) struct UniformSlot<T> {
    pub buf: wgpu::Buffer,
    active: bool,
    _t: PhantomData<T>,
}

impl<T: Pod + Zeroable> UniformSlot<T> {
    pub(super) fn new(device: &wgpu::Device, label: &str) -> Self {
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: Self::SIZE.get(), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: the shader's "off"
        buf.unmap();
        Self { buf, active: false, _t: PhantomData }
    }

    const SIZE: wgpu::BufferSize = match wgpu::BufferSize::new(std::mem::size_of::<T>() as u64) {
        Some(s) => s,
        None => panic!("an empty uniform"),
    };

    /// This frame's values.
    pub(super) fn write(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, value: &T) {
        self.active = true;
        belt.write_buffer(encoder, &self.buf, 0, Self::SIZE).copy_from_slice(bytemuck::bytes_of(value));
    }

    /// This frame's values but the bytes in `skip` (a range no shader reads this frame: they keep what they held). Offsets on
    /// 4-byte steps (a buffer copy's).
    pub(super) fn write_except(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, value: &T, skip: std::ops::Range<usize>) {
        let bytes = bytemuck::bytes_of(value);
        if skip.is_empty() {
            return self.write(belt, encoder, value);
        }
        self.active = true;
        for (at, part) in [(0, &bytes[..skip.start]), (skip.end, &bytes[skip.end..])] {
            if let Some(size) = wgpu::BufferSize::new(part.len() as u64) {
                belt.write_buffer(encoder, &self.buf, at as u64, size).copy_from_slice(part);
            }
        }
    }

    /// The light is out this frame: zero the uniform if the last frame had it on.
    pub(super) fn clear(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        if self.active {
            self.write(belt, encoder, &T::zeroed());
            self.active = false;
        }
    }
}

/// A shadow map array (see the module doc).
pub(super) struct DepthArray {
    pub size: u32,
    pub layers: u32,
    _texture: wgpu::Texture,
    pub array_view: wgpu::TextureView,
    pub layer_views: Vec<wgpu::TextureView>,
    /// per layer: its last pass drew nothing (the layer holds the clear's 1.0 everywhere); false for a new texture (zeroed)
    cleared: Vec<bool>,
}

impl DepthArray {
    /// Whether layer `layer`'s pass must run this frame, `empty` = it has no caster: not when the layer is already all cleared
    /// (a pass would write the same 1.0 everywhere). Notes the layer's state after the pass.
    pub(super) fn needs_pass(&mut self, layer: usize, empty: bool) -> bool {
        let run = !(empty && self.cleared[layer]);
        self.cleared[layer] = empty;
        run
    }

    pub(super) fn new(device: &wgpu::Device, label: &str, size: u32, layers: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: gfx::RT_USAGE,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layer_views = (0..layers)
            .map(|i| texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() }))
            .collect();
        tracing::info!(label, size, layers, "shadow map");
        Self { size, layers, _texture: texture, array_view, layer_views, cleared: vec![false; layers as usize] }
    }

    /// The 1 x 1 placeholder's texture and array view, bound until the light's map exists.
    pub(super) fn placeholder(device: &wgpu::Device, label: &str) -> wgpu::TextureView {
        Self::placeholder_texture(device, label).create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
    }

    /// The same for a plain 2-D depth binding (E15's `t_spot`, a one-layer map read through its layer view).
    pub(super) fn placeholder_2d(device: &wgpu::Device, label: &str) -> wgpu::TextureView {
        Self::placeholder_texture(device, label).create_view(&Default::default())
    }

    /// (the view keeps its texture alive)
    fn placeholder_texture(device: &wgpu::Device, label: &str) -> wgpu::Texture {
        gfx::texture_2d(device, label, 1, 1, DEPTH_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING)
    }
}

/// A shadow map array's binding in the globals group (fragment stage, a depth `texture_depth_2d_array`).
pub(super) fn array_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
        count: None,
    }
}

/// One light matrix per layer of a map, each in its own bind group of the depth-only pipelines' group 0.
pub(super) struct LightMats {
    bufs: Vec<wgpu::Buffer>,
    bgs: Vec<wgpu::BindGroup>,
}

impl LightMats {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, label: &str, n: usize) -> Self {
        let bufs: Vec<wgpu::Buffer> = (0..n).map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: LIGHT_MAT_BYTES, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })).collect();
        let bgs = bufs.iter().map(|b| gfx::buffer_bg(device, label, layout, b)).collect();
        Self { bufs, bgs }
    }

    /// Layer `k`'s matrix for this frame.
    pub(super) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, k: usize, m: &[[f32; 4]; 4]) {
        let size = wgpu::BufferSize::new(LIGHT_MAT_BYTES).expect("light size");
        belt.write_buffer(encoder, &self.bufs[k], 0, size).copy_from_slice(bytemuck::bytes_of(m));
    }

    pub(super) fn bind_group(&self, k: usize) -> &wgpu::BindGroup {
        &self.bgs[k]
    }
}

/// A depth-only pass clearing `view` (one layer of a shadow map), timed as `prof` by the bench.
pub(super) fn depth_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, prof: &'static str, view: &wgpu::TextureView) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: crate::prof::rp(prof),
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// One item of the frame that casts into the shadow maps ([`Core::frame_casters`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct Caster {
    /// the item's index in the frame's items (the object buffer's instance)
    pub idx: u32,
    /// its world bounding sphere
    pub centre: Vec3,
    pub radius: f32,
    /// a car part (its light has a direction): the cars-only layers' (E51) and the lamp maps' (E47) only casters
    pub car: bool,
    /// its mesh's shape (`GpuMesh::shape`): a flat level sheet (a road layer, a marking) E9 and E15 leave out, E47 keeps; E9's
    /// shape rule (SHADOW-UNIFY, `csm::part_casts`) leaves out a broad terrain-like part
    pub shape: super::csm::MeshShape,
}

impl Core {
    /// The frame's casters among its first `n` items into `out` (the frame's kept list, empty): [`Core::csm_caster`]s with a mesh
    /// that has triangles and a bounding sphere.
    pub(super) fn frame_casters(&self, scene: &Scene, n: usize, out: &mut Vec<Caster>) {
        for (i, item) in scene.items.iter().take(n).enumerate() {
            if !self.csm_caster(item) {
                continue;
            }
            let mesh = &self.meshes[item.mesh.0 as usize];
            if mesh.index_count == 0 || mesh.bounds.1 <= 0.0 {
                continue;
            }
            let t = &item.transform;
            let scale = t.x_axis.truncate().length().max(t.y_axis.truncate().length()).max(t.z_axis.truncate().length());
            out.push(Caster { idx: i as u32, centre: t.transform_point3(Vec3::from(mesh.bounds.0)), radius: mesh.bounds.1 * scale, car: item.car_lit(), shape: mesh.shape });
        }
    }

    /// Draw `casters` (indices into `scene.items`) into a depth-only shadow pass whose light group (0) is set: E9's opaque /
    /// cut-out pipelines, the object buffer as group 1 (E9's cascades, E15's headlight map and E47's lamp maps).
    pub(super) fn draw_shadow_casters(&self, pass: &mut wgpu::RenderPass<'_>, scene: &Scene, casters: &[u32]) {
        pass.set_bind_group(1, &self.object_bg, &[]);
        let (mut cur_pipe, mut cur_mat, mut cur_mesh) = (None, u32::MAX, u32::MAX);
        let mut material_bound = false;
        for &idx in casters {
            let item = &scene.items[idx as usize];
            let mat = &self.materials[item.material.0 as usize];
            // the alpha test: the cut-out pipeline and the material's texture
            // ENHANCEMENT E9 / E15 / E47 (FOLIAGE-OVERDRAW): not for a draw whose test can never fail (`Core::solid_draws`): `fs_cut`
            // would keep every fragment, so the depth is the opaque pipeline's, which the GPU draws without running a fragment stage
            let cut = (mat.alpha_blend || mat.device_states || item.alpha_test) && !self.solid.get(idx as usize).copied().unwrap_or(false);
            if cur_pipe != Some(cut) {
                pass.set_pipeline(if cut { &self.csm.pipe_cut } else { &self.csm.pipe_opaque });
                cur_pipe = Some(cut);
                cur_mat = u32::MAX;
            }
            // (the opaque pipeline's layout has the material group too, unread: a pass whose first caster is opaque binds that
            // caster's, FOLIAGE-OVERDRAW: a solid cut-out can come first)
            if (cut && cur_mat != item.material.0) || !material_bound {
                pass.set_bind_group(2, &mat.bind_group, &[]);
                cur_mat = item.material.0;
                material_bound = true;
            }
            let mesh = &self.meshes[item.mesh.0 as usize];
            if cur_mesh != item.mesh.0 {
                pass.set_vertex_buffer(0, mesh.vertex_buf.slice(mesh.vertex_offset..mesh.vertex_offset + mesh.vertex_len.max(4)));
                pass.set_index_buffer(mesh.index_buf.slice(mesh.index_offset..mesh.index_offset + mesh.index_len.max(4)), wgpu::IndexFormat::Uint32);
                cur_mesh = item.mesh.0;
            }
            crate::prof::count_draw(u64::from(mesh.index_count / 3));
            pass.draw_indexed(0..mesh.index_count, 0, idx..idx + 1);
        }
    }
}
