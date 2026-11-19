//! ENHANCEMENT E23: the street lamps as point lights, the application's half: the per-frame choice of the lamps that light the view.
//! The lamps themselves (classification, painted bulbs, clustering) are data of the track: [`nfs_game::lamps`]; the nearest-slice index
//! and the view cone are [`nfs_game::spatial`]. Render-only: nothing here reads or writes the simulation, a replay or the net.

use super::*;
use nfs_game::lamps::{SiteInputs, street_lamp_sites};
pub(crate) use nfs_game::lamps::{DAY_TUNNEL_GAIN, DUSK_STREET_GAIN, LampSite};
use nfs_game::spatial::ViewCone;
use nfs_renderer::{Lamps, StreetLamp, MAX_LAMPS};

/// AUTOBAHN-LAMPS / SHADOW-UNIFY: whether `track` lights its open street lamps by day: a day track with no sun from any source
/// (`nfs_game::data_sun::dusk_lamps`, `TrackWorld::dusk_lamps`: a data rule, APPROXIMATION; it was a per-track choice
/// for autobahn until SHADOW-UNIFY; the per-track override `NFS_DUSK_TRACKS` was removed 2026-11-19, REFACTOR-0119). No world (the
/// front end): none.
pub(crate) fn dusk_lamps(world: Option<&nfs_game::world::TrackWorld>) -> bool {
    world.is_some_and(|w| w.dusk_lamps)
}

/// Lamps farther than this from the eye are not considered (metres).
const REACH: f32 = 150.0;

/// MONACO-TUNNEL: painted bulbs farther than this from the eye are not lit (metres; faded out over its last 30 %). The harbour
/// tunnel's rows hold a bulb every ~12 m on each side, so within [`REACH`] the 32 slots all went to them, each reaching 24 m: the
/// lamp loop over every tunnel pixel cost about 4 ms at 2560 x 1600 (monaco1, M1 Pro). Their added light is a small share (the bake
/// is their light, [`PAINTED_GAIN`]) and E47's four shadow slots go to the lamps nearest the cars, so the far ones showed nothing.
/// APPROXIMATION (by the frame time and the stills).
const PAINTED_REACH: f32 = 60.0;

/// Where the fade-out of the lamps beyond the nearest [`MAX_LAMPS`] starts, as a share of the first left-out lamp's distance, and the
/// share of it the fade takes.
const FADE_START: f32 = 0.75;
const FADE_SPAN: f32 = 0.25;
/// MONACO-TUNNEL: the same for a painted bulb, as shares of [`PAINTED_REACH`].
const PAINTED_FADE_START: f32 = 0.7;
const PAINTED_FADE_SPAN: f32 = 0.3;

/// 1 for a distance `d` up to `start * reach`, falling linearly to 0 at `(start + span) * reach`.
fn fade_out(d: f32, reach: f32, start: f32, span: f32) -> f32 {
    1.0 - ((d - start * reach) / (span * reach)).clamp(0.0, 1.0)
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
        Some(ViewCone::new(c.position, fwd, right, right.cross(fwd), tan_y * aspect, tan_y, c.far))
    }

    /// The track's lamps, classified once per race (`None` until the first frame asks).
    pub(crate) fn street_lamp_sites(&mut self) -> &[LampSite] {
        if self.ambient.street_lamps.is_none() {
            // LAMP-TUNE: the covered stretches (the original's enclosure test of the road slices, `cover.rs`): a street lamp inside one
            // lights the tunnel on a day track too
            let boxes = if self.world.is_some() && self.slices.is_some() { self.cover_boxes() } else { Default::default() };
            let sites = street_lamp_sites(&SiteInputs { world: self.world.as_ref(), slices: self.slices.as_ref(), flare_defs: &self.ambient.flare_defs, track: &self.track, boxes: &boxes });
            self.ambient.street_lamps = Some(sites);
        }
        self.ambient.street_lamps.as_deref().unwrap_or(&[])
    }

    /// This frame's lamps for the main view: the nearest [`MAX_LAMPS`] within [`REACH`], the far ones
    /// faded out so the selection never pops (the fade starts at [`FADE_START`] of the first left-out lamp's
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
            let fade = fade_out(*d, cut, FADE_START, FADE_SPAN);
            let k = nfs_math::smooth01(fade);
            // LAMP-TUNE: a tunnel lamp on a day track lights at its share (the tube's bake already holds its light)
            // AUTOBAHN-LAMPS: an open street lamp of a dusk track at DUSK_STREET_GAIN
            // MONACO-TUNNEL: a painted bulb fades out toward PAINTED_REACH
            let k = if s.painted {
                let f = fade_out(*d, PAINTED_REACH, PAINTED_FADE_START, PAINTED_FADE_SPAN);
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
    use super::{fade_out, nearest_first};

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

    /// The fades are the shares of the reach they were spelled with: full inside, half-way in the span, none past it.
    #[test]
    fn the_fade_runs_over_its_span() {
        assert_eq!(fade_out(0.0, 100.0, 0.75, 0.25), 1.0);
        assert_eq!(fade_out(75.0, 100.0, 0.75, 0.25), 1.0);
        assert!((fade_out(87.5, 100.0, 0.75, 0.25) - 0.5).abs() < 1e-6);
        assert_eq!(fade_out(100.0, 100.0, 0.75, 0.25), 0.0);
        assert_eq!(fade_out(500.0, 100.0, 0.75, 0.25), 0.0);
    }
}
