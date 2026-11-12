//! The WGSL pieces the full-screen passes share and a tiny composer.
//!
//! A pass's shader is its own text plus the pieces it names, joined by [`compose`]: WGSL declarations at
//! module scope may come in any order, so a piece is simply put in front. Every source a pass can build is
//! run through naga's front end and validator by the test at the end of this file.

/// The full-screen triangle (three vertices, no buffers), position only: the fragment stage reads
/// `@builtin(position)`.
pub(crate) const FULLSCREEN_VS: &str = r#"
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// The full-screen triangle with a `uv` (0..1 over the target, y down) in `VsOut`.
pub(crate) const FULLSCREEN_VS_UV: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
"#;

/// Interleaved gradient noise (Jimenez 2014), 0..1 per pixel.
pub(crate) const IGN: &str = r#"
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}
"#;

/// The frame's depth as group 1, binding 0 (`dtex`) of the motion blur, the depth of field and the windscreen
/// drops: a plain or a multisampled depth image.
pub(crate) fn dtex(multisampled: bool) -> &'static str {
    if multisampled { "\n@group(1) @binding(0) var dtex: texture_depth_multisampled_2d;\n" } else { "\n@group(1) @binding(0) var dtex: texture_depth_2d;\n" }
}

/// `dload(p, dims)`: the depth under pixel `p` of a target `dims` in size, from [`dtex`].
pub(crate) const DLOAD: &str = r#"
// ENHANCEMENT E37: the depth image may be smaller than the image the pass writes (the post chain runs at the window's size
// on FSR's output while the depth is the internal one): a pixel of the target reads the depth texel under its centre.
// With the sizes equal this is the plain load.
fn dload(p: vec2<i32>, dims: vec2<i32>) -> f32 {
    let dd = vec2<i32>(textureDimensions(dtex));
    if all(dd == dims) {
        return textureLoad(dtex, p, 0);
    }
    return textureLoad(dtex, min(vec2<i32>((vec2<f32>(p) + 0.5) * vec2<f32>(dd) / vec2<f32>(dims)), dd - vec2<i32>(1)), 0);
}
"#;

/// What the scene-depth passes (light shafts, haze, lighthouse beam) read as the depth: the depth image, the
/// multisampled one (its sample 0), or the soft-particle pass's single-sample `R32Float` conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DepthKind {
    Single = 0,
    Multi = 1,
    /// PERF: the depth as the soft-particle pass already converted it (`R32Float`, the nearest sample of each pixel): a plain
    /// single-sample read, far friendlier to the cache than a multisampled depth when the rays fan out over the screen
    Soft = 2,
}

impl DepthKind {
    #[cfg(test)]
    pub const ALL: [DepthKind; 3] = [DepthKind::Single, DepthKind::Multi, DepthKind::Soft];

    /// The image's binding in group 0.
    pub fn binding(self) -> u32 {
        [1, 0, 5][self as usize]
    }

    /// The image's declaration, `depth_at(p)` (the raw depth 0..1 at pixel `p`) and `depth_dims()` (its size).
    pub fn wgsl(self) -> &'static str {
        match self {
            DepthKind::Single => {
                "\n@group(0) @binding(1) var depth_1: texture_depth_2d;\n\
                 fn depth_at(p: vec2<i32>) -> f32 { return textureLoad(depth_1, p, 0); }\n\
                 fn depth_dims() -> vec2<u32> { return textureDimensions(depth_1); }\n"
            }
            DepthKind::Multi => {
                "\n@group(0) @binding(0) var depth_ms: texture_depth_multisampled_2d;\n\
                 fn depth_at(p: vec2<i32>) -> f32 { return textureLoad(depth_ms, p, 0); }\n\
                 fn depth_dims() -> vec2<u32> { return textureDimensions(depth_ms); }\n"
            }
            DepthKind::Soft => {
                "\n@group(0) @binding(5) var soft_t: texture_2d<f32>;\n\
                 fn depth_at(p: vec2<i32>) -> f32 { return textureLoad(soft_t, p, 0).r; }\n\
                 fn depth_dims() -> vec2<u32> { return textureDimensions(soft_t); }\n"
            }
        }
    }

    /// The image's layout entry (fragment stage).
    pub fn layout_entry(self) -> wgpu::BindGroupLayoutEntry {
        let sample_type = if self == DepthKind::Soft { wgpu::TextureSampleType::Float { filterable: false } } else { wgpu::TextureSampleType::Depth };
        crate::gfx::bgl_tex(self.binding(), sample_type, self == DepthKind::Multi)
    }
}

/// A shader from its parts, in order.
pub(crate) fn compose(parts: &[&str]) -> String {
    parts.concat()
}

#[cfg(test)]
mod tests {
    use super::DepthKind;

    /// Parse `src` with naga's WGSL front end and validate it (the capabilities wgpu grants a device with multisampled
    /// shading, which every backend of the game has; `sample_index` needs it).
    fn validate(name: &str, src: &str) -> Result<(), String> {
        let module = naga::front::wgsl::parse_str(src).map_err(|e| format!("{name}: {}", e.emit_to_string(src)))?;
        let caps = naga::valid::Capabilities::default() | naga::valid::Capabilities::MULTISAMPLED_SHADING;
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), caps).validate(&module).map_err(|e| format!("{name}: {:?}", e.into_inner()))?;
        Ok(())
    }

    /// Every WGSL source the renderer can build, after composition, for every variant the passes use.
    fn every_source() -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = vec![
            ("mesh shader".into(), crate::classes::mesh_shader_source(crate::classes::feat::ALL)),
            ("mesh shader (all off)".into(), crate::classes::mesh_shader_source(0)),
            ("sprite".into(), include_str!("sprite.wgsl").into()),
            ("shadow".into(), include_str!("gpu/shadow.wgsl").into()),
            ("probe".into(), include_str!("probe.wgsl").into()),
            ("eye measure".into(), crate::adapt::measure_source()),
            ("ao".into(), crate::ao::source()),
            ("bloom".into(), crate::bloom::source()),
            ("coat prefilter".into(), crate::coat::source()),
            ("smaa".into(), crate::smaa::source()),
            ("sharpen".into(), crate::sharpen::source()),
            ("soft depth".into(), crate::soft::source()),
            ("temporal pass".into(), crate::temporal::pass_source()),
            ("gamma".into(), crate::gpu::gamma_source()),
            ("near mask".into(), crate::gpu::near_mask_source()),
        ];
        for ms in [false, true] {
            v.push((format!("motion blur ms={ms}"), crate::camfx::blur_source(ms)));
            v.push((format!("dof ms={ms}"), crate::dof::source(ms)));
            for in_place in [false, true] {
                v.push((format!("drops ms={ms} in_place={in_place}"), crate::drops::source(ms, in_place)));
            }
        }
        for eye in [false, true] {
            for edr in [false, true] {
                v.push((format!("scale blit eye={eye} edr={edr}"), crate::gpu::blit_source(eye, edr)));
            }
        }
        for kind in DepthKind::ALL {
            v.push((format!("shafts {kind:?}"), crate::shafts::source(kind)));
            for steps in [10, 14] {
                v.push((format!("haze {kind:?} steps={steps}"), crate::haze::source(kind, steps)));
            }
            for samples in [1, 4] {
                v.push((format!("lbeam {kind:?} samples={samples}"), crate::lbeam::source(kind, samples)));
            }
        }
        #[cfg(target_os = "macos")]
        v.push(("edr present".into(), crate::edr::present_source()));
        // PERF-B: the mesh shader with each part alone and with every part but one, and the light preset's and the mirror's sets
        use crate::classes::feat;
        for (name, bit) in feat::NAMES {
            v.push((format!("mesh shader {name} only"), crate::classes::mesh_shader_source(bit)));
            v.push((format!("mesh shader without {name}"), crate::classes::mesh_shader_source(feat::ALL & !bit)));
        }
        let lite = crate::classes::mesh_features(&crate::Enhancements::lite());
        v.push(("mesh shader (light preset)".into(), crate::classes::mesh_shader_source(lite)));
        v.push(("mesh shader (light preset, mirror)".into(), crate::classes::mesh_shader_source(lite & !feat::NOT_IN_MIRROR)));
        v.push(("mesh shader (light preset, plain items)".into(), crate::classes::mesh_shader_source(lite & !feat::PER_ITEM)));
        v.push(("mesh shader (light preset, surfaces)".into(), crate::classes::mesh_shader_source(lite & !feat::EFFECT)));
        v.push(("mesh shader (light preset, effects)".into(), crate::classes::mesh_shader_source(lite & !feat::SURFACE)));
        v.push(("mesh shader (plain items)".into(), crate::classes::mesh_shader_source(feat::ALL & !feat::PER_ITEM)));
        v.push(("mesh shader (mirror)".into(), crate::classes::mesh_shader_source(feat::ALL & !feat::NOT_IN_MIRROR)));
        v
    }

    #[test]
    fn every_shader_parses_and_validates() {
        let sources = every_source();
        let errors: Vec<String> = sources.iter().filter_map(|(name, src)| validate(name, src).err()).collect();
        assert!(errors.is_empty(), "{} of {} shaders fail:\n{}", errors.len(), sources.len(), errors.join("\n"));
    }

    /// The composer only joins: a piece is in a shader once (no doubled `vs` / `ign` / `dload` / depth access).
    #[test]
    fn each_piece_is_in_a_shader_once() {
        for (name, src) in every_source() {
            for f in ["fn vs(", "fn ign(", "fn dload(", "fn depth_at(", "fn depth_dims("] {
                assert!(src.matches(f).count() <= 1, "{name}: `{f}` twice");
            }
        }
    }
}
