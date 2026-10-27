//! ENHANCEMENT E26 — the wet road and the rain's look (render only).
//!
//! The wetness is the original's own: the sky `.bin`'s track B field 5 (`SkyBin::sample_track_b`), the
//! value that already drives the wheel spray (`WTSP`), the lamps' reflection on the road (0x451D60) and the AI's
//! wet speed table. Every road part (a track article part of surface type 1, the type `wheel_input` tests for
//! the wet spray) gets the value at the slice its mesh's centre lies in (once per mesh, cached); the shader then
//! darkens it, shines it, lays puddles and splash rings on it and shows the lit car lamps as streaks. A dry
//! slice has wetness 0 and the item stays on the original's path. Nothing here is read by the simulation, the
//! network or the replay: the values go only into the scene's items and header.

use crate::*;
use std::collections::HashMap;

/// What the wet road keeps between frames.
#[derive(Default)]
pub(crate) struct RainState {
    /// the track the caches belong to
    track: String,
    /// the wetness of each road mesh (the field 5 at the slice of its centre)
    wet: HashMap<nfs_renderer::MeshId, f32>,
    /// the track has a wet stretch at all (`None` = not looked at yet)
    pub(crate) any: Option<bool>,
    /// every step of the wetness along the track (computed with `any`)
    edges: Vec<nfs_renderer::RainEdge>,
    /// this frame's lit car lamps (world position, colour x strength), pushed by `lamp_draws`
    pub(crate) lamps: std::cell::RefCell<Vec<(Vec3, [f32; 3])>>,
}

/// ENHANCEMENT E26: a skid strip's wetness: the field 5 at the slice of the strip's first point (`hint` = the camera's
/// slice for the search). Returns the item's `csm` bit and `wet`; a dry track or stretch gives wetness 0 (the shader skips it).
pub(crate) fn strip_wet(table: Option<&SliceTable>, bin: Option<&nfs_formats::sky::SkyBin>, hint: Option<usize>, p: Vec3) -> (u8, f32) {
    let (Some(table), Some(bin)) = (table, bin) else { return (0, 0.0) };
    let lap = table.slices.len() as i32;
    if lap == 0 {
        return (0, 0.0);
    }
    (nfs_renderer::CSM_SKID, bin.sample_track_b(5, table.find_slice(p, hint.unwrap_or(0)) as i32, lap).clamp(0.0, 1.0))
}

impl App {
    /// ENHANCEMENT E26: the main view's rain: the road parts' wetness and `scene.rain`. Called after the scene's
    /// items are built; with the switch off, on a dry track or in the front end it only clears `scene.rain`.
    pub(crate) fn rain_frame(&mut self) {
        self.scene.rain = None;
        let lamps = std::mem::take(&mut *self.rain.lamps.borrow_mut());
        if !self.enh.is_on(nfs_renderer::E26) || self.vehicle.is_none() {
            return;
        }
        // the classified street lamps (E23's list, `street_lamps.rs`: lamp heads and tunnel lamps, not blinkers or glints)
        // within 100 m are lights the road shows streaks of, besides the lit car lamps
        let mut lamps = lamps;
        if self.rain.any != Some(false) {
            let eye0 = self.camera.position;
            // PERF: only the lamps that can show a streak in the view (a 30 m sphere round the lamp touches the frustum)
            let cone = self.view_cone();
            let sites: Vec<(Vec3, [f32; 3])> = self.street_lamp_sites().iter().filter(|s| (s.pos - eye0).length_squared() < 10000.0 && cone.is_none_or(|c| c.sees(s.pos, 30.0))).map(|s| (s.pos, s.color.map(|c| c * 0.9))).collect();
            lamps.extend(sites);
        }
        let (Some(world), Some(table), Some(bin)) = (self.world.as_ref(), self.slices.as_ref(), self.track.sky_bin.as_ref()) else { return };
        if self.rain.track != self.track.name {
            self.rain = RainState { track: self.track.name.clone(), ..Default::default() };
        }
        let lap = table.slices.len() as i32;
        if lap == 0 {
            return;
        }
        if self.rain.any.is_none() {
            // the steps of field 5 between two neighbouring keys (at most 12 slices apart; a longer span is a slow ramp the
            // per-part values follow well enough): the boundary plane stands at the middle slice, normal toward higher slices
            let mut edges = Vec::new();
            for k in bin.track_b.windows(2) {
                let ((a, va), (b, vb)) = ((k[0].0, k[0].1[5]), (k[1].0, k[1].1[5]));
                if (va - vb).abs() > 0.05 && b > a && b - a <= 12 {
                    let mid = ((a + b) / 2).max(0) as usize;
                    if let (Some(p), Some(q)) = (table.slices.get(mid), table.slices.get(mid + 1)) {
                        edges.push(nfs_renderer::RainEdge { pos: p.centre, dir: (q.centre - p.centre).normalize_or_zero(), before: va, after: vb, half: 8.0 });
                    }
                }
            }
            self.rain.edges = edges;
        }
        let any = *self.rain.any.get_or_insert_with(|| (0..lap).step_by(8).any(|s| bin.sample_track_b(5, s, lap) > 0.05));
        if !any {
            return;
        }
        for it in self.scene.items.iter_mut().filter(|it| it.csm & nfs_renderer::CSM_ROAD != 0) {
            it.wet = *self.rain.wet.entry(it.mesh).or_insert_with(|| world.road_centres.get(&it.mesh).map_or(0.0, |c| bin.sample_track_b(5, table.find_slice(*c, 0) as i32, lap).clamp(0.0, 1.0)));
        }
        let eye = self.camera.position;
        let mut lamps = lamps;
        lamps.sort_by(|a, b| (a.0 - eye).length_squared().total_cmp(&(b.0 - eye).length_squared()));
        lamps.truncate(nfs_renderer::RAIN_LAMPS);
        let mut env = nfs_renderer::RainEnv {
            zenith: [0.0; 3],
            horizon: [0.0; 3],
            camera_wet: bin.sample_track_b(5, self.cam_slice.unwrap_or(0) as i32, lap),
            time: (self.tick as f32 / 64.0) % 1000.0,
            lamps: [(Vec3::ZERO, [0.0; 3]); nfs_renderer::RAIN_LAMPS],
            n_lamps: lamps.len(),
            edges: [nfs_renderer::RainEdge::default(); nfs_renderer::RAIN_EDGES],
            n_edges: 0,
        };
        for (d, l) in env.lamps.iter_mut().zip(lamps.iter()) {
            *d = *l;
        }
        // the nearest wetness edges within 150 m of the eye
        let mut near: Vec<(f32, nfs_renderer::RainEdge)> = self.rain.edges.iter().map(|e| ((e.pos - eye).length_squared(), *e)).filter(|(d, _)| *d < 150.0 * 150.0).collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (d, (_, e)) in env.edges.iter_mut().zip(near.iter()) {
            *d = *e;
        }
        env.n_edges = near.len().min(nfs_renderer::RAIN_EDGES);
        // the sky the road shows: the sky pass's clear colour (the pale horizon the player sees), a little darker overhead
        let c = self.scene.clear_color;
        env.horizon = [c[0] as f32, c[1] as f32, c[2] as f32];
        env.zenith = [env.horizon[0] * 0.75, env.horizon[1] * 0.75, env.horizon[2] * 0.75];
        self.scene.rain = Some(env);
    }

    /// ENHANCEMENT E26: the mirror's road parts get the same wetness (the cache `rain_frame` filled).
    pub(crate) fn rain_items(&self, items: &mut [nfs_renderer::DrawItem]) {
        if self.scene.rain.is_none() {
            return;
        }
        for it in items.iter_mut().filter(|it| it.csm & nfs_renderer::CSM_ROAD != 0) {
            it.wet = self.rain.wet.get(&it.mesh).copied().unwrap_or(0.0);
        }
    }
}
