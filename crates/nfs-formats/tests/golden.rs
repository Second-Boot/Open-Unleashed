//! Characterisation ("golden hash") tests recorded from the tree BEFORE the 2026-10-10 refactor:
//! every optimisation / constant extraction of the parsers must leave these
//! hashes unchanged. Skipped (with a message) when the original game data is not present.
//!
//! The hash is FNV-1a 64 over the bytes (or over the `Debug` text) of the decoded value — stable
//! across Rust versions, unlike `DefaultHasher`. Run with `NFS_GOLDEN_PRINT=1 --nocapture` to
//! print the current values.
use nfs_formats::{crp::Crp, fsh::Fsh, refpack};
use std::fmt::Write as _;
use std::path::PathBuf;

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

fn game_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
        return Some(PathBuf::from(p));
    }
    let p = PathBuf::from(std::env::var_os("HOME")?).join("Games/NFS-Porsche-Unleashed/game_info/data");
    p.join("GameData").is_dir().then_some(p)
}

macro_rules! need_game {
    () => {
        match game_root() {
            Some(r) => r,
            None => {
                eprintln!("skipping: original game data not found");
                return;
            }
        }
    };
}

fn check(label: &str, got: u64, expect: u64) {
    if std::env::var_os("NFS_GOLDEN_PRINT").is_some() {
        eprintln!("GOLDEN {label} {got:#018x}");
    } else {
        assert_eq!(got, expect, "golden hash {label} changed (got {got:#018x})");
    }
}

fn unpack(root: &std::path::Path, rel: &str) -> Vec<u8> {
    let d = std::fs::read(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    refpack::maybe_decompress(&d).unwrap().into_owned()
}

#[test]
fn golden_decompress() {
    let r = need_game!();
    for (rel, expect) in [
        ("GameData/Track/alps.crp", GOLD_DECOMPRESS[0]),
        ("GameData/CarModel/356_1.crp", GOLD_DECOMPRESS[1]),
        ("GameData/Track/alps.fsh", GOLD_DECOMPRESS[2]),
        ("GameData/Track/skidpad.crp", GOLD_DECOMPRESS[3]),
        ("GameData/Track/monaco1.fsh", GOLD_DECOMPRESS[4]),
    ] {
        let d = unpack(&r, rel);
        let mut h = Fnv::new();
        h.u64(d.len() as u64);
        h.bytes(&d);
        check(&format!("decompress {rel}"), h.0, expect);
    }
}

/// Every level of every entry of a pack, hashed with its dimensions and source format.
fn hash_pack(data: &[u8]) -> u64 {
    let fsh = Fsh::parse(data).unwrap();
    let mut h = Fnv::new();
    for (i, e) in fsh.entries.iter().enumerate() {
        h.bytes(e.name.as_bytes());
        for level in 0..e.blocks[0].mip_levels() {
            match fsh.decode_mip(data, i, level) {
                Ok(img) => {
                    h.u64(img.width as u64);
                    h.u64(img.height as u64);
                    write!(h, "{:?}", img.source_format).unwrap();
                    h.bytes(&img.rgba);
                }
                Err(e) => write!(h, "ERR {e:?}").unwrap(),
            }
        }
        let _ = write!(h, "{:?}", fsh.entry_caption(data, i));
        h.u64(fsh.image_block_len(data.len(), i) as u64);
    }
    h.0
}

#[test]
fn golden_decode_mip() {
    let r = need_game!();
    let one = [
        "GameData/Track/alps.fsh",
        "GameData/Track/alps.env",
        "GameData/CarModel/356_1D.fsh",
        "GameData/CarModel/356_1L.fsh",
        "GameData/CarModel/Shadow.fsh",
        "GameData/CarModel/INTGLASS.FSH",
        "GameData/CarModel/Cabrio.fsh",
        "FEData/Art/FullOval.fsh",
    ];
    for (rel, expect) in one.iter().zip(GOLD_DECODE) {
        let d = unpack(&r, rel);
        check(&format!("decode_mip {rel}"), hash_pack(&d), expect);
    }
    // every pack of the front end's art directory, in name order, in one hash
    let mut names: Vec<_> = std::fs::read_dir(r.join("FEData/Art")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fsh"))).collect();
    names.sort();
    let mut h = Fnv::new();
    for p in &names {
        let d = std::fs::read(p).unwrap();
        let d = refpack::maybe_decompress(&d).unwrap();
        h.bytes(p.file_name().unwrap().to_string_lossy().as_bytes());
        h.u64(hash_pack(&d));
    }
    check("decode_mip FEData/Art/*.fsh", h.0, GOLD_DECODE[one.len()]);
}

fn hash_crp(data: &[u8], geometry: &mut Fnv, materials: &mut Fnv) {
    let crp = Crp::parse(data).unwrap();
    for a in crp.articles(data).unwrap() {
        match crp.article_geometry(data, &a) {
            Ok(g) => write!(geometry, "{g:?}").unwrap(),
            Err(e) => write!(geometry, "ERR {e:?}").unwrap(),
        }
        write!(geometry, "{:?}", crp.article_animation(data, &a)).unwrap();
        write!(geometry, "{:?}", crp.bundle_of(data, &a).ok()).unwrap();
    }
    let mats = crp.materials(data).unwrap();
    write!(materials, "{mats:?}").unwrap();
    for m in &mats {
        let _ = write!(materials, "{}{}", m.texture_name(), m.page_index());
    }
    let methods = crp.render_methods(data).unwrap();
    write!(materials, "{methods:?}").unwrap();
    for m in &methods {
        for k in (0..48u32).chain([0x416D_0001, 0x5273_0001, 0x1_0000]) {
            write!(materials, "{:?}{:?}", m.resolve(k), m.block(k)).unwrap();
        }
        for flags in [0u32, 1, 2, 0xFF, u32::MAX] {
            for mask in [0u32, 1, 4, u32::MAX] {
                write!(materials, "{:?}", m.built_ops(flags, mask).collect::<Vec<_>>()).unwrap();
            }
        }
    }
    write!(materials, "{:?}", crp.bound_points(data).ok()).unwrap();
}

#[test]
fn golden_crp_geometry_and_materials() {
    let r = need_game!();
    let files = ["GameData/Track/alps.crp", "GameData/CarModel/356_1.crp", "FEData/Models/garage.crp"];
    for (i, rel) in files.iter().enumerate() {
        let d = unpack(&r, rel);
        let (mut g, mut m) = (Fnv::new(), Fnv::new());
        hash_crp(&d, &mut g, &mut m);
        check(&format!("article_geometry {rel}"), g.0, GOLD_GEOMETRY[i]);
        check(&format!("materials+render_methods {rel}"), m.0, GOLD_MATERIALS[i]);
    }
}

/// P1 probe (`cargo test -p nfs-formats --test golden timing -- --ignored --nocapture`): the whole
/// mip chain of every `alps.fsh` entry decoded level by level (each call unpacks the block again)
/// against `decode_levels` (once per entry). Same pixels, asserted.
#[test]
#[ignore]
fn timing_decode_levels() {
    let r = need_game!();
    let d = unpack(&r, "GameData/Track/alps.fsh");
    let fsh = Fsh::parse(&d).unwrap();
    let t0 = std::time::Instant::now();
    let mut per_level = Vec::new();
    for i in 0..fsh.entries.len() {
        for level in 0..fsh.entries[i].blocks[0].mip_levels() {
            per_level.push(fsh.decode_mip(&d, i, level).unwrap().rgba);
        }
    }
    let a = t0.elapsed();
    let t0 = std::time::Instant::now();
    let mut once = Vec::new();
    for i in 0..fsh.entries.len() {
        once.extend(fsh.decode_levels(&d, i, fsh.entries[i].blocks[0].mip_levels()).unwrap().into_iter().map(|x| x.rgba));
    }
    let b = t0.elapsed();
    assert!(per_level == once);
    eprintln!("TIMING alps.fsh all levels: decode_mip per level {a:?}, decode_levels {b:?}");
}

/// The first 40 video frames of every shipped movie, decoded and converted to RGBA (needs the
/// tables out of `nfs5.exe`): guards the IDCT / motion / colour-conversion chain.
#[test]
fn golden_mad_frames() {
    use nfs_formats::mad::{chunks, MadDecoder, MadHeader, MadTables};
    let r = need_game!();
    let Ok(exe) = std::fs::read(r.join("nfs5.exe")) else {
        eprintln!("skipping: nfs5.exe not found");
        return;
    };
    let mut names: Vec<_> = std::fs::read_dir(r.join("FEData/Movies")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mad"))).collect();
    names.sort();
    let mut h = Fnv::new();
    for p in &names {
        let d = std::fs::read(p).unwrap();
        let video: Vec<_> = chunks(&d).filter(|c| c.is_video()).take(40).map(|c| c.data.to_vec()).collect();
        let hdr = MadHeader::parse(&video[0]).unwrap();
        let mut dec = MadDecoder::new(MadTables::from_exe(&exe).unwrap(), &hdr);
        h.bytes(p.file_name().unwrap().to_string_lossy().as_bytes());
        for c in &video {
            let f = dec.decode_frame(c);
            h.u64(f.width as u64);
            h.u64(f.height as u64);
            h.bytes(&f.rgba);
        }
    }
    check("mad frames", h.0, GOLD_MAD);
}

/// Every shipped sound bank: the parsed directory (all PT headers, `Debug`) and every used slot
/// decoded to PCM.
#[test]
fn golden_bnk_banks() {
    use nfs_formats::bnk::Bnk;
    let r = need_game!();
    let mut files = Vec::new();
    let mut stack = vec![r.join("GameData")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bnk")) {
                files.push(p);
            }
        }
    }
    files.sort();
    assert!(!files.is_empty());
    let mut h = Fnv::new();
    for p in &files {
        let d = std::fs::read(p).unwrap();
        h.bytes(p.file_name().unwrap().to_string_lossy().as_bytes());
        let b = Bnk::parse(&d).unwrap();
        write!(h, "{b:?}").unwrap();
        for s in b.used_slots() {
            match b.decode_slot(&d, s) {
                Ok(pcm) => {
                    write!(h, "{:?}{}{}", pcm.loop_range, pcm.rate, pcm.channels).unwrap();
                    for v in &pcm.samples {
                        h.bytes(&v.to_le_bytes());
                    }
                }
                Err(e) => write!(h, "ERR {e:?}").unwrap(),
            }
        }
    }
    check("bnk banks", h.0, GOLD_BNK);
}

const GOLD_BNK: u64 = 0xe24136e30db1dce9;
const GOLD_MAD: u64 = 0x5c3453753ca1e51d;
const GOLD_DECOMPRESS: [u64; 5] = [0x342c9c5791f9a6d3, 0x3bee828ee76fe5a2, 0xabc780f0f0aa729c, 0x923995eba31be76d, 0xdf0e66fa5601fb75];
const GOLD_DECODE: [u64; 9] = [0x9f712d2d04930b96, 0x949b4360b9c44a05, 0x410b5a2a996104a1, 0x1e04f3403e49f9b7, 0x342c10a558d34fff, 0x94aef341427df812, 0xa202a965e296a6d0, 0xbd28e77de2432d7c, 0x83001a5babeaef7d];
const GOLD_GEOMETRY: [u64; 3] = [0xe0fc96135beca73f, 0x71dfcadefaa705b4, 0xae7555d238d5cdea];
const GOLD_MATERIALS: [u64; 3] = [0xf53647dae4d169f1, 0xb3a0831d18f6265a, 0x6df05f619226a183];
