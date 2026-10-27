// ENHANCEMENT E35: SMAA 1x on the tone-mapped (display-referred) image: luma edge detection, blending
// weights from the generated area table, neighbourhood blending. See smaa.rs for the credits
// (Jimenez, Echevarria, Sousa, Gutierrez, "SMAA: Enhanced Subpixel Morphological Antialiasing", MIT).
//
// Pixel coordinates are integers (`textureLoad`); an edge flag `.x` at P is the edge between P and
// its left neighbour, `.y` the edge between P and the pixel above. A horizontal run along the top
// edge of P ends where a vertical edge crosses it (pattern bit 0 = the crossing is in the row above,
// bit 1 = in P's row) or where the top edge stops (pattern 0).

const THRESHOLD: f32 = 0.1;
const LOCAL_CONTRAST: f32 = 2.0;
const MAX_STEPS: i32 = 32;
// SMAA_CORNER_ROUNDING 25 -> 0.25
const CORNER: f32 = 0.25;
const AREA_RUN: i32 = 33;

// ---- pass 1: edges (luma) ----
@group(0) @binding(0) var src: texture_2d<f32>;

fn luma_at(p: vec2<i32>) -> f32 {
    let d = vec2<i32>(textureDimensions(src));
    let c = textureLoad(src, clamp(p, vec2<i32>(0), d - vec2<i32>(1)), 0).rgb;
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment fn fs_edges(@builtin(position) pos: vec4<f32>) -> @location(0) vec2<f32> {
    let p = vec2<i32>(pos.xy);
    let l = luma_at(p);
    let ln = vec2<f32>(luma_at(p + vec2<i32>(-1, 0)), luma_at(p + vec2<i32>(0, -1)));
    let delta = abs(l - ln);
    var edges = step(vec2<f32>(THRESHOLD), delta);
    if edges.x + edges.y == 0.0 {
        discard;
    }
    // local contrast adaptation: an edge next to a much stronger one is not an edge
    let d2 = abs(l - vec2<f32>(luma_at(p + vec2<i32>(1, 0)), luma_at(p + vec2<i32>(0, 1))));
    let d3 = abs(ln - vec2<f32>(luma_at(p + vec2<i32>(-2, 0)), luma_at(p + vec2<i32>(0, -2))));
    let m = max(max(delta, d2), d3);
    let fin = max(m.x, m.y);
    edges = edges * step(vec2<f32>(fin), LOCAL_CONTRAST * delta);
    return edges;
}

// ---- pass 2: blending weights ----
@group(0) @binding(0) var edge_tex: texture_2d<f32>;
@group(0) @binding(1) var area_tex: texture_2d<f32>;

fn eg(p: vec2<i32>) -> vec2<f32> {
    let d = vec2<i32>(textureDimensions(edge_tex));
    if p.x < 0 || p.y < 0 || p.x >= d.x || p.y >= d.y {
        return vec2<f32>(0.0);
    }
    return textureLoad(edge_tex, p, 0).rg;
}

// (weight toward the + side, weight toward the - side) of the pixel `l` from the run's near end, `r` from
// the far end, the two end patterns; the table is stored doubled (an area is at most 0.5)
fn area(pl: u32, pr: u32, l: i32, r: i32) -> vec2<f32> {
    let row = i32(pl * 4u + pr) * AREA_RUN + min(r, AREA_RUN - 1);
    return textureLoad(area_tex, vec2<i32>(min(l, AREA_RUN - 1), row), 0).rg * 0.5;
}

@fragment fn fs_weights(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let e = eg(p);
    var w = vec4<f32>(0.0);
    if e.y > 0.5 {
        // the horizontal run along the edge between P and the pixel above
        var l = 0;
        var pl = 0u;
        for (var i = 0; i < MAX_STEPS; i++) {
            let x = p.x - l;
            let up = eg(vec2<i32>(x, p.y - 1)).x > 0.5;
            let dn = eg(vec2<i32>(x, p.y)).x > 0.5;
            if up || dn {
                pl = u32(up) | (u32(dn) << 1u);
                break;
            }
            if eg(vec2<i32>(x - 1, p.y)).y < 0.5 {
                break;
            }
            l++;
        }
        var r = 0;
        var pr = 0u;
        for (var i = 0; i < MAX_STEPS; i++) {
            let x = p.x + r + 1;
            let up = eg(vec2<i32>(x, p.y - 1)).x > 0.5;
            let dn = eg(vec2<i32>(x, p.y)).x > 0.5;
            if up || dn {
                pr = u32(up) | (u32(dn) << 1u);
                break;
            }
            if eg(vec2<i32>(x, p.y)).y < 0.5 {
                break;
            }
            r++;
        }
        var a = area(pl, pr, l, r);
        // corner rounding: a real corner (the crossing edge goes on past the next row) keeps its shape
        let lr = vec2<f32>(select(0.0, 1.0, l <= r), select(0.0, 1.0, r <= l));
        let rd = (1.0 - CORNER) * lr / (lr.x + lr.y);
        let xl = p.x - l;
        let xr = p.x + r + 1;
        let f = vec2<f32>(
            1.0 - rd.x * eg(vec2<i32>(xl, p.y + 1)).x - rd.y * eg(vec2<i32>(xr, p.y + 1)).x,
            1.0 - rd.x * eg(vec2<i32>(xl, p.y - 2)).x - rd.y * eg(vec2<i32>(xr, p.y - 2)).x,
        );
        a = a * clamp(f, vec2<f32>(0.0), vec2<f32>(1.0));
        w.r = a.x;
        w.g = a.y;
    }
    if e.x > 0.5 {
        // the vertical run along the edge between P and the pixel to its left
        var u = 0;
        var pu = 0u;
        for (var i = 0; i < MAX_STEPS; i++) {
            let y = p.y - u;
            let lc = eg(vec2<i32>(p.x - 1, y)).y > 0.5;
            let rc = eg(vec2<i32>(p.x, y)).y > 0.5;
            if lc || rc {
                pu = u32(lc) | (u32(rc) << 1u);
                break;
            }
            if eg(vec2<i32>(p.x, y - 1)).x < 0.5 {
                break;
            }
            u++;
        }
        var d = 0;
        var pd = 0u;
        for (var i = 0; i < MAX_STEPS; i++) {
            let y = p.y + d + 1;
            let lc = eg(vec2<i32>(p.x - 1, y)).y > 0.5;
            let rc = eg(vec2<i32>(p.x, y)).y > 0.5;
            if lc || rc {
                pd = u32(lc) | (u32(rc) << 1u);
                break;
            }
            if eg(vec2<i32>(p.x, y)).x < 0.5 {
                break;
            }
            d++;
        }
        var a = area(pu, pd, u, d);
        let lr = vec2<f32>(select(0.0, 1.0, u <= d), select(0.0, 1.0, d <= u));
        let rd = (1.0 - CORNER) * lr / (lr.x + lr.y);
        let yt = p.y - u;
        let yb = p.y + d + 1;
        let f = vec2<f32>(
            1.0 - rd.x * eg(vec2<i32>(p.x + 1, yt)).y - rd.y * eg(vec2<i32>(p.x + 1, yb)).y,
            1.0 - rd.x * eg(vec2<i32>(p.x - 2, yt)).y - rd.y * eg(vec2<i32>(p.x - 2, yb)).y,
        );
        a = a * clamp(f, vec2<f32>(0.0), vec2<f32>(1.0));
        w.b = a.x;
        w.a = a.y;
    }
    if dot(w, vec4<f32>(1.0)) == 0.0 {
        discard;
    }
    return w;
}

// ---- pass 3: neighbourhood blending ----
@group(0) @binding(0) var col_tex: texture_2d<f32>;
@group(0) @binding(1) var wgt_tex: texture_2d<f32>;
@group(0) @binding(2) var lin: sampler;

fn wt(p: vec2<i32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(wgt_tex));
    if p.x >= d.x || p.y >= d.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(wgt_tex, p, 0);
}

@fragment fn fs_blend(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let cur = textureLoad(wgt_tex, p, 0);
    // right = the right neighbour's .a, down = the pixel below's .g, left = .b, up = .r of this pixel
    let a = vec4<f32>(wt(p + vec2<i32>(1, 0)).a, wt(p + vec2<i32>(0, 1)).g, cur.b, cur.r);
    if dot(a, vec4<f32>(1.0)) < 1e-5 {
        return textureLoad(col_tex, p, 0);
    }
    let px = 1.0 / vec2<f32>(textureDimensions(col_tex));
    let uv = pos.xy * px;
    let h = max(a.x, a.z) > max(a.y, a.w);
    var off = vec4<f32>(0.0, a.y, 0.0, a.w);
    var w = a.yw;
    if h {
        off = vec4<f32>(a.x, 0.0, a.z, 0.0);
        w = a.xz;
    }
    w = w / (w.x + w.y);
    // bilinear filtering mixes this pixel with the chosen neighbour by the weight
    let c1 = textureSampleLevel(col_tex, lin, uv + off.xy * px, 0.0);
    let c2 = textureSampleLevel(col_tex, lin, uv - off.zw * px, 0.0);
    return w.x * c1 + w.y * c2;
}
