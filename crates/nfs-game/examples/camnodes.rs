//! Diagnostic: how many track-side camera nodes each track carries — `0x46D960()`'s answer, which
//! picks between the replay's "TV" camera and its fallback.
fn main() {
    let paths = nfs_assets::GamePaths::locate().unwrap();
    for name in nfs_assets::track::TrackAssets::list(&paths) {
        let Ok(t) = nfs_assets::track::TrackAssets::load(&paths, &name) else { continue };
        let n = t.cameras.as_ref().map(|s| nfs_game::cameras::TrackCameras::from_scene(s).cameras.len()).unwrap_or(0);
        println!("{name:12} camera nodes: {n}");
    }
}
