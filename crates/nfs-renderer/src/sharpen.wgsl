// ENHANCEMENT E39: robust contrast-adaptive sharpening (RCAS) after the MetalFX scalers, at the window's size. See sharpen.rs
// for the credit (AMD FidelityFX FSR 1, `FsrRcasF`, MIT) and what differs.
//
// The cross of five taps (b above, d left, e the pixel, f right, h below) gives a negative lobe weight `w` for the four
// neighbours: out = (w · (b + d + f + h) + e) / (4w + 1). The weight is the largest that keeps every channel of the result
// inside [0, 1] given the ring's min / max (no ringing past the local range), capped at RCAS_LIMIT, scaled by CON and
// damped where the pixel stands out of a flat ring by more than the ring's own range (noise, FSR_RCAS_DENOISE).

// exp2(-stops): FSR 3's mapping of a 0..1 sharpness s is stops = 2 - 2s (sharpness 0.3 -> 0.379)
override CON: f32 = 0.379;
// 1 = the float scene (display-referred values that may exceed 1): the taps go through the invertible c / (1 + max(c))
// so the limits work on 0..1 and a bright highlight does not throw a dark halo; 0 = an 8-bit image, as it is
override HDR: u32 = 0u;

const RCAS_LIMIT: f32 = 0.25 - 1.0 / 16.0;

@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;

fn max3(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b));
}

fn tap(p: vec2<i32>, dims: vec2<i32>) -> vec3<f32> {
    let c = max(textureLoad(t, clamp(p, vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb, vec3<f32>(0.0));
    if HDR == 1u {
        return c / (1.0 + max3(c));
    }
    return min(c, vec3<f32>(1.0));
}

fn untap(c: vec3<f32>) -> vec3<f32> {
    if HDR == 1u {
        return c / max(1.0 - max3(c), 1.0 / 4096.0);
    }
    return c;
}

// FSR's luma times two: 0.5 R + G + 0.5 B
fn luma2(c: vec3<f32>) -> f32 {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(t));
    let p = vec2<i32>(pos.xy);
    let b = tap(p + vec2<i32>(0, -1), dims);
    let d = tap(p + vec2<i32>(-1, 0), dims);
    let e = tap(p, dims);
    let f = tap(p + vec2<i32>(1, 0), dims);
    let h = tap(p + vec2<i32>(0, 1), dims);
    let alpha = textureLoad(t, clamp(p, vec2<i32>(0), dims - vec2<i32>(1)), 0).a;
    // noise detection (FSR_RCAS_DENOISE): 1 on an edge or a gradient, down to 0.5 on a lone speck
    let bl = luma2(b);
    let dl = luma2(d);
    let el = luma2(e);
    let fl = luma2(f);
    let hl = luma2(h);
    let range = max(max(max(bl, dl), max(el, fl)), hl) - min(min(min(bl, dl), min(el, fl)), hl);
    var nz = 0.0;
    if range > 0.0 {
        nz = clamp(abs(0.25 * (bl + dl + fl + hl) - el) / range, 0.0, 1.0);
    }
    nz = 1.0 - 0.5 * nz;
    // the ring's min / max per channel and the lobe that keeps the result inside [0, 1]
    let mn4 = min(min(b, d), min(f, h));
    let mx4 = max(max(b, d), max(f, h));
    let hit_min = min(mn4, e) / max(4.0 * mx4, vec3<f32>(1.0 / 65536.0));
    let hit_max = (vec3<f32>(1.0) - max(mx4, e)) / min(4.0 * mn4 - vec3<f32>(4.0), vec3<f32>(-1.0 / 65536.0));
    let lobe3 = max(-hit_min, hit_max);
    let lobe = max(-RCAS_LIMIT, min(max3(lobe3), 0.0)) * CON * nz;
    let out = (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0);
    return vec4<f32>(untap(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0))), alpha);
}
