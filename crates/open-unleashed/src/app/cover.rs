//! ENHANCEMENT E9 (ENH-TUNPOOL): the track's covered stretches (tunnels, galleries) as boxes along the road slices.
//!
//! E9's sun shadow darkens the track's baked colour where the shadow map says "shadowed" (`shader.wgsl`, the track receiver). Inside
//! a tunnel the map says shadowed everywhere (the shell is a caster), and the baked colour there already carries the lamps' pools and
//! no sun: darkening it again made the pools dull and smeared, but only while the camera was OUTSIDE (the cover gate of
//! `render_scene.rs` is per camera, so it faded the shadows out once the camera was inside: the pools "lit up" at the entry).
//! The fix is per surface: a fragment inside a covered road slice's box takes no sun-shadow darkening at all. The covered state is
//! the original's own enclosure test (`TrackWorld::overhead_flags`, 0x473330's flags `& 0xC000`, the one the car's light and the gate
//! use) of every road slice; the ceiling's height over the slice (the same query) bounds the box.

use super::*;
use nfs_renderer::CoverBox;
use std::sync::Arc;

/// Over how many metres a covered run's ends ramp from open to covered (no seam at a portal).
const RAMP: f32 = 6.0;
/// Margin over the road's own width: the tunnel's walls stand beyond the lanes.
const SIDE_MARGIN: f32 = 4.0;
/// The longest hole (slices) in a covered run that is filled.
const MAX_HOLE: usize = 2;
/// Margin over the ceiling the overhead query found (m).
const CEIL_MARGIN: f32 = 0.5;

impl App {
    /// The track's covered boxes, built once per race (empty until the world and the slices exist).
    pub(crate) fn cover_boxes(&mut self) -> Arc<Vec<CoverBox>> {
        if self.cover.is_none() {
            let mut out = Vec::new();
            if let (Some(w), Some(t)) = (self.world.as_ref(), self.slices.as_ref()) {
                out = build(w, &t.slices);
            }
            tracing::info!(boxes = out.len(), "covered stretches (ENHANCEMENT E9 ENH-TUNPOOL)");
            self.cover = Some(Arc::new(out));
        }
        self.cover.clone().unwrap_or_default()
    }
}

fn build(w: &nfs_game::world::TrackWorld, slices: &[nfs_game::race::Slice]) -> Vec<CoverBox> {
    let n = slices.len();
    // the covered state and the ceiling's height over each slice
    let mut info: Vec<Option<f32>> = slices
        .iter()
        .map(|s| {
            let p = s.centre + s.up * 0.5;
            let hit = w.collision.ceiling(p.x, p.y, p.z, 3.5, 200.0)?;
            ((hit.surface & 0xC000) != 0).then(|| (hit.point - s.centre).dot(s.up))
        })
        .collect();
    let gap = |a: usize, b: usize| slices[a].centre.distance(slices[b].centre);
    // a gap of up to `MAX_HOLE` slices in a covered run is a hole in the road's overhead flags (castle: one slice inside a tunnel reads
    // open), not the tunnel's end: it is filled from its neighbours' ceilings
    for k in 1..n.saturating_sub(1) {
        if info[k].is_some() {
            continue;
        }
        let before = (k.saturating_sub(MAX_HOLE)..k).rev().find(|&p| info[p].is_some());
        let after = (k + 1..(k + 1 + MAX_HOLE).min(n)).find(|&q| info[q].is_some());
        if let (Some(p), Some(q)) = (before, after)
            && gap(p, q) < 40.0
        {
            info[k] = Some((info[p].unwrap_or(6.0) + info[q].unwrap_or(6.0)) * 0.5);
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if info[i].is_none() {
            i += 1;
            continue;
        }
        // a run: consecutive slice indices, each one next to the last
        let mut j = i;
        while j + 1 < n && info[j + 1].is_some() && gap(j, j + 1) < 40.0 {
            j += 1;
        }
        // the slice's own share of the road (half the gap to each neighbour; the faces the weights are given at), and the tested extent
        // (a whole gap either side: the boxes of a bend overlap, the outside of the curve has no wedge between them)
        let half = |k: usize| {
            let prev = if k > i { gap(k - 1, k) } else { 0.0 };
            let next = if k < j { gap(k, k + 1) } else { 0.0 };
            (prev.max(next) * 0.5).max(0.5)
        };
        let mut cum = vec![0.0f32; j - i + 1];
        for k in i + 1..=j {
            cum[k - i] = cum[k - i - 1] + gap(k - 1, k);
        }
        let (t0, t1) = (-half(i), cum[j - i] + half(j));
        tracing::debug!(first = i, last = j, length = t1 - t0, "covered run (ENHANCEMENT E9 ENH-TUNPOOL)");
        let ramp = |t: f32| (t - t0).min(t1 - t) / RAMP;
        for k in i..=j {
            let s = &slices[k];
            let hl = half(k);
            let c = cum[k - i];
            out.push(CoverBox {
                centre: s.centre,
                forward: s.forward,
                lateral: s.lateral,
                up: s.up,
                half_len: hl * 2.0,
                half_nom: hl,
                half_wid: s.width_a.max(s.width_b) + SIDE_MARGIN,
                ceil: info[k].unwrap_or(6.0) + CEIL_MARGIN,
                w_back: ramp(c - hl),
                w_front: ramp(c + hl),
            });
        }
        i = j + 1;
    }
    out
}
