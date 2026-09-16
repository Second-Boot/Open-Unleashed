use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::skill::SkillTest;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let scene = t.load_scene(&a[1]).unwrap().unwrap();
    let st = SkillTest::from_scene(&scene, &a[0]);
    println!("arrows={:?}", st.arrows);
    for c in &st.checkpoints { println!("elem {} kind {} seq {} link {:?} chime {}", c.element, c.kind, c.sequence, c.link, c.chime); }
}
