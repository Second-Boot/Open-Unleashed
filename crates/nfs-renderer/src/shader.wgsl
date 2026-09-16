struct Globals {
    view_proj: mat4x4<f32>,
    sun_dir: vec4<f32>,
    ambient: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Object {
    model: mat4x4<f32>,
};
@group(1) @binding(0) var<uniform> object: Object;

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
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = normalize((object.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.sun_dir.xyz)), 0.0);
    let light = globals.ambient.x + (1.0 - globals.ambient.x) * diffuse;
    let base = tex * in.color;
    return vec4<f32>(base.rgb * light, base.a);
}
