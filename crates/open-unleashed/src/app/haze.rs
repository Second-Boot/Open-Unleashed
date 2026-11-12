//! ENHANCEMENT E36: the air of the main race view for the haze and the visible headlight beams.
//!
//! What the renderer needs (`nfs_renderer::Haze`): the track's own fog density at the camera (the scene fog the sky manager
//! sets, 0x45D3B0), the rain wetness the sky `.bin` carries (track B field 5, the value behind the spray, the lamps'
//! road reflection 0x451D60 and the AI wet table), how far inside a covered stretch the camera is (`TrackWorld::in_tunnel`,
//! bit 15 of the road's overhead flags, held and ramped like E24's lamps), how dark the sky is, and the lamps that glow
//! in the air (E23's classified list: the tunnel lamps inside a tunnel, the street lamps on a night track in fog or
//! rain). Render-only: nothing here reads or writes the simulation, a replay or the net.

use super::*;
use nfs_renderer::{HAZE_LAMPS, Haze, HazeLamp};

/// How far a lamp's glow in the air reaches (m) at the most, and how far from the eye a lamp counts.
const LAMP_RADIUS: f32 = 16.0;
const LAMP_REACH: f32 = 60.0;

/// The key of the camera's covered-stretch value in [`App::tunnel_lamps`] (cars use 0, 1 + i, 0x100 + i, 0x200 + i).
const COVER_KEY: u32 = 0x400;

impl App {
    /// ENHANCEMENT E36: this frame's `scene.haze`; `None` with the switch (or E7) off, in the front end, or when the air is
    /// clean (no fog, no rain, no covered stretch) so the renderer runs no pass.
    pub(crate) fn haze_frame(&mut self) {
        self.scene.haze = None;
        if !self.enh.is_on(nfs_renderer::E36) || !self.enh.is_on(nfs_renderer::E7) || self.vehicle.is_none() {
            return;
        }
        let eye = self.camera.position;
        let lamp_ms = (self.tick as f64 + self.pause_offset + self.pause_ticks) * 1000.0 / 64.0;
        let covered = self.world.as_ref().is_some_and(|w| w.in_tunnel(eye));
        let v = self.tunnel_lamps.borrow_mut().value_held(COVER_KEY, covered, lamp_ms);
        // a smoothstep of E24's ramp (350 ms in, 700 ms out): the haze neither starts nor stops with a jump
        let cover = v * v * (3.0 - 2.0 * v);
        let fog = self.scene.fog.map_or(0.0, |f| f.density);
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        let wet = match (self.track.sky_bin.as_ref(), self.cam_slice) {
            (Some(b), Some(s)) => b.sample_track_b(5, s as i32, lap).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let c = self.scene.clear_color;
        let lum = 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
        let dark = (1.25 - 1.5 * lum).clamp(0.1, 1.0);
        let night = self.track.is_night();
        let mut haze = Haze { fog, wet, cover, dark, day: !night, ..Haze::default() };
        let (beam, lamp) = haze.air(self.enh.level(nfs_renderer::E36));
        // TEST GLUE: `NFS_HAZE_LOG=1` logs the air per frame (for stills)
        static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *LOG.get_or_init(|| std::env::var_os("NFS_HAZE_LOG").is_some()) {
            eprintln!("haze tick {} fog {fog:.5} wet {wet:.3} cover {cover:.3} dark {dark:.3} night {night} -> beam {beam:.5} lamp {lamp:.5}", self.tick);
        }
        if beam <= 1e-5 && lamp <= 1e-5 {
            return;
        }
        if cover > 0.01 || night {
            let cone = self.view_cone();
            let mut near: Vec<(f32, Vec3, [f32; 3], f32)> = self
                .street_lamp_sites()
                .iter()
                .filter(|s| (s.air && cover > 0.01) || night)
                .filter(|s| s.pos.distance(eye) < LAMP_REACH && cone.is_none_or(|c| c.sees(s.pos, LAMP_RADIUS)))
                .map(|s| (s.pos.distance(eye), s.pos, s.color, s.height))
                .collect();
            near.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (d, pos, color, height) in near.into_iter().take(HAZE_LAMPS) {
                // the far ones fade out toward the reach; a tunnel lamp on a day track also with the covered value
                let fade = 1.0 - ((d - 0.7 * LAMP_REACH) / (0.3 * LAMP_REACH)).clamp(0.0, 1.0);
                let k = fade * fade * (3.0 - 2.0 * fade) * if night { 1.0 } else { cover * super::street_lamps::DAY_TUNNEL_GAIN };
                haze.lamps[haze.n as usize] = HazeLamp { pos, color: color.map(|c| c * k), radius: (1.2 * height + 6.0).clamp(9.0, LAMP_RADIUS) };
                haze.n += 1;
            }
        }
        self.scene.haze = Some(haze);
    }

    /// ENHANCEMENT E44: this frame's `scene.lbeam` (the lighthouse beam as soft light) and the hiding of the original's translucent
    /// sheets. Needs E7 (the float scene the beam is added into): with E44 or E7 off the sheets are drawn as in master and
    /// nothing is queued. The beam's transform is the animated article's own (`TrackWorld::lbeams`); within the draw distance
    /// only (the lighthouse is drawn there too). Render-only: nothing reaches the simulation, a replay or the net.
    pub(crate) fn lbeam_frame(&mut self) {
        self.scene.lbeam = None;
        let on = self.enh.is_on(nfs_renderer::E44) && self.enh.is_on(nfs_renderer::E7) && self.vehicle.is_some();
        let reach = 700.0 * self.enh_reach();
        let eye = self.camera.position;
        let Some(world) = self.world.as_mut() else { return };
        world.lbeam_hide(on);
        if !on {
            return;
        }
        let mut beams = nfs_renderer::LBeams::default();
        for b in world.lbeams() {
            if (beams.n as usize) < nfs_renderer::MAX_LBEAMS && b.apex.distance(eye) < reach {
                beams.beams[beams.n as usize] = b;
                beams.n += 1;
            }
        }
        if beams.n == 0 {
            return;
        }
        let c = self.scene.clear_color;
        let lum = 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
        beams.dark = (1.25 - 1.5 * lum).clamp(0.1, 1.0);
        self.scene.lbeam = Some(beams);
    }
}
