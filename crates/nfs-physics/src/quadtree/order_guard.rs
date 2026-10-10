//! Guard for the lazy segment walk: the recursive query it replaced (kept as
//! `QuadTree::query_segment_reference`) against `SegmentIter` on random trees — same entries, same
//! order, including segments that stop exactly on a node's centre line.

use super::*;

struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 >> 8
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 20_000) as f32 / 10_000.0 - 1.0
    }
}

struct Boxed {
    c: (f32, f32),
    e: ((f32, f32), (f32, f32)),
}
impl Entry for Boxed {
    fn centroid(&self) -> (f32, f32) {
        self.c
    }
    fn extent(&self) -> ((f32, f32), (f32, f32)) {
        self.e
    }
}

fn random_entries(r: &mut Lcg, n: usize, spread: f32) -> Vec<Boxed> {
    // a few clusters so the tree goes deep in places, plus scattered entries and big straddlers
    let clusters: Vec<(f32, f32)> = (0..4).map(|_| (r.unit() * 3000.0, r.unit() * 3000.0)).collect();
    (0..n)
        .map(|i| {
            let (ox, oz) = clusters[i % 4];
            let (x, z) = if i % 7 == 0 { (r.unit() * 6000.0, r.unit() * 6000.0) } else { (ox + r.unit() * spread, oz + r.unit() * spread) };
            let (w, h) = if i % 11 == 0 { (r.unit().abs() * 300.0, r.unit().abs() * 300.0) } else { (r.unit().abs() * 2.0, r.unit().abs() * 2.0) };
            Boxed { c: (x, z), e: ((x - w, z - h), (x + w, z + h)) }
        })
        .collect()
}

#[test]
fn the_lazy_walk_equals_the_recursive_query_on_random_trees() {
    let mut r = Lcg(99);
    let mut deepest = 0usize;
    for round in 0..12 {
        let entries = random_entries(&mut r, 300 + 200 * round, [5.0, 40.0, 400.0][round % 3]);
        let tree = QuadTree::build(&entries);
        for k in 0..600 {
            let a = (r.unit() * 3500.0, r.unit() * 3500.0);
            let mut b = if k % 3 == 0 { (a.0 + r.unit() * 10.0, a.1 + r.unit() * 10.0) } else { (r.unit() * 3500.0, r.unit() * 3500.0) };
            // segments that end exactly on a centre line of the root / of its children
            match k % 8 {
                1 => b.0 = 0.0,
                2 => b.1 = 0.0,
                3 => {
                    b.0 = 4096.0;
                    b.1 = -4096.0;
                }
                4 => b = a,
                _ => {}
            }
            let new: Vec<usize> = tree.segment_iter(a, b).collect();
            assert_eq!(new, tree.query_segment_reference(a, b), "segment {a:?} -> {b:?}");
            assert_eq!(tree.query_segment(a, b), new);
            deepest = deepest.max(new.len());
        }
    }
    assert!(deepest > 100, "the segments reach enough entries to mean something: {deepest}");
}

#[test]
fn the_lazy_walk_of_an_empty_tree_is_empty() {
    let none: Vec<Boxed> = Vec::new();
    let tree = QuadTree::build(&none);
    assert_eq!(tree.segment_iter((-1.0, -1.0), (1.0, 1.0)).count(), 0);
}

/// `cargo test -p nfs-physics --lib quadtree -- --ignored --nocapture`: the recursive query (a
/// `Vec<Vec<usize>>` plus a flattened `Vec` per call) against the lazy walk, summing the entries so
/// neither is optimised away. Debug-build numbers; only the ratio is of interest.
#[test]
#[ignore = "timing, prints"]
fn timing_of_the_recursive_query_against_the_lazy_walk() {
    let mut r = Lcg(5);
    let entries = random_entries(&mut r, 4000, 40.0);
    let tree = QuadTree::build(&entries);
    let segs: Vec<((f32, f32), (f32, f32))> = (0..20_000)
        .map(|_| {
            let a = (r.unit() * 3000.0, r.unit() * 3000.0);
            (a, (a.0 + r.unit() * 8.0, a.1 + r.unit() * 8.0))
        })
        .collect();
    let t0 = std::time::Instant::now();
    let mut sum_old = 0usize;
    for (a, b) in &segs {
        sum_old += tree.query_segment_reference(*a, *b).iter().sum::<usize>();
    }
    let old = t0.elapsed();
    let t1 = std::time::Instant::now();
    let mut sum_new = 0usize;
    for (a, b) in &segs {
        sum_new += tree.segment_iter(*a, *b).sum::<usize>();
    }
    let new = t1.elapsed();
    assert_eq!(sum_old, sum_new);
    println!("20000 segment queries: recursive {old:?}, lazy {new:?}");
}
