//! Buffers the per-frame scene build keeps between frames, so a frame does not allocate its lists
//! again: each is taken out of `App::scratch`, cleared and filled, and put back with its capacity.

use crate::*;

#[derive(Default)]
pub(crate) struct SceneScratch {
    /// the track's near / far / rest lists of the main pass and of the mirror's (`track_draw_lists_into`)
    pub(crate) lists: nfs_game::world::TrackLists,
    pub(crate) mirror_lists: nfs_game::world::TrackLists,
    /// the far list with the cars' and skid marks' entries, the one farthest-first run is sorted from
    pub(crate) blended: Vec<(u32, nfs_renderer::DrawItem)>,
    /// `(key, index)` pairs the runs are sorted as (`nfs_renderer::sort_order`)
    pub(crate) order: Vec<(u32, u32)>,
    /// one car's own entries, the shadow patch's vertices and its lamp draws
    pub(crate) car_items: Vec<nfs_renderer::DrawItem>,
    pub(crate) shadow_verts: Vec<nfs_renderer::Vertex>,
    pub(crate) lamp_draws: Vec<LampDraw>,
    /// a skid strip's and a streak run's vertices
    pub(crate) skid_verts: Vec<nfs_renderer::Vertex>,
    pub(crate) streak_verts: Vec<nfs_renderer::Vertex>,
    /// the effects' entries of a pass
    pub(crate) effects: Vec<nfs_renderer::DrawItem>,
    /// the mirror's own cars' and skid marks' entries, its merged track list and its scene's items
    pub(crate) mirror_draws: Vec<Option<nfs_game::world::ArticleDraw>>,
    pub(crate) mirror_dyn: Vec<(u32, nfs_renderer::DrawItem)>,
    pub(crate) mirror_merged: Vec<nfs_renderer::DrawItem>,
    pub(crate) mirror_items: Vec<nfs_renderer::DrawItem>,
}
