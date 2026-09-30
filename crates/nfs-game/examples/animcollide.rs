//! for every shipped track, the collision triangles / props the port's grid holds for the
//! articles of the bundles the original's `0x488090` never feeds to `0x4750B0` (`WAnimBundle`,
//! `WMidGroundBundle`: their `PCollidableBundle*` at `+0xC` stays 0) — frame-local bbox, and how near
//! any slice of the track's road (both directions use the same slices) comes to that bbox.
//! `cargo run -p nfs-game --example animcollide`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::crp::BundleKind;
use nfs_game::race::SliceTable;
use nfs_game::world::track_props;

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in TrackAssets::list(&paths) {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let Ok(g) = t.build_geometry() else { continue };
        let mats = t.materials().unwrap();
        let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
        let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
        let arts = t.crp.articles(&t.crp_data).unwrap();
        let props = track_props(&t);
        let slices = SliceTable::from_crp(&t.crp, &t.crp_data).map(|s| s.slices).unwrap_or_default();
        for kind in [BundleKind::Anim, BundleKind::MidGround] {
            let mut n_art = 0;
            let (mut ntri, mut ndriv, mut nprop) = (0usize, 0usize, 0usize);
            let (mut lo, mut hi) = (glam::Vec3::splat(1e9), glam::Vec3::splat(-1e9));
            let mut worst = 1e9f32;
            let mut worst_name = String::new();
            for a in &g.world {
                if t.crp.bundle_of(&t.crp_data, &arts[a.index]).ok() != Some(kind) {
                    continue;
                }
                n_art += 1;
                let (mut alo, mut ahi) = (glam::Vec3::splat(1e9), glam::Vec3::splat(-1e9));
                let mut at = 0;
                for tr in grid.triangles().iter().filter(|x| x.source.is_some_and(|s| s.article == a.index)) {
                    at += 1;
                    if tr.surface & 0xF != 0 {
                        ndriv += 1;
                    }
                    for p in [tr.a, tr.b, tr.c] {
                        alo = alo.min(p);
                        ahi = ahi.max(p);
                    }
                }
                let ap = props.iter().filter(|p| p.article == a.index).count();
                for p in props.iter().filter(|p| p.article == a.index) {
                    println!("    prop of {}: breakable {} drive_through {} class {} aabb {:.0?}..{:.0?}", a.name, p.breakable.is_some(), p.drive_through, p.class, p.aabb_min, p.aabb_max);
                }
                nprop += ap;
                ntri += at;
                if at == 0 && ap == 0 {
                    continue;
                }
                lo = lo.min(alo);
                hi = hi.max(ahi);
                // nearest slice edge (width_a/width_b to each side) horizontally to the article's bbox
                let mut d_art = 1e9f32;
                for s in &slices {
                    let reach = s.width_a.max(s.width_b);
                    let dx = (alo.x - s.centre.x).max(0.0).max(s.centre.x - ahi.x);
                    let dz = (alo.z - s.centre.z).max(0.0).max(s.centre.z - ahi.z);
                    d_art = d_art.min((dx * dx + dz * dz).sqrt() - reach);
                }
                if d_art < worst {
                    worst = d_art;
                    worst_name = a.name.clone();
                }
                if kind == BundleKind::Anim {
                    println!("  {name} ANIM {:<16} tris {at:4} props {ap} bbox {:.0?}..{:.0?} gap-to-road {:.0} m", a.name, alo, ahi, d_art);
                }
            }
            if n_art > 0 {
                println!("{name} {kind:?}: articles {n_art} tris {ntri} (surface!=0: {ndriv}) props {nprop} bbox {lo:.0?}..{hi:.0?} nearest-road-edge gap {worst:.0} m ({worst_name})");
            }
        }
    }
}
