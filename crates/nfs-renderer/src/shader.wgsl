struct Globals {
    view_proj: mat4x4<f32>,
    sun_dir: vec4<f32>,
    ambient: vec4<f32>,
    cam_pos: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Object {
    model: mat4x4<f32>,
    env_mix: vec4<f32>,
};
@group(1) @binding(0) var<uniform> object: Object;

@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var s_diffuse: sampler;
@group(2) @binding(2) var t_env: texture_2d<f32>;

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
    @location(3) world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = normalize((object.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv = in.uv;
    out.color = in.color;
    out.world = world.xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.sun_dir.xyz)), 0.0);
    let light = globals.ambient.x + (1.0 - globals.ambient.x) * diffuse;
    var base = tex * in.color;
    let mix_amt = object.env_mix.x;
    if mix_amt > 0.0 {
        // sphere-map lookup of the reflected view vector (approximation of the original's env layer)
        let v = normalize(globals.cam_pos.xyz - in.world);
        let r = reflect(-v, n);
        let m = 2.0 * sqrt(r.x * r.x + r.y * r.y + (r.z + 1.0) * (r.z + 1.0)) + 1e-4;
        let env = textureSample(t_env, s_diffuse, vec2<f32>(r.x / m + 0.5, 0.5 - r.y / m));
        base = vec4<f32>(mix(base.rgb, env.rgb, mix_amt), base.a);
    }
    return vec4<f32>(base.rgb * light, base.a);
}
