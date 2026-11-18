//! ENHANCEMENT E23: the street lamps as point lights.
//!
//! Source: the track's kind-2 `ef` parts (the flares, `crp-geometry.md` section 8), world positions
//! from [`nfs_game::world::TrackWorld::all_flares`], the colour of the part's `flare.dat` record.
//! Which flare ids are street lamps is classified in `ENHANCEMENTS.md` (E23): the table [`class_of`].
//! The data has no radius, brightness or cone: those are authored (an enhancement, 2026-10-03),
//! see [`nfs_renderer::Lamps`] and the shader's `lamp_light`. Render-only: nothing here reads or
//! writes the simulation, a replay or the net.

use super::*;
use nfs_renderer::{Lamps, StreetLamp, MAX_LAMPS};

/// A classified street lamp of the track.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LampSite {
    pub pos: Vec3,
    /// the flare record's first layer colour, scaled so its largest channel is 1
    pub color: [f32; 3],
    /// the head's height over the nearest road slice (m)
    pub height: f32,
    /// the lamp is inside a tunnel: lit on any track (a street lamp only on a night track). LAMP-TUNE: a tunnel-lamp id, or a
    /// street-lamp id standing in a covered stretch (coastal's two tunnels are lit by `ydlb` sodium lamps)
    pub tunnel: bool,
    /// LAMP-TUNE: a tunnel-lamp id (`class_of`): E36's air glows by day take only these. The covered street lamps (coastal) light
    /// the road and cast the cars' shadows by day but have no air glow: E36's glow of coastal's 29 sodium lamps washed the tunnel
    /// out (its haze is tuned on the tunnel-lamp tracks)
    pub air: bool,
    /// LAMP-TUNE: how many flare heads make this light (a multi-head lamp post is one light, [`CLUSTER_GAP`])
    pub heads: u8,
    /// the light's reach (m) at the level "средне", [`lamp_radius`]
    pub radius: f32,
    /// MONACO-TUNNEL: a bulb painted into a lit tunnel's luminaires ([`fixture_bulbs`]): the bake holds its light
    pub painted: bool,
}

/// LAMP-TUNE: heads of one lamp post (monaco1's casino square: three `pmal` heads 0.72 m apart on each post) closer than this (m)
/// are one light. APPROXIMATION (the data has no post grouping; the nearest separate lamps of the census stand 2.6 m apart).
const CLUSTER_GAP: f32 = 1.5;

/// LAMP-TUNE: the brightness of a tunnel lamp lit on a DAY track (a share of E23's): the tunnel's baked colour there is already
/// the lamps' light (the original bakes their pools into the tube), so E23's full term on top doubled it, and with the rows of
/// lamps 7-20 m apart (each road point inside 3-5 pools) clipped the road and the walls to white (coastal by day: the road
/// band's mean luma 61 original -> 137, 1.7 % clipped; alps' tubes alike, 49 -> 156). At this share the pools and the cars' lamp
/// shadows show and nothing clips. Night tracks keep the whole term (their bake is the dark night). APPROXIMATION (by the
/// histograms, `ENHANCEMENTS.md` LAMP-TUNE).
pub(crate) const DAY_TUNNEL_GAIN: f32 = 0.35;

/// AUTOBAHN-LAMPS: the brightness of an open street lamp on a track whose open street lamps light by day
/// (`dusk_lamps`), times E23's night term. The rainy dusk's baked
/// road is much lighter than a night track's and the motorway posts are high (16-20 m: wide, faint pools), so at the night term the
/// pools and the cars' lamp shadows hardly showed (the lower screen's mean luma +5 near a post); at 1.6 +7 there and +14 right under
/// a post, the clipped share 0.04 -> 0.35 % (the car's lit roof). APPROXIMATION (by the stills, `ENHANCEMENTS.md` E47 AUTOBAHN-LAMPS).
pub(crate) const DUSK_STREET_GAIN: f32 = 1.6;

/// AUTOBAHN-LAMPS / SHADOW-UNIFY: whether `track` lights its open street lamps by day: a day track with no sun from any source
/// (`nfs_game::data_sun::dusk_lamps`, `TrackWorld::dusk_lamps`: a data rule, APPROXIMATION; it was a per-track choice
/// for autobahn until SHADOW-UNIFY). No world (the front end): none.
pub(crate) fn dusk_lamps(track: &str, world: Option<&nfs_game::world::TrackWorld>) -> bool {
    // TEST GLUE: `NFS_DUSK_TRACKS=<a,b,..>` replaces the rule (comparison stills only; `none` = no track)
    if let Some(list) = nfs_game::config::cfg().nfs_dusk_tracks.as_str() {
        return list.split(',').any(|t| t.trim().eq_ignore_ascii_case(track));
    }
    world.is_some_and(|w| w.dusk_lamps)
}

/// LAMP-TUNE: the reach of a lamp `height` m over the road made of `heads` heads (m): E23's `clamp(1.5 h + 6, 10, 30)` per head,
/// times `sqrt(heads)` for a post of several heads (the distance a given brightness reaches grows with the root of the
/// intensity), at most 36: monaco1's three-head 3 m posts reach 18 m (were three 10.5 m lights). A wider single lamp was tried
/// (`clamp(2 h + 8, 12, 32)`) and dropped: the rows of lamps 7-10 m apart (coastal's tunnels, autobahn's) then add six pools
/// at every point and wash the road out. APPROXIMATION: the data has no radius (`ENHANCEMENTS.md` E23, authored).
pub(crate) fn lamp_radius(height: f32, heads: u8) -> f32 {
    ((1.5 * height + 6.0).clamp(10.0, 30.0) * (heads.max(1) as f32).sqrt()).min(36.0)
}

/// LAMP-TUNE: the heads merged into lights: heads of the same kind (tunnel or street) within [`CLUSTER_GAP`] of a light's first
/// head join it; the light stands at their mean, its colour is their mean colour, its height their mean, its pool
/// [`lamp_radius`] `sqrt(heads)` wider: the pool's energy (peak x radius^2) is the heads' sum, spread wider instead of a 3x
/// hot spot (a `sqrt(heads)` peak on top was tried: alps' paired tunnel heads then gave 2.8x a pair's light, the tube clipped).
fn cluster(heads: Vec<(LampSite, Vec3)>) -> Vec<LampSite> {
    // (first head, sum of positions, sum of colours, sum of heights, count, tunnel, air, painted)
    let mut groups: Vec<(Vec3, Vec3, Vec3, f32, u8, bool, bool, bool)> = Vec::new();
    for (s, c) in heads {
        match groups.iter_mut().find(|g| g.5 == s.tunnel && g.6 == s.air && g.7 == s.painted && g.0.distance(s.pos) < CLUSTER_GAP && g.4 < u8::MAX) {
            Some(g) => {
                g.1 += s.pos;
                g.2 += c;
                g.3 += s.height;
                g.4 += 1;
            }
            None => groups.push((s.pos, s.pos, c, s.height, 1, s.tunnel, s.air, s.painted)),
        }
    }
    groups
        .into_iter()
        .map(|(_, p, c, h, n, tunnel, air, painted)| {
            let k = n as f32;
            let colour = c / k;
            let height = h / k;
            let radius = if painted { PAINTED_RADIUS } else { lamp_radius(height, n) };
            LampSite { pos: p / k, color: colour.to_array(), height, tunnel, air, heads: n, radius, painted }
        })
        .collect()
}

/// MONACO-TUNNEL: a tunnel light painted into a wall texture instead of standing as a flare (`ef` part). Monaco's long harbour
/// tunnel (monaco1 slices 141-244, monaco2 379-481, monaco4 481-584, monaco5 0-103; the same `MONTUNCEN*` chunks) has no flare in it
/// (`flarecensus`: every classified head lies outside its covered run), so E23 had no lamp there and E47 nothing to cast with. Its
/// lights are the luminaires of `ttm1`'s top band (the slanted strip between the wall and the ceiling, v 0..0.195, 6.0-7.75 m over
/// the road on both sides), one lit bulb per texture repeat, its warm glow centred at (u 0.25, v 0.16) of the 128 px texture (the
/// glow's weighted centroid). Each place the mesh maps that texel to is a lamp.
struct Fixture {
    texture: [u8; 4],
    /// the bulb's texel (u modulo 1, v)
    bulb: (f32, f32),
    /// the luminaire band in v: a triangle whose three corners lie in it carries the bulbs
    band: (f32, f32),
}

const FIXTURES: &[Fixture] = &[Fixture { texture: *b"ttm1", bulb: (0.25, 0.16), band: (0.0, 0.2) }];

/// MONACO-TUNNEL: a painted bulb's lamp stands this far (m) in front of its luminaire, toward the road (a point light on the surface
/// would light its own face at full strength and sit inside the shadow map's near plane). APPROXIMATION.
const FIXTURE_OFFSET: f32 = 0.3;

/// MONACO-TUNNEL: a painted bulb's brightness (a share of E23's night term, folded into its colour). The tube's bake already is the
/// luminaires' light (as in coastal's / alps' tunnels by day, LAMP-TUNE's `DAY_TUNNEL_GAIN`), and the rows (one bulb every ~12 m on
/// each side) add up: at the whole term the walls and the road under the luminaires washed out to white; swept 0.15 / 0.2 / 0.25 /
/// 0.35 / 0.5 by the stills. What makes the cars' shadows show is the bake's share they take (`nfs_renderer::BAKED_SHADOW`), not
/// this. APPROXIMATION.
const PAINTED_GAIN: f32 = 0.2;

/// MONACO-TUNNEL: a painted bulb's reach (m). The luminaires stand over the walls, 11 m off the centreline of a 23 m tube, and the
/// bake lights the whole road from them; E23's `lamp_radius` of their height (15.5 m) ended its pools short of the lanes.
const PAINTED_RADIUS: f32 = 24.0;

/// MONACO-TUNNEL: the painted bulbs of [`FIXTURES`] in world space (one point per mapped bulb, duplicates on shared edges merged).
/// Empty on a track whose materials name none of the textures. Reads the static world articles' finest LOD only.
pub(crate) fn fixture_bulbs(track: &TrackAssets) -> Vec<Vec3> {
    let Ok(mats) = track.materials() else { return Vec::new() };
    let fix: std::collections::HashMap<u16, &Fixture> =
        mats.iter().enumerate().filter_map(|(i, m)| Some((i as u16, FIXTURES.iter().find(|f| f.texture == m.texture)?))).collect();
    if fix.is_empty() {
        return Vec::new();
    }
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
    let mut out: Vec<Vec3> = Vec::new();
    let mut skipped = 0usize;
    for a in &arts {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) || !g.primitives.iter().any(|p| fix.contains_key(&p.material)) {
            continue;
        }
        let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for mm in nfs_assets::mesh::article_meshes(&g, lod, &mut skipped) {
            let Some(f) = fix.get(&mm.material) else { continue };
            for t in mm.mesh.indices.chunks_exact(3) {
                let v = [0, 1, 2].map(|k| mm.mesh.vertices[t[k] as usize]);
                let uv = v.map(|x| glam::Vec2::from(x.uv));
                let p = v.map(|x| tr.transform_point3(Vec3::from(x.position)));
                for w in triangle_bulbs(f, uv, p) {
                    if !out.iter().any(|o| o.distance_squared(w) < 0.25) {
                        out.push(w);
                    }
                }
            }
        }
    }
    out
}

/// MONACO-TUNNEL: the places a triangle (texture coordinates `uv`, world corners `p`) maps the fixture's bulb texel to, one per
/// texture repeat it spans along u; none unless the whole triangle lies in the luminaire band.
fn triangle_bulbs(f: &Fixture, uv: [glam::Vec2; 3], p: [Vec3; 3]) -> Vec<Vec3> {
    let mut out = Vec::new();
    if uv.iter().any(|q| q.y < f.band.0 - 1e-3 || q.y > f.band.1 + 1e-3) {
        return out;
    }
    let (lo, hi) = uv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| (lo.min(q.x), hi.max(q.x)));
    // barycentric coordinates of the bulb's texel in the triangle's UV space, for each repeat it spans
    let (e1, e2) = (uv[1] - uv[0], uv[2] - uv[0]);
    let det = e1.perp_dot(e2);
    if det.abs() < 1e-9 {
        return out;
    }
    for k in (lo - f.bulb.0).floor() as i32..=(hi - f.bulb.0).ceil() as i32 {
        let q = glam::Vec2::new(k as f32 + f.bulb.0, f.bulb.1) - uv[0];
        let (b1, b2) = (q.perp_dot(e2) / det, e1.perp_dot(q) / det);
        if b1 < -1e-4 || b2 < -1e-4 || b1 + b2 > 1.0 + 1e-4 {
            continue;
        }
        out.push(p[0] + (p[1] - p[0]) * b1 + (p[2] - p[0]) * b2);
    }
    out
}

/// MONACO-TUNNEL round 2 (2026-11-15: "try fewer painted lamps"): along each side of the road, a painted bulb lights only when
/// it stands at least this far (m) from the last lit one on that side, walking the slices in order. The luminaires repeat every ~12 m,
/// so 20 m keeps every second one (~24 m apart on each side): monaco1 99 bulbs -> 51 lamps, the tunnel's frame 17.96 -> 16.4 ms
/// (with the E47 lookup skip in `lamp_light`). 30 m (35 lamps, 15.75 ms) left the car with one faint shadow or none on some
/// stretches. APPROXIMATION (by the frame time and the stills: the cars keep a fan of 2-4 shadows).
const PAINTED_GAP: f32 = 20.0;

/// MONACO-TUNNEL: the painted bulbs that light, [`PAINTED_GAP`] apart along each side (`(side, slice, place, height)` in, `(place,
/// height)` out; ties in a slice by the place's x, so the result does not depend on the mesh order).
fn thin_bulbs(mut placed: Vec<(bool, usize, Vec3, f32)>, gap: f32) -> Vec<(Vec3, f32)> {
    placed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.x.total_cmp(&b.2.x)));
    let mut out = Vec::new();
    let mut last: Option<(bool, Vec3)> = None;
    for (side, _, pos, height) in placed {
        if last.is_some_and(|(s, p)| s == side && p.distance(pos) < gap) {
            continue;
        }
        last = Some((side, pos));
        out.push((pos, height));
    }
    out
}

/// Lamps farther than this from the eye are not considered (metres).
const REACH: f32 = 150.0;

/// MONACO-TUNNEL: painted bulbs farther than this from the eye are not lit (metres; faded out over its last 30 %). The harbour
/// tunnel's rows hold a bulb every ~12 m on each side, so within [`REACH`] the 32 slots all went to them, each reaching 24 m: the
/// lamp loop over every tunnel pixel cost about 4 ms at 2560 x 1600 (monaco1, M1 Pro). Their added light is a small share (the bake
/// is their light, [`PAINTED_GAIN`]) and E47's four shadow slots go to the lamps nearest the cars, so the far ones showed nothing.
/// APPROXIMATION (by the frame time and the stills).
const PAINTED_REACH: f32 = 60.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LampClass {
    Street,
    Tunnel,
}

/// The classification of a `flare.dat` id (`ENHANCEMENTS.md` E23, the table of the ids). `None` =
/// not a lamp that lights the road: warning blinkers, beacons, aircraft / helicopter lights, window
/// glows, rear lights, sign and sea glints.
pub(crate) fn class_of(id: u32) -> Option<LampClass> {
    let name = id.to_le_bytes();
    match &name {
        // `str?` (strg / stry / strb): the heads of the tall street lamps (Monaco, Industrial), `lopl`
        // lamp poles, `lmpy` / `lamp` lamps, `bldy` the sodium lamps
        // AUTOBAHN-LAMPS: `thws` (BLUR + BLUR, blue-white) was read as a water glint; castle's 18 hang 0.2 m under the ceiling of
        // its tunnel (slices 208-276), on the centreline every 4 slices: the tunnel's ceiling lamps. A street id, so the covered
        // rule makes those lit on the day track and leaves alps' three in the open (8-11 m over the road, slices 721-768) unlit
        b"grts" | b"yrts" | b"brts" | b"lopl" | b"pmal" | b"ypml" | b"ydlb" | b"thws" => Some(LampClass::Street),
        b"bnut" | b"ynut" | b"onut" | b"tnac" => Some(LampClass::Tunnel),
        _ => None,
    }
}

/// PERF: the main view's frustum as a cone (eye, axes, the half-angle tangents), for the question "can a light with this
/// reach touch what the camera sees": a sphere test, conservative (the sides use the exact plane distance), no pop-in
/// because a light whose sphere is outside lights nothing inside.
#[derive(Clone, Copy)]
pub(crate) struct ViewCone {
    eye: Vec3,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
    tan_x: f32,
    tan_y: f32,
    far: f32,
}

impl ViewCone {
    /// Whether a sphere (`centre`, `radius`) intersects the frustum (with a 10 % margin on the angles).
    pub(crate) fn sees(&self, centre: Vec3, radius: f32) -> bool {
        let v = centre - self.eye;
        let z = v.dot(self.fwd);
        if z < -radius || z > self.far + radius {
            return false;
        }
        let (x, y) = (v.dot(self.right).abs(), v.dot(self.up).abs());
        let (tx, ty) = (self.tan_x * 1.1, self.tan_y * 1.1);
        x <= z.max(0.0) * tx + radius * (1.0 + tx * tx).sqrt() && y <= z.max(0.0) * ty + radius * (1.0 + ty * ty).sqrt()
    }
}

/// PERF: the nearest road slice of a point, the answer of the scan it replaces — `slices.iter().map(|s|
/// s.centre.distance_squared(p)).min_by(total_cmp)`, the FIRST slice of the smallest distance — without visiting every slice
/// for every lamp (thousands x thousands at a race's start). The centres are ordered by x once; a query walks outward from
/// `p.x` on both sides and stops a side where `dx * dx` alone exceeds the best distance: the distance is the same
/// `distance_squared` (so the same bits) and, its other terms being >= 0, never below that side's `dx * dx` (rounding is
/// monotonic), so nothing past the stop can be nearer or tie. Ties go to the lower index, as `min_by` does. A centre or a
/// point that is not finite falls back to the scan itself.
pub(crate) struct SliceIndex {
    centres: Vec<Vec3>,
    /// slice indices by centre x (then by index)
    by_x: Vec<u32>,
    finite: bool,
}

impl SliceIndex {
    pub(crate) fn new(centres: Vec<Vec3>) -> Self {
        let mut by_x: Vec<u32> = (0..centres.len() as u32).collect();
        by_x.sort_by(|&a, &b| centres[a as usize].x.total_cmp(&centres[b as usize].x).then(a.cmp(&b)));
        let finite = centres.iter().all(|c| c.is_finite());
        Self { centres, by_x, finite }
    }

    /// The scan the index replaces (the fallback, and the tests' reference).
    pub(crate) fn scan(centres: &[Vec3], p: Vec3) -> Option<usize> {
        centres.iter().enumerate().map(|(i, c)| (i, c.distance_squared(p))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
    }

    /// The index of the slice nearest to `p` (`None` without slices).
    pub(crate) fn nearest(&self, p: Vec3) -> Option<usize> {
        if self.centres.is_empty() {
            return None;
        }
        if !self.finite || !p.is_finite() {
            return Self::scan(&self.centres, p);
        }
        let k = self.by_x.partition_point(|&i| self.centres[i as usize].x < p.x);
        let mut best = (f32::INFINITY, usize::MAX);
        // right of `p.x` in ascending x, then left of it in descending x
        for &i in &self.by_x[k..] {
            if !self.visit(i as usize, p, &mut best) {
                break;
            }
        }
        for &i in self.by_x[..k].iter().rev() {
            if !self.visit(i as usize, p, &mut best) {
                break;
            }
        }
        Some(best.1)
    }

    /// Slice `i` against the best `(distance, index)` so far; `false` = past it on this side nothing can be nearer.
    fn visit(&self, i: usize, p: Vec3, best: &mut (f32, usize)) -> bool {
        let c = self.centres[i];
        let dx = c.x - p.x;
        if dx * dx > best.0 {
            return false;
        }
        let d = c.distance_squared(p);
        if d < best.0 || (d == best.0 && i < best.1) {
            *best = (d, i);
        }
        true
    }
}

impl App {
    /// PERF: this frame's view cone of the main camera (`None` when there is no renderer to give the aspect: no culling).
    pub(crate) fn view_cone(&self) -> Option<ViewCone> {
        let aspect = self.renderer.as_ref()?.aspect();
        let c = &self.camera;
        let fwd = (c.target - c.position).normalize_or_zero();
        let right = fwd.cross(c.up).normalize_or_zero();
        if fwd == Vec3::ZERO || right == Vec3::ZERO {
            return None;
        }
        let tan_y = (0.5 * c.fov_y_radians).tan();
        Some(ViewCone { eye: c.position, fwd, right, up: right.cross(fwd), tan_x: tan_y * aspect, tan_y, far: c.far })
    }

    /// The track's lamps, classified once per race (`None` until the first frame asks).
    pub(crate) fn street_lamp_sites(&mut self) -> &[LampSite] {
        if self.street_lamps.is_none() {
            let mut heads: Vec<(LampSite, Vec3)> = Vec::new();
            // LAMP-TUNE: the covered stretches (the original's enclosure test of the road slices, `cover.rs`): a street lamp inside one
            // lights the tunnel on a day track too
            let boxes = if self.world.is_some() && self.slices.is_some() { self.cover_boxes() } else { Default::default() };
            if let Some(w) = self.world.as_ref() {
                // PERF: the slices' nearest-centre index and the `flare.dat` records by id (the first
                // record of an id, as `iter().find` took it), built once here instead of a search per flare
                let near = self.slices.as_ref().map(|t| SliceIndex::new(t.slices.iter().map(|s| s.centre).collect()));
                let mut defs: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
                for (k, d) in self.flare_defs.iter().enumerate() {
                    defs.entry(d.id).or_insert(k);
                }
                for f in w.all_flares() {
                    let Some(class) = class_of(f.id) else { continue };
                    let Some(def) = defs.get(&f.id).map(|&k| &self.flare_defs[k]) else { continue };
                    // the wide glow layer's colour (the second; the first is the white core), a third of the way to white
                    let argb = if def.textures[1].is_some() { def.colours[1] } else { def.colours[0] };
                    let [r, g, b, _] = argb_rgba(argb);
                    let c = [r, g, b];
                    let m = c[0].max(c[1]).max(c[2]).max(1e-3);
                    let c = c.map(|v| v / m * 0.7 + 0.3);
                    let height = self.slices.as_ref().zip(near.as_ref()).and_then(|(t, n)| n.nearest(f.position).map(|i| t.slices[i].centre.y)).map_or(6.0, |y| f.position.y - y);
                    let covered = nfs_renderer::cover_weight(&boxes, f.position) > 0.5;
                    tracing::debug!(id = %String::from_utf8_lossy(&f.id.to_le_bytes()), pos = ?f.position, height, covered, "street lamp head (ENHANCEMENT E23)");
                    let site = LampSite { pos: f.position, color: c, height: height.clamp(2.5, 20.0), tunnel: class == LampClass::Tunnel || covered, air: class == LampClass::Tunnel, heads: 1, radius: 0.0, painted: false };
                    heads.push((site, Vec3::from(c)));
                }
            }
            // MONACO-TUNNEL: the bulbs painted into a covered stretch's luminaires are tunnel lamps (only inside a covered box: the
            // texture is not used for lights elsewhere). The colour is the bulb glow's mean texel (218, 179, 100), treated as a
            // flare's: stretched to full brightness, a third of the way to white.
            let bulbs = fixture_bulbs(&self.track);
            let c = [1.0, 0.82, 0.46].map(|v: f32| (v * 0.7 + 0.3) * PAINTED_GAIN);
            let centres: Vec<Vec3> = self.slices.as_ref().map(|t| t.slices.iter().map(|s| s.centre).collect()).unwrap_or_default();
            let near = (!bulbs.is_empty()).then(|| SliceIndex::new(centres.clone()));
            // (side of the road, slice, the lamp's place, its height)
            let mut placed: Vec<(bool, usize, Vec3, f32)> = Vec::new();
            for b in bulbs {
                // (the covered test at the road under the bulb: the luminaires stand over the walls, past the boxes' side margin)
                let Some(i) = near.as_ref().and_then(|n| n.nearest(b)) else { continue };
                let centre = centres[i];
                if nfs_renderer::cover_weight(&boxes, centre + Vec3::Y * 2.0) <= 0.5 {
                    continue;
                }
                let toward = Vec3::new(centre.x - b.x, 0.0, centre.z - b.z).normalize_or_zero();
                let pos = b + toward * FIXTURE_OFFSET;
                let height = (pos.y - centre.y).clamp(2.5, 20.0);
                let fwd = self.slices.as_ref().map_or(Vec3::Z, |t| t.slices[i].forward);
                placed.push((fwd.cross(Vec3::Y).dot(b - centre) > 0.0, i, pos, height));
            }
            let n_bulbs = placed.len();
            let lit = thin_bulbs(placed, PAINTED_GAP);
            for &(pos, height) in &lit {
                heads.push((LampSite { pos, color: c, height, tunnel: true, air: false, heads: 1, radius: 0.0, painted: true }, Vec3::from(c)));
            }
            if n_bulbs > 0 {
                tracing::info!(bulbs = n_bulbs, lamps = lit.len(), "painted tunnel lamps (ENHANCEMENT E23, MONACO-TUNNEL)");
            }
            let out = cluster(heads);
            tracing::info!(lamps = out.len(), tunnel = out.iter().filter(|s| s.tunnel).count(), heads = out.iter().map(|s| s.heads as usize).sum::<usize>(), night = self.track.is_night(), "street lamps (ENHANCEMENT E23)");
            self.street_lamps = Some(out);
        }
        self.street_lamps.as_deref().unwrap_or(&[])
    }

    /// This frame's lamps for the main view: the nearest [`MAX_LAMPS`] within [`REACH`], the far ones
    /// faded out so the selection never pops (the fade starts at 75 % of the first left-out lamp's
    /// distance). `Lamps::default()` (none) with E23 off or on a day track without tunnel lamps. `dusk_track`: the track is one of
    /// [`dusk_lamps`] (`RenderOptions::dusk_lamps`).
    pub(crate) fn frame_lamps(&mut self, dusk_track: bool) -> Lamps {
        let level = self.enh.level(nfs_renderer::E23);
        if level == 0 {
            return Lamps::default();
        }
        let night = self.track.is_night();
        // AUTOBAHN-LAMPS: a dusk track's open street lamps light too, at their share
        let dusk = !night && dusk_track;
        let eye = self.camera.position;
        // PERF: only the lamps whose pool (the radius below) reaches into the view; the 32 slots then go to lamps that
        // light something on screen, not to the ones behind the camera, and the shader loops over fewer
        let cone = self.view_cone();
        let scale = nfs_renderer::street_lamp_pool(level).0;
        let mut near: Vec<(f32, u32, LampSite)> = self
            .street_lamp_sites()
            .iter()
            .filter(|s| night || dusk || s.tunnel)
            .filter(|s| cone.is_none_or(|c| c.sees(s.pos, s.radius * scale)))
            .map(|s| (s.pos.distance(eye), *s))
            .filter(|(d, s)| *d < if s.painted { PAINTED_REACH } else { REACH })
            .enumerate()
            .map(|(i, (d, s))| (d, i as u32, s))
            .collect();
        // the nearest MAX_LAMPS in the order a stable sort by distance gives (ties in the sites' order), and the next one's distance:
        // only those are ordered (the sites' index breaks every tie, so the unstable selection keeps the stable sort's set and order)
        let cut = nearest_first(&mut near, MAX_LAMPS).map_or(REACH, |c| c.0);
        let mut out = Lamps::default();
        // soft: a smaller, dimmer pool; normal: the authored default
        let (scale, gain) = nfs_renderer::street_lamp_pool(level);
        out.gain = gain;
        for (d, _, s) in near.iter().take(MAX_LAMPS) {
            let fade = 1.0 - ((d - 0.75 * cut) / (0.25 * cut)).clamp(0.0, 1.0);
            let k = fade * fade * (3.0 - 2.0 * fade);
            // LAMP-TUNE: a tunnel lamp on a day track lights at its share (the tube's bake already holds its light)
            // AUTOBAHN-LAMPS: an open street lamp of a dusk track at DUSK_STREET_GAIN
            // MONACO-TUNNEL: a painted bulb fades out toward PAINTED_REACH
            let k = if s.painted {
                let f = 1.0 - ((d - 0.7 * PAINTED_REACH) / (0.3 * PAINTED_REACH)).clamp(0.0, 1.0);
                k * f * f * (3.0 - 2.0 * f)
            } else {
                k
            };
            let k = if night {
                k
            } else if s.tunnel {
                k * DAY_TUNNEL_GAIN
            } else {
                k * DUSK_STREET_GAIN
            };
            out.lamps[out.n as usize] = StreetLamp {
                pos: s.pos,
                color: [s.color[0] * k, s.color[1] * k, s.color[2] * k],
                radius: s.radius * scale,
                // MONACO-TUNNEL round 2 (2026-11-15): every tunnel lamp's light is in its tube's bake, so its shadow takes its
                // share of the bake too (alps, coastal, castle, canyon, autobahn, the monaco bnut tunnel, industrial's)
                baked: s.painted || s.tunnel,
            };
            out.n += 1;
        }
        out
    }
}

/// Orders the first `n` entries of `v` (distance, the entry's index, the item) nearest first and puts the next nearest at `v[n]`
/// (returned): what a stable sort of the whole list by distance gives at those places — the index breaks every tie — without
/// ordering the rest (`select_nth_unstable_by` then a sort of the `n`).
fn nearest_first<T: Copy>(v: &mut [(f32, u32, T)], n: usize) -> Option<(f32, u32, T)> {
    let key = |a: &(f32, u32, T), b: &(f32, u32, T)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
    if v.len() > n {
        v.select_nth_unstable_by(n, key);
        v[..n].sort_unstable_by(key);
    } else {
        v.sort_unstable_by(key);
    }
    v.get(n).copied()
}

#[cfg(test)]
mod tests {
    use super::{nearest_first, cluster, lamp_radius, thin_bulbs, triangle_bulbs, LampSite, SliceIndex, FIXTURES};

    /// `nearest_first` keeps the stable sort's first `n` (ties in the input order) and its `n`-th.
    #[test]
    fn nearest_first_is_the_stable_sort_s_head() {
        let ds = [5.0f32, 1.0, 3.0, 1.0, 7.0, 3.0, 0.5, 3.0, 9.0, 1.0];
        for n in 0..12 {
            let mut v: Vec<(f32, u32, usize)> = ds.iter().enumerate().map(|(i, &d)| (d, i as u32, i)).collect();
            let mut stable = v.clone();
            stable.sort_by(|a, b| a.0.total_cmp(&b.0));
            let next = nearest_first(&mut v, n);
            let m = n.min(ds.len());
            assert_eq!(v[..m].iter().map(|e| e.2).collect::<Vec<_>>(), stable[..m].iter().map(|e| e.2).collect::<Vec<_>>());
            assert_eq!(next.map(|e| e.2), stable.get(n).map(|e| e.2));
        }
    }

    /// MONACO-TUNNEL round 2: bulbs every 12 m on two sides keep every second one per side, whatever the input order.
    #[test]
    fn the_painted_bulbs_are_thinned_per_side() {
        let mut v = Vec::new();
        for k in 0..10 {
            v.push((false, 2 * k, glam::Vec3::new(12.0 * k as f32, 6.0, -11.0), 6.0));
            v.push((true, 2 * k, glam::Vec3::new(12.0 * k as f32 + 3.0, 6.0, 11.0), 6.0));
        }
        let mut rev = v.clone();
        rev.reverse();
        let a = thin_bulbs(v, 20.0);
        assert_eq!(a.len(), 10);
        assert_eq!(a.iter().filter(|p| p.0.z < 0.0).map(|p| p.0.x).collect::<Vec<_>>(), [0.0, 24.0, 48.0, 72.0, 96.0]);
        assert_eq!(a, thin_bulbs(rev, 20.0));
    }
    use glam::Vec3;

    /// LAMP-TUNE: the three heads of a post (0.72 m apart) are one light at their mean, sqrt(3) brighter and wider; a lamp 2.6 m
    /// away and a tunnel head at the same spot stay lights of their own
    /// MONACO-TUNNEL: a luminaire quad of monaco1's tunnel (`ttm1`'s band, u 0..2 over 8 m along x, v 0 at the top edge 7.75 m,
    /// 0.195 at the bottom 6 m) carries a bulb at u 0.25 and 1.25, 1 m and 5 m along, at the height of v 0.16; a wall triangle
    /// below the band carries none.
    #[test]
    fn the_painted_bulbs_lie_where_the_texture_maps_them() {
        let f = &FIXTURES[0];
        let uv = |u: f32, v: f32| glam::Vec2::new(u, v);
        let (a, b, c, d) = (Vec3::new(0.0, 7.75, 0.0), Vec3::new(8.0, 7.75, 0.0), Vec3::new(8.0, 6.0, 0.0), Vec3::new(0.0, 6.0, 0.0));
        let mut got = triangle_bulbs(f, [uv(0.0, 0.0), uv(2.0, 0.0), uv(2.0, 0.195)], [a, b, c]);
        got.extend(triangle_bulbs(f, [uv(0.0, 0.0), uv(2.0, 0.195), uv(0.0, 0.195)], [a, c, d]));
        got.sort_by(|p, q| p.x.total_cmp(&q.x));
        let y = 7.75 - 1.75 * 0.16 / 0.195;
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].distance(Vec3::new(1.0, y, 0.0)) < 1e-3 && got[1].distance(Vec3::new(5.0, y, 0.0)) < 1e-3, "{got:?}");
        assert!(triangle_bulbs(f, [uv(0.0, 0.195), uv(2.0, 0.195), uv(2.0, 1.0)], [d, c, Vec3::new(8.0, 0.0, 0.0)]).is_empty());
    }

    #[test]
    fn the_heads_of_a_post_are_one_light() {
        let head = |x: f32, z: f32, tunnel: bool| (LampSite { pos: Vec3::new(x, 3.0, z), color: [1.0, 0.8, 0.5], height: 3.0, tunnel, air: tunnel, heads: 1, radius: 0.0, painted: false }, Vec3::new(1.0, 0.8, 0.5));
        let out = cluster(vec![head(0.0, 0.0, false), head(0.72, 0.0, false), head(0.36, 0.62, false), head(2.6, 0.0, false), head(0.0, 0.0, true)]);
        assert_eq!(out.len(), 3);
        let post = &out[0];
        assert_eq!(post.heads, 3);
        assert!((post.pos - Vec3::new(0.36, 3.0, 0.62 / 3.0)).length() < 1e-5, "{:?}", post.pos);
        assert!((post.color[0] - 1.0).abs() < 1e-5);
        assert!((post.radius - lamp_radius(3.0, 1) * 3.0f32.sqrt()).abs() < 1e-4);
        assert_eq!((out[1].heads, out[2].heads, out[2].tunnel), (1, 1, true));
        // a single head keeps E23's reach
        assert_eq!(lamp_radius(3.0, 1), 10.5);
        assert_eq!(lamp_radius(20.0, 1), 30.0);
        assert_eq!(lamp_radius(16.0, 4), 36.0);
    }

    /// Every query of `pts` answered as the scan answers it.
    fn same(centres: &[Vec3], pts: &[Vec3]) -> usize {
        let idx = SliceIndex::new(centres.to_vec());
        for p in pts {
            assert_eq!(idx.nearest(*p), SliceIndex::scan(centres, *p), "{p:?}");
        }
        pts.len()
    }

    /// ties (the same centre twice, a point midway), an empty table, a non-finite point.
    #[test]
    fn the_index_breaks_ties_as_the_scan() {
        let c = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(-2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 5.0)];
        same(&c, &[Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 3.0, 0.0), Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 2.5), Vec3::new(f32::NAN, 0.0, 0.0), Vec3::new(f32::INFINITY, 0.0, 0.0)]);
        assert_eq!(SliceIndex::new(Vec::new()).nearest(Vec3::ONE), None);
        // a non-finite centre: the scan itself
        same(&[Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ONE], &[Vec3::ZERO]);
    }

    /// on the real tracks' slices (skipped without the game data) the index picks the scan's slice for
    /// every centre, points round them where lamps stand (beside and above the road), points midway between neighbours (the
    /// ties) and a deterministic scatter over the track's box.
    #[test]
    fn the_index_picks_the_scans_slice_on_real_tracks() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else {
            eprintln!("skipping: no game data");
            return;
        };
        let mut checked = 0;
        for track in ["alps", "monaco1", "coastal", "autobahn"] {
            let Ok(t) = nfs_assets::track::TrackAssets::load(&paths, track) else { continue };
            let Ok(table) = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data) else { continue };
            let c: Vec<Vec3> = table.slices.iter().map(|s| s.centre).collect();
            let mut pts = Vec::new();
            for (i, s) in table.slices.iter().enumerate() {
                pts.push(s.centre);
                let side = s.forward.cross(Vec3::Y).normalize_or_zero();
                for (a, h) in [(8.0, 6.0), (-8.0, 6.0), (14.0, 9.0), (-3.0, 12.0)] {
                    pts.push(s.centre + side * a + Vec3::Y * h);
                }
                if let Some(n) = c.get(i + 1) {
                    pts.push((s.centre + *n) * 0.5);
                }
            }
            let (lo, hi) = c.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
            let mut r = 0x1234_5678u32;
            let mut next = || {
                r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (r >> 8) as f32 / (1u32 << 24) as f32
            };
            for _ in 0..4000 {
                pts.push(lo + (hi - lo) * Vec3::new(next(), next(), next()));
            }
            checked += same(&c, &pts);
        }
        eprintln!("{checked} queries");
        assert!(checked > 0, "no track loaded");
    }
}
