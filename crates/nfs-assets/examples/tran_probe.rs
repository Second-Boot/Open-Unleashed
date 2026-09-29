//! Diagnostic: the `animdefs.txt` `tag=TRAN` article in `autobahn.crp` — its `Base` flags, bundle
//! classification, scene-membership and (if it has one) its own `Anqt` path, plus whether any
//! `.scn` this track loads places an instance of it. Written 2026-09-30 to settle whether the
//! Autobahn train (`COVERAGE-2026-09-26.md` §14.38) is drawn anywhere in the shipped game: it
//! finds article 624, `"Locomotive"`, `is_library=false`/`bundle=Anim` (drawn directly, no `.scn`
//! placement needed) with a 251-frame `Anqt` track whose first and last frame are identical — a
//! closed loop, real world coordinates (not a library article's local-space rest pose).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, "autobahn").unwrap();
    let mut found = 0;
    for a in t.crp.articles(&t.crp_data).unwrap() {
        let Ok(g) = t.crp.article_geometry(&t.crp_data, &a) else { continue };
        let Some(base) = g.base.as_ref() else { continue };
        if &base.tag != b"TRAN" {
            continue;
        }
        found += 1;
        let bundle = t.crp.bundle_of(&t.crp_data, &a);
        let scene_member = t.crp.is_scene_member(&t.crp_data, &a);
        let anim = t.crp.article_animation(&t.crp_data, &a);
        println!(
            "article {:?} name={:?} flags={:#010x} is_library={} bundle={:?} is_scene_member={:?} has_anim={} centre={:?} bbox_min={:?} bbox_max={:?} radius={}",
            a.level_index,
            g.name,
            base.flags,
            base.is_library(),
            bundle,
            scene_member,
            anim.is_some(),
            base.centre,
            base.bbox_min,
            base.bbox_max,
            base.radius,
        );
        let tr = g.transform_for_lod(0);
        println!("  transform_for_lod(0) = {:?}", tr);
        if let Some(track) = anim {
            println!("  frames = {}", track.frames.len());
            for (i, f) in track.frames.iter().enumerate() {
                if i < 3 || i + 3 >= track.frames.len() {
                    println!("  frame {i}: pos={:?} rot={:?} scale={:?}", f.position, f.rotation, f.scale);
                }
            }
        }
    }
    println!("total TRAN articles found: {found}");

    // Also check every .scn this track loads for a GEOM element referencing this article.
    for suffix in ["", "_bstart", "_fstart", "_st14", "_st112", "_cameras", "_Audio", "_audio"] {
        match t.load_scene(suffix) {
            Ok(Some(_scn)) => println!("scene {suffix:?} loaded ok"),
            Ok(None) => println!("scene {suffix:?} absent"),
            Err(e) => println!("scene {suffix:?} error: {e}"),
        }
    }
}
