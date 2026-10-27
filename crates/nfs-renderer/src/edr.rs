//! ENHANCEMENT E33: real HDR / EDR output (macOS: a `CAMetalLayer` with `wantsExtendedDynamicRangeContent`, the surface
//! `Rgba16Float` in wgpu's `ExtendedSrgbLinear` colour space; the screen's `NSScreen` EDR headroom).
//!
//! The picture is still drawn by the byte pipeline (and E7's float scene + tone map) into an ordinary 8-bit
//! frame image. With E33 on the frame goes through three extra things, all of them here:
//! * the tone map writes a second output, a per-pixel linear-light GAIN (`ScaleBlit` `fs_edr`): 1 wherever the scene
//!   stays within the SDR range, and where a channel of the float scene is above white (the sun core, lamps, sparks,
//!   bloom peaks, specular overshoot) the factor that takes the 8-bit (clamped) pixel to a soft-shouldered value up
//!   to [`CAP`] times SDR white;
//! * a copy of the frame image taken before the 2D sprites (HUD, front end) are drawn;
//! * the present pass: the gamma ramp, the sRGB decode of the 8-bit frame to linear light (white = 1.0), the gain
//!   where the 2D drew nothing (the pixel is byte-equal to the copy), the display's headroom as a ceiling, written into
//!   the `Rgba16Float` surface. A pixel the 2D touched keeps gain 1, so the HUD is exactly SDR white.
//!
//! With E33 off, on another platform, or on a display without EDR nothing here runs and the surface is master's.

#[cfg(target_os = "macos")]
pub(crate) use mac::*;
#[cfg(not(target_os = "macos"))]
pub(crate) use other::*;

/// The brightest the highlights go, in multiples of SDR white (a soft shoulder from 1.0 up to this); the tone map's
/// `EDR_CAP`. The display's own headroom lowers it further in the present pass.
pub(crate) const CAP: f32 = 3.0;

/// The softness of the shoulder's start above white (linear, white = 1): the tone map's `EDR_KNEE`. A channel `t`
/// over white is lifted by `t^2 / (t + KNEE)` (slope 0 at white, no threshold; 0.3 over white lifts 0.11, 1.0 lifts 0.67).
pub(crate) const KNEE: f32 = 0.5;

/// The WGSL of the tone map's EDR entry point, appended to the blit's source; `EXPOSURE` / `//EYE_APPLY` are
/// replaced with the rest. The SDR output is exactly `fs`'s.
pub(crate) const TONE_EDR_WGSL: &str = r#"
// ENHANCEMENT E33: the sRGB transfer function, extended above 1 (the value is the display-referred one)
fn edr_dec(v: vec3<f32>) -> vec3<f32> {
    let a = max(v, vec3<f32>(0.0));
    return select(pow((a + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), a / 12.92, a <= vec3<f32>(0.04045));
}
// the soft shoulder of ONE channel (linear light, white = 1): the identity up to white, above it a rise that starts
// flat (the overshoot of ordinary additive layers, sky and specular, stays SDR white: `h` grows as t^2 / (t + KNEE),
// slope 0 at white, no threshold) and bends to EDR_CAP exponentially. Monotone and C1 above white, continuous at it.
// A per-CHANNEL curve on purpose: a pixel is never scaled as a whole, so the channels at or under white keep their SDR
// value and only the ones above it are lifted (the first version scaled the whole pixel by shoulder(max) / max from a
// threshold on the brightest channel: the other channels fell by a third on the threshold's contour, a hard-edged
// darker band round the sun and round every lens-flare element (SUNBLOB, 2026-10-04))
fn edr_shoulder(x: f32) -> f32 {
    let t = max(x - 1.0, 0.0);
    let h = t * t / (t + EDR_KNEE);
    return select(x, 1.0 + (EDR_CAP - 1.0) * (1.0 - exp(-h / (EDR_CAP - 1.0))), x > 1.0);
}
struct EdrOut { @location(0) color: vec4<f32>, @location(1) gain: vec4<f32> };
@fragment fn fs_edr(in: VsOut) -> EdrOut {
    var c = textureSampleLevel(t, s, in.uv, 0.0);
    //EYE_APPLY
    var o: EdrOut;
    var dv = c.rgb;
    if MODE == 0u {
        o.color = dither(c, in.pos.xy);
    } else {
        let lin = pow(max(c.rgb, vec3<f32>(0.0)), vec3<f32>(2.2)) * EXPOSURE;
        let mapped = aces(lin) / aces(vec3<f32>(EXPOSURE));
        o.color = dither(vec4<f32>(pow(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), c.a), in.pos.xy);
        dv = pow(max(mapped, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    // per channel: 1 for a channel at or under white, else the shoulder over the clamped (SDR) value
    let base = edr_dec(dv);
    let sdr = edr_dec(clamp(dv, vec3<f32>(0.0), vec3<f32>(1.0)));
    let want = vec3<f32>(edr_shoulder(base.r), edr_shoulder(base.g), edr_shoulder(base.b));
    let gain = select(vec3<f32>(1.0), clamp(want / max(sdr, vec3<f32>(0.001)), vec3<f32>(1.0), vec3<f32>(8.0)), base > vec3<f32>(1.0));
    o.gain = vec4<f32>(gain, 1.0);
    return o;
}
"#;

#[cfg(target_os = "macos")]
mod mac {
    use super::CAP;
    use crate::gfx::{self, Fullscreen};
    use crate::wgsl;

    /// The present pass: 8-bit frame -> linear light, the gamma ramp, the gain, the headroom ceiling.
    const PRESENT_WGSL: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var pre2d: texture_2d<f32>;
@group(0) @binding(2) var gainmap: texture_2d<f32>;
// x = the gamma ramp's g (state 0x2E), y = the ceiling (the display's headroom), z = 1 when the frame has a gain map
@group(0) @binding(3) var<uniform> u: vec4<f32>;
@group(0) @binding(4) var gs: sampler;
fn dec(v: vec3<f32>) -> vec3<f32> {
    return select(pow((v + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let c = textureLoad(frame, p, 0);
    // the display ramp exactly as GammaPass writes it (identity at g = 1)
    let i = round(c.rgb * 255.0);
    let w = min(floor(i * 257.0 * u.x), vec3<f32>(65535.0)) / 65535.0;
    var lin = dec(w);
    if u.z > 0.5 {
        // a pixel the 2D (HUD, front end) did not touch: byte-equal to the copy taken before the sprites
        let before = textureLoad(pre2d, p, 0);
        if all(before.rgb == c.rgb) {
            // the gain map is the tone map's output size (smaller with MetalFX): bilinear at this pixel
            lin = lin * textureSampleLevel(gainmap, gs, pos.xy / vec2<f32>(textureDimensions(frame)), 0.0).rgb;
        }
    }
    let m = max(lin.r, max(lin.g, lin.b));
    if m > u.y {
        lin = lin * (u.y / m);
    }
    return vec4<f32>(lin, 1.0);
}
"#;

    struct Targets {
        size: (u32, u32),
        frame: wgpu::Texture,
        frame_view: wgpu::TextureView,
        pre2d: wgpu::Texture,
        pre_view: wgpu::TextureView,
    }

    /// The pipeline, its layout, the uniform and the gain map's sampler.
    type Present = (wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Buffer, wgpu::Sampler);
    pub(crate) struct Edr {
        /// the surface offers `Rgba16Float` + `ExtendedSrgbLinear` and the display's potential headroom is above 1
        pub ok: bool,
        /// the display's headroom (`maximumPotentialExtendedDynamicRangeColorComponentValue`), 1.0 when unknown
        pub headroom: f32,
        /// the surface is configured for EDR (E33 on and `ok`)
        pub on: bool,
        /// this frame goes through the EDR chain (set by `Core::render` around the encode)
        pub pass: bool,
        /// this frame's tone map wrote the gain map
        pub gain_used: bool,
        pipeline: Option<Present>,
        targets: Option<Targets>,
        /// the tone map's gain map (size, texture, view), at the size of the image the tone map writes
        gain: Option<((u32, u32), wgpu::Texture, wgpu::TextureView)>,
        bind_group: Option<wgpu::BindGroup>,
    }

    impl Edr {
        /// Probe the surface and the display. Main thread (the Metal backend reads `NSScreen`).
        pub fn probe(surface: &wgpu::Surface, adapter: &wgpu::Adapter, caps: &wgpu::SurfaceCapabilities) -> Self {
            let spaces = caps.color_spaces(wgpu::TextureFormat::Rgba16Float);
            let supported = spaces.contains(wgpu::SurfaceColorSpaces::EXTENDED_SRGB_LINEAR);
            let info = surface.display_hdr_info(adapter);
            let potential = info.headroom.and_then(|h| h.potential).filter(|h| h.is_finite());
            // diagnostic: `NFS_EDR_HEADROOM=<x>` pretends the display has that headroom (a display without EDR, an asleep screen)
            let potential = std::env::var("NFS_EDR_HEADROOM").ok().and_then(|v| v.parse::<f32>().ok()).or(potential);
            let ok = supported && potential.is_some_and(|h| h > 1.0);
            tracing::info!(supported, headroom = ?info.headroom, ok, "E33 EDR probe (Rgba16Float + ExtendedSrgbLinear, display headroom)");
            Self { ok, headroom: potential.unwrap_or(1.0), on: false, pass: false, gain_used: false, pipeline: None, targets: None, gain: None, bind_group: None }
        }

        /// The ceiling of a pixel's linear value in the present pass: the shoulder's cap, lowered to the display's headroom.
        /// The display does extended-range output (the switch's row is live).
        pub fn available(&self) -> bool {
            self.ok
        }

        /// E33 at `level`: on (level 1) when the display can do it. `true` when the state changed (the surface is then
        /// configured again by the caller).
        pub fn apply(&mut self, level: u8) -> bool {
            let want = self.ok && level == 1;
            if want == self.on {
                return false;
            }
            self.on = want;
            true
        }

        /// The log line of a change [`Self::apply`] made.
        pub fn log_output(&self) {
            tracing::info!(on = self.on, headroom = self.headroom, ceiling = self.ceiling(), "E33 EDR output");
        }

        pub fn ceiling(&self) -> f32 {
            CAP.min(self.headroom).max(1.0)
        }

        /// The window-sized frame image, its view and the pre-2D copy, kept while the size and format hold.
        pub fn targets(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> (wgpu::Texture, wgpu::TextureView, wgpu::Texture) {
            if self.pipeline.is_none() {
                self.pipeline = Some(build_present(device));
            }
            if self.targets.as_ref().is_none_or(|t| t.size != size || t.frame.format() != format) {
                let make = |label: &str, usage| {
                    gfx::texture_2d(device, label, size.0, size.1, format, usage)
                };
                let frame = make("edr frame", wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
                let pre2d = make("edr pre-2d", wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC);
                let frame_view = frame.create_view(&Default::default());
                let pre_view = pre2d.create_view(&Default::default());
                self.targets = Some(Targets { size, frame, frame_view, pre2d, pre_view });
                self.bind_group = None;
            }
            let t = self.targets.as_ref().expect("edr targets");
            (t.frame.clone(), t.frame_view.clone(), t.pre2d.clone())
        }

        /// The pre-2D copy (diagnostic).
        pub fn pre2d(&self) -> wgpu::Texture {
            self.targets.as_ref().expect("edr targets").pre2d.clone()
        }

        /// The gain map's view for a tone map that writes an image of `size` (the window's, or the internal size under
        /// MetalFX); made again when the size changes.
        pub fn gain_view(&mut self, device: &wgpu::Device, size: (u32, u32)) -> Option<wgpu::TextureView> {
            self.targets.as_ref()?;
            if self.gain.as_ref().is_none_or(|g| g.0 != size) {
                let tex = gfx::texture_2d(device, "edr gain", size.0.max(1), size.1.max(1), wgpu::TextureFormat::Rgba16Float, gfx::RT_USAGE);
                let view = tex.create_view(&Default::default());
                self.gain = Some((size, tex, view));
                self.bind_group = None;
            }
            self.gain.as_ref().map(|g| g.2.clone())
        }

        /// The present pass's bind group (made when a target changed).
        fn ensure_bind_group(&mut self, device: &wgpu::Device) {
            if self.bind_group.is_some() {
                return;
            }
            if self.gain.is_none() {
                self.gain_view(device, (1, 1));
            }
            let (Some((_, layout, uniform, sampler)), Some(t), Some((_, _, gain_view))) = (self.pipeline.as_ref(), self.targets.as_ref(), self.gain.as_ref()) else { return };
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("edr present"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.frame_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.pre_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(gain_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            }));
        }

        /// Copy the frame image (before the 2D) and mark the frame as one with a gain map.
        pub fn copy_pre_2d(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &wgpu::Texture, width: u32, height: u32) {
            let Some(t) = self.targets.as_ref() else { return };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: frame, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: &t.pre2d, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
            self.gain_used = true;
        }

        /// The present pass into the `Rgba16Float` surface view.
        pub fn present(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, surface_view: &wgpu::TextureView, gamma: f32) {
            self.ensure_bind_group(device);
            let (Some((pipeline, _, uniform, _)), Some(bind_group)) = (self.pipeline.as_ref(), self.bind_group.as_ref()) else { return };
            queue.write_buffer(uniform, 0, bytemuck::cast_slice(&[gamma, self.ceiling(), if self.gain_used { 1.0f32 } else { 0.0 }, 0.0]));
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("edr present") });
            {
                let mut pass = gfx::color_pass(&mut enc, "edr present", "edr present", surface_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            crate::prof::resolve(&mut enc);
            queue.submit(Some(enc.finish()));
        }
    }

    /// The present pass's shader.
    pub(crate) fn present_source() -> String {
        wgsl::compose(&[wgsl::FULLSCREEN_VS, PRESENT_WGSL])
    }

    fn build_present(device: &wgpu::Device) -> Present {
        let tex = |binding, filterable| gfx::bgl_tex(binding, wgpu::TextureSampleType::Float { filterable }, false);
        let layout = gfx::bgl(
            device,
            "edr present",
            &[tex(0, false), tex(1, false), tex(2, true), gfx::bgl_uniform(3, wgpu::ShaderStages::FRAGMENT), gfx::bgl_sampler(4, wgpu::SamplerBindingType::Filtering)],
        );
        let module = gfx::wgsl_module(device, "edr present", &present_source());
        let pl = gfx::pipeline_layout(device, "edr present", &[&layout]);
        let pipeline = Fullscreen::new("edr present", &pl, &module).build(device, &[gfx::target(wgpu::TextureFormat::Rgba16Float, None)]);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("edr present"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        (pipeline, layout, uniform, sampler)
    }
}

/// Other platforms: no EDR output. The type has the macOS one's calls the shared code makes, all no-ops;
/// `pass` (read by the tone map's setup) is never set, and the readers of the
/// macOS type's `on` are macOS-only code.
#[cfg(not(target_os = "macos"))]
mod other {
    pub(crate) struct Edr {
        pub pass: bool,
    }

    impl Edr {
        /// No display probe: never available.
        pub fn probe(_: &wgpu::Surface, _: &wgpu::Adapter, _: &wgpu::SurfaceCapabilities) -> Self {
            Self { pass: false }
        }

        pub fn available(&self) -> bool {
            false
        }

        /// Never changes the state.
        pub fn apply(&mut self, _level: u8) -> bool {
            false
        }

        pub fn log_output(&self) {}

        pub fn gain_view(&mut self, _: &wgpu::Device, _: (u32, u32)) -> Option<wgpu::TextureView> {
            None
        }

        pub fn copy_pre_2d(&mut self, _: &mut wgpu::CommandEncoder, _: &wgpu::Texture, _: u32, _: u32) {}
    }
}
