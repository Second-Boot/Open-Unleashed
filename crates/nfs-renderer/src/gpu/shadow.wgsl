// ENHANCEMENT E9: the sun's shadow map pass — depth only, one cascade per pass. The vertex stage reads
// the same object storage buffer as the main pass (`first_instance` names the item); the cut-out
// variant tests the texture's alpha so foliage and fences cast their silhouette.
struct Object {
    model: mat4x4<f32>,
    env_mix: vec4<f32>,
    light: vec4<f32>,
    lit_a: vec4<f32>,
    lit_d: vec4<f32>,
    lit_dir: vec4<f32>,
    shadow_u: vec4<f32>,
    shadow_v: vec4<f32>,
    shadow_on: vec4<f32>,
    sky_u: vec4<f32>,
    sky_v: vec4<f32>,
    sky_on: vec4<f32>,
    refl: vec4<f32>,
    // the main shader's `soft` / `hdr` (E12 / E11 / E18): not read here, but the array stride must match `ObjectUniform` (SHADOWFIX: without
    // them `objects[i]` read the item at i * 13 / 15, so a caster took another item's transform and alpha test whenever its index changed)
    soft: vec4<f32>,
    hdr: vec4<f32>,
    mvp: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> light_vp: mat4x4<f32>;
@group(1) @binding(0) var<storage, read> objects: array<Object>;
@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var s_diffuse: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) @interpolate(flat) obj: u32,
};

@vertex
fn vs_main(in: VsIn, @builtin(instance_index) inst: u32) -> VsOut {
    let o = objects[inst];
    var out: VsOut;
    out.clip = light_vp * (o.model * vec4<f32>(in.position, 1.0));
    // pancake: a caster nearer the sun than the cascade's box is held at the near plane, so it still
    // shades what is behind it instead of being clipped away
    out.clip.z = max(out.clip.z, 0.0);
    out.uv = in.uv + o.env_mix.zw;
    out.alpha = in.color.a * o.light.a;
    out.obj = inst;
    return out;
}

@fragment
fn fs_cut(in: VsOut) {
    let o = objects[in.obj];
    let a = textureSampleLevel(t_diffuse, s_diffuse, in.uv, 0.0).a * in.alpha;
    // the item's own alpha test where it has one (reference + 1/2 in bytes), else half
    var thr = 0.5;
    if o.shadow_on.w > 0.0 {
        thr = o.shadow_on.w / 255.0;
    }
    if a < thr {
        discard;
    }
}
