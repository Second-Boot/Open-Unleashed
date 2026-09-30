//! every article with an `Anqt` level on every track — the record's own count `b`
//! (what `WAnimBundle::Load` 0x4777F0 hands the player) against the `Anim` part's first dword and
//! `Anqt` length / 52.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let (mut n, mut differ, mut no_anim) = (0, 0, 0);
    for name in TrackAssets::list(&paths) {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        for a in t.crp.articles(&t.crp_data).unwrap() {
            let Some(q) = a.level(b"Anqt") else { continue };
            let anim = a.level(b"Anim").map(|e| {
                let o = e.data_offset();
                u32::from_le_bytes(t.crp_data[o..o + 4].try_into().unwrap())
            });
            n += 1;
            no_anim += anim.is_none() as u32;
            let by_len = (q.data_len() / 52) as u32;
            if anim != Some(q.b()) || by_len != q.b() {
                differ += 1;
                println!("{name}: b {} anim {:?} len/52 {}", q.b(), anim, by_len);
            }
        }
    }
    println!("{n} Anqt articles, {no_anim} without Anim, {differ} where b, Anim[0] and len/52 differ");
}
