//! ENHANCEMENT E32 baked bounce light ("глобальное освещение"), branch `remaster` — NOT the original.
//!
//! The original's track light is baked into the CRP vertex colours (`RE/notes/lighting.md` §5.4: a
//! track item is texture × vertex colour, nothing else), and nobody knows what EA's tool put there
//! (direct sun + some ambient, perhaps a radiosity pass). So this does **not** recompute the
//! lighting. It bounces the original's own picture once: from every track vertex rays go over the
//! hemisphere of its normal (cosine weighted), and a ray that hits track geometry brings back what
//! the original *shows* at that point — the hit triangle's texture mean × its interpolated vertex
//! colour. The mean of those over the hemisphere (misses bring nothing: the sky's share is already
//! in the baked colour) is the light the surroundings throw onto the vertex: green next to grass,
//! warm next to sandstone, the wall's colour at the road's edge. It is **added** to the vertex
//! colour (`colour + k · bounce`), so it brightens and tints where there is something to bounce
//! from and leaves an open road under the sky nearly as it was. A second, chroma-only term moves a
//! vertex the sun does not reach (our own shadow ray) and the original also keeps dark toward the
//! sky's colour by how much sky it sees ("blue fill in shadow"); its luminance is kept.
//!
//! The bake is the CPU's (std threads, a hand-rolled BVH over the track's world articles at their
//! finest LOD): `bake`. The result is cached per track in the data dir (`gi/<track>.bin`,
//! versioned, keyed by a hash of the inputs, rebuilt when missing or stale): [`load_or_bake`].
//! [`shade`] combines a vertex's original colour with its sample at a level. Everything here is
//! render-only: nothing reaches the simulation, the network or the replays.

use glam::Vec3;
use std::path::{Path, PathBuf};

/// The cache file's format version (bump when the bake or the file changes).
pub const VERSION: u32 = 1;
/// Rays per vertex over the hemisphere.
pub const RAYS: u32 = 256;
/// How far a bounce ray looks (metres): light from farther away is the sky's business.
pub const REACH: f32 = 60.0;

/// One receiving vertex's bake result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// the mean radiance the hemisphere's hits bring back (rgb, the original's display units)
    pub bounce: [f32; 3],
    /// the fraction of the hemisphere's rays that leave upward unhit (0..1)
    pub sky: f32,
    /// how much of the sun reaches the vertex (0..1; 0 on a track without a sun)
    pub sun: f32,
}

/// What the bake traces: the track's static geometry in world space.
#[derive(Default, Clone)]
pub struct Occluders {
    pub pos: Vec<Vec3>,
    /// the displayed colour at each vertex (texture mean × vertex colour)
    pub col: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// how much of a triangle stops a ray (its texture's mean alpha for a cut-out / blended material, else 1)
    pub cover: Vec<f32>,
}

/// The bake's input: the occluders, the receiving vertices (world position and normal) and the light.
#[derive(Default, Clone)]
pub struct Input {
    pub occ: Occluders,
    pub recv_pos: Vec<Vec3>,
    pub recv_nrm: Vec<Vec3>,
    /// unit vector toward the sun, `None` on a track without one (sky-only fill)
    pub to_sun: Option<Vec3>,
}

impl Input {
    /// A hash of everything the result depends on (the cache key).
    pub fn hash(&self) -> u64 {
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        h.u32(VERSION);
        h.u32(RAYS);
        h.f32(REACH);
        for p in &self.occ.pos {
            h.vec(*p);
        }
        for c in &self.occ.col {
            h.vec(*c);
        }
        for t in &self.occ.tris {
            t.iter().for_each(|&i| h.u32(i));
        }
        for c in &self.occ.cover {
            h.f32(*c);
        }
        for (p, n) in self.recv_pos.iter().zip(&self.recv_nrm) {
            h.vec(*p);
            h.vec(*n);
        }
        h.u32(self.recv_pos.len() as u32);
        if let Some(s) = self.to_sun {
            h.vec(s);
        }
        h.0
    }
}

struct Fnv(u64);
impl Fnv {
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn vec(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
}

// ---------------------------------------------------------------------------------------------
// BVH

#[derive(Clone, Copy)]
struct Node {
    lo: [f32; 3],
    /// a leaf's first triangle, or an inner node's right child (the left one follows it)
    first: u32,
    hi: [f32; 3],
    /// a leaf's triangle count; 0 for an inner node
    count: u32,
}

/// A bounding volume hierarchy over the occluders' triangles (median split on the widest axis of
/// the centroids, leaves of up to 4).
pub struct Bvh {
    nodes: Vec<Node>,
    /// triangle indices in leaf order
    order: Vec<u32>,
    /// per triangle in leaf order: v0, e1, e2
    tri: Vec<[Vec3; 3]>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(occ: &Occluders) -> Self {
        let n = occ.tris.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let cent: Vec<Vec3> = occ.tris.iter().map(|t| (occ.pos[t[0] as usize] + occ.pos[t[1] as usize] + occ.pos[t[2] as usize]) / 3.0).collect();
        let tb: Vec<(Vec3, Vec3)> = occ
            .tris
            .iter()
            .map(|t| {
                let (a, b, c) = (occ.pos[t[0] as usize], occ.pos[t[1] as usize], occ.pos[t[2] as usize]);
                (a.min(b).min(c), a.max(b).max(c))
            })
            .collect();
        let mut nodes = Vec::with_capacity(2 * n / LEAF + 1);
        if n > 0 {
            Self::split(&mut nodes, &mut order, 0, &cent, &tb);
        }
        let tri = order
            .iter()
            .map(|&k| {
                let t = occ.tris[k as usize];
                let a = occ.pos[t[0] as usize];
                [a, occ.pos[t[1] as usize] - a, occ.pos[t[2] as usize] - a]
            })
            .collect();
        Bvh { nodes, order, tri }
    }

    fn split(nodes: &mut Vec<Node>, order: &mut [u32], base: usize, cent: &[Vec3], tb: &[(Vec3, Vec3)]) -> usize {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &k in order.iter() {
            lo = lo.min(tb[k as usize].0);
            hi = hi.max(tb[k as usize].1);
            clo = clo.min(cent[k as usize]);
            chi = chi.max(cent[k as usize]);
        }
        let me = nodes.len();
        nodes.push(Node { lo: lo.to_array(), first: base as u32, hi: hi.to_array(), count: order.len() as u32 });
        let ext = chi - clo;
        if order.len() <= LEAF || ext.max_element() <= 0.0 {
            return me;
        }
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        let mid = order.len() / 2;
        order.select_nth_unstable_by(mid, |a, b| cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis]));
        let (l, r) = order.split_at_mut(mid);
        Self::split(nodes, l, base, cent, tb);
        let right = Self::split(nodes, r, base + mid, cent, tb);
        nodes[me].first = right as u32;
        nodes[me].count = 0;
        me
    }

    /// The nearest hit along `o + t·d` for `t` in `(tmin, tmax)` whose triangle `accept`s it:
    /// `(t, triangle, u, v)` with `u`, `v` the barycentrics of corners 1 and 2.
    pub fn hit(&self, o: Vec3, d: Vec3, tmin: f32, mut tmax: f32, accept: &dyn Fn(u32) -> bool) -> Option<(f32, u32, f32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        // a zero component would give 0 · inf = NaN in the slab test
        let nz = |x: f32| if x.abs() < 1e-12 { 1e-12f32.copysign(x) } else { x };
        let inv = Vec3::new(1.0 / nz(d.x), 1.0 / nz(d.y), 1.0 / nz(d.z));
        let mut best = None;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            // the slab test
            let t0 = (Vec3::from(node.lo) - o) * inv;
            let t1 = (Vec3::from(node.hi) - o) * inv;
            let near = t0.min(t1).max_element().max(tmin);
            let far = t0.max(t1).min_element().min(tmax);
            if near > far {
                continue;
            }
            if node.count == 0 {
                let me = stack[sp] as usize;
                if sp + 2 > stack.len() {
                    continue;
                }
                stack[sp] = node.first;
                stack[sp + 1] = me as u32 + 1;
                sp += 2;
                continue;
            }
            for i in node.first..node.first + node.count {
                let [a, e1, e2] = self.tri[i as usize];
                // Möller–Trumbore, both faces
                let p = d.cross(e2);
                let det = e1.dot(p);
                if det.abs() < 1e-12 {
                    continue;
                }
                let id = 1.0 / det;
                let s = o - a;
                let u = s.dot(p) * id;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = s.cross(e1);
                let v = d.dot(q) * id;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let t = e2.dot(q) * id;
                if t > tmin && t < tmax {
                    let tri = self.order[i as usize];
                    if accept(tri) {
                        tmax = t;
                        best = Some((t, tri, u, v));
                    }
                }
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------------------------
// the bake

fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn unit(x: u32) -> f32 {
    (x >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// An orthonormal frame around `n`.
fn frame(n: Vec3) -> (Vec3, Vec3) {
    let s = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    (Vec3::new(1.0 + s * n.x * n.x * a, s * b, -s * n.x), Vec3::new(b, s + n.y * n.y * a, -n.y))
}

/// Bake every receiving vertex (all threads). Deterministic: the same input gives the same result.
pub fn bake(input: &Input) -> Vec<Sample> {
    let bvh = Bvh::build(&input.occ);
    let n = input.recv_pos.len();
    let mut out = vec![Sample::default(); n];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).max(1);
    let chunk = n.div_ceil(threads * 8).max(256);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<&mut [Sample]>> = out.chunks_mut(chunk).map(std::sync::Mutex::new).collect();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let c = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(slot) = slots.get(c) else { break };
                    let mut part = slot.lock().expect("bake chunk");
                    for (j, s) in part.iter_mut().enumerate() {
                        *s = bake_vertex(input, &bvh, c * chunk + j);
                    }
                }
            });
        }
    });
    out
}

fn bake_vertex(input: &Input, bvh: &Bvh, i: usize) -> Sample {
    let occ = &input.occ;
    let p = input.recv_pos[i];
    let mut n = input.recv_nrm[i];
    if !n.is_finite() || n.length_squared() < 1e-6 {
        n = Vec3::Y;
    }
    let n = n.normalize();
    let (t1, t2) = frame(n);
    let o = p + n * 0.05;
    let seed = hash32(i as u32 ^ 0x9e37_79b9);
    // a cut-out triangle stops a ray with the probability of its coverage (a per-ray, per-triangle hash)
    let accept = |ray: u32| {
        move |tri: u32| {
            let c = occ.cover[tri as usize];
            c >= 1.0 || unit(hash32(tri.wrapping_mul(0x2545_f491) ^ ray)) < c
        }
    };
    let mut sum = Vec3::ZERO;
    let mut sky = 0u32;
    // stratified cosine-weighted directions (a 16 x 16 grid), rotated per vertex
    let side = (RAYS as f32).sqrt() as u32;
    let rot = unit(seed) * std::f32::consts::TAU;
    for k in 0..RAYS {
        let h = hash32(seed ^ k.wrapping_mul(0x68e3_1da4));
        let (gx, gy) = (k % side, k / side);
        let u1 = (gx as f32 + unit(h)) / side as f32;
        let u2 = (gy as f32 + unit(hash32(h))) / side as f32;
        let r = u1.sqrt();
        let phi = std::f32::consts::TAU * u2 + rot;
        let d = (t1 * (r * phi.cos()) + t2 * (r * phi.sin()) + n * (1.0 - u1).max(0.0).sqrt()).normalize();
        let ray = seed ^ k;
        match bvh.hit(o, d, 0.02, REACH, &accept(ray)) {
            Some((_, tri, u, v)) => {
                let t = occ.tris[tri as usize];
                let c = occ.col[t[0] as usize] * (1.0 - u - v) + occ.col[t[1] as usize] * u + occ.col[t[2] as usize] * v;
                sum += c;
            }
            None => {
                if d.y > 0.0 {
                    sky += 1;
                }
            }
        }
    }
    let mut sun = 0.0;
    if let Some(s) = input.to_sun {
        if n.dot(s) > 0.0 {
            // four rays inside a ~1.5 degree cone: a soft edge at vertex density anyway
            let (s1, s2) = frame(s);
            let mut lit = 0;
            for k in 0..4u32 {
                let h = hash32(seed ^ 0xabcd ^ k);
                let a = unit(h) * std::f32::consts::TAU;
                let r = 0.026 * unit(hash32(h)).sqrt();
                let d = (s + s1 * (r * a.cos()) + s2 * (r * a.sin())).normalize();
                if bvh.hit(o, d, 0.02, 2000.0, &accept(seed ^ 0x5555 ^ k)).is_none() {
                    lit += 1;
                }
            }
            sun = lit as f32 / 4.0;
        }
    }
    let b = sum / RAYS as f32;
    Sample { bounce: b.to_array(), sky: sky as f32 / RAYS as f32, sun }
}

// ---------------------------------------------------------------------------------------------
// the combination

/// How a level combines a vertex's original colour with its sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// the bounce's weight (`colour + k · bounce`)
    pub bounce: f32,
    /// the strength of the sky's colour in a vertex in shadow
    pub sky: f32,
}

/// The level's parameters: 0 off, 1 weak (слабо), 2 medium (средне).
pub fn params(level: u8) -> Option<Params> {
    match level {
        0 => None,
        1 => Some(Params { bounce: 0.6, sky: 0.35 }),
        _ => Some(Params { bounce: 1.0, sky: 0.6 }),
    }
}

/// What [`shade`] needs of the track as a whole.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// the sky's colour, luminance 1 (the hue a shadow takes)
    pub sky_chroma: Vec3,
    /// the luminance under which a baked colour counts as the original's own shadow
    pub dark: f32,
    /// whether the track has a sun (on a sunless one every vertex takes `SUNLESS_FILL` of the fill)
    pub sun: bool,
    /// the receivers' median luminance: the bounce is taken relative to it
    pub median: f32,
    /// the receivers' mean bounce (rgb): half of it is the level a bounce must exceed to brighten
    pub mean_bounce: Vec3,
}

fn luma(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.299, 0.587, 0.114))
}

/// The track's context: the sky colour as a hue (held within a moderate range so a strongly
/// coloured sky does not paint the track) and the dark limit from the receivers' baked colours.
pub fn context(sky: Vec3, colours: &[[f32; 4]], samples: &[Sample], sun: bool) -> Context {
    let l = luma(sky).max(1e-3);
    let sky_chroma = (sky / l).clamp(Vec3::splat(0.75), Vec3::splat(1.3));
    let sky_chroma = sky_chroma / luma(sky_chroma).max(1e-3);
    let mut ls: Vec<f32> = colours.iter().map(|c| luma(Vec3::new(c[0], c[1], c[2]))).collect();
    let median = if ls.is_empty() {
        0.6
    } else {
        let m = ls.len() / 2;
        ls.select_nth_unstable_by(m, f32::total_cmp);
        ls[m]
    };
    let mean_bounce = samples.iter().fold(Vec3::ZERO, |a, s| a + Vec3::from(s.bounce)) / samples.len().max(1) as f32;
    Context { sky_chroma, dark: median * 0.8, sun, median, mean_bounce }
}

/// The new colour of a vertex: the original's `c` times `1 + k · (bounce − mean / 2) / median` per channel
/// (held between [`FLOOR`] and `1 + CAP · k`), its shade moved toward the sky's hue. Multiplying keeps the original's contrast
/// (a baked shadow stays as dark relative to its surroundings; an added grey would wash it out), the half
/// mean taken off makes the change follow *where* light bounces rather than lift the whole track.
pub fn shade(c: [f32; 4], s: &Sample, p: &Params, ctx: &Context) -> [f32; 4] {
    let v = Vec3::new(c[0], c[1], c[2]);
    // the sky's hue where our sun ray is blocked AND the original keeps the vertex dark (a shadow both
    // agree on), by how much sky the vertex sees; on a sunless track half of it everywhere
    let shadow = if ctx.sun { (1.0 - s.sun) * (1.0 - smooth(ctx.dark * 0.6, ctx.dark, luma(v))) } else { SUNLESS_FILL };
    let t = (p.sky * shadow * s.sky).clamp(0.0, 1.0);
    let tinted = v * Vec3::ONE.lerp(ctx.sky_chroma, t);
    let tinted = tinted * (luma(v) / luma(tinted).max(1e-4));
    let rel = (Vec3::from(s.bounce) - ctx.mean_bounce * 0.5) / ctx.median.max(0.2);
    let out = tinted * (Vec3::ONE + rel * p.bounce).clamp(Vec3::splat(FLOOR), Vec3::splat(1.0 + CAP * p.bounce));
    [out.x, out.y, out.z, c[3]]
}

/// The sky fill's share everywhere on a track without a sun (overcast: the sky is the light)
const SUNLESS_FILL: f32 = 0.25;
/// The least a vertex keeps of its colour (a spot with less bounce than half the mean darkens this far at most)
const FLOOR: f32 = 0.97;
/// The most a vertex gains per unit of the bounce weight (a column standing on a bright road would glow otherwise)
const CAP: f32 = 0.3;

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------------------------
// the track's input

/// One receiving part: a world article's mesh at one LOD; vertex `j` takes sample `samples[j]`.
pub struct Part {
    /// the article's index in the `.crp` (`ArticleMeshes::index`)
    pub article: usize,
    /// the LOD's position in `ArticleMeshes::lods`, the part's position in that LOD's meshes
    pub lod: usize,
    pub part: usize,
    /// per vertex its welded receiver (the meshes are unindexed triangles: the corners that meet at a
    /// point with nearly the same normal share one sample, so no facet shows the noise of its own rays)
    pub samples: Vec<u32>,
    /// the original mesh (its colours are what [`shade`] starts from)
    pub mesh: nfs_renderer::MeshData,
}

/// A texture's mean colour (weighted by the texels that pass the alpha test) and its coverage
/// (the share of texels the test keeps), from a small mip.
fn texture_mean(t: &nfs_assets::texture::TextureImage) -> (Vec3, f32) {
    let (mut w, mut h) = (t.width.max(1), t.height.max(1));
    let mut level = 0;
    while level + 1 < t.mips.len() && w * h > 4096 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        level += 1;
    }
    let px = &t.mips[level];
    let (mut sum, mut kept, mut all) = (Vec3::ZERO, 0u32, 0u32);
    for p in px.chunks_exact(4) {
        all += 1;
        if p[3] > 10 {
            kept += 1;
            sum += Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0;
        }
    }
    if kept == 0 {
        return (Vec3::splat(0.5), 0.0);
    }
    (sum / kept as f32, kept as f32 / all.max(1) as f32)
}

/// The bake's input for a day track: every world article but `skip`'s (the ones drawn apart: the
/// breakables', the smackables', the animated) — occluders at their finest LOD, receivers at every
/// LOD. A blended material neither occludes nor receives (decals, ground shadows, glass); a flat
/// `Am` material (its colour is a block, not light) and a scrolling one (water, flags) do not
/// receive; a cut-out (a texture with alpha under the alpha test) occludes by its coverage.
pub fn build(track: &nfs_assets::track::TrackAssets, geometry: &nfs_assets::track::TrackGeometry, skip: &dyn Fn(&nfs_assets::track::ArticleMeshes) -> bool, to_sun: Option<Vec3>) -> (Input, Vec<Part>) {
    let materials = track.materials().unwrap_or_default();
    let methods = track.crp.render_methods(&track.crp_data).unwrap_or_default();
    let flat = nfs_assets::track::ambient_colours(&track.crp, &track.crp_data);
    let means: std::collections::HashMap<String, (Vec3, f32, bool)> = track.textures.iter().filter(|t| !t.mips.is_empty()).map(|t| {
        let (c, a) = texture_mean(t);
        (t.name.clone(), (c, a, t.has_alpha))
    }).collect();
    // per material: (albedo, coverage, occludes, receives)
    let class = |id: u16| -> (Vec3, f32, bool, bool) {
        let Some(m) = materials.get(id as usize) else { return (Vec3::splat(0.5), 1.0, true, false) };
        let (albedo, cover, has_alpha) = means.get(&m.texture_name()).copied().unwrap_or((Vec3::splat(0.5), 1.0, false));
        let rs = nfs_assets::track::render_states(&methods, m);
        let blend = has_alpha && rs.map_or(m.render_method != "Opaque", |rs| rs[1] == 2);
        let scrolls = nfs_assets::track::uv_animate(&methods, m).is_some();
        let cover = if has_alpha { cover } else { 1.0 };
        (albedo, cover, !blend, !blend && !flat.contains_key(&id) && !scrolls)
    };
    let mut input = Input { to_sun, ..Input::default() };
    let mut parts = Vec::new();
    // the welding: 1 cm cells of the world position, and within a cell the receivers by normal
    let mut weld: std::collections::HashMap<(i32, i32, i32), Vec<u32>> = std::collections::HashMap::new();
    let mut nsum: Vec<Vec3> = Vec::new();
    for art in geometry.world.iter().filter(|a| !skip(a)) {
        let xf = art.transform;
        for (l, (_, meshes)) in art.lods.iter().enumerate() {
            for (k, mm) in meshes.iter().enumerate() {
                let (albedo, cover, occludes, receives) = class(mm.material);
                if l == 0 && occludes && cover > 0.0 {
                    let base = input.occ.pos.len() as u32;
                    for v in &mm.mesh.vertices {
                        input.occ.pos.push(xf.transform_point3(Vec3::from(v.position)));
                        input.occ.col.push(albedo * Vec3::new(v.color[0], v.color[1], v.color[2]));
                    }
                    for t in mm.mesh.indices.chunks_exact(3) {
                        input.occ.tris.push([base + t[0], base + t[1], base + t[2]]);
                        input.occ.cover.push(cover);
                    }
                }
                if receives && !mm.mesh.vertices.is_empty() {
                    let samples = mm
                        .mesh
                        .vertices
                        .iter()
                        .map(|v| {
                            let p = xf.transform_point3(Vec3::from(v.position));
                            let n = xf.transform_vector3(Vec3::from(v.normal)).normalize_or_zero();
                            let key = (p * 100.0).round().as_ivec3();
                            let cell = weld.entry((key.x, key.y, key.z)).or_default();
                            if let Some(&u) = cell.iter().find(|&&u| nsum[u as usize].normalize_or_zero().dot(n) > WELD_COS) {
                                nsum[u as usize] += n;
                                u
                            } else {
                                let u = input.recv_pos.len() as u32;
                                input.recv_pos.push(p);
                                nsum.push(n);
                                cell.push(u);
                                u
                            }
                        })
                        .collect();
                    parts.push(Part { article: art.index, lod: l, part: k, samples, mesh: mm.mesh.clone() });
                }
            }
        }
    }
    input.recv_nrm = nsum.into_iter().map(|n| n.normalize_or_zero()).collect();
    (input, parts)
}

/// Corners at one point whose normals are closer than this cosine (about 30 degrees) share a sample.
const WELD_COS: f32 = 0.85;

/// The sky's colour for the fill: the main view's clear colour at the start (`SkyBin::background_at`,
/// detail 0), else a neutral light blue.
pub fn sky_colour(track: &nfs_assets::track::TrackAssets) -> Vec3 {
    track.sky_bin.as_ref().and_then(|b| b.background_at(0, 0, 0, 0)).map(Vec3::from).unwrap_or(Vec3::new(0.6, 0.7, 0.85))
}

/// The unit vector toward the sun, `None` on a night track or one whose sun is straight overhead
/// and off (`TrackWorld::sun_direction` = straight down: autobahn, canyon, foothills, forest).
pub fn to_sun(track: &nfs_assets::track::TrackAssets) -> Option<Vec3> {
    if track.is_night() {
        return None;
    }
    let d = track.sky_bin.as_ref().map(|b| -Vec3::from(b.sun_position()).normalize_or_zero())?;
    (d.x.abs() + d.z.abs() > 1e-3).then_some(-d)
}

// ---------------------------------------------------------------------------------------------
// the live state (the track world's)

/// A receiving part resolved to its draw item: `TrackWorld::track_lods[g].lods[l].1[k]`.
pub struct Target {
    pub g: usize,
    pub l: usize,
    pub k: usize,
    pub part: Part,
    pub original: nfs_renderer::MeshId,
    /// the recoloured copy, uploaded at the first use
    pub variant: Option<nfs_renderer::MeshId>,
}

/// E32's state on a track: the input (until baked), the targets, the samples, what is applied.
pub struct State {
    pub track: String,
    pub input: Option<std::sync::Arc<Input>>,
    pub targets: Vec<Target>,
    pub sky: Vec3,
    pub sun: bool,
    pub samples: Option<Vec<Sample>>,
    pub pending: Option<std::thread::JoinHandle<(Vec<Sample>, Option<f32>)>>,
    /// the level whose colours the variants hold (0 = none yet)
    pub level: u8,
    /// whether the draw items point at the variants
    pub active: bool,
}

impl State {
    /// At the track's load with the switch on: the cache, or the bake right here (the loading screen waits
    /// for it once per track; a release build takes a few seconds), so the race starts lit.
    pub fn prepare(&mut self) {
        if self.samples.is_some() || self.pending.is_some() {
            return;
        }
        let Some(input) = self.input.take() else { return };
        let (s, secs) = load_or_bake(&self.track, &input);
        match secs {
            Some(secs) => tracing::info!(track = %self.track, receivers = s.len(), triangles = input.occ.tris.len(), seconds = secs, "ENHANCEMENT E32: bounce light baked"),
            None => tracing::info!(track = %self.track, receivers = s.len(), "ENHANCEMENT E32: bounce light from the cache"),
        }
        self.samples = Some(s);
    }

    /// The samples, once they are there: the cache is read at the first call (on this thread, so a
    /// valid cache is in from the first frame), a bake runs on its own thread and is picked up when done.
    pub fn poll(&mut self) -> Option<&[Sample]> {
        if self.samples.is_none() {
            if let Some(h) = self.pending.take_if(|h| h.is_finished()) {
                match h.join() {
                    Ok((s, secs)) => {
                        tracing::info!(track = %self.track, vertices = s.len(), seconds = secs.unwrap_or(0.0), "ENHANCEMENT E32: bounce light baked");
                        self.samples = Some(s);
                    }
                    Err(_) => tracing::warn!("ENHANCEMENT E32: the bake failed"),
                }
                self.input = None;
            } else if self.pending.is_none() {
                let input = self.input.clone()?;
                let path = cache_dir().join(format!("{}.bin", self.track.to_lowercase()));
                let t = std::time::Instant::now();
                if let Some(s) = read(&path, input.hash(), input.recv_pos.len()) {
                    tracing::info!(track = %self.track, vertices = s.len(), ms = t.elapsed().as_millis() as u64, "ENHANCEMENT E32: bounce light from the cache");
                    self.samples = Some(s);
                    self.input = None;
                } else {
                    let name = self.track.clone();
                    tracing::info!(track = %name, receivers = input.recv_pos.len(), triangles = input.occ.tris.len(), "ENHANCEMENT E32: baking the bounce light");
                    self.pending = Some(std::thread::spawn(move || load_or_bake(&name, &input)));
                }
            }
        }
        self.samples.as_deref()
    }
}

// ---------------------------------------------------------------------------------------------
// the cache

/// The cache's directory: `NFS_GI_DIR`, else `gi/` in `NFS5_SAVE_DIR` (a scripted run), else in the
/// port's data dir (next to `enhancements.json`).
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NFS_GI_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("NFS5_SAVE_DIR").map(PathBuf::from).unwrap_or_else(nfs_assets::platform::data_dir).join("gi")
}

const MAGIC: &[u8; 4] = b"NGI1";

/// The file: magic, version, the input hash, the count, then per sample 5 f32 (LE).
pub fn write(path: &Path, hash: u64, samples: &[Sample]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(20 + samples.len() * 20);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&hash.to_le_bytes());
    b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for s in samples {
        for x in [s.bounce[0], s.bounce[1], s.bounce[2], s.sky, s.sun] {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &b)?;
    std::fs::rename(&tmp, path)
}

/// The cached samples when the file is there, of this version and of this input.
pub fn read(path: &Path, hash: u64, count: usize) -> Option<Vec<Sample>> {
    let b = std::fs::read(path).ok()?;
    let u32_at = |o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().expect("4 bytes")));
    if b.get(0..4)? != MAGIC || u32_at(4)? != VERSION || u64::from_le_bytes(b.get(8..16)?.try_into().ok()?) != hash || u32_at(16)? as usize != count || b.len() != 20 + count * 20 {
        return None;
    }
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"));
    Some((0..count).map(|i| {
        let o = 20 + i * 20;
        Sample { bounce: [f(o), f(o + 4), f(o + 8)], sky: f(o + 12), sun: f(o + 16) }
    }).collect())
}

/// The track's samples: the cache when it is valid, else a bake (written to the cache). Returns the
/// samples and the bake's time in seconds (`None` = read from the cache).
pub fn load_or_bake(track: &str, input: &Input) -> (Vec<Sample>, Option<f32>) {
    let hash = input.hash();
    let path = cache_dir().join(format!("{}.bin", track.to_lowercase()));
    if let Some(s) = read(&path, hash, input.recv_pos.len()) {
        return (s, None);
    }
    let t = std::time::Instant::now();
    let s = bake(input);
    let secs = t.elapsed().as_secs_f32();
    if let Err(e) = write(&path, hash, &s) {
        tracing::warn!(path = %path.display(), error = %e, "ENHANCEMENT E32: the bake could not be cached");
    }
    (s, Some(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor (y = 0, 20 x 20 m, grey) and a green wall 2 m high (x = 1).
    fn floor_and_wall() -> Occluders {
        let mut o = Occluders::default();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3, col: Vec3| {
            let k = o.pos.len() as u32;
            o.pos.extend([a, b, c, d]);
            o.col.extend([col; 4]);
            o.tris.push([k, k + 1, k + 2]);
            o.tris.push([k, k + 2, k + 3]);
            o.cover.extend([1.0, 1.0]);
        };
        quad(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(-10.0, 0.0, 10.0), Vec3::splat(0.5));
        quad(Vec3::new(1.0, 0.0, -10.0), Vec3::new(1.0, 2.0, -10.0), Vec3::new(1.0, 2.0, 10.0), Vec3::new(1.0, 0.0, 10.0), Vec3::new(0.0, 0.8, 0.0));
        o
    }

    #[test]
    fn a_floor_next_to_a_green_wall_takes_green() {
        let input = Input { occ: floor_and_wall(), recv_pos: vec![Vec3::new(0.8, 0.0, 0.0), Vec3::new(-8.0, 0.0, 0.0)], recv_nrm: vec![Vec3::Y; 2], to_sun: Some(Vec3::new(1.0, 1.0, 0.0).normalize()) };
        let s = bake(&input);
        // next to the wall: a lot of green, little else; the sun (behind the wall) is blocked
        assert!(s[0].bounce[1] > 0.15 && s[0].bounce[0] < 0.01, "{:?}", s[0]);
        assert_eq!(s[0].sun, 0.0);
        // far from it: little bounce, the sun reaches
        assert!(s[1].bounce[1] < s[0].bounce[1] * 0.3, "{:?}", s[1]);
        assert_eq!(s[1].sun, 1.0);
        assert!(s[1].sky > 0.8);
        // deterministic
        assert_eq!(bake(&input), s);
    }

    #[test]
    fn the_shade_takes_the_bounce_and_keeps_alpha() {
        let ctx = Context { sky_chroma: Vec3::new(0.9, 1.0, 1.2), dark: 0.4, sun: true, median: 0.5, mean_bounce: Vec3::ZERO };
        let p = params(2).expect("medium");
        let s = Sample { bounce: [0.0, 0.2, 0.0], sky: 0.5, sun: 1.0 };
        let c = shade([0.5, 0.5, 0.5, 0.7], &s, &p, &ctx);
        assert!((c[1] - 0.65).abs() < 1e-5 && (c[0] - 0.5).abs() < 1e-5 && c[3] == 0.7, "{c:?}");
        // in the sun with no bounce: unchanged
        let s = Sample { bounce: [0.0; 3], sky: 1.0, sun: 1.0 };
        assert_eq!(shade([0.3, 0.4, 0.5, 1.0], &s, &p, &ctx), [0.3, 0.4, 0.5, 1.0]);
    }

    #[test]
    fn the_cache_round_trips_and_rejects_a_stale_file() {
        let d = std::env::temp_dir().join(format!("gi_test_{}", std::process::id()));
        let p = d.join("t.bin");
        let s = vec![Sample { bounce: [0.1, 0.2, 0.3], sky: 0.4, sun: 0.5 }; 3];
        write(&p, 42, &s).expect("write");
        assert_eq!(read(&p, 42, 3), Some(s));
        assert_eq!(read(&p, 43, 3), None);
        assert_eq!(read(&p, 42, 4), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
