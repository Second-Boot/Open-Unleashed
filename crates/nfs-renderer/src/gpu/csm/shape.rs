//! ENHANCEMENT E9 (SHADOW-UNIFY): a mesh's shape and the casters' rule over it ([`MeshShape`], [`mesh_bounds`], [`part_casts`]).

use super::*;

/// SHADOW-UNIFY (casters): a scenery part whose bounding sphere is larger than this (m) is BROAD: it casts only when it is not
/// terrain-like ([`terrain_like`]). Until SHADOW-UNIFY this was a plain limit (`MAX_CASTER_RADIUS`: every part over 60 m left out,
/// so a hillside or a cliff face would not fill the map with a shade the bake never had); but a track's parts are one mesh per
/// material per article, and the limit also left out whole objects whose one material spans an article: castle's courtyard ring
/// wall (`ct42`, 77.8 m) and its roof slopes, the terraced walls, rows of tree sheets (`fhtm`, `fht1`, `lin1`, `ftl5`, `trl1`),
/// farmyard walls and fence rows, the coastal freighter's hull, Monaco's facades (the caster audit of 2026-11-18).
pub(super) const BROAD_CASTER_RADIUS: f32 = 60.0;
/// SHADOW-UNIFY (APPROXIMATION, by the caster audit's scan of every track's parts): a broad part is upright, so an object and a
/// caster, when the mean `|n.y|` of its vertex normals is under [`UPRIGHT_UP`] (a wall, a facade, a fence, a hull, a sheet of
/// trees: on the shipped tracks every broad part under 0.1 is one, but canyon's `mn02` rock face), or when it is built: low
/// (under [`BUILT_HEIGHT`] m tall) and steep (mean `|n.y|` under [`BUILT_UP`]: castle's roof slopes `ct59`, a fence row on a
/// slope, a ship's superstructure). Every other broad part is terrain-like (hills, cliffs, slopes, fields, the sea bed: on the
/// shipped tracks they are either over 30 m tall or, lower, mostly facing up, mean `|n.y|` 0.65 and over) and casts nothing: its
/// shade along the sun would contradict the bake, which lights the road beside it evenly.
pub(super) const UPRIGHT_UP: f32 = 0.1;
pub(super) const BUILT_UP: f32 = 0.65;
pub(super) const BUILT_HEIGHT: f32 = 30.0;
/// The most vertices of a flat sheet's thickness (m) and the least mean `|n.y|` of its normals (see [`mesh_bounds`]).
pub(super) const FLAT_THICKNESS: f32 = 0.5;
pub(super) const FLAT_UP: f32 = 0.85;

/// ENHANCEMENT E9: a mesh's shape as the casters' rule reads it ([`mesh_bounds`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::gpu) struct MeshShape {
    /// a level flat sheet (a road layer, a marking, a ground decal): never a caster
    pub flat: bool,
    /// SHADOW-UNIFY: upright or built (see [`UPRIGHT_UP`]): a broad part of this shape is an object, not terrain
    pub upright: bool,
    /// the part's height in mesh space (m; the track's articles and props are not scaled): TEST GLUE's small-parts rule
    pub height: f32,
}

/// SHADOW-UNIFY: whether a scenery part of bounding radius `radius` (m, world) and `shape` is terrain-like, so no caster under
/// the shape rule: broad ([`BROAD_CASTER_RADIUS`]) and neither upright nor built ([`UPRIGHT_UP`]).
pub(crate) fn terrain_like(radius: f32, shape: MeshShape) -> bool {
    radius > BROAD_CASTER_RADIUS && !shape.upright
}

/// SHADOW-UNIFY: whether a scenery part of bounding radius `radius` (m, world) and `shape` casts under `rule` (never a flat sheet).
pub(crate) fn part_casts(rule: crate::SceneryCasters, radius: f32, shape: MeshShape) -> bool {
    !shape.flat
        && match rule {
            crate::SceneryCasters::Shape => !terrain_like(radius, shape),
            crate::SceneryCasters::Small { radius: r, height: h } => radius <= r && shape.height <= h,
        }
}

/// SHADOW-UNIFY: [`part_casts`] of a mesh's vertices placed at `scale` (`crate::scenery_part_casts`).
pub(crate) fn part_casts_vertices(vertices: &[Vertex], scale: f32, rule: crate::SceneryCasters) -> bool {
    let (_, r, shape) = mesh_bounds(vertices);
    !vertices.is_empty() && part_casts(rule, r * scale, shape)
}

/// A mesh's bounding sphere (the box's centre, the farthest vertex from it) and its shape: whether it is a flat sheet lying level
/// (thinner than half a metre with mostly upward normals: a road layer, a marking, a ground decal), which casts nothing worth the
/// coplanar acne it would give the road under it, and (SHADOW-UNIFY) whether it is upright or built ([`UPRIGHT_UP`]; the height in
/// mesh space: the track's articles and props are not scaled).
pub(in crate::gpu) fn mesh_bounds(vertices: &[Vertex]) -> ([f32; 3], f32, MeshShape) {
    if vertices.is_empty() {
        return ([0.0; 3], 0.0, MeshShape::default());
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v.position[i]);
            hi[i] = hi[i].max(v.position[i]);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let r2 = vertices.iter().map(|v| (0..3).map(|i| (v.position[i] - c[i]) * (v.position[i] - c[i])).sum::<f32>()).fold(0.0, f32::max);
    let up = vertices.iter().map(|v| v.normal[1].abs()).sum::<f32>() / vertices.len() as f32;
    let height = hi[1] - lo[1];
    let shape = MeshShape { flat: height < FLAT_THICKNESS && up > FLAT_UP, upright: up < UPRIGHT_UP || (up < BUILT_UP && height < BUILT_HEIGHT), height };
    (c, r2.sqrt(), shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_the_vertices() {
        let v = |x: f32, y: f32, z: f32| Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let (c, r, shape) = mesh_bounds(&[v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(1.0, 2.0, 0.0)]);
        assert_eq!(c, [1.0, 1.0, 0.0]);
        assert!((r - 2f32.sqrt()).abs() < 1e-6);
        assert!(!shape.flat);
    }

    /// SHADOW-UNIFY: the casters' shape rule: a broad part casts when upright or built, not when terrain-like; a flat sheet never
    #[test]
    fn the_shape_rule_keeps_terrain_out() {
        let v = |x: f32, y: f32, z: f32, ny: f32| Vertex { position: [x, y, z], normal: [(1.0 - ny * ny).sqrt(), ny, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let part = |w: f32, h: f32, ny: f32| [v(-w, 0.0, 0.0, ny), v(w, 0.0, 0.0, ny), v(0.0, h, 1.0, ny)];
        let casts = |p: &[Vertex]| part_casts_vertices(p, 1.0, crate::SceneryCasters::Shape);
        assert!(casts(&part(80.0, 34.0, 0.02)), "a ring wall 160 m round (castle ct42)");
        assert!(casts(&part(80.0, 120.0, 0.0)), "a sheet of trees 120 m tall (forest ftl6)");
        assert!(casts(&part(75.0, 15.0, 0.58)), "roof slopes (castle ct59)");
        assert!(!casts(&part(70.0, 120.0, 0.4)), "a cliff (canyon clfa)");
        assert!(!casts(&part(70.0, 60.0, 0.8)), "a hillside (farmland grs2)");
        assert!(!casts(&part(70.0, 12.0, 0.98)), "a field (farmland fm03)");
        assert!(casts(&part(20.0, 40.0, 0.8)), "anything under 60 m casts as before");
        assert!(!casts(&part(20.0, 0.2, 1.0)), "a flat sheet never");
        assert!(!part_casts_vertices(&[], 1.0, crate::SceneryCasters::Shape));
        let small = crate::SceneryCasters::Small { radius: 15.0, height: 8.0 };
        assert!(part_casts_vertices(&part(1.0, 2.0, 0.0), 1.0, small), "a sign");
        assert!(part_casts_vertices(&part(12.0, 3.0, 0.0), 1.0, small), "a roadside wall");
        assert!(!part_casts_vertices(&part(12.0, 20.0, 0.4), 1.0, small), "a cliff chunk (too tall)");
        assert!(!part_casts_vertices(&part(40.0, 3.0, 0.0), 1.0, small), "a long gorge wall (too broad)");
    }
}
