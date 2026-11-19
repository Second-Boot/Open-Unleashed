//! The track's props for the physics tree (`nfs_physics::props`) and the original's tree insertion order.
//! Moved out of `world.rs` as it was (the simulation reads these: no code changed).

use super::*;

/// The AABB fold 0x486340 (and 0x47B660) run: eight corners of a local box, each carried through
/// a placement matrix, min/maxed per axis. **Read 2026-09-27 (this pass): the source is resolved.**
/// `0x44FE70` builds an 8-byte `{ptr, byte=1, word}` handle whose `ptr` is not per-article (it is
/// the 2nd stack argument of the loader `0x4750B0` itself, read straight off `0x4750B0`'s own
/// frame at both use sites 0x4751F2/0x4752E9 — the same object for every article of the track);
/// the per-article piece is the `word` (the instance id, `esi+0x2e`). `0x486340`'s
/// `call [[handle.ptr]+8]` (0x48639A) — traced to **`0x4893A0`** (found by scanning `.rdata` for
/// the vtable holding `handle.ptr`'s class: `getBBox` sits at that vtable's slot 4, byte +0x10
/// (`0x477F40`; `crp-geometry.md` §2.2's "getTransform slot3 / getBBox slot4" — `0x477DB0` is
/// slot3), so slot 2, byte +8, is a *different* entry, one table row over: `0x4893A0`) — takes the
/// `this`/id and calls `getBBox` on itself (`call [eax+0x10]` at 0x4893BB, i.e. the vtable's own
/// slot 4) to fill two local `f32[3]`s, then combines their six components (x/y/z of each) into
/// the eight corners of the axis-aligned box **those two vectors already are** — no rotation, no
/// scale, in `0x4893A0` itself. `getBBox` (`0x477F40`) in turn reads `Base+0x14` / `Base+0x20` —
/// the article's own **stored** `bbox_min`/`bbox_max` fields (`crp-geometry.md` §2.2, CONFIRMED
/// equal to the LOD0 `vt` min/max for every article of two tracks tested there) — not a vertex
/// walk. So **the article's local box is `Base.bbox_min`/`Base.bbox_max`**, read once per article,
/// not folded from any mesh; the previous pass's "lowest-LOD vertex extent" stand-in was numerically
/// exact (by that same CONFIRMED equality) but attributed to the wrong field. `0x486340`'s own fold
/// (0x486404…0x486478) does not itself carry a rotation matrix — no `fmul`/`fadd` against `tr`
/// appears in it, only `fcomp`/conditional-store — so whatever rotation the placed instance gets is
/// applied by the *caller* of the constructor chain (the `kBox` branch's own footprint transform at
/// 0x475241…0x4752BE calls `0x474CB0` before `0x486C20`, confirming the transform-then-store pattern
/// there; the cylinder/AABB path's own transform site was not pinned further, so the port keeps
/// applying `tr` to the box's eight corners here, matching the general "rotated corners" rule
/// `main.rs`/f8198d01 already settled for the car/world-grid sibling **0x47B660**, and matching this
/// pass's own sweep — see `track_props` below — which found the choice makes no numeric difference
/// for any shipped rotated prop).
pub(super) fn rotated_local_box_aabb(lo: Vec3, hi: Vec3, tr: Mat4) -> (Vec3, Vec3) {
    let corners = [
        Vec3::new(lo.x, lo.y, lo.z),
        Vec3::new(lo.x, lo.y, hi.z),
        Vec3::new(lo.x, hi.y, lo.z),
        Vec3::new(lo.x, hi.y, hi.z),
        Vec3::new(hi.x, lo.y, lo.z),
        Vec3::new(hi.x, lo.y, hi.z),
        Vec3::new(hi.x, hi.y, lo.z),
        Vec3::new(hi.x, hi.y, hi.z),
    ];
    corners
        .iter()
        .map(|c| tr.transform_point3(*c))
        .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p)))
}

/// The collision object of one article placed by `tr` — the part [`track_props`] and
/// [`scene_props`] share once each has chosen its article, its `animdefs.txt` definition (`def`) and
/// its placement: `None` when the article has no geometry at its lowest LOD or its `collideType` is
/// neither `kCylinder` nor `kBox` (the callers then skip it, as before). `article` is the `.crp`
/// index the prop carries.
pub(super) fn build_prop(tags: &[(u32, Option<[u32; 2]>)], g: &nfs_formats::crp::ArticleGeometry, b: &nfs_formats::crp::Base, def: &nfs_formats::animdefs::AnimDef, tr: Mat4, article: usize) -> Option<nfs_physics::props::Prop> {
    use nfs_physics::props::{Breakable, Prop, PropShape, box_footprint};
    let lod = g.primitives.iter().map(|p| p.lod).min()?;
    let mut skipped = 0;
    let local: Vec<Vec3> = article_meshes(g, lod, &mut skipped)
        .iter()
        .flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
        .collect();
    if local.is_empty() {
        return None;
    }
    // The AABB's own source is `Base.bbox_min`/`Base.bbox_max` (getBBox, 0x477F40, reading
    // Base+0x14/+0x20 — `rotated_local_box_aabb`'s doc above), not the lowest-LOD vertex fold
    // this pass replaces; `local` still feeds `box_footprint` below, a separate field.
    let (mn, mx) = rotated_local_box_aabb(Vec3::from(b.bbox_min), Vec3::from(b.bbox_max), tr);
    // 0x476380 finds each piece's `QuatGroup` among the instances by tag (0x478640 / 0x478670)
    // and gives up at one whose `Anim` part is missing or has both words 0 (0x476501…0x47651F)
    let breakable = (!def.pieces.is_empty()).then(|| Breakable {
        bands: def.bands.clone(),
        band_pieces: def.band_pieces.clone(),
        resolved: def
            .pieces
            .iter()
            .map(|(_, p)| first_by_tag(tags, group_tag(&p.quat_group)).and_then(|k| tags[k].1).is_some_and(|w| w != [0, 0]))
            .collect(),
        started: std::cell::Cell::new(None),
        article,
    });
    let shape = match def.collide_type.as_str() {
        "kCylinder" => PropShape::Cylinder { radius: 0.25 },
        "kBox" => {
            let corners = box_footprint(&local).map(|c| tr.transform_point3(c));
            let c = (mn + mx) * 0.5;
            let radius = Vec3::new(mx.x - c.x, 0.0, mx.z - c.z).length() * 1.05;
            PropShape::Box { corners, radius }
        }
        _ => return None,
    };
    let class = if matches!(shape, PropShape::Cylinder { .. }) { def.radius as i32 as u32 } else { 0 };
    Some(Prop { shape, aabb_min: mn, aabb_max: mx, class, breakable, drive_through: def.drive_through, article })
}

/// The track loader's collision objects (0x4750B0 at 0x475152…0x475302): every article of the
/// track's `.crp` whose `Base` byte 0 lacks 0x40 (0x47515A) and whose tag is set, through
/// `AnimDefs::lookup` (0x47ED90 — an unknown tag gets the file's first definition, `ENDW`):
///
/// * `kCylinder` → a cylinder whose radius is the constant **0.25** the loader pushes, with
///   `int(def.radius)` — `_ftol` 0x5A0F98 — where the base keeps the event class `+0x20`
///   (0x4751D5…0x47520B; the arguments are read as the image passes them);
/// * `kBox` → a box on 0x474CB0's footprint of the article's vertices, through its `tr`, class 0
///   (0x475241…0x475302), radius `|(max.x − c.x, 0, max.z − c.z)| × 1.05`.
///
/// The AABB is the axis-aligned box of the article's **local** box's eight corners, each carried
/// through the instance's placement `tr` (rotation and translation) — not the AABB of the
/// individually-transformed mesh vertices (which under-bounds a rotated article, since a rotated
/// point's extreme coordinate need not come from a vertex the local box itself reaches). Read
/// 2026-09-27: both object constructors call the shared base ctor 0x486340 first
/// (`WTreeElem_CylinderObject` 0x486690 at 0x4866AE, `WTreeElem_BoxObject` 0x486C20 at 0x486C42),
/// which calls a vtable+8 slot (0x48639A) that fills an 8×0x10-byte-stride buffer, then folds it
/// into a running per-axis min/max over exactly 8 iterations (0x486404…0x486478) — the identical
/// shape as the world-box fold `main.rs`/f8198d01 already traced at 0x47B6E2…0x47B760 below
/// 0x47B6C6's own vtable+8 call. No inflation follows either fold. The `kBox` shape's own four
/// XZ footprint corners (0x474CB0, [`box_footprint`]) are a separate field the loader builds and
/// transforms the same way (0x475241…0x4752BE) — stored at `+0x34…+0x58`, not fed back into the
/// AABB; only the box's *radius* (`|(max−c).xz| × 1.05`, 0x486C91) reads the AABB `mn`/`mx`.
///
/// **Read 2026-09-27, later (this pass): the local box's own source is resolved** — see
/// [`rotated_local_box_aabb`]'s doc. It is `Base.bbox_min`/`Base.bbox_max` (`getBBox`, 0x477F40,
/// itself called by the vtable+8 function 0x4893A0 above), not `local`'s vertex fold; `local` is
/// kept only for `box_footprint` below, an unrelated field. No shipped prop's `mn`/`mx` changes:
/// swept `coastal` (42 box/cylinder props), `alps` (293) and `monaco1` (64) — every one is
/// bit-identical to the old vertex-derived value (`crp-geometry.md` §2.2's own CONFIRMED equality
/// of `bbox_min`/`bbox_max` to the LOD0 `vt` extent already predicted this).
pub fn track_props(track: &TrackAssets) -> Vec<nfs_physics::props::Prop> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for (index, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        // a `WAnimBundle` / `WMidGroundBundle` article has no `PCollideArticleBundle`
        // (`[bundle+0xC] = 0`, so `0x488090` never calls `0x4750B0` for it, 0x48838B) — no
        // collision object either
        if matches!(track.crp.bundle_of(&track.crp_data, a), Ok(nfs_formats::crp::BundleKind::Anim | nfs_formats::crp::BundleKind::MidGround)) {
            continue;
        }
        // A scene-library article (`'tr'` ∧ `Base.flags & 0x8000`, e.g. `EndWall`, `ExplodingBox`,
        // `Crabcrate_Rigid03`) is a *template* the `.scn` GEOM elements spawn from, not an object of
        // the track: `WInstanceBundle::Load` 0x4781A0 parks it in its auxiliary `+0x34` table, never
        // in the `+0x18`/`+0x1C` pair `0x4750B0` walks (`track_tree_order`, "Library articles are
        // excluded"). Its collision object exists only where a scene element places it
        // ([`scene_props`]). Without this gate `Car_propPass`' "every prop is tried" loop collided
        // with the template at its own `tr` — `EndWall`'s 12 m box at the world origin was an
        // invisible wall across the `0M01` skidpad course (`RE/notes/skill-tests.md` §14, FD-WALL).
        if track.crp.is_scene_member(&track.crp_data, a).unwrap_or(false) {
            continue;
        }
        let Some(def) = track.animdefs.lookup(b.tag_u32()) else { continue };
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        out.extend(build_prop(&tags, &g, b, def, tr, index));
    }
    out
}

/// `WSceneBundle`'s own tree contribution: the scene's `GEOM_ELEMENT`s spawned by `0x46D1D0`
/// (called once at `0x487abd`) → `0x483260` → **0x482EA0** (template by fourcc, `0x478640`; a
/// fourcc miss takes template index 0, `0x482ebe`…`0x482ec0`; at most 64 live slots, `0x482ec2`),
/// placed by the **element's** transform, never the article's `tr` (`0x482BF1`); the `0x4750B0`
/// walk then builds one [`Prop`] for a `kCylinder` / `kBox` template article and nothing for any
/// other `collideType`. Empty on every shipped non-skill-test race
/// (`scene_props_are_empty_on_a_shipped_race`); `ENDW` (`kBox`) appears only in a skill test's
/// `_ST<N>.scn`. The full trace (the cast count `0x46D9C0`, the `DFLT` fallback, the survey of the
/// shipped `.scn` files): `RE/notes/game-history.md` §33.
pub fn scene_props(track: &TrackAssets, geometry: &nfs_assets::track::TrackGeometry) -> Vec<nfs_physics::props::Prop> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for inst in geometry.instances.iter().take(64) {
        let Some(art) = geometry.library.iter().find(|a| a.tag == inst.tag).or_else(|| geometry.library.first()) else { continue };
        let Some(a) = arts.get(art.index) else { continue };
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        // the matched **template's** own tag (`art.tag`), not the element's unmatched fourcc — the
        // fallback substitutes the whole `{tag, Article*}` pair (0x482EA0's own copy), so collision
        // behaviour follows the template that was actually spawned.
        let Some(def) = track.animdefs.lookup(art.tag) else { continue };
        // the element's own placement, never the library article's `tr` ("The scene bundle's
        // items": `0x482BF1` stores the template record as identity/zero).
        let tr = inst.transform * Mat4::from_scale(inst.scale);
        out.extend(build_prop(&tags, &g, b, def, tr, art.index));
    }
    out
}

/// The `insertAll(C)` half of `WTrack+0x24`'s real insertion order (`RE/RENDERING.md` "The tree,
/// built" / "Bundle order, read 2026-09-28"): `0x4750B0` is called once per bundle **that owns a
/// `PCollideArticleBundle`** (`0x488090`, 0x488364…0x48839F: `dynamic_cast` of `[bundle+0xC]`, a null
/// skips) — `WInstanceBundle` (0x47860F), `WStaticBundle` (0x483796) and `WSceneBundle`
/// (0x4830A2, below) — in the order `WInstanceBundle → WStaticBundle` here (`Crp::bundle_of`'s
/// variants; `WAnimBundle` keeps `[+0xC] = 0` from 0x489160 and `WMidGroundBundle::Load` writes it
/// 0 at 0x47CED2 / 0x47CEE7: **neither is walked** — `RE/notes/COVERAGE-2026-09-26.md`
/// §14.80; `WLineBundle` neither), each bundle's claimed articles in `.crp` file order,
/// and within one article either its patches (`0x4751BD`'s `type == 2` path, `TriangleGrid::
/// patches`' own primitive order — one [`nfs_physics::track_tree::TreeItem::Patch`] per patch) or
/// its one `kCylinder`/`kBox` object (`type == 1`, one [`nfs_physics::track_tree::TreeItem::Prop`])
/// — never both, in the original (`0x4751BD`/`0x475238`, read 2026-09-28).
///
/// **Library articles are excluded**, not merely reordered: `WInstanceBundle::Load` (`0x4781A0`)
/// copies a library survivor into its *auxiliary* `+0x34` table, not the generic `+0x18`/`+0x1c`
/// pair `0x4750B0`'s tree-fill walk actually reads (`RE/RENDERING.md` "Bundle order, read
/// 2026-09-28") — so a library article never reaches the tree through `WInstanceBundle` at all.
/// `world` (`TrackGeometry::world`, already the non-library split `TrackAssets::build_geometry`
/// makes) is exactly this port's stand-in for "every article the generic array can hold" — its own
/// `index` is the `.crp` article index [`nfs_physics::collision::Patch::article`] and
/// [`nfs_physics::props::Prop::article`] carry too.
///
/// **`WSceneBundle` is walked separately, first** (corrected 2026-09-28, `scene_props` above):
/// unlike the four bundles this function's own loop covers, its `[+0x1c]` live-slot array is not
/// keyed by `.crp` article file order at all — it is keyed by the *spawn order* of the track's
/// `.scn` `GEOM_ELEMENT`s (`0x46D1D0` over `[0x628168]`, `scene_props`'s own doc has the full
/// address trace), so this function cannot reproduce it from `arts`/`world` the way it does for
/// `Instance`/`Anim`/`MidGround`/`Static`. The caller instead computes [`scene_props`]' own output
/// once, up front, and passes this function the flat `props` indices it landed at (`scene`,
/// below) — this function only has to put them first, matching `0x487abd`'s own call to
/// `0x46D1D0` running before any of the other five bundles' ctor/`Load`/`insertAll(C)` calls
/// (`RE/RENDERING.md` "Bundle order, read 2026-09-28"'s "`WSceneBundle`'s own timing, settled").
/// `WLineBundle` contributes zero items regardless (established, unchanged).
pub fn track_tree_order(
    crp: &nfs_formats::crp::Crp,
    crp_data: &[u8],
    arts: &[nfs_formats::crp::Article],
    world: &[nfs_assets::track::ArticleMeshes],
    patches: &[nfs_physics::collision::Patch],
    props: &[nfs_physics::props::Prop],
    scene: &[usize],
) -> Vec<nfs_physics::track_tree::TreeItem> {
    use nfs_formats::crp::BundleKind;
    use nfs_physics::track_tree::TreeItem;
    let world_indices: std::collections::HashSet<usize> = world.iter().map(|a| a.index).collect();
    let mut patches_by_article: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, p) in patches.iter().enumerate() {
        patches_by_article.entry(p.article).or_default().push(i);
    }
    let mut prop_by_article: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (i, p) in props.iter().enumerate() {
        prop_by_article.insert(p.article, i);
    }
    // `WSceneBundle` first (`0x487abd` runs before any of the other five bundles' `Load`, "Bundle
    // order, read 2026-09-28"'s "`WSceneBundle`'s own timing, settled") — the caller's own spawn
    // order, `scene_props`'s doc.
    let mut order: Vec<TreeItem> = scene.iter().copied().map(TreeItem::Prop).collect();
    // `WAnimBundle` and `WMidGroundBundle` are not walked: no `PCollideArticleBundle` (// `ArticleMeshes::collides`)
    for rank in [BundleKind::Instance, BundleKind::Static] {
        for (index, art) in arts.iter().enumerate() {
            if !world_indices.contains(&index) {
                continue; // a library article: not in the generic array `0x4750B0` walks
            }
            if crp.bundle_of(crp_data, art).ok() != Some(rank) {
                continue;
            }
            // GAP, not invented (see this function's own doc): the original's per-article type
            // dispatch (`0x4751BD`) never gives one article both a patch run and a prop, but this
            // port's `collision_grid` takes every `world` article's mesh into the triangle grid
            // regardless of its `animdefs.txt` collide type, so an article that is both a
            // `pr`-patch source and a `kCylinder`/`kBox` prop in this port's own data (the
            // original has no such article) contributes both here, patches first. No shipped
            // track's race goldens moved from this.
            if let Some(idxs) = patches_by_article.get(&index) {
                order.extend(idxs.iter().map(|&i| TreeItem::Patch(i)));
            }
            if let Some(&i) = prop_by_article.get(&index) {
                order.push(TreeItem::Prop(i));
            }
        }
    }
    order
}

/// Every article's `Base+0x44` tag and its `Anim` part's first two words (the frame count and the
/// vertex-frame count), in `.crp` order — what 0x478640 / 0x478670 search and 0x476380 reads.
pub(super) fn article_tags(track: &TrackAssets, arts: &[nfs_formats::crp::Article]) -> Vec<(u32, Option<[u32; 2]>)> {
    arts.iter()
        .map(|art| {
            let tag = track.crp.article_geometry(&track.crp_data, art).ok().and_then(|g| g.base.map(|b| b.tag_u32())).unwrap_or(0);
            let anim = art.level(b"Anim").and_then(|e| track.crp.entry_data(&track.crp_data, e).ok()).filter(|d| d.len() >= 8).map(|d| {
                [bytes::read_u32(d, 0), bytes::read_u32(d, 4)]
            });
            (tag, anim)
        })
        .collect()
}

/// 0x478640: the first instance whose tag is `tag` (a linear search, `-1` when none).
pub(super) fn first_by_tag(tags: &[(u32, Option<[u32; 2]>)], tag: u32) -> Option<usize> {
    tags.iter().position(|t| t.0 == tag)
}

/// A piece's group name as the parser keeps it: four characters upper-cased (0x5A262C at 0x47E8D7)
/// packed into the dword the search compares, 0 when the key is absent.
pub(super) fn group_tag(name: &Option<String>) -> u32 {
    let Some(n) = name else { return 0 };
    let mut b = [0u8; 4];
    for (k, c) in n.bytes().take(4).enumerate() {
        b[k] = c.to_ascii_uppercase();
    }
    u32::from_le_bytes(b)
}

#[cfg(test)]
mod prop_box_tests {
    use super::*;

    /// An unrotated article: the eight-corner fold matches the plain per-vertex AABB (translation
    /// only moves the box, it does not enlarge it).
    #[test]
    fn unrotated_box_matches_vertex_aabb() {
        let (lo, hi) = (Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 3.0, 2.0));
        let tr = Mat4::from_translation(Vec3::new(10.0, 0.0, -5.0));
        let (mn, mx) = rotated_local_box_aabb(lo, hi, tr);
        assert!((mn - Vec3::new(9.0, 0.0, -7.0)).length() < 1e-5, "{mn:?}");
        assert!((mx - Vec3::new(11.0, 3.0, -3.0)).length() < 1e-5, "{mx:?}");
    }

    /// 0x486340's fold takes the AABB of the local box's **rotated corners**, which for a mesh
    /// whose actual vertices never sit at the same box corner on both axes at once (a diamond, not
    /// a crate) reaches past the AABB of the individually-rotated mesh vertices the port used
    /// before this fix (f8198d01 / 0x47B6E2…0x47B760): the diamond's own vertices, turned 45°
    /// about y, only reach ±half_x = ±2/√2 ≈ ±1.41, but the local **box**'s corner
    /// (half_x, half_z) = (2, 0.5) turned the same way reaches ±(2 + 0.5)/√2 ≈ ±1.77.
    #[test]
    fn rotated_box_reaches_past_the_individually_rotated_vertices() {
        // a diamond: no single vertex sits at the box's corner (±2, ±0.5) on both axes at once,
        // but the box built from their extents (lo=(-2,0,-0.5), hi=(2,1,0.5)) does
        let local = [Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -0.5), Vec3::new(0.0, 1.0, 0.5)];
        let tr = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let (lo, hi) = local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let (mn, mx) = rotated_local_box_aabb(lo, hi, tr);
        let want = (2.0f32 + 0.5) / std::f32::consts::SQRT_2;
        let old = local.iter().map(|p| tr.transform_point3(*p)).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p)));
        assert!((mx.x - want).abs() < 1e-4, "{mx:?} want {want}");
        assert!((mx.z - want).abs() < 1e-4, "{mx:?} want {want}");
        assert!((mn.x + want).abs() < 1e-4, "{mn:?} want {want}");
        assert!((mn.z + want).abs() < 1e-4, "{mn:?} want {want}");
        // the y range is untouched by a pure yaw
        assert!((mn.y - 0.0).abs() < 1e-5 && (mx.y - 1.0).abs() < 1e-5);
        // and the corner-based box is strictly looser than the pre-fix per-vertex one
        assert!(mx.x > old.1.x + 0.1, "new {mx:?} old {old:?}");
    }
}
