struct VsIn {
    @location(0) pos: vec3<f32>,   // clip space; z the screen depth of a depth-state sprite
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) alpha_ref: f32,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) alpha_ref: f32,
};
@group(0) @binding(0) var t_sprite: texture_2d<f32>;
@group(0) @binding(1) var s_sprite: sampler;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.alpha_ref = in.alpha_ref;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_sprite, s_sprite, in.uv) * in.color;
}

// The device's alpha test for a sprite under the front end's depth state (`SpriteDepth`):
// `ALPHAFUNC` `GREATER`, `ALPHAREF` the sprite's, over the pixel's alpha after the texture stage
// (`ALPHAOP` `MODULATE`: texture × diffuse). A dropped pixel writes neither colour nor depth. A
// reference of −1 is the test switched off (Thrash 0x24 ← 0): nothing is dropped.
@fragment
fn fs_alpha_test(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_sprite, s_sprite, in.uv) * in.color;
    if round(clamp(c.a, 0.0, 1.0) * 255.0) <= in.alpha_ref {
        discard;
    }
    return c;
}
