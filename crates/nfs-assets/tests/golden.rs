//! Characterisation ("golden hash") tests recorded from the tree BEFORE the 2026-10-10 refactor:
//! `TrackAssets::load`, `build_geometry` and `CarAssets::build_pages` must
//! keep producing exactly these bytes (same textures, same order, same ids). Skipped when the
//! original game data is not present. `NFS_GOLDEN_PRINT=1 ... -- --nocapture` prints the values.
use nfs_assets::{
    car::{CarAssets, CarStyle, PageText, Paint},
    texture::TextureImage,
    track::TrackAssets,
    GamePaths,
};
use std::fmt::Write as _;

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}
impl std::fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

fn paths() -> Option<GamePaths> {
    let p = GamePaths::locate().ok();
    if p.is_none() {
        eprintln!("skipping: original game data not found");
    }
    p
}

fn check(label: &str, got: u64, expect: u64) {
    if std::env::var_os("NFS_GOLDEN_PRINT").is_some() {
        eprintln!("GOLDEN {label} {got:#018x}");
    } else {
        assert_eq!(got, expect, "golden hash {label} changed (got {got:#018x})");
    }
}

fn hash_textures(h: &mut Fnv, t: &[TextureImage]) {
    h.u64(t.len() as u64);
    for x in t {
        h.bytes(x.name.as_bytes());
        h.u64(x.width as u64);
        h.u64(x.height as u64);
        h.u64(x.mips.len() as u64);
        for m in &x.mips {
            h.u64(m.len() as u64);
            h.bytes(m);
        }
        write!(h, "{} {} {} {} {} {:?} {:?}", x.has_alpha, x.code, x.page, x.x, x.y, x.place, x.text).unwrap();
    }
}

fn hash_track(t: &TrackAssets) -> u64 {
    let mut h = Fnv::new();
    h.bytes(t.name.as_bytes());
    h.u64(t.crp_data.len() as u64);
    h.bytes(&t.crp_data);
    hash_textures(&mut h, &t.textures);
    hash_textures(&mut h, &t.env_textures);
    hash_textures(&mut h, &t.sky_textures);
    write!(h, "{:?}", t.crp).unwrap();
    write!(h, "{:?}{:?}{:?}{:?}", t.map, t.start_forward, t.start_backward, t.cameras).unwrap();
    write!(h, "{:?}{:?}{:?}", t.draw_keys_forward, t.draw_keys_backward, t.extra_scenes).unwrap();
    write!(h, "{:?}{:?}{:?}{:?}", t.animdefs, t.edges, t.sky_params, t.sky_bin).unwrap();
    write!(h, "{} {}", t.reverse, t.is_night()).unwrap();
    h.0
}

fn hash_geometry(t: &TrackAssets) -> u64 {
    let mut h = Fnv::new();
    write!(h, "{:?}", t.materials().unwrap()).unwrap();
    write!(h, "{:?}", t.build_geometry().unwrap()).unwrap();
    h.0
}

#[test]
fn golden_track_load() {
    let Some(p) = paths() else { return };
    for (i, name) in ["alps", "monaco1"].into_iter().enumerate() {
        let mut t = TrackAssets::load(&p, name).unwrap();
        check(&format!("TrackAssets::load({name})"), hash_track(&t), GOLD_TRACK[i]);
        check(&format!("build_geometry({name})"), hash_geometry(&t), GOLD_GEOM[i]);
        // the texture release / ensure cycle gives the same pixels back
        let before = t.textures.clone();
        let env_before = t.env_textures.clone();
        t.release_textures();
        assert!(t.textures.is_empty());
        t.ensure_textures().unwrap();
        let (mut a, mut b) = (Fnv::new(), Fnv::new());
        hash_textures(&mut a, &before);
        hash_textures(&mut a, &env_before);
        hash_textures(&mut b, &t.textures);
        hash_textures(&mut b, &t.env_textures);
        assert_eq!(a.0, b.0, "ensure_textures({name}) differs from load");
    }
}

fn hash_pages(c: &CarAssets, p: &GamePaths, paint: impl Into<Paint>, style: &CarStyle) -> u64 {
    let mut h = Fnv::new();
    match c.build_pages(p, paint, style) {
        Ok(pages) => {
            h.u64(pages.len() as u64);
            for pg in &pages {
                write!(h, "{} {} {} {} {}", pg.width, pg.height, pg.is_window, pg.is_alpha, pg.bpp).unwrap();
                h.bytes(&pg.rgba);
            }
        }
        Err(e) => panic!("build_pages failed: {e}"),
    }
    h.0
}

const CARS: [&str; 12] = ["356_1", "993", "550", "959", "boxster", "gt1", "f996", "cop993", "eas3", "eden1", "914", "928"];

#[test]
fn golden_car_pages() {
    let Some(p) = paths() else { return };
    let custom = Paint { exterior: [0x20, 0x60, 0xC0], stripe1: [0xFF, 0xE0, 0x10], stripe2: [0x10, 0xA0, 0x30], interior: [0xC0, 0x30, 0x30], package: 3 };
    let mut all = Vec::new();
    for (i, model) in CARS.iter().enumerate() {
        let mut c = CarAssets::load(&p, model).unwrap();
        let mut h = Fnv::new();
        hash_textures(&mut h, &c.textures_day);
        hash_textures(&mut h, &c.textures_night);
        write!(h, "{:?}", c.tpg).unwrap();
        check(&format!("CarAssets::load({model}) textures"), h.0, GOLD_CAR_TEX[i]);
        let default = CarStyle::default();
        let style1 = CarStyle::from_tpg(&c.tpg, 1);
        let fe = CarStyle { front_end: true, night: true, skidpad: Some(true), ..CarStyle::from_tpg(&c.tpg, 2) };
        let mut h = Fnv::new();
        h.u64(hash_pages(&c, &p, [0x80, 0x40, 0x20], &default));
        h.u64(hash_pages(&c, &p, custom, &style1));
        h.u64(hash_pages(&c, &p, custom, &fe));
        c.page_text = Some(PageText { license: "ABC1234".into(), race_number: 7 });
        c.cabrio = true;
        c.style_index = 1;
        h.u64(hash_pages(&c, &p, custom, &default));
        h.u64(hash_pages(&c, &p, [0xF0, 0xF0, 0xF0], &style1));
        all.push((model, h.0));
        check(&format!("build_pages({model})"), h.0, GOLD_PAGES[i]);
    }
    // a front-end room: textures embedded in the `.crp`, `.tpg` pages 565 / 4444
    let mut room = CarAssets::load_in(&p.fe_data().join("Models"), "garage").unwrap();
    room.front_end_room = true;
    let style = CarStyle { front_end: true, ..CarStyle::default() };
    check("build_pages(garage)", hash_pages(&room, &p, [0x80, 0x80, 0x80], &style), GOLD_PAGES[CARS.len()]);
    let _ = all;
}

const GOLD_TRACK: [u64; 2] = [0xe2e52c881072a4b5, 0x344397a4efeca4c3];
const GOLD_GEOM: [u64; 2] = [0x6a42e496ccaa050c, 0x8e1799c0d66d6d27];
const GOLD_CAR_TEX: [u64; 12] = [0xe532ca0d3b81756a, 0x95aaf478859b14ac, 0x0c19e920574245b6, 0x314c4c9c9824b3dc, 0x7fff7b8703f03fbc, 0xfae613830639b947, 0xa636f382f32f19ae, 0x484127aa1d9d7f78, 0x9d51afdb7dc9e168, 0x7109762944b06caa, 0xdc7dbef1436f37b6, 0xa06d089067dcf65b];
const GOLD_PAGES: [u64; 13] = [0x0f25893ed3c8f52a, 0x0b72a814eca1baf0, 0xcf52136dc7fb34c0, 0xc7f1e3807342d50a, 0x202846478c462ead, 0x2b0eb7c2ed5b9c2b, 0x95c27a2ee5db36b6, 0xcc92cb21bfbd9325, 0x131d6ec9446b8d0e, 0xed4962fb018265a8, 0xd496f806f7d978d2, 0x2bb54d28e2bdfb3d, 0x07e4680745796070];

/// Load-time probe (`cargo test -p nfs-assets --test golden timing -- --ignored --nocapture`).
#[test]
#[ignore]
fn timing() {
    let Some(p) = paths() else { return };
    let t0 = std::time::Instant::now();
    let t = TrackAssets::load(&p, "alps").unwrap();
    eprintln!("TIMING TrackAssets::load(alps): {:?} ({} textures)", t0.elapsed(), t.textures.len());
    let c = CarAssets::load(&p, "356_1").unwrap();
    let t0 = std::time::Instant::now();
    for _ in 0..5 {
        c.build_pages(&p, [0x80, 0x40, 0x20], &CarStyle::default()).unwrap();
    }
    eprintln!("TIMING build_pages(356_1) x5: {:?}", t0.elapsed());
}
