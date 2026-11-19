//! The race's surroundings the scene feed keeps between frames: the weather and its kept copy, the drops quad, the rain state, the tunnel / spot / street lamps, the flare definitions, the cover boxes and the lens flare.

use super::*;

/// The race's surroundings the scene feed keeps between frames: the weather and its kept copy, the drops quad, the rain state, the tunnel / spot / street lamps, the flare definitions, the cover boxes and the lens flare.
pub(crate) struct Ambient {
    /// the weather layer `[0x627E08]` (`nfs_game::weather`, `RE/notes/particles.md` §7): made with
    /// the track's particle systems, whose generator `[0x5CEAE0]` it draws from too
    pub(crate) weather: Option<nfs_game::weather::Weather>,
    /// The layer a restart or the automatic replay carries into its pass, to be primed again
    /// rather than made (0x4690A0 → 0x459260; it is deleted only at the session's end, 0x4690F0 →
    /// 0x458BB0).
    pub(crate) weather_kept: Option<nfs_game::weather::Weather>,
    /// ENHANCEMENT E40: the windscreen's corners of the car (keyed by model, style and the tiny-car flag) and the clock until
    /// which the renderer keeps the glass after the rain
    pub(crate) drops_quad: Option<((String, usize, bool), Option<[Vec3; 4]>)>,
    pub(crate) drops_until: f64,
    /// ENHANCEMENT E26: the wet road's caches and this frame's lamps (`rain.rs`)
    pub(crate) rain: rain::RainState,
    /// ENHANCEMENT E24: the cars' render-side tunnel lamp values (never part of a car's state).
    pub(crate) tunnel_lamps: std::cell::RefCell<car_lights::TunnelLamps>,
    /// ENHANCEMENT E15: the same ramp for the spotlights' gain at day (`App::spot_gain`).
    pub(crate) spot_cover: std::cell::RefCell<car_lights::TunnelLamps>,
    /// ENHANCEMENT E23: the track's street lamps, classified once per race
    pub(crate) street_lamps: Option<Vec<street_lamps::LampSite>>,
    /// `GameData/Render/flare.dat` (0x4522B0) and a sprite for every texture its records name
    pub(crate) flare_defs: Vec<nfs_formats::flare::FlareDef>,
    /// ENHANCEMENT E9 (ENH-TUNPOOL): the track's covered boxes, built once per race
    pub(crate) cover: Option<std::sync::Arc<Vec<nfs_renderer::CoverBox>>>,
    /// the sun object's lens-flare state (0x460460), per race
    pub(crate) lens: lens_flare::LensState,
}
