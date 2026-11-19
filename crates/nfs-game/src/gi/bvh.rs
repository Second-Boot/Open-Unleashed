//! The bounding volume hierarchy over the occluders (the bake's ray tracer; `bake_light`'s diagnostic uses it too).
use super::Occluders;
use glam::Vec3;

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
                let Some((t, u, v)) = nfs_math::ray::moller_trumbore(o, d, a, e1, e2, 1e-12) else { continue };
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
