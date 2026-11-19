//! The full-screen pass that looks each glass pixel up in the drops' grid ([`DropsPass`]).

use super::*;

use super::glass::*;
use super::sim::*;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct DropsUniform {
    /// NDC -> (u w, v w, w), three columns (the 4th lane unused)
    pub(super) h0: [f32; 4],
    pub(super) h1: [f32; 4],
    pub(super) h2: [f32; 4],
    /// 1 / distance = a x + b y + c, the glass's width (m) in the 4th
    pub(super) inv: [f32; 4],
    /// image width, height, glass height (m), strength
    pub(super) a: [f32; 4],
    /// `far / (near - far)`, near, half the image height times the projection's `1 / tan(fov / 2)`, refraction reach
    pub(super) b: [f32; 4],
    /// the sky's colour (the scene's clear colour): what the drops' highlights reflect
    pub(super) sky: [f32; 4],
    /// the colour source's origin in image pixels and its size (the scratch copy of the glass's box, or the whole image)
    pub(super) bx: [f32; 4],
    /// the part of it that holds the scene: width, height
    pub(super) bz: [f32; 4],
}

/// The two ways the pass runs. `Full`: a new image the size of the frame (the scene was already put in an image of its own
/// by an earlier pass: the blur, the depth of field, FSR). `InPlace`: the scene's own image, a scratch copy of the glass's
/// box as the source, a draw over that box only (a pixel without a drop is discarded = keeps the scene).
pub(super) const MODE_FULL: &str = r#"
fn keep(p: vec2<f32>) -> vec4<f32> { return base(p); }
"#;
pub(super) const MODE_IN_PLACE: &str = r#"
fn keep(p: vec2<f32>) -> vec4<f32> { discard; }
"#;

pub(super) const DROPS_WGSL: &str = include_str!("drops.wgsl");

/// The frame's numbers the pass needs besides the glass.
#[derive(Clone, Copy)]
pub(crate) struct Look {
    pub strength: f32,
    /// the sky's colour (the scene's clear colour)
    pub sky: [f32; 3],
    /// the frame's size in pixels
    pub width: u32,
    pub height: u32,
}

/// The pass's shader over a plain or a `multisampled` depth, the full image or `in_place` (the box over the scene).
pub(crate) fn source(multisampled: bool, in_place: bool) -> String {
    let sizes = format!("const GRID: vec2<i32> = vec2<i32>({GRID_COLS}, {GRID_ROWS});\nconst SLOTS: i32 = {CELL_SLOTS};\nconst NONE: u32 = {NO_INSTANCE:#X}u;\nconst INST_W: i32 = {INST_W};");
    let body = DROPS_WGSL.replace("//MODE", if in_place { MODE_IN_PLACE } else { MODE_FULL }).replace("//SIZES", &sizes);
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::dtex(multisampled), wgsl::IGN, wgsl::DLOAD, wgsl::ANGLES, wgsl::LUMA_709, wgsl::PROJ_DIST, wgsl::NDC_OF_PX, &body])
}

pub(crate) struct DropsPass {
    /// the pipeline per `(in place, multisampled depth)`, built at its first use
    pub(super) pipes: Variants<(bool, bool), wgpu::RenderPipeline>,
    pub(super) image_layout: wgpu::BindGroupLayout,
    pub(super) depth: DepthBinding,
    pub(super) u_layout: wgpu::BindGroupLayout,
    pub(super) u_buf: UniformBuf<DropsUniform>,
    pub(super) u_bg: wgpu::BindGroup,
    pub(super) inst_tex: wgpu::Texture,
    pub(super) grid_tex: wgpu::Texture,
    /// `InPlace`: the scratch copy of the glass's box (size, texture, bind group), grown when the box outgrows it
    pub(super) scratch: Option<(u32, u32, wgpu::Texture, wgpu::BindGroup)>,
    /// the water, kept while the view shows the windscreen
    pub sim: Option<Sim>,
    pub(super) inst: Vec<[f32; 4]>,
    pub(super) grid: Vec<u32>,
    /// what `inst` / `grid` were built from (the water's step count, the glass's size) and their count; `None` = build again
    pub(super) built: Option<((u64, [u32; 2]), usize)>,
    /// `inst` / `grid` were rebuilt and are not on the GPU yet
    pub(super) upload_due: bool,
}

impl DropsPass {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let u_layout = gfx::bgl(
            device,
            "drops",
            &[gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT), gfx::bgl_float(1, false), gfx::bgl_tex(2, wgpu::TextureSampleType::Uint, false)],
        );
        let u_buf = UniformBuf::<DropsUniform>::new(device, "drops");
        let data_tex = |label: &'static str, w: u32, h: u32, format: wgpu::TextureFormat| {
            gfx::texture_2d(device, label, w, h, format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST)
        };
        let inst_tex = data_tex("drops instances", INST_W, INST_H, wgpu::TextureFormat::Rgba32Float);
        let grid_tex = data_tex("drops grid", (GRID_COLS * CELL_SLOTS) as u32, GRID_ROWS as u32, wgpu::TextureFormat::R32Uint);
        let (iv, gv) = (inst_tex.create_view(&Default::default()), grid_tex.create_view(&Default::default()));
        let u_bg = gfx::bind_group(device, "drops", &u_layout, &[
                gfx::bg_buf(0, &u_buf.buf),
                gfx::bg_view(1, &iv),
                gfx::bg_view(2, &gv),
            ]);
        let depth = DepthBinding::new(device, "drops depth");
        Self { pipes: Variants::default(), image_layout: layout.clone(), depth, u_layout, u_buf, u_bg, inst_tex, grid_tex, scratch: None, sim: None, inst: Vec::new(), grid: Vec::new(), built: None, upload_due: false }
    }

    /// The pass's pipeline (`in_place` or the full image) and bind groups 1 (the depth) and 2 (the uniform and the data).
    pub(super) fn bind(&mut self, device: &wgpu::Device, depth: DepthSrc, in_place: bool) -> (&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::BindGroup) {
        let ms = depth.multisampled;
        let (image_layout, depth_layout, u_layout) = (&self.image_layout, self.depth.layout(ms), &self.u_layout);
        let pipe = self.pipes.get((in_place, ms), || {
            let module = gfx::wgsl_module(device, "drops", &source(ms, in_place));
            let pl = gfx::pipeline_layout(device, "drops", &[image_layout, depth_layout, u_layout]);
            Fullscreen::new("drops", &pl, &module).build(device, &[gfx::target(HDR_FORMAT, None)])
        });
        (pipe, self.depth.bind(device, depth), &self.u_bg)
    }

    /// Step the water to the scene's clock and say whether there is anything to draw (`None` = skip the pass; the water
    /// is dropped by the caller, so the next look at the windscreen starts from a fresh pre-roll).
    pub fn prepare(&mut self, d: &Drops, g: &Glass) -> Option<usize> {
        let p = Params { wet: d.wet, cover: d.cover, speed: d.speed, gravity: g.gravity, size: g.size };
        // a fresh glass (the first look, a jump of the clock) is built again whatever its step count
        let fresh = match self.sim.as_mut() {
            Some(s) => s.advance(d.time, &p),
            None => {
                self.sim = Some(Sim::new(d.time, &p));
                true
            }
        };
        if fresh {
            self.built = None;
        }
        let s = self.sim.as_mut().expect("drops sim");
        if !s.busy(&p) {
            return None;
        }
        // PERF: the instances are a function of the water and the glass's size: built (and uploaded) only when either changed
        let key = (s.steps, g.size.map(f32::to_bits));
        let n = match self.built {
            Some((k, n)) if k == key => n,
            _ => {
                let n = s.build(g.size, &mut self.inst, &mut self.grid);
                self.built = Some((key, n));
                self.upload_due = true;
                n
            }
        };
        (n > 0).then_some(n)
    }

    /// The glass's box on the screen in pixels `(x0, y0, x1, y1)` (padded, clamped to the frame); `None` when it is empty.
    pub(super) fn pixel_box(g: &Glass, width: u32, height: u32) -> Option<[u32; 4]> {
        let [nx0, ny0, nx1, ny1] = g.ndc_box;
        let (w, h) = (width as f32, height as f32);
        let px = |v: f32, n: f32| v.clamp(0.0, n);
        let x0 = px((nx0 * 0.5 + 0.5) * w - BOX_PAD, w).floor() as u32;
        let x1 = px((nx1 * 0.5 + 0.5) * w + BOX_PAD, w).ceil() as u32;
        let y0 = px((0.5 - ny1 * 0.5) * h - BOX_PAD, h).floor() as u32;
        let y1 = px((0.5 - ny0 * 0.5) * h + BOX_PAD, h).ceil() as u32;
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }

    /// The data of this frame: the instance texels and the grid (when [`Self::prepare`] rebuilt them; queued writes, once a
    /// frame: only this pass reads them), and the uniform (through the belt, `cx.encoder`).
    pub(super) fn upload(&mut self, cx: &mut PostCtx, glass: &Glass, look: Look, bx: [f32; 4], bz: [f32; 4]) {
        let (queue, cam) = (cx.queue, cx.camera);
        if std::mem::take(&mut self.upload_due) {
            self.inst.resize((INST_W * INST_H) as usize, [0.0; 4]);
            let write = |tex: &wgpu::Texture, bytes: &[u8], w: u32, h: u32, bpp: u32| {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    bytes,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            };
            write(&self.inst_tex, bytemuck::cast_slice(&self.inst), INST_W, INST_H, INST_TEXEL_BYTES);
            write(&self.grid_tex, bytemuck::cast_slice(&self.grid), (GRID_COLS * CELL_SLOTS) as u32, GRID_ROWS as u32, GRID_TEXEL_BYTES);
        }
        let (cols, inv) = (glass.to_uv.to_cols_array_2d(), glass.inv_dist);
        let h = look.height as f32;
        let u = DropsUniform {
            h0: [cols[0][0], cols[0][1], cols[0][2], 0.0],
            h1: [cols[1][0], cols[1][1], cols[1][2], 0.0],
            h2: [cols[2][0], cols[2][1], cols[2][2], 0.0],
            inv: [inv.x, inv.y, inv.z, glass.size[0]],
            a: [look.width as f32, h, glass.size[1], look.strength],
            b: [cam.far / (cam.near - cam.far), cam.near, 0.5 * h / (0.5 * cam.fov_y_radians).tan(), REFRACT],
            sky: [look.sky[0], look.sky[1], look.sky[2], 0.0],
            bx,
            bz,
        };
        self.u_buf.write(cx.belt, cx.encoder, &u);
    }

    /// Run the pass over `src` (`look`'s size, its bind group `src_bg`) into `out` (the chain's image of that size), reading
    /// `cx.depth`. `prepare` has just said there are instances.
    pub fn run(&mut self, cx: &mut PostCtx, src_bg: &wgpu::BindGroup, glass: &Glass, look: Look, out: &wgpu::TextureView) {
        let PostCtx { device, depth, .. } = *cx;
        let (w, h) = (look.width as f32, look.height as f32);
        self.upload(cx, glass, look, [0.0, 0.0, w, h], [w, h, 0.0, 0.0]);
        let (pipe, depth_bg, u_bg) = self.bind(device, depth, false);
        gfx::fullscreen_pass(cx.encoder, "drops", "windscreen drops", out, wgpu::LoadOp::Clear(wgpu::Color::BLACK), pipe, &[src_bg, depth_bg, u_bg]);
    }

    /// Run the pass over the scene's own image `scene` (`COPY_SRC`, the float image): a copy of the glass's box as the colour
    /// source and a blended draw over that box only - the rest of the frame is not touched, read or written.
    pub fn run_in_place(&mut self, cx: &mut PostCtx, scene: &wgpu::Texture, scene_view: &wgpu::TextureView, glass: &Glass, look: Look) {
        let PostCtx { device, layout, sampler, depth, .. } = *cx;
        let Some([x0, y0, x1, y1]) = Self::pixel_box(glass, look.width, look.height) else { return };
        let (bw, bh) = (x1 - x0, y1 - y0);
        if !self.scratch.as_ref().is_some_and(|(w, h, ..)| *w >= bw && *h >= bh) {
            let (w, h) = (bw + SCRATCH_SLACK, bh + SCRATCH_SLACK);
            let texture = gfx::texture_2d(device, "drops scratch", w, h, HDR_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            let bg = gfx::tex_sampler_bg(device, "drops scratch", layout, &texture.create_view(&Default::default()), sampler);
            self.scratch = Some((w, h, texture, bg));
        }
        let (sw, sh, scratch, scratch_bg) = self.scratch.as_ref().expect("drops scratch");
        cx.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: scene, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: scratch, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: bw, height: bh, depth_or_array_layers: 1 },
        );
        let bx = [x0 as f32, y0 as f32, *sw as f32, *sh as f32];
        let bz = [bw as f32, bh as f32, 0.0, 0.0];
        let scratch_bg = scratch_bg.clone();
        self.upload(cx, glass, look, bx, bz);
        let (pipe, depth_bg, u_bg) = self.bind(device, depth, true);
        let mut pass = gfx::color_pass(cx.encoder, "drops", "windscreen drops", scene_view, wgpu::LoadOp::Load);
        pass.set_scissor_rect(x0, y0, bw, bh);
        gfx::draw_fullscreen(&mut pass, pipe, &[&scratch_bg, depth_bg, u_bg]);
    }
}
