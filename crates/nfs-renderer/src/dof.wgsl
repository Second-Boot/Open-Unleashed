
struct U { a: vec4<f32>, b: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
// the distance in front of the camera of a depth-buffer value (the projection's own inverse)
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.b.x, u.b.y);
}
fn coc_of(d: f32) -> f32 {
    return min(u.a.y * abs(1.0 - u.a.x / max(d, 0.01)), u.a.z);
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(i32(u.b.z), i32(u.b.w));
    let c = textureSampleLevel(t, s, in.uv, 0.0);
    let d0 = lin(dload(vec2<i32>(in.pos.xy), dims));
    let c0 = coc_of(d0);
    let reach = u.a.z;
    let n = i32(u.a.w);
    var sum = min(c.rgb, vec3<f32>(24.0));
    var wsum = 1.0;
    // the disc's turn of this pixel, applied to the table's unit directions (PERF: no per-tap cos / sin)
    let rot = ign(in.pos.xy) * TAU;
    let rc = cos(rot);
    let rs = sin(rot);
    for (var i = 0; i < n; i = i + 1) {
        let fi = f32(i) + 0.5;
        let r = sqrt(fi / f32(n)) * reach;
        let v = VOGEL_DIR[i];
        let sp = in.pos.xy + vec2<f32>(v.x * rc - v.y * rs, v.x * rs + v.y * rc) * r;
        let sc = clamp(vec2<i32>(sp), vec2<i32>(0), dims - vec2<i32>(1));
        let dt = lin(dload(sc, dims));
        var ct = coc_of(dt);
        // a tap behind this pixel cannot blur it more than the pixel's own circle
        if dt > d0 {
            ct = min(ct, c0 + 0.5);
        }
        let w = clamp(ct - r + 0.5, 0.0, 1.0);
        if w > 0.0 {
            let tc = textureSampleLevel(t, s, sp / vec2<f32>(dims), 0.0).rgb;
            sum = sum + min(tc, vec3<f32>(24.0)) * w;
            wsum = wsum + w;
        }
    }
    return vec4<f32>(sum / wsum, c.a);
}
