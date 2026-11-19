//! The E41 high-definition twin set of the track's textures: the pack lookup, the loaded twins, their upload and the swap of
//! one set for another. Moved out of `world.rs` as it was; the sky's twin set uses the same jobs and `replace_hd_set`.

use super::*;

/// ENHANCEMENT E41: the HD pack of this run for the level the switch is at (`None`: off, or no
/// pack beside the game, the original is used), the game root taken from the track's own directory (`<root>/GameData/Track`).
/// At level 0 nothing is looked for: no file is touched.
pub(crate) fn hd_pack_for(r: &Renderer, track: &TrackAssets) -> Option<&'static nfs_assets::hdpack::HdPack> {
    nfs_assets::hdpack::scale_of_level(r.enhancements().level(nfs_renderer::E41))?;
    let root = track.dir.parent()?.parent()?.to_path_buf();
    nfs_assets::hdpack::HdPack::global(&nfs_assets::GamePaths { root })
}

/// ENHANCEMENT E41: one texture of a pack as the uploader takes it.
pub(crate) enum HdLoaded {
    /// RGBA8 levels (a PNG with the chain made here, or a KTX2 decoded on the CPU)
    Rgba(u32, u32, Vec<Vec<u8>>),
    /// BC7 blocks, every level (a KTX2 on a device that takes BC)
    Bc7(u32, u32, Vec<Vec<u8>>),
}

/// ENHANCEMENT E41: load one pack texture of `key` for the original `t`-sized (`ow x oh`, level 0 `orig`) texture: BC7 when the device takes it
/// and the file is one, else RGBA8 with its mip chain (the file's own for a KTX2, E2's generator for a PNG). `alpha` is the chain's
/// alpha mode for a PNG. `None`: no file / wrong size / unreadable (the original is used).
pub(crate) fn load_hd_texture(pack: &nfs_assets::hdpack::HdPack, key: &str, orig: &[u8], ow: u32, oh: u32, bc7: bool, alpha: nfs_renderer::MipAlpha) -> Option<HdLoaded> {
    use nfs_assets::hdpack::HdFile;
    Some(match pack.load_any(key, orig, ow, oh, bc7)? {
        HdFile::Bc7(img) if img.width % 4 == 0 && img.height % 4 == 0 => HdLoaded::Bc7(img.width, img.height, img.levels),
        HdFile::Bc7(img) => HdLoaded::Rgba(img.width, img.height, nfs_assets::hdpack::decode_chain(&img)),
        HdFile::Chain { width, height, mips } => HdLoaded::Rgba(width, height, mips),
        HdFile::Rgba(img) => {
            let rest = nfs_renderer::mip_chain(&img.rgba, img.width, img.height, alpha);
            let (w, h) = (img.width, img.height);
            let mut mips = vec![img.rgba];
            mips.extend(rest);
            HdLoaded::Rgba(w, h, mips)
        }
    })
}

/// ENHANCEMENT ENH-LIVETEX: the HD twins one loader made (a track's, the sky's, a car's pages): the scale they were loaded at, the original
/// textures they are twins of, and their own ids (given back to the renderer when another scale replaces them).
#[derive(Debug, Clone, Default)]
pub struct HdSet {
    pub scale: u32,
    pub bases: Vec<nfs_renderer::TextureId>,
    pub twins: Vec<nfs_renderer::TextureId>,
}

/// ENHANCEMENT ENH-LIVETEX: one texture to look up in the pack: the ORIGINAL's id (already uploaded, as master does), the pack key, the
/// original's pixels (the size and the alpha policy of the pack image's check) and the alpha mode of a PNG's generated chain.
pub(crate) struct HdJob<'a> {
    pub base: nfs_renderer::TextureId,
    pub key: String,
    pub orig: &'a nfs_assets::texture::TextureImage,
    pub alpha: nfs_renderer::MipAlpha,
}

/// ENHANCEMENT ENH-LIVETEX: the pack scale E41's current level asks for (`None`: level 0).
pub(crate) fn hd_scale_wanted(r: &Renderer) -> Option<u32> {
    nfs_assets::hdpack::scale_of_level(r.enhancements().level(nfs_renderer::E41))
}

/// ENHANCEMENT ENH-LIVETEX: make `new` the current set of `cur`: a base the old set had and the new one lacks goes back to its original,
/// the replaced twins give their video memory back (the originals stay, so the A/B key flips without a reload).
pub(crate) fn replace_hd_set(r: &mut Renderer, cur: &mut Option<HdSet>, new: HdSet) {
    if let Some(old) = cur.take() {
        let kept: std::collections::HashSet<nfs_renderer::TextureId> = new.bases.iter().copied().collect();
        for b in old.bases.iter().filter(|b| !kept.contains(b)) {
            r.set_texture_twin(*b, None);
        }
        for t in old.twins {
            r.release_texture(t);
        }
    }
    *cur = Some(new);
}

/// ENHANCEMENT E41 / ENH-LIVETEX: upload the HD twins of `jobs` - each replaced by its pack image where the pack has one - and register them
/// with the renderer ([`Renderer::set_texture_twin`]); the originals are already there. Shown while E41 is on.
///
/// A twin gets the pack's `scale x` pixels with a full mip chain (the file's own for a KTX2 / BC7, made by E2's generator
/// ([`nfs_renderer::mip_chain`]; an alpha-tested texture keeps its coverage, `Test(10)`) for a PNG), so the sampler path is the
/// original's. BC7 files are uploaded as they are when the device takes BC (`TEXTURE_COMPRESSION_BC`), else decoded on the CPU. The pixels are
/// read and chained on worker threads and uploaded in runs of `HD_RUN` textures, so the CPU never holds the whole HD set.
pub(crate) fn upload_hd_twins(r: &mut Renderer, pack: &nfs_assets::hdpack::HdPack, jobs: &[HdJob<'_>], what: &str) -> HdSet {
    const HD_RUN: usize = 24;
    type Loaded = Option<HdLoaded>;
    let started = std::time::Instant::now();
    let bc7 = r.supports_bc7();
    let mut set = HdSet { scale: pack.scale(), ..Default::default() };
    let (mut direct, mut bytes, mut orig_bytes) = (0usize, 0u64, 0u64);
    for run in jobs.chunks(HD_RUN) {
        let load = |j: &HdJob<'_>| -> Loaded {
            if j.orig.mips.is_empty() {
                return None;
            }
            load_hd_texture(pack, &j.key, &j.orig.mips[0], j.orig.width, j.orig.height, bc7, j.alpha)
        };
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(run.len());
        let per = run.len().div_ceil(workers.max(1));
        let loaded: Vec<Loaded> = std::thread::scope(|s| {
            let load = &load;
            let handles: Vec<_> = run.chunks(per).map(|c| s.spawn(move || c.iter().map(load).collect::<Vec<_>>())).collect();
            handles.into_iter().flat_map(|h| h.join().expect("HD texture worker")).collect()
        });
        // the RGBA8 ones in one call, the BC7 ones in another; each texture is named by its slot in the run
        let mut ids: Vec<Option<nfs_renderer::TextureId>> = vec![None; run.len()];
        let rgba_slots: Vec<usize> = (0..run.len()).filter(|&i| matches!(loaded[i], Some(HdLoaded::Rgba(..)))).collect();
        let items: Vec<(u32, u32, &[Vec<u8>], Option<nfs_renderer::MipAlpha>)> = rgba_slots
            .iter()
            .filter_map(|&i| if let Some(HdLoaded::Rgba(w, h, mips)) = &loaded[i] { Some((*w, *h, mips.as_slice(), None)) } else { None })
            .collect();
        for (&i, id) in rgba_slots.iter().zip(r.upload_textures_gen(&items)) {
            ids[i] = Some(id);
        }
        let bc_slots: Vec<usize> = (0..run.len()).filter(|&i| matches!(loaded[i], Some(HdLoaded::Bc7(..)))).collect();
        let bc_items: Vec<(u32, u32, &[Vec<u8>])> = bc_slots.iter().filter_map(|&i| if let Some(HdLoaded::Bc7(w, h, l)) = &loaded[i] { Some((*w, *h, l.as_slice())) } else { None }).collect();
        for (&i, id) in bc_slots.iter().zip(r.upload_textures_bc7(&bc_items)) {
            ids[i] = Some(id);
        }
        for (i, j) in run.iter().enumerate() {
            let Some(id) = ids[i] else { continue };
            orig_bytes += j.orig.mips.iter().map(|m| m.len() as u64).sum::<u64>();
            match &loaded[i] {
                Some(HdLoaded::Rgba(_, _, mips)) => bytes += mips.iter().map(|m| m.len() as u64).sum::<u64>(),
                Some(HdLoaded::Bc7(_, _, levels)) => {
                    direct += 1;
                    bytes += levels.iter().map(|m| m.len() as u64).sum::<u64>();
                }
                None => {}
            }
            r.set_texture_twin(j.base, Some(id));
            set.bases.push(j.base);
            set.twins.push(id);
        }
    }
    tracing::info!(what, scale = pack.scale(), twins = set.twins.len(), bc7 = direct, total = jobs.len(), hd_mib = bytes >> 20, original_mib_resident_next_to_it = orig_bytes >> 20, ms = started.elapsed().as_millis() as u64, "HD textures uploaded next to the originals (ENHANCEMENT E41)");
    set
}

/// ENHANCEMENT E41 / ENH-LIVETEX: a track's HD jobs: every `.fsh` texture but the car-shadow projector's map (sampled by a projective
/// matrix, not a surface UV); a name that occurs again later in the pack is not looked up (the later one is the one the materials name,
/// and the pack holds the last under the key). `ids`: the originals' ids by name (the later duplicate wins, as in the materials).
pub(super) fn track_hd_jobs<'a>(track: &'a TrackAssets, ids: &HashMap<String, (nfs_renderer::TextureId, bool)>) -> Vec<HdJob<'a>> {
    let exclude: std::collections::HashSet<String> = track_projectors(track).iter().map(|p| nfs_assets::hdpack::safe(&p.texture)).collect();
    let last: HashMap<String, usize> = track.textures.iter().enumerate().map(|(i, t)| (nfs_assets::hdpack::safe(&t.name), i)).collect();
    track
        .textures
        .iter()
        .enumerate()
        .filter(|(i, t)| !t.mips.is_empty() && !exclude.contains(&nfs_assets::hdpack::safe(&t.name)) && last.get(&nfs_assets::hdpack::safe(&t.name)) == Some(i))
        .filter_map(|(_, t)| {
            let base = ids.get(&t.name)?.0;
            let alpha = nfs_renderer::MipAlpha::for_texture(t.has_alpha);
            Some(HdJob { base, key: nfs_assets::hdpack::key_track(&track.name, &t.name), orig: t, alpha })
        })
        .collect()
}
