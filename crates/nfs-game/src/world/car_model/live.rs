//! ENHANCEMENT ENH-LIVETEX: the car's twin textures (E41 / E42 / E43 / E8 kinds) that can arrive after the car was built: what is
//! wanted, what a car holds, the cache of built twins by source and `sync_live` / `adopt`. Moved out of `car_model.rs` as it was.

use super::*;

/// ENHANCEMENT ENH-LIVETEX: the levels a load builds twins for (0: none). A first load builds what the switches say; a live sync builds
/// only what the model lacks.
#[derive(Clone, Copy, Debug)]
pub struct LiveWant {
    pub e41: u8,
    pub e42: u8,
    /// ENHANCEMENT ENH-LIVELEAK: a live build for the twins alone - the load's own copies of the originals (meshes, pages, materials) are
    /// not uploaded: they would be thrown away, and a mesh arena cannot give its memory back
    pub twins_only: bool,
}

/// ENHANCEMENT ENH-LIVETEX: a car's twins (E41 pages, E42 meshes) and what was last looked up.
#[derive(Clone, Default)]
pub(super) struct LiveTwins {
    /// the pack scale the pages were last looked up at (0: never - E41 was off); set even when the pack held nothing for this car
    pub(super) hd_tried: u32,
    pub(super) hd: Option<HdCar>,
    /// the E42 level the refined meshes were last built for (0: never)
    pub(super) round_tried: u8,
    /// (original mesh, refined mesh) of the parts refined, in the renderer's twin table
    pub(super) round_static: Vec<(MeshId, MeshId)>,
    /// the refined sibling of each [`DeformPart`] (parallel to `CarModel::deform`): the same part's crush on the refined vertices
    pub(super) round_deform: Vec<Option<DeformPart>>,
}

/// ENHANCEMENT ENH-LIVETEX: the HD twins of a car's pages. `pages` / `tex` are parallel to `CarModel::page_textures`; the pixels of a page
/// no damage decal lands on are not kept (as `damage_pages`), `patches` are the decals taken to the HD scale.
#[derive(Clone)]
pub(super) struct HdCar {
    pub(super) pages: Vec<Option<nfs_assets::car::TexturePage>>,
    pub(super) patches: Vec<nfs_assets::car::DamagePatch>,
    pub(super) tex: Vec<Option<nfs_renderer::TextureId>>,
}

/// ENHANCEMENT ENH-LIVETEX: the arguments a car was loaded with, kept (with the [`nfs_assets::car::CarRecipe`] that reads its files again) so a
/// live change of the HD textures / rounded parts can build the missing twins later. `id` names the load: instances made from a model keep it.
#[derive(Debug)]
pub struct CarSource {
    pub(super) id: u64,
    pub(super) recipe: nfs_assets::car::CarRecipe,
    pub(super) room: bool,
    pub(super) lod: u8,
    pub(super) paint: nfs_assets::car::Paint,
    pub(super) env: CarEnv,
    pub(super) night: bool,
    pub(super) race_roof: Option<u32>,
    pub(super) skidpad: Option<bool>,
}

/// ENHANCEMENT ENH-LIVETEX: what one pass of live syncs shares between the models of one load (a template and its instances): the twins the
/// first of them took, and its page ids.
#[derive(Default)]
pub struct LiveCache {
    pub(super) by_source: HashMap<u64, (LiveTwins, Vec<nfs_renderer::TextureId>)>,
}

impl CarSource {
    pub(super) fn recipe_model(&self) -> String {
        self.recipe.model().to_string()
    }
}

impl CarModel {
    /// ENHANCEMENT ENH-LIVETEX: the model still lacks twins for the levels E41 / E42 are at now (never built, or built for another scale /
    /// level). Flipping a switch to 0 or back to a level whose twins are resident needs nothing.
    pub fn live_pending(&self, r: &Renderer) -> bool {
        let Some(src) = &self.source else { return false };
        let e = r.enhancements();
        let want41 = nfs_assets::hdpack::scale_of_level(e.level(nfs_renderer::E41)).unwrap_or(0);
        let want42 = e.level(nfs_renderer::E42);
        (want41 != 0 && !src.room && self.live.hd_tried != want41) || (want42 != 0 && self.live.round_tried != want42)
    }

    /// ENHANCEMENT ENH-LIVETEX: bring this model's twins to the levels E41 / E42 are at now (see [`Self::live_pending`]). The first model of
    /// a load builds them by loading the car again (`CarSource`) with only the missing kinds asked for and adopts the result; the others of
    /// the load (traffic / police instances) copy from it through `cache`. A short hitch, once per change of level, never per A/B flip.
    pub fn sync_live(&mut self, r: &mut Renderer, paths: &nfs_assets::GamePaths, cache: &mut LiveCache) {
        if !self.live_pending(r) {
            return;
        }
        let Some(src) = self.source.clone() else { return };
        let e = r.enhancements();
        let want41 = nfs_assets::hdpack::scale_of_level(e.level(nfs_renderer::E41)).unwrap_or(0);
        let want42 = e.level(nfs_renderer::E42);
        let need_hd = want41 != 0 && !src.room && self.live.hd_tried != want41;
        let need_round = want42 != 0 && self.live.round_tried != want42;
        if let Some((tw, pages)) = cache.by_source.get(&src.id) {
            let (tw, pages) = (tw.clone(), pages.clone());
            self.adopt(r, tw, None, Some(&pages), need_hd, need_round);
            return;
        }
        let started = std::time::Instant::now();
        let mut load_ms = 0;
        let fresh = src.recipe.load().map_err(anyhow::Error::from).and_then(|assets| {
            load_ms = started.elapsed().as_millis() as u64;
            let want = LiveWant { e41: if need_hd { e.level(nfs_renderer::E41) } else { 0 }, e42: if need_round { want42 } else { 0 }, twins_only: true };
            Self::upload_inner(r, &assets, paths, src.lod, src.paint, CarUpload { env: src.env, night: src.night, race_roof: src.race_roof, skidpad: src.skidpad }, want)
        });
        match fresh {
            Ok(fresh) => {
                let parts = fresh.part_meshes();
                self.adopt(r, fresh.live, Some(&parts), None, need_hd, need_round);
                tracing::info!(model = %src.recipe_model(), hd = need_hd, round = need_round, load_ms, ms = started.elapsed().as_millis() as u64, "car twins built live (ENHANCEMENT ENH-LIVETEX)");
            }
            Err(err) => {
                tracing::warn!(model = %src.recipe_model(), error = %err, "car twins could not be built live (ENHANCEMENT ENH-LIVETEX)");
                // not tried again for these levels
                if need_hd {
                    self.live.hd_tried = want41;
                }
                if need_round {
                    self.live.round_tried = want42;
                }
            }
        }
        cache.by_source.insert(src.id, (self.live.clone(), self.page_textures.clone()));
    }

    /// ENHANCEMENT ENH-LIVETEX: make `new`'s twins this model's. `fresh_parts`: `new` is a fresh load's own (its ids are taken over, the parts
    /// paired by position with this model's); `None` with `template_pages`: `new` is a template's (this instance's private page / crush
    /// resources get copies of their own, everything shared is the template's twin already).
    pub(super) fn adopt(&mut self, r: &mut Renderer, new: LiveTwins, fresh_parts: Option<&[MeshId]>, template_pages: Option<&[nfs_renderer::TextureId]>, need_hd: bool, need_round: bool) {
        if need_hd {
            self.live.hd_tried = new.hd_tried;
            let old = self.live.hd.take();
            if let Some(nh) = new.hd {
                let mut tex: Vec<Option<nfs_renderer::TextureId>> = vec![None; self.page_textures.len()];
                for (i, slot) in tex.iter_mut().enumerate() {
                    let Some(nt) = nh.tex.get(i).copied().flatten() else { continue };
                    let base = self.page_textures[i];
                    match template_pages {
                        None => {
                            r.set_texture_twin(base, Some(nt));
                            *slot = Some(nt);
                        }
                        Some(tp) if tp.get(i) == Some(&base) => *slot = Some(nt),
                        Some(_) => {
                            // a page of this instance's own (a damage decal page): a copy of the HD base page
                            if let Some(Some(pg)) = nh.pages.get(i).filter(|p| p.as_ref().is_some_and(|p| !p.rgba.is_empty())) {
                                let alpha = nfs_renderer::MipAlpha::for_texture(!(pg.is_window || pg.is_alpha));
                                let id = r.upload_texture_generated(pg.width, pg.height, &pg.rgba, alpha);
                                r.set_texture_twin(base, Some(id));
                                *slot = Some(id);
                            }
                        }
                    }
                }
                self.live.hd = Some(HdCar { pages: nh.pages, patches: nh.patches, tex });
            }
            if let Some(old) = old {
                for (i, o) in old.tex.iter().enumerate() {
                    let Some(o) = o else { continue };
                    let now = self.live.hd.as_ref().and_then(|h| h.tex.get(i).copied().flatten());
                    if now.is_none() {
                        r.set_texture_twin(self.page_textures[i], None);
                    }
                    if now != Some(*o) {
                        r.release_texture(*o);
                    }
                }
            }
        }
        if need_round {
            self.live.round_tried = new.round_tried;
            let old_static = std::mem::take(&mut self.live.round_static);
            let mut now_static: Vec<(MeshId, MeshId)> = Vec::new();
            match fresh_parts {
                Some(fp) => {
                    let map: HashMap<MeshId, MeshId> = new.round_static.iter().copied().collect();
                    let mine = self.part_meshes();
                    if mine.len() == fp.len() {
                        for (m, f) in mine.iter().zip(fp) {
                            if let Some(&rid) = map.get(f) {
                                r.set_mesh_twin(*m, Some(rid));
                                now_static.push((*m, rid));
                            }
                        }
                    } else {
                        tracing::warn!(mine = mine.len(), fresh = fp.len(), "car twins: the part lists differ, the refined meshes are not adopted");
                    }
                }
                // an instance shares every non-crush mesh with its template: the template's pairs are this model's
                None => now_static = new.round_static.clone(),
            }
            for (b, _) in &old_static {
                if !now_static.iter().any(|(nb, _)| nb == b) {
                    r.set_mesh_twin(*b, None);
                }
            }
            // ENHANCEMENT ENH-LIVELEAK: the replaced refined meshes (the previous level's) give their memory back once no twin entry names them.
            // The static ones are shared with the instances of the same load, which take the new ones from the cache: only the model that
            // built them (`fresh_parts`) releases them; the crush siblings are private to each model.
            let old_deform: Vec<MeshId> = self.live.round_deform.iter().flatten().map(|d| d.mesh).collect();
            let mut dead: Vec<MeshId> = old_deform;
            if fresh_parts.is_some() {
                dead.extend(old_static.iter().map(|(_, t)| *t));
            }
            self.live.round_static = now_static;
            // the crush siblings
            let mut round_deform: Vec<Option<DeformPart>> = Vec::with_capacity(self.deform.len());
            for (j, p) in self.deform.iter().enumerate() {
                let mut nd = new.round_deform.get(j).cloned().flatten();
                if let Some(d) = nd.as_mut() {
                    if template_pages.is_some() {
                        d.mesh = r.upload_mesh(&MeshData { vertices: d.base.clone(), indices: d.indices.clone() });
                    }
                    r.set_mesh_twin(p.mesh, Some(d.mesh));
                } else if self.live.round_deform.get(j).is_some_and(|d| d.is_some()) {
                    r.set_mesh_twin(p.mesh, None);
                }
                round_deform.push(nd);
            }
            // the twin entries now name the new meshes: the old ones can go (a mesh the new set still holds stays)
            let keep: std::collections::HashSet<MeshId> = self.live.round_static.iter().map(|(_, t)| *t).chain(round_deform.iter().flatten().map(|d| d.mesh)).collect();
            let mut done: std::collections::HashSet<MeshId> = std::collections::HashSet::new();
            for m in dead.into_iter().filter(|m| !keep.contains(m)) {
                if done.insert(m) {
                    r.release_mesh(m);
                }
            }
            self.live.round_deform = round_deform;
        }
        // the new twins start pristine: the next `update_damage` redraws the crush / decals of every damaged zone on them
        self.damage_shown = [0.0; 10];
    }
}
