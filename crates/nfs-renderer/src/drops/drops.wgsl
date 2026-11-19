
struct U { h0: vec4<f32>, h1: vec4<f32>, h2: vec4<f32>, inv: vec4<f32>, a: vec4<f32>, b: vec4<f32>, sky: vec4<f32>, bx: vec4<f32>, bz: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
@group(2) @binding(1) var inst: texture_2d<f32>;
@group(2) @binding(2) var grid: texture_2d<u32>;
fn base(p: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t, s, (p - u.bx.xy) / u.bx.zw, 0.0);
}
//MODE
fn done(p: vec2<f32>, col: vec3<f32>, k: f32) -> vec4<f32> {
    let c = base(p);
    return vec4<f32>(mix(c.rgb, col, k), c.a);
}
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.b.x, u.b.y);
}
//SIZES
// (the drops' tuned look, ENHANCEMENT E40; not the original's)
// a segment's least squared length (m^2); the depth a surface may sit in front of the glass before it hides the drops (m)
const MIN_SEG2: f32 = 1e-12;
const GLASS_SLACK: f32 = 0.03;
// the drop's height to its coverage, the border of the glass the drops thin out over (m), the least coverage drawn
const HEIGHT_COVER: f32 = 4.0;
const EDGE_FADE: f32 = 0.015;
const MIN_INSIDE: f32 = 0.002;
// the lens blur: its radius per drop radius (pixels) held to [BLUR_MIN, BLUR_MAX]; the brightest a tap counts (float scene)
const BLUR_PER_R: f32 = 0.22;
const BLUR_MIN: f32 = 0.5;
const BLUR_MAX: f32 = 2.5;
const TAP_MAX: f32 = 24.0;
// the pixel and its 6 taps
const N_TAPS: i32 = 6;
const TAP_WEIGHTS: f32 = 7.0;
// the little lens: the rim's darkening (its curve, depth), the middle's gain and cool tint
const RIM_POW: f32 = 1.6;
const RIM_DARK: f32 = 0.55;
const MID_GAIN: f32 = 1.12;
const MID_TINT: vec3<f32> = vec3<f32>(0.018, 0.024, 0.03);
// the sky over the car (up and to the left, as a direction on the drop), its gain and the scene light's share
const LIGHT_DIR: vec2<f32> = vec2<f32>(-0.5, 0.7);
const SKY_GAIN: f32 = 1.3;
const SCENE_SHARE: f32 = 0.6;
// the highlight's radius / offset, the crescent's (outer, inner) radius / offset and its strength, the body's sky light
// (at the top, more toward the bottom), the highlight's and the crescent's shares
const HL_R: f32 = 0.42;
const CR_R: vec2<f32> = vec2<f32>(0.6, 0.1);
const CR_GAIN: f32 = 0.4;
const BODY_SKY: f32 = 0.10;
const HL_SHARE: f32 = 0.9;
const CR_SHARE: f32 = 0.45;
const TAPS = array<vec2<f32>, 6>(vec2<f32>(0.2041, 0.0), vec2<f32>(-0.2898, 0.2652), vec2<f32>(0.0302, -0.5138), vec2<f32>(0.4167, 0.4366), vec2<f32>(-0.7137, -0.1052), vec2<f32>(0.6573, -0.5909));
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(i32(u.a.x), i32(u.a.y));
    let ndc = ndc_of_px(pos.xy, u.a.xy);
    let q = u.h0.xyz * ndc.x + u.h1.xyz * ndc.y + u.h2.xyz;
    if q.z <= 0.0 { return keep(pos.xy); }
    let uv = q.xy / q.z;
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return keep(pos.xy); }
    let gw = u.inv.w;
    let gh = u.a.z;
    // the grid cell and its instances; a cell with none needs nothing else (no depth read)
    let cell = clamp(vec2<i32>(floor(uv * vec2<f32>(GRID))), vec2<i32>(0), GRID - vec2<i32>(1));
    if textureLoad(grid, vec2<i32>(cell.x * SLOTS, cell.y), 0).x == NONE { return keep(pos.xy); }
    let inv_d = u.inv.x * ndc.x + u.inv.y * ndc.y + u.inv.z;
    if inv_d <= 0.0 { return keep(pos.xy); }
    let wg = 1.0 / inv_d;
    let pm = vec2<f32>(uv.x * gw, uv.y * gh);
    var best_h = 0.0;
    var best_n = vec2<f32>(0.0);
    var best_p = vec2<f32>(0.0);
    var best_r = 0.0;
    for (var k = 0; k < SLOTS; k = k + 1) {
        let id = textureLoad(grid, vec2<i32>(cell.x * SLOTS + k, cell.y), 0).x;
        if id == NONE { break; }
        let i0 = i32(id) * 2;
        let g0 = textureLoad(inst, vec2<i32>(i0 % INST_W, i0 / INST_W), 0);
        let g1 = textureLoad(inst, vec2<i32>((i0 + 1) % INST_W, (i0 + 1) / INST_W), 0);
        let a = g0.xy;
        let ab = g0.zw - g0.xy;
        let l2 = max(dot(ab, ab), MIN_SEG2);
        let tt = clamp(dot(pm - a, ab) / l2, 0.0, 1.0);
        let off = pm - (a + ab * tt);
        let r = mix(g1.x, g1.y, tt);
        let dd = dot(off, off) / (r * r);
        if dd < 1.0 {
            let h = sqrt(1.0 - dd) * g1.z;
            if h > best_h {
                best_h = h;
                best_n = off;
                best_p = off / r;
                best_r = r;
            }
        }
    }
    if best_h <= 0.0 { return keep(pos.xy); }
    // the glass's distance here; a nearer surface (dash, pillar, mirror) is not seen through the glass
    let d0 = lin(dload(vec2<i32>(pos.xy), dims));
    if d0 < wg - GLASS_SLACK { return keep(pos.xy); }
    // the border of the glass: the drops thin out over the last 1.5 cm
    let edge = min(min(pm.x, gw - pm.x), min(pm.y, gh - pm.y));
    let inside = clamp(best_h * HEIGHT_COVER, 0.0, 1.0) * clamp(edge / EDGE_FADE, 0.0, 1.0) * u.a.w;
    if inside <= MIN_INSIDE { return keep(pos.xy); }
    // pixels per metre on the glass here
    let ppm = u.b.z / wg;
    // the lens: the point on the other side of the drop's centre, inverted and wider; screen y is down, glass v is up
    let so = vec2<f32>(-best_n.x, best_n.y) * u.b.w * ppm;
    let rpx = best_r * ppm;
    let blur = clamp(BLUR_PER_R * rpx, BLUR_MIN, BLUR_MAX);
    let c = base(pos.xy);
    // 6 taps of a fixed Vogel disc, turned per pixel by interleaved gradient noise (no trig per tap)
    let rot = ign(pos.xy) * TAU;
    let rc = vec2<f32>(cos(rot), sin(rot));
    var sum = min(c.rgb, vec3<f32>(TAP_MAX));
    for (var i = 0; i < N_TAPS; i = i + 1) {
        let v = TAPS[i];
        let sp = pos.xy + so + vec2<f32>(v.x * rc.x - v.y * rc.y, v.x * rc.y + v.y * rc.x) * blur;
        // a tap that falls off the glass (onto a pillar, the dash) is not the world behind it: it keeps the pixel's own colour
        let nt = ndc_of_px(sp, u.a.xy);
        let qt = u.h0.xyz * nt.x + u.h1.xyz * nt.y + u.h2.xyz;
        let ut = qt.xy / qt.z;
        var tc = c.rgb;
        if qt.z > 0.0 && ut.x >= 0.0 && ut.x <= 1.0 && ut.y >= 0.0 && ut.y <= 1.0 {
            tc = textureSampleLevel(t, s, (sp - u.bx.xy) / u.bx.zw, 0.0).rgb;
        }
        sum = sum + min(tc, vec3<f32>(TAP_MAX));
    }
    let wsum = TAP_WEIGHTS;
    var col = sum / wsum;
    // a drop is a little lens: the rim is dark (total reflection), the middle a touch brighter and cooler, a highlight sits up
    // and to the left (the sky over the car) and a faint bright crescent opposite it (the light that came through)
    let rim = pow(clamp(1.0 - best_h, 0.0, 1.0), RIM_POW);
    col = col * (1.0 - RIM_DARK * rim) * MID_GAIN + MID_TINT * (1.0 - rim);
    let l = normalize(LIGHT_DIR);
    let lum = dot(col, LUMA_709);
    // what they reflect: the sky over the car and, a little, the light in the scene itself (a lamp, a bright road)
    let sky = max(u.sky.xyz, vec3<f32>(0.0)) * SKY_GAIN + vec3<f32>(SCENE_SHARE * min(lum, 1.0));
    let hl = smoothstep(HL_R, 0.0, length(best_p - l * HL_R));
    let cr = smoothstep(CR_R.x, CR_R.y, length(best_p + l * CR_R.x)) * CR_GAIN;
    // the whole drop gathers a little sky light too, most toward the lower part
    let body = (1.0 - rim) * (BODY_SKY + BODY_SKY * (0.5 - 0.5 * best_p.y));
    col = col + sky * (hl * HL_SHARE + cr * CR_SHARE + body);
    return done(pos.xy, col, inside);
}
