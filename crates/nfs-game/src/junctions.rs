//! The track's junction graph (`RE/notes/race-logic.md` §10).
//!
//! There is no junction file. `SliceTable::build` 0x480140 derives the graph from the slice geometry at
//! load time: `splitSegmentsAtCrossings` 0x480900 cuts a segment wherever another segment's **end**
//! passes within 16 m of it in plan view (leaving at least six slices on each side), and `buildNodes`
//! 0x480B40 groups the segment ends that sit within the same 16 m into junction nodes of up to eight
//! links. `<track>.jnc` only supplies the branch weights, and only when its size matches the node count
//! exactly.
//!
//! A "link" is not a record of its own: it is one of a segment's two ends, so its direction flag is free
//! — 0 for a segment's first slice, 1 for its last. Going forward you can only leave a node through a
//! link that is some segment's first slice.
//!
//! The **main lap is the longest forward-link path out of node 0** (`0x481910` / `0x4819E0`), not
//! "section 0"; that is what gives every node a lap fraction and the track its lap length.

use glam::Vec3;

/// `[0x5B47B8]` — the squared radius, in metres², that both passes use.
pub const JOIN_RADIUS_SQ: f32 = 256.0;
/// A cut must leave at least this many slices on each side (0x480A1D / 0x480A27).
pub const MIN_SEGMENT_SLACK: usize = 6;
/// `node+0x00..0x1C` is eight pointers.
pub const MAX_LINKS: usize = 8;

/// A post-split segment: a contiguous run of slices with a junction at each end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub first: usize,
    pub last: usize,
    /// index into `nodes`, if some node holds this end
    pub entry_node: Option<usize>,
    pub exit_node: Option<usize>,
    /// `link+8` per end: the branch weight, `1/n` by default and overwritten from `.jnc`
    pub weight: [f32; 2],
}

impl Segment {
    pub fn end_slice(&self, end: usize) -> usize {
        if end == 0 { self.first } else { self.last }
    }
    pub fn len(&self) -> usize {
        self.last + 1 - self.first
    }
    pub fn is_empty(&self) -> bool {
        self.last < self.first
    }
}

/// One end of one segment, as a junction sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub segment: usize,
    /// 0 = the segment's first slice (`link+0xC == 0`), 1 = its last
    pub end: usize,
}

impl Link {
    /// `link+0xC` — 0 going forward into the segment, 1 going backward.
    pub fn dir(&self) -> usize {
        self.end
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub links: Vec<Link>,
    /// `node+0x24` — the longest forward distance in slices from node 0
    pub lap_slices: i32,
    /// `node+0x28` — that over the lap length
    pub lap_fraction: f32,
    /// `node+0x2C` — the mean of the linked slices
    pub position: Vec3,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Junctions {
    pub segments: Vec<Segment>,
    pub nodes: Vec<Node>,
    /// `this+0x14` — the node index per slice, or `None`
    pub slice_node: Vec<Option<usize>>,
    /// `this+0x04` — the slice length of the main lap
    pub lap_slices: usize,
    /// `this+0x08` — the last slice of the segment whose exit fraction reaches 1.0
    pub finish_slice: usize,
    /// the segments of the main lap, in driving order — the path the longest forward walk took
    pub lap_route: Vec<usize>,
}

fn dist2_xz(a: Vec3, b: Vec3) -> f32 {
    let (dx, dz) = (a.x - b.x, a.z - b.z);
    dx * dx + dz * dz
}

impl Junctions {
    /// Build the graph from the slice centres and the `SimT` section ranges.
    pub fn build(centres: &[Vec3], sections: &[(usize, usize)]) -> Junctions {
        let mut segments: Vec<Segment> = if sections.len() > 1 {
            sections.iter().map(|&(first, last)| Segment { first, last, entry_node: None, exit_node: None, weight: [0.0; 2] }).collect()
        } else {
            vec![Segment { first: 0, last: centres.len().saturating_sub(1), entry_node: None, exit_node: None, weight: [0.0; 2] }]
        };

        // 0x480900: cut every segment that an *original* segment's end passes close to
        let originals = segments.len();
        for i in 0..originals {
            for e in 0..2 {
                let target = centres[segments[i].end_slice(e)];
                let mut j = 0;
                while j < segments.len() {
                    if j != i {
                        let (mut best, mut best_k) = (f32::MAX, None);
                        // the segment's own last slice is excluded from the scan (0x4809FC)
                        for k in segments[j].first..segments[j].last {
                            let d2 = dist2_xz(target, centres[k]);
                            if d2 < best {
                                best = d2;
                                best_k = Some(k);
                            }
                        }
                        if let Some(k) = best_k
                            && best < JOIN_RADIUS_SQ
                            && k - segments[j].first >= MIN_SEGMENT_SLACK
                            && segments[j].last - k >= MIN_SEGMENT_SLACK
                        {
                            let tail = Segment { first: k + 1, ..segments[j] };
                            segments[j].last = k;
                            segments.push(tail);
                        }
                    }
                    j += 1;
                }
            }
        }

        // 0x480B40: group the ends that share a 16 m circle, in segment then end order
        let mut owned: Vec<[bool; 2]> = vec![[false; 2]; segments.len()];
        let mut nodes: Vec<Node> = Vec::new();
        for i in 0..segments.len() {
            for e in 0..2 {
                if owned[i][e] {
                    continue;
                }
                owned[i][e] = true;
                let mut links = vec![Link { segment: i, end: e }];
                let target = centres[segments[i].end_slice(e)];
                for j in 0..segments.len() {
                    for f in 0..2 {
                        if (j == i && f == e) || owned[j][f] || links.len() >= MAX_LINKS {
                            continue;
                        }
                        if dist2_xz(target, centres[segments[j].end_slice(f)]) >= JOIN_RADIUS_SQ {
                            continue;
                        }
                        owned[j][f] = true;
                        links.push(Link { segment: j, end: f });
                    }
                }
                let position = links.iter().map(|l| centres[segments[l.segment].end_slice(l.end)]).sum::<Vec3>() / links.len() as f32;
                nodes.push(Node { links, lap_slices: 0, lap_fraction: 0.0, position });
            }
        }

        // the default weights are 1/n inside each direction class
        let mut weights: Vec<[f32; 2]> = vec![[0.0; 2]; segments.len()];
        for n in &nodes {
            for dirclass in 0..2 {
                let count = n.links.iter().filter(|l| l.dir() == dirclass).count();
                if count == 0 {
                    continue;
                }
                for l in n.links.iter().filter(|l| l.dir() == dirclass) {
                    weights[l.segment][l.end] = 1.0 / count as f32;
                }
            }
        }
        for (s, w) in segments.iter_mut().zip(weights) {
            s.weight = w;
        }

        // the entry / exit node of every segment, matched on the slice value (0x48031A)
        let node_of_slice = |nodes: &[Node], segments: &[Segment], slice: usize| -> Option<usize> {
            nodes.iter().position(|n| n.links.iter().any(|l| segments[l.segment].end_slice(l.end) == slice))
        };
        for i in 0..segments.len() {
            let (f, la) = (segments[i].first, segments[i].last);
            segments[i].entry_node = node_of_slice(&nodes, &segments, f);
            segments[i].exit_node = node_of_slice(&nodes, &segments, la);
        }
        let mut slice_node = vec![None; centres.len()];
        for (ni, n) in nodes.iter().enumerate() {
            for l in &n.links {
                let s = segments[l.segment].end_slice(l.end);
                if s < slice_node.len() {
                    slice_node[s] = Some(ni);
                }
            }
        }

        // 0x481910: the lap is the longest forward-link walk out of node 0
        let mut lap = 0i32;
        let mut walk = Walk { prev: vec![None; nodes.len()], close: None, best: 0 };
        if !nodes.is_empty() {
            let mut visiting = vec![false; nodes.len()];
            longest_forward_walk(&mut nodes, &segments, 0, &mut lap, &mut visiting, &mut walk);
        }
        // reconstruct the path the record-setting walk took, so the caller can lay the lap out in order
        let mut lap_route = Vec::new();
        if let Some((from, seg)) = walk.close {
            let mut chain = vec![seg];
            let mut at = from;
            while at != 0 {
                let Some((p, s)) = walk.prev[at] else { break };
                chain.push(s);
                at = p;
            }
            chain.reverse();
            lap_route = chain;
        } else if segments.len() == 1 {
            lap_route = vec![0];
        }
        let lap_slices = if nodes.len() <= 1 { centres.len() } else { lap.max(0) as usize };
        for n in &mut nodes {
            n.lap_fraction = if lap_slices > 0 { n.lap_slices as f32 / lap_slices as f32 } else { 0.0 };
        }
        // the finish is the last slice of the segment whose exit fraction reaches 1.0
        let finish_slice = segments
            .iter()
            .filter(|s| s.exit_node.map(|n| nodes[n].lap_fraction >= 0.999).unwrap_or(false))
            .map(|s| s.last)
            .next_back()
            .unwrap_or(centres.len().saturating_sub(1));

        Junctions { segments, nodes, slice_node, lap_slices, finish_slice, lap_route }
    }

    /// Overwrite the weights from `<track>.jnc`: `nNodes` records of `{u32 index, f32 w[8]}`, `w[k]`
    /// going to node `index`'s link `k`. A size mismatch rejects the whole file (0x481080).
    pub fn apply_jnc(&mut self, data: &[u8]) -> bool {
        if data.len() != 36 * self.nodes.len() {
            return false;
        }
        for rec in data.chunks_exact(36) {
            let idx = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]) as usize;
            let Some(node) = self.nodes.get(idx) else { continue };
            for (k, l) in node.links.iter().enumerate().take(MAX_LINKS) {
                let o = 4 + k * 4;
                let w = f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
                self.segments[l.segment].weight[l.end] = w;
            }
        }
        true
    }

    /// The node a step of `delta` slices from `slice` would cross, if any (`0x4816F0`).
    pub fn junction_within(&self, slice: usize, delta: i32) -> Option<usize> {
        let seg = self.segments.iter().position(|s| slice >= s.first && slice <= s.last)?;
        let s = slice as i32 + delta;
        let (first, last) = (self.segments[seg].first as i32, self.segments[seg].last as i32);
        if s >= first && s <= last {
            return None;
        }
        let edge = if s > last { last } else { first } as usize;
        self.slice_node[edge]
    }
}

/// Bookkeeping the original does not need: the edge that gave each node its distance, so the port can
/// lay the lap out in driving order instead of re-deriving it geometrically.
struct Walk {
    prev: Vec<Option<(usize, usize)>>,
    close: Option<(usize, usize)>,
    best: i32,
}

/// `0x4819E0` — the recursive longest forward walk that defines the lap.
fn longest_forward_walk(nodes: &mut Vec<Node>, segments: &[Segment], node: usize, out: &mut i32, visiting: &mut Vec<bool>, walk: &mut Walk) {
    if visiting[node] {
        return;
    }
    visiting[node] = true;
    for k in 0..nodes[node].links.len().min(MAX_LINKS) {
        let l = nodes[node].links[k];
        if l.dir() != 0 {
            continue;
        }
        let seg = &segments[l.segment];
        let len = seg.len() as i32;
        let Some(other) = seg.exit_node else { continue };
        if other == node {
            continue;
        }
        let total = nodes[node].lap_slices + len;
        if total <= nodes[other].lap_slices {
            continue;
        }
        if total > *out {
            *out = total;
        }
        if total > walk.best {
            walk.best = total;
            walk.close = Some((node, l.segment));
        }
        if other == 0 {
            continue;
        }
        nodes[other].lap_slices = total;
        walk.prev[other] = Some((node, l.segment));
        longest_forward_walk(nodes, segments, other, out, visiting, walk);
    }
    visiting[node] = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight line of slices 6 m apart with a branch that leaves and rejoins.
    fn line(n: usize) -> Vec<Vec3> {
        (0..n).map(|i| Vec3::new(0.0, 0.0, i as f32 * 6.0)).collect()
    }

    #[test]
    fn a_single_section_is_one_segment_and_one_node_pair() {
        let c = line(100);
        let j = Junctions::build(&c, &[(0, 99)]);
        assert_eq!(j.segments.len(), 1);
        // the two ends are 594 m apart, so they make two separate nodes
        assert_eq!(j.nodes.len(), 2);
        assert_eq!(j.lap_slices, 100, "a graph with no forks reports the whole slice count");
    }

    #[test]
    fn a_branch_that_rejoins_cuts_the_main_road_in_two_places() {
        // main road 0..99 along z; a branch 100..139 that leaves near slice 20 and rejoins near slice 80
        let mut c = line(100);
        for i in 0..40 {
            let t = i as f32 / 39.0;
            c.push(Vec3::new(20.0 * (t * std::f32::consts::PI).sin(), 0.0, (20.0 + t * 60.0) * 6.0));
        }
        let j = Junctions::build(&c, &[(0, 99), (100, 139)]);
        // the branch's two ends each cut the main road, so it becomes three segments
        assert_eq!(j.segments.len(), 4, "{:?}", j.segments.iter().map(|s| (s.first, s.last)).collect::<Vec<_>>());
        // and the two cut points become junction nodes with three links each
        let forks = j.nodes.iter().filter(|n| n.links.len() >= 3).count();
        assert_eq!(forks, 2, "{:?}", j.nodes.iter().map(|n| n.links.len()).collect::<Vec<_>>());
        // the lap follows the longest forward path, which is the main road
        assert!(j.lap_slices >= 100, "lap {} should take the long way round", j.lap_slices);
        // the default weights split each direction class evenly
        let node = j.nodes.iter().find(|n| n.links.len() == 3).unwrap();
        let forward: Vec<f32> = node.links.iter().filter(|l| l.dir() == 0).map(|l| j.segments[l.segment].weight[l.end]).collect();
        assert!(forward.iter().all(|w| (*w - 1.0 / forward.len() as f32).abs() < 1e-6), "{forward:?}");
    }

    #[test]
    fn a_jnc_of_the_wrong_size_is_rejected_whole() {
        let c = line(100);
        let mut j = Junctions::build(&c, &[(0, 99)]);
        assert!(!j.apply_jnc(&[0u8; 35]));
        let mut good = vec![0u8; 36 * j.nodes.len()];
        for (i, rec) in good.chunks_exact_mut(36).enumerate() {
            rec[..4].copy_from_slice(&(i as u32).to_le_bytes());
            rec[4..8].copy_from_slice(&0.85f32.to_le_bytes());
        }
        assert!(j.apply_jnc(&good));
        assert_eq!(j.segments[0].weight[0], 0.85);
    }
}
