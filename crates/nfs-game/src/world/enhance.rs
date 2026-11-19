//! The enhancement passes of the track's load (`TrackWorld::upload`), one function each: what the data gives for the sun, the water /
//! translucent-sheet / decal tags of the draw items, the lighthouse beams and the bounce-light input. Moved out of `upload` as they
//! were, in the order `upload` ran them; all of them are render-only (nothing here is read by the simulation).

use super::*;

/// Every draw item of the track: the scene items, then each track article's items of every LOD.
pub(super) fn all_items_mut<'a>(items: &'a mut [DrawItem], track_lods: &'a mut [TrackArticleGpu]) -> impl Iterator<Item = &'a mut DrawItem> {
    items.iter_mut().chain(track_lods.iter_mut().flat_map(|a| a.lods.iter_mut().flat_map(|(_, v)| v.iter_mut())))
}

/// The index in `track_lods` of each article's entry (the first, as a `find` by article would take).
fn lods_by_article(track_lods: &[TrackArticleGpu]) -> HashMap<usize, usize> {
    let mut by_article = HashMap::new();
    for (g, a) in track_lods.iter().enumerate() {
        by_article.entry(a.article).or_insert(g);
    }
    by_article
}

/// The sun and the light the track's data gives (the original's own sun direction, and ENHANCEMENT E51 / E9 / E23 / E47 / E50 on top).
pub(super) struct SunInfo {
    pub sun_direction: Vec3,
    pub data_sun: Option<crate::data_sun::DataSun>,
    pub dusk_lamps: bool,
    pub bake_light: Option<nfs_renderer::BakeLight>,
    pub sky_blocks_sun: bool,
}

/// The sun of the track's load: `sun_direction` from the sky file, then the render-only enhancement inputs read from the data.
pub(super) fn sun_info(track: &TrackAssets, geometry: &nfs_assets::track::TrackGeometry, materials: &[nfs_formats::crp::Material]) -> SunInfo {
    let sun_direction = track.sky_bin.as_ref().map(|b| -Vec3::from(b.sun_position()).normalize_or_zero()).filter(|d| *d != Vec3::ZERO).unwrap_or(Vec3::NEG_Y);
    // ENHANCEMENT E51 / E9 (CORSICA-SUN, SUN-ALL, SUN-FULL): a track whose sky has no sun (the cars' light straight down): the
    // sun its data gives and the frame it drives (render only)
    let data_sun = crate::data_sun::measure(track, geometry, materials, sun_direction);
    let dusk_lamps = crate::data_sun::dusk_lamps(track.is_night(), sun_direction, data_sun.as_ref());
    tracing::info!(dusk_lamps, "open street lamps lit by day (ENHANCEMENT E23 / E47, a day with no sun)");
    // ENHANCEMENT E9 (GATE-REL): the bake's light model for the relative gate (render only)
    let bake_light = (!track.is_night()).then(|| crate::bake_light::gate_light(geometry, materials)).flatten();
    // (SHADOW-FLOOR: the sun frame's darkness at most `1 - a / (a + b)`, `nfs_renderer` `gpu/csm/mode.rs` floored_strength)
    let floor_darkness = bake_light.map(|b| 1.0 - (b.base / (b.base + b.gain).max(1e-4)).clamp(0.0, 1.0));
    tracing::info!(?bake_light, ?floor_darkness, "the bake light for E9's relative gate (GATE-REL) and its shadow floor (SHADOW-FLOOR)");
    // ENHANCEMENT E50 (SHADOW-OVERCAST): whether the sky's layers cover the sun (render only)
    let sky_blocks_sun = nfs_formats::sky::SkyFile::parse(&track.sky_params.join("\n")).is_ok_and(|f| f.blocks_the_sun());
    SunInfo { sun_direction, data_sun, dusk_lamps, bake_light, sky_blocks_sun }
}

/// ENHANCEMENT E17 / ENH-BEAM: flag the draw items of horizontal water, and take the translucent sheets (glow, haze, foam) out of the
/// sun's shadows. Returns the `.crp` materials that are sheets (the lighthouse-beam search reads them).
pub(super) fn tag_water_and_sheets(
    track: &TrackAssets,
    materials: &[nfs_formats::crp::Material],
    methods: &[nfs_formats::crp::RenderMethod],
    mat_ids: &HashMap<u16, MaterialId>,
    items: &mut [DrawItem],
    track_lods: &mut [TrackArticleGpu],
) -> std::collections::HashSet<u16> {
    // ENHANCEMENT E17: the materials that are horizontal water (`nfs_assets::track::is_water`)
    let water: std::collections::HashSet<MaterialId> = mat_ids
        .iter()
        .filter(|(id, _)| materials.get(**id as usize).is_some_and(|m| nfs_assets::track::uv_animate(methods, m).is_some_and(|a| nfs_assets::track::is_water(m, a))))
        .map(|(_, mid)| *mid)
        .collect();
    for it in all_items_mut(items, track_lods) {
        it.water = water.contains(&it.material);
    }
    // ENHANCEMENT ENH-BEAM (E9): a translucent glow sheet takes no sun shadow and casts none. A material whose texture's
    // alpha is faint ([`is_translucent_sheet`]: max below 192, mean below 64; the lighthouse beam `lths` peaks at 119 / 255, forest's god rays `bem2` at 136; on the shipped
    // tracks also coastal's / canyon's `wtr4` foam strips) is a sheet of light or haze, not an object: the shadow pass's
    // own cut (`shadow_on.w`, the alpha test GREATER 10 of the track's blended items) made every such texel a solid
    // occluder (the beam's box shadow on the road) and the receiving darkened the glow into a grey box with straight
    // edges. Rule by the data's alpha, no name list; horizontal water keeps its roles; the original path (switches off)
    // never reads `csm`.
    let mut texture_named = HashMap::new();
    for t in &track.textures {
        texture_named.entry(t.name.as_str()).or_insert(t);
    }
    let sheet_crp: std::collections::HashSet<u16> = mat_ids
        .iter()
        .filter(|(id, mid)| {
            !water.contains(*mid) && materials.get(**id as usize).and_then(|m| texture_named.get(m.texture_name().as_str()).copied()).is_some_and(|t| t.has_alpha && t.mips.first().is_some_and(|px| is_translucent_sheet(px)))
        })
        .map(|(id, _)| *id)
        .collect();
    {
        let sheets: std::collections::HashSet<MaterialId> = mat_ids.iter().filter(|(id, _)| sheet_crp.contains(*id)).map(|(_, mid)| *mid).collect();
        for it in all_items_mut(items, track_lods) {
            if sheets.contains(&it.material) {
                it.csm &= !(nfs_renderer::CSM_CAST | nfs_renderer::CSM_RECEIVE);
            }
        }
    }
    sheet_crp
}

/// ENHANCEMENT ENH-DBLSH: the track's `sh80` ground-shadow decals (the depth-biased `S_Normal` / `S_UVAnimate` blocks,
/// `thrash.md` section 6d.3) that have a sun-shadow caster over them fade out where E9 draws that caster's shadow
/// (`CSM_DECAL`): a decal is tagged when some caster (an instance prop or a non-ground article part that casts by E9's
/// casters' rule, `nfs_renderer::scenery_part_casts`: SHADOW-UNIFY's shape rule) stands inside its footprint; a decal over
/// no caster (a hillside, a terrain-like part) stays. A part over 60 m (SHADOW-UNIFY: a wall, a row of tree sheets, a fence
/// row) counts by its whole box, not its centre: the decals along a 200 m tree row lie far from the row's middle.
pub(super) fn tag_decals_under_casters(
    geometry: &nfs_assets::track::TrackGeometry,
    materials: &[nfs_formats::crp::Material],
    methods: &[nfs_formats::crp::RenderMethod],
    mat_ids: &HashMap<u16, MaterialId>,
    items: &[DrawItem],
    track_lods: &mut [TrackArticleGpu],
) {
    let decal_mats: std::collections::HashSet<MaterialId> = mat_ids
        .iter()
        .filter(|(id, _)| materials.get(**id as usize).and_then(|m| nfs_assets::track::render_states(methods, m)).is_some_and(|rs| rs[4] != 0))
        .map(|(_, mid)| *mid)
        .collect();
    let rule = nfs_renderer::scenery_casters();
    let by_article = lods_by_article(track_lods);
    // the casters' world boxes (a point for a prop and a part up to 60 m, as before)
    let mut casters: Vec<(Vec3, Vec3)> = items.iter().filter(|i| i.csm & nfs_renderer::CSM_CAST != 0 && !decal_mats.contains(&i.material)).map(|i| (i.transform.w_axis.truncate(), i.transform.w_axis.truncate())).collect();
    for art in &geometry.world {
        if let Some(g) = by_article.get(&art.index).map(|&g| &track_lods[g]) {
            let t = art.transform;
            let scale = t.x_axis.truncate().length().max(t.y_axis.truncate().length()).max(t.z_axis.truncate().length());
            for ((_, meshes), (_, drawn)) in art.lods.iter().zip(g.lods.iter()) {
                for (mm, it) in meshes.iter().zip(drawn.iter()) {
                    if it.csm & nfs_renderer::CSM_CAST != 0 && !decal_mats.contains(&it.material) && nfs_renderer::scenery_part_casts(&mm.mesh.vertices, scale, rule) {
                        let (lo, hi) = nfs_math::aabb_of(mm.mesh.vertices.iter().map(|v| Vec3::from(v.position))).unwrap_or(nfs_math::EMPTY_AABB);
                        if (hi - lo).length() * 0.5 * scale <= DECAL_BOX_WHOLE {
                            let c = t.transform_point3((lo + hi) * 0.5);
                            casters.push((c, c));
                        } else {
                            let (wl, wh) = nfs_math::aabb_of(mm.mesh.vertices.iter().map(|v| t.transform_point3(Vec3::from(v.position)))).unwrap_or(nfs_math::EMPTY_AABB);
                            casters.push((wl, wh));
                        }
                    }
                }
            }
        }
    }
    let (mut tagged, mut total) = (0, 0);
    for art in &geometry.world {
        let Some(&g) = by_article.get(&art.index) else { continue };
        let g = &mut track_lods[g];
        for ((_, meshes), (_, drawn)) in art.lods.iter().zip(g.lods.iter_mut()) {
            for (mm, it) in meshes.iter().zip(drawn.iter_mut()) {
                if !decal_mats.contains(&it.material) || mm.mesh.vertices.is_empty() {
                    continue;
                }
                let (lo, hi) = nfs_math::aabb_of(mm.mesh.vertices.iter().map(|v| art.transform.transform_point3(Vec3::from(v.position)))).unwrap_or(nfs_math::EMPTY_AABB);
                total += 1;
                if casters.iter().any(|(cl, ch)| ch.x >= lo.x - DECAL_MARGIN_XZ && cl.x <= hi.x + DECAL_MARGIN_XZ && ch.z >= lo.z - DECAL_MARGIN_XZ && cl.z <= hi.z + DECAL_MARGIN_XZ && ch.y >= lo.y - DECAL_MARGIN_Y && cl.y <= hi.y + DECAL_REACH_UP) {
                    it.csm |= nfs_renderer::CSM_DECAL;
                    tagged += 1;
                }
            }
        }
    }
    tracing::info!(decals = total, with_caster = tagged, "track sh80 decals (ENHANCEMENT ENH-DBLSH)");
}

/// ENHANCEMENT E44: the lighthouse beams (animated translucent sheets) and the draw items that are their boxes.
pub(super) fn find_lbeams(
    geometry: &nfs_assets::track::TrackGeometry,
    anim_articles: &[AnimArticle],
    sheet_crp: &std::collections::HashSet<u16>,
    track_lods: &[TrackArticleGpu],
) -> Vec<LBeamSrc> {
    let by_article = lods_by_article(track_lods);
    let lbeams: Vec<LBeamSrc> = {
        let mut cands: Vec<(usize, Vec<Vec3>)> = Vec::new();
        for art in geometry.world.iter().filter(|a| a.animation.is_some() && anim_articles.iter().any(|x| x.article == a.index)) {
            let Some((_, meshes)) = art.lods.first() else { continue };
            let verts: Vec<Vec3> = meshes.iter().filter(|m| sheet_crp.contains(&m.material)).flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position))).collect();
            if verts.len() >= 3 {
                cands.push((art.index, verts));
            }
        }
        let frame0: HashMap<usize, Mat4> = anim_articles.iter().map(|a| (a.article, a.matrix)).collect();
        let mut out = lbeam_sources(&cands, &frame0);
        // every draw item of the sheet materials of each source's articles: what the box hides
        for src in out.iter_mut() {
            for &art_index in &src.articles {
                let Some(art) = geometry.world.iter().find(|a| a.index == art_index) else { continue };
                let Some(&g) = by_article.get(&art_index) else { continue };
                for (l, ((_, meshes), (_, drawn))) in art.lods.iter().zip(track_lods[g].lods.iter()).enumerate() {
                    for (k, (mm, _)) in meshes.iter().zip(drawn.iter()).enumerate() {
                        if sheet_crp.contains(&mm.material) {
                            src.items.push((g, l, k));
                        }
                    }
                }
            }
        }
        out
    };
    if !lbeams.is_empty() {
        tracing::info!(beams = lbeams.len(), items = lbeams.iter().map(|b| b.items.len()).sum::<usize>(), "ENHANCEMENT E44: lighthouse beams found");
    }
    lbeams
}

    // ENHANCEMENT E32: the bounce light's input (the bake itself waits until the switch is on), on a day
    // track, not in the hard original (`NFS_ENH=off`): every world article drawn through `track_lods`
    // but the animated ones (their matrix moves)
pub(super) fn build_gi_state(
    track: &TrackAssets,
    geometry: &nfs_assets::track::TrackGeometry,
    track_lods: &[TrackArticleGpu],
    anim_articles: &[AnimArticle],
    breakable_articles: &std::collections::HashSet<usize>,
    smackable_articles: &std::collections::HashSet<usize>,
) -> Option<crate::gi::State> {
    (crate::enhanced::enabled() && !track.is_night()).then(|| {
        let t = std::time::Instant::now();
        let animated: std::collections::HashSet<usize> = anim_articles.iter().map(|a| a.article).collect();
        let skip = |a: &nfs_assets::track::ArticleMeshes| breakable_articles.contains(&a.index) || smackable_articles.contains(&a.index) || a.animation.is_some() || animated.contains(&a.index);
        let (input, parts) = crate::gi::build(track, geometry, &skip, crate::gi::to_sun(track));
        let by_article: HashMap<usize, usize> = track_lods.iter().enumerate().map(|(g, a)| (a.article, g)).collect();
        let targets: Vec<crate::gi::Target> = parts
            .into_iter()
            .filter_map(|p| {
                let g = *by_article.get(&p.article)?;
                let item = track_lods[g].lods.get(p.lod)?.1.get(p.part)?;
                Some(crate::gi::Target { g, l: p.lod, k: p.part, original: item.mesh, variant: None, part: p })
            })
            .collect();
        tracing::info!(receivers = input.recv_pos.len(), triangles = input.occ.tris.len(), parts = targets.len(), ms = t.elapsed().as_millis() as u64, "ENHANCEMENT E32: bounce light input");
        crate::gi::State { track: track.name.clone(), input: Some(std::sync::Arc::new(input)), targets, sky: crate::gi::sky_colour(track), sun: crate::gi::to_sun(track).is_some(), samples: None, pending: None, level: 0, active: false }
    })
}
