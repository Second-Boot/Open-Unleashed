//! The track's projectors (kind-4 `ef` parts of the placed articles): a pure function of the track's data, so the tools
//! (`asset-extractor hd-export`) can read them without the game crate. Moved out of `nfs_game::world::lights` as it was.

use crate::track::TrackAssets;
use glam::{Mat4, Vec3};
use nfs_formats::bytes;

/// One kind-4 `ef` part: its sixteen floats from `+8`.
#[derive(Debug, Clone, PartialEq)]
pub struct Projector {
    /// the `.crp` index of the article carrying it, and the part's own index
    pub article: usize,
    pub part: u16,
    /// `[0…2]`, `[3]`
    pub point: Vec3,
    pub radius: f32,
    /// `[6…8]`, `[10]` and `[11…13]`, `[15]`: `u = u.xyz · world + u.w`, likewise `v` (0x446130)
    pub u: [f32; 4],
    pub v: [f32; 4],
    /// `+0x1C` of the part, the texture's name (0x44B000)
    pub texture: String,
}

/// The kind-4 `ef` parts of every placed article. The planes are taken as the file has them: the
/// `Shadow` articles that carry them have no `tr` on any shipped track (canyon 59, farmland 18,
/// castle 13), so they are in world space already; the point still goes through the transform.
pub fn track_projectors(track: &TrackAssets) -> Vec<Projector> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x48 || bytes::read_u32(d, 0) != 4 {
                continue;
            }
            let f = |i: usize| {
                let o = 8 + 4 * i;
                bytes::read_f32(d, o)
            };
            out.push(Projector {
                article,
                part: p.part_index(),
                point: tr.transform_point3(Vec3::new(f(0), f(1), f(2))),
                radius: f(3),
                u: [f(6), f(7), f(8), f(10)],
                v: [f(11), f(12), f(13), f(15)],
                texture: d[0x1C..0x20].iter().map(|&c| c as char).collect(),
            });
        }
    }
    out
}
