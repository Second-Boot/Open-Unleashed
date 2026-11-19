//! The track's smackables (breakable roadside props): spawn data, broken-prop records, piece players and their samplers.
//! Moved out of `world.rs` as it was (the simulation reads the spawn data: no code changed).

use super::*;

/// A mission-scene instance's smackable, when its `animdefs.txt` definition (0x47ED90's lookup)
/// says `kSmackable` (0x4772F0's `def+0x10 == 3`): a car record of its own is built from it
/// (0x476ED0, then 0x4A17F0 / 0x4A1280) with its points from the library article's vertices in
/// the instance's frame. The article's own `tr` is **not** applied: 0x476ED0 reads only the
/// article's `Base` and `vt` (0x476F35, 0x476FAB), and 0x483360 takes the pose from the scene
/// record's matrix (0x4774F8…0x477515), which has no `tr` in it (0x482BF1 / 0x482EA0) — CONFIRMED.
/// (history: `game-history.md` §9.7) MEDIUM: the GEOM scale is folded
/// into the points and the instance's rotation and position kept as the body's pose — 0x476A80
/// splits the record matrix into the unscaled pose and the axis lengths `rec+0x58`, and where the
/// latter is used again was not read; and the vertices are the port's article meshes (the original
/// reads the `vt` stream).
pub fn smackable_spawn(track: &TrackAssets, art: &nfs_assets::track::ArticleMeshes, inst: &nfs_assets::track::PropInstance) -> Option<SmackableSpawn> {
    if inst.source != 1 {
        return None;
    }
    // the matched **template's** own tag (`art.tag`), not the element's unmatched fourcc — same
    // fourcc-miss fallback substitution as the draw loop's `loop_piece` and `scene_props`
    // (0x482ebe…0x482ec0, e517d04f), missed here until now: `art` is already the caller's
    // fallback-resolved article ([`TrackWorld::upload`]'s own `art` at the `smackable_spawn` call
    // site), so this lookup must follow it too. Unreached on shipped data (every mission-scene tag
    // matches one of the 70 shared library articles), same as the other two.
    let def = track.animdefs.lookup(art.tag).filter(|d| d.collide_type == "kSmackable")?;
    let local = Mat4::from_scale(inst.scale);
    let verts: Vec<Vec3> = art.meshes().iter().flat_map(|mm| mm.mesh.vertices.iter().map(|v| local.transform_point3(Vec3::from(v.position)))).collect();
    let (_, rot, pos) = inst.transform.to_scale_rotation_translation();
    // 0x483360: the record's points are centred and its position moves by the centre
    let mut points = nfs_physics::smackable::point_cloud(&verts);
    let position = nfs_physics::smackable::centre_record(&mut points, pos);
    Some(SmackableSpawn {
        owner: SmackableOwner::Element(inst.element),
        position,
        orientation: glam::Mat3::from_quat(rot).transpose(),
        points,
        mass: def.mass,
        audio_type: def.audio_type_index().unwrap_or(0),
    })
}

/// A track article's smackable (0x4772F0 over the instance bundle's items — every article with a
/// `tr`, 0x47859E — whose `Base` byte 0 lacks 0x40, 0x477377, and whose definition says
/// `kSmackable`, 0x4773EE): its pose is the item record's — the `tr`'s position and its rows,
/// normalised as 0x4A1510 does — and its points `SIGN`'s 0x476B30 (the vertices scaled by the
/// rows' lengths, `nfs_physics::smackable::sign_point_cloud`) or everyone else's 0x476ED0, which
/// takes no scale (`suspension-collision.md` §9 "The track's smackables"). MEDIUM: the vertices
/// are the port's lowest-LOD meshes (the original reads a `vt` stream: 0x476ED0 the article's,
/// 0x476B30 the one the LOD record `Base+0x64 + 12·Base[+4]` names).
pub fn track_smackable_spawn(track: &TrackAssets, art: &nfs_assets::track::ArticleMeshes) -> Option<SmackableSpawn> {
    let def = track.animdefs.lookup(art.tag).filter(|d| d.collide_type == "kSmackable")?;
    let tr = art.transform;
    let rows = [tr.x_axis.truncate(), tr.y_axis.truncate(), tr.z_axis.truncate()];
    let lengths = Vec3::new(rows[0].length(), rows[1].length(), rows[2].length());
    let verts: Vec<Vec3> = art.meshes().iter().flat_map(|mm| mm.mesh.vertices.iter().map(|v| Vec3::from(v.position))).collect();
    let mut points = if def.tag.eq_ignore_ascii_case("SIGN") { nfs_physics::smackable::sign_point_cloud(&verts, lengths) } else { nfs_physics::smackable::point_cloud(&verts) };
    // 0x483360: the record's points are centred and its position moves by the centre
    let position = nfs_physics::smackable::centre_record(&mut points, tr.w_axis.truncate());
    let unit = |v: Vec3| v.normalize_or_zero();
    Some(SmackableSpawn {
        owner: SmackableOwner::Article(art.index),
        position,
        orientation: glam::Mat3::from_cols(unit(rows[0]), unit(rows[1]), unit(rows[2])).transpose(),
        points,
        mass: def.mass,
        audio_type: def.audio_type_index().unwrap_or(0),
    })
}

/// What a smackable's body draws: a mission scene's GEOM element, or a track article.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmackableOwner {
    Element(usize),
    Article(usize),
}

/// A smackable, as `nfs_physics::smackable::Smackable::new` takes it.
#[derive(Debug, Clone)]
pub struct SmackableSpawn {
    pub owner: SmackableOwner,
    pub position: Vec3,
    /// rows = the body axes, as `Vehicle::orientation`
    pub orientation: glam::Mat3,
    /// `nfs_physics::smackable::point_cloud` of the article, in the body's frame
    pub points: Vec<Vec3>,
    /// the definition's `mass` (kg)
    pub mass: f32,
    /// the definition's `audioType` index
    pub audio_type: u32,
}

impl SmackableSpawn {
    /// The authored pose, body to world.
    pub fn pose(&self) -> Mat4 {
        body_pose(self.position, self.orientation)
    }
}

/// A body's pose as a matrix from its position and its orientation (rows = the body axes).
pub fn body_pose(position: Vec3, orientation: glam::Mat3) -> Mat4 {
    Mat4::from_translation(position) * Mat4::from_mat3(orientation.transpose())
}

/// A breakable prop drawn in the world (`TrackWorld::break_props`).
pub(super) struct BrokenProp {
    /// its index in `TrackWorld::props`
    pub(super) prop: usize,
    /// its own article's draw items, every LOD — drawn while it is whole, those of the LOD the walk
    /// answers its [`Self::slot`] with
    pub(super) items: std::ops::Range<usize>,
    /// the instance's matrix, `world+0x40 + 56·id` (0x539CD0 at 0x476A54)
    pub(super) instance: Mat4,
    /// `def+0x80`, already scaled by 0.001
    pub(super) velocity_factor: f32,
    /// one per piece of the definition
    pub(super) templates: Vec<PieceTemplate>,
    /// the pieces started (the record's entries `rec+0x34`, `rec+0x14` of them)
    pub(super) players: Vec<PiecePlayer>,
    /// this frame's draw items of the pieces
    pub(super) pieces: Vec<DrawItem>,
    /// this frame's sample of the lead player (`rec+0x1C`, piece 0's twin) — slot 5's
    /// `lead sample × instance` (0x4769E0 with −1) for the walk's sphere
    pub(super) lead: Option<Mat4>,
    pub(super) state: BreakState,
    /// its [`TrackWorld::instance_culls`] entry (none without a `Base`: drawn always)
    pub(super) slot: Option<usize>,
}

/// A track smackable's anim-world record (0x4772F0): entry 0 and the lead are the one player
/// 0x471180 on its prop record (0x4775F7), whose sample 0x4711E0 follows the body.
#[derive(Debug, Clone)]
pub(super) struct SmackAnim {
    /// the `.crp` article index — the record's own article, which `rec+8` takes (0x476876)
    pub(super) article: usize,
    /// its [`TrackWorld::instance_culls`] entry (none without a `Base`: drawn always, never sampled)
    pub(super) slot: Option<usize>,
    /// the prop record's position `+0x28…+0x30` — the item record's, where the body started
    pub(super) home: Vec3,
    /// the body's position now (`car+0x330`, through the pose pointers 0x483590)
    pub(super) position: Vec3,
    /// the player's state `+0xC`: 0, or 1 once a draw's sample found the body 1 cm off its place
    pub(super) state: u8,
    /// `rec+8` set (0x476876) — kept for the race
    pub(super) armed: bool,
}

impl BrokenProp {
    pub(super) fn hides(&self, item: usize) -> bool {
        !matches!(self.state, BreakState::Whole) && self.items.contains(&item)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BreakState {
    Whole,
    /// started on game tick `start`; `lead_done` once the lead player has run out
    Playing { start: u32, lead_done: bool },
    Gone,
}

pub(super) struct PieceTemplate {
    pub(super) meshes: Vec<(MeshId, MaterialId)>,
    /// the `BaseGroup` article's `Base` sphere in its own frame (0x4786A0's entry-0 article)
    pub(super) base: Option<(Vec3, f32)>,
    pub(super) track: Option<nfs_formats::crp::AnimTrack>,
    pub(super) seconds_per_frame: f32,
    pub(super) flags: nfs_formats::animdefs::PieceFlags,
}

/// 0x471490's player: the track's `+0x10` heading (radians here) and `+0x14` drift.
pub(super) struct PiecePlayer {
    pub(super) template: usize,
    pub(super) heading: f32,
    pub(super) drift: Vec3,
}

/// `AnimPlayer::sample` 0x470E90 with 0x470F70 for a piece `ticks` game ticks after its start:
/// `rate = secondsPerFrame / (1/64)` ticks a frame (`[0x628938]+8`, 0x471700's 0.015625);
/// `f = ticks / rate`; a looping track wraps at `frames − 2`, a one-shot holds `frames − 1`; the
/// quaternion and the position are lerped component by component (0x48A150, **not** normalised),
/// the matrix built from that quaternion as it is (0x48A1E0) with the lerped scale; `DIRECTED`
/// (flag 8) turns it after about y by the heading (0x47108A…0x4710C0, `M·R`), `LANDFLAT` (flag
/// 0x10) before about y by `−heading·(1 − (i + frac)/frames)` (0x4710C3…0x47112C, `R·M`); the
/// translation then gains `drift · ticks · rate` (0x470F17…0x470F51). The answer's flag is
/// `i + 1 < frames`, the player still running.
pub(super) fn sample_piece(track: &nfs_formats::crp::AnimTrack, seconds_per_frame: f32, flags: nfs_formats::animdefs::PieceFlags, heading: f32, drift: Vec3, ticks: u32) -> (Mat4, bool) {
    let n = track.frames.len();
    if n == 0 {
        return (Mat4::IDENTITY, false);
    }
    let rate = seconds_per_frame / 0.015625;
    let f = ticks as f32 / rate;
    let mut i = f as usize;
    let mut frac = f - i as f32;
    if flags.looping && n > 2 {
        i %= n - 2;
    } else if i + 2 >= n {
        i = n - 1;
        frac = 0.0;
    }
    let (a, b) = (&track.frames[i], &track.frames[(i + 1).min(n - 1)]);
    let lerp = |x: f32, y: f32| x + (y - x) * frac;
    let q = glam::Quat::from_xyzw(lerp(a.rotation[0], b.rotation[0]), lerp(a.rotation[1], b.rotation[1]), lerp(a.rotation[2], b.rotation[2]), lerp(a.rotation[3], b.rotation[3]));
    let pos = Vec3::from(a.position).lerp(Vec3::from(b.position), frac);
    let scale = Vec3::from(a.scale).lerp(Vec3::from(b.scale), frac);
    let mut m = Mat4::from_translation(pos) * Mat4::from_mat3(glam::Mat3::from_quat(q)) * Mat4::from_scale(scale);
    if flags.directed {
        // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable rotation, not glam/std; original rotation builder 0x539BD0 at 0x4710AC / 0x47111D.
        m = nfs_math::portable::mat4_rotation_y(heading) * m;
    }
    if flags.landflat {
        m *= nfs_math::portable::mat4_rotation_y(-heading * (1.0 - (i as f32 + frac) / n as f32));
    }
    let off = drift * ticks as f32 * rate;
    m.w_axis += off.extend(0.0);
    (m, i + 1 < n)
}

/// `[WTrack+0x34]` holds at most **125** smackables: 0x4888B0 stops adding at `size ≥ 0x7D`
/// (0x488926), the mission scene's first (0x487DEE), then the instance bundle's (0x487DFE).
pub const MAX_SMACKABLES: usize = 125;

/// The `.crp` articles 0x4772F0 makes smackables of among the instance bundle's items, in `.crp`
/// order (the bundle's item order): an article with a `tr` (`WInstanceBundle::Load` takes those,
/// 0x47859E) that is not a library template (`Base & 0x8000` — the scene bundle's,
/// `RENDERING.md` "The scene item's draw matrix"; whether a template record is itself a scene
/// item the anim world builds for is not read, so none is taken here), whose `Base` byte 0 lacks
/// 0x40 (0x477377) and whose definition (0x47ED90) says `kSmackable` (0x4773EE). The loader
/// 0x4750B0 builds no collision object for these (0x47533A), so 0x498DE0 never meets them
/// (`suspension-collision.md` §9 "The track's smackables").
pub fn track_smackable_articles(track: &TrackAssets) -> Vec<usize> {
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
    arts.iter()
        .enumerate()
        .filter_map(|(k, a)| {
            let g = track.crp.article_geometry(&track.crp_data, a).ok()?;
            let b = g.base.as_ref()?;
            (g.transform.is_some() && !b.is_library() && b.flags & 0x40 == 0 && track.animdefs.lookup(b.tag_u32()).is_some_and(|d| d.collide_type == "kSmackable")).then_some(k)
        })
        .collect()
}
