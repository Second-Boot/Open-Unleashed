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
    /// the lamp is inside a tunnel: lit on any track (a street lamp only on a night track)
    pub tunnel: bool,
}

/// Lamps farther than this from the eye are not considered (metres).
const REACH: f32 = 150.0;

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
        b"grts" | b"yrts" | b"brts" | b"lopl" | b"pmal" | b"ypml" | b"ydlb" => Some(LampClass::Street),
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
            let mut out = Vec::new();
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
                    let c = [((argb >> 16) & 0xFF) as f32 / 255.0, ((argb >> 8) & 0xFF) as f32 / 255.0, (argb & 0xFF) as f32 / 255.0];
                    let m = c[0].max(c[1]).max(c[2]).max(1e-3);
                    let c = c.map(|v| v / m * 0.7 + 0.3);
                    let height = self.slices.as_ref().zip(near.as_ref()).and_then(|(t, n)| n.nearest(f.position).map(|i| t.slices[i].centre.y)).map_or(6.0, |y| f.position.y - y);
                    out.push(LampSite { pos: f.position, color: c, height: height.clamp(2.5, 20.0), tunnel: class == LampClass::Tunnel });
                }
            }
            tracing::info!(lamps = out.len(), night = self.track.is_night(), "street lamps (ENHANCEMENT E23)");
            self.street_lamps = Some(out);
        }
        self.street_lamps.as_deref().unwrap_or(&[])
    }

    /// This frame's lamps for the main view: the nearest [`MAX_LAMPS`] within [`REACH`], the far ones
    /// faded out so the selection never pops (the fade starts at 75 % of the first left-out lamp's
    /// distance). `Lamps::default()` (none) with E23 off or on a day track without tunnel lamps.
    pub(crate) fn frame_lamps(&mut self) -> Lamps {
        let level = self.enh.level(nfs_renderer::E23);
        if level == 0 {
            return Lamps::default();
        }
        let night = self.track.is_night();
        let eye = self.camera.position;
        // PERF: only the lamps whose pool (the radius below) reaches into the view; the 32 slots then go to lamps that
        // light something on screen, not to the ones behind the camera, and the shader loops over fewer
        let cone = self.view_cone();
        let scale = nfs_renderer::street_lamp_pool(level).0;
        let mut near: Vec<(f32, LampSite)> = self
            .street_lamp_sites()
            .iter()
            .filter(|s| night || s.tunnel)
            .filter(|s| cone.is_none_or(|c| c.sees(s.pos, (1.5 * s.height + 6.0).clamp(10.0, 30.0) * scale)))
            .map(|s| (s.pos.distance(eye), *s))
            .filter(|(d, _)| *d < REACH)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cut = near.get(MAX_LAMPS).map_or(REACH, |c| c.0);
        let mut out = Lamps::default();
        // soft: a smaller, dimmer pool; normal: the authored default
        let (scale, gain) = nfs_renderer::street_lamp_pool(level);
        out.gain = gain;
        for (d, s) in near.iter().take(MAX_LAMPS) {
            let fade = 1.0 - ((d - 0.75 * cut) / (0.25 * cut)).clamp(0.0, 1.0);
            let k = fade * fade * (3.0 - 2.0 * fade);
            out.lamps[out.n as usize] = StreetLamp { pos: s.pos, color: [s.color[0] * k, s.color[1] * k, s.color[2] * k], radius: (1.5 * s.height + 6.0).clamp(10.0, 30.0) * scale };
            out.n += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::SliceIndex;
    use glam::Vec3;

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
