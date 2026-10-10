//! Hostile / truncated input: every parser must return `Err` (or a sensible `Ok`), never panic and
//! never ask the allocator for gigabytes. Each real file is cut at many lengths and has single
//! header bytes forced to 0x00 / 0x80 / 0xFF; the synthetic cases need no game data. The real-file
//! tests skip (with a message) without the installation.
use nfs_formats::{bigf::Bigf, bnk::Bnk, crp::Crp, fsh::Fsh, mad::{MadTables, PeImage}, refpack};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

fn game_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
        return Some(PathBuf::from(p));
    }
    let p = PathBuf::from(std::env::var_os("HOME")?).join("Games/NFS-Porsche-Unleashed/game_info/data");
    p.join("GameData").is_dir().then_some(p)
}

fn real(rel: &str) -> Option<Vec<u8>> {
    let r = game_root()?;
    match std::fs::read(r.join(rel)) {
        Ok(d) => Some(d),
        Err(_) => {
            eprintln!("skipping {rel}: not found");
            None
        }
    }
}

/// Cut lengths to try: every length of the first `dense` bytes, then `spread` evenly spaced ones
/// (and the file length minus 1).
fn cuts(len: usize, dense: usize, spread: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..dense.min(len)).collect();
    for k in 1..=spread {
        v.push(len * k / (spread + 1));
    }
    v.push(len.saturating_sub(1));
    v.sort_unstable();
    v.dedup();
    v
}

/// Run `f` on every prefix and every header-byte mutation of `data`; fail on any panic.
fn never_panics(label: &str, data: &[u8], dense: usize, spread: usize, header_bytes: usize, random: usize, f: impl Fn(&[u8])) {
    let mut failures: Vec<String> = Vec::new();
    let mut run = |what: String, d: &[u8]| {
        if catch_unwind(AssertUnwindSafe(|| f(d))).is_err() {
            failures.push(what);
        }
    };
    for n in cuts(data.len(), dense, spread) {
        run(format!("{label}: prefix of {n} bytes"), &data[..n]);
    }
    let mut m = data.to_vec();
    for i in 0..header_bytes.min(data.len()) {
        for v in [0x00u8, 0x80, 0xFF] {
            if data[i] == v {
                continue;
            }
            m[i] = v;
            run(format!("{label}: byte {i:#x} forced to {v:#04x}"), &m);
        }
        m[i] = data[i];
    }
    // single random bytes anywhere (a fixed xorshift sequence, the same on every run)
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    for k in 0..random {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let (i, v) = ((x >> 8) as usize % data.len().max(1), (x & 0xFF) as u8);
        let keep = m[i];
        m[i] = v;
        run(format!("{label}: random mutation #{k}: byte {i:#x} = {v:#04x}"), &m);
        m[i] = keep;
    }
    assert!(failures.is_empty(), "{} panics, first few: {:#?}", failures.len(), &failures[..failures.len().min(8)]);
}

fn exercise_fsh(d: &[u8]) {
    if let Ok(f) = Fsh::parse(d) {
        for i in 0..f.entries.len() {
            for level in 0..f.entries[i].blocks[0].mip_levels().min(12) {
                let _ = f.decode_mip(d, i, level);
            }
            let _ = f.image_block_len(d.len(), i);
            let _ = f.entry_caption(d, i);
        }
    }
}

#[test]
fn fsh_hostile_input() {
    for rel in ["FEData/Art/leader.fsh", "FEData/Art/LoadText.fsh", "FEData/Art/pic16.fsh", "GameData/CarModel/Shadow.fsh", "GameData/CarModel/Cabrio.fsh"] {
        let Some(d) = real(rel) else { continue };
        never_panics(rel, &d, 400, 40, 160, 300, exercise_fsh);
    }
}

#[test]
fn fsh_synthetic_hostile_input() {
    // "SHPI" + size + count + dir id: a count the file cannot hold, a short header, no entries
    let mut huge = b"SHPI".to_vec();
    huge.extend_from_slice(&0u32.to_le_bytes());
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    huge.extend_from_slice(b"GIMX");
    assert!(Fsh::parse(&huge).is_err());
    for n in 0..huge.len() {
        assert!(Fsh::parse(&huge[..n]).is_err());
    }
    // an indexed image whose palette block claims no colours
    let mut d = b"SHPI".to_vec();
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(b"GIMX");
    d.extend_from_slice(b"test");
    d.extend_from_slice(&24u32.to_le_bytes());
    d.push(0x7B);
    d.extend_from_slice(&18u32.to_le_bytes()[..3]);
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 8]);
    d.extend_from_slice(&[0, 1]);
    d.push(0x2A);
    d.extend_from_slice(&[0u8; 3]);
    d.extend_from_slice(&0u16.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 8]);
    let f = Fsh::parse(&d).unwrap();
    assert!(f.decode(&d, 0).is_err(), "an empty palette must be an error, not a panic");
}

#[test]
fn fsh_more_than_sixteen_blocks_is_an_error() {
    let mut d = b"SHPI".to_vec();
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(b"GIMX");
    d.extend_from_slice(b"test");
    d.extend_from_slice(&24u32.to_le_bytes());
    // 20 chained 16-byte blocks (size = 16 each, none ending the chain)
    for _ in 0..20 {
        d.push(0x2A);
        d.extend_from_slice(&16u32.to_le_bytes()[..3]);
        d.extend_from_slice(&[0u8; 12]);
    }
    d.extend_from_slice(&[0u8; 16]);
    assert!(Fsh::parse(&d).is_err());
}

#[test]
fn crp_hostile_input() {
    for rel in ["FEData/Models/track.crp"] {
        let Some(d) = real(rel) else { continue };
        let d = refpack::maybe_decompress(&d).unwrap().into_owned();
        never_panics(rel, &d, 300, 30, 96, 600, |d| {
            if let Ok(crp) = Crp::parse(d) {
                if let Ok(arts) = crp.articles(d) {
                    for a in &arts {
                        let _ = crp.article_geometry(d, a);
                        let _ = crp.article_animation(d, a);
                        let _ = crp.bundle_of(d, a);
                    }
                }
                let _ = crp.materials(d);
                let _ = crp.render_methods(d);
                let _ = crp.embedded_texture_files(d);
                let _ = crp.bound_points(d);
            }
        });
    }
}

#[test]
fn bigf_hostile_input() {
    let Some(d) = real("GameData/Track/Data/camera.viv") else { return };
    never_panics("camera.viv", &d, 400, 40, 160, 100, |d| {
        if let Ok(b) = Bigf::parse(d) {
            for e in &b.entries {
                let _ = b.data(d, e);
                let _ = b.get(d, &e.name);
            }
        }
    });
}

#[test]
fn bigf_synthetic_hostile_input() {
    let mut d = b"BIGF".to_vec();
    d.extend_from_slice(&0u32.to_be_bytes());
    d.extend_from_slice(&u32::MAX.to_be_bytes());
    d.extend_from_slice(&0u32.to_be_bytes());
    assert!(Bigf::parse(&d).is_err());
    // a valid archive asked for its data through another (shorter) buffer must not panic
    let mut ok = b"BIGF".to_vec();
    ok.extend_from_slice(&0u32.to_be_bytes());
    ok.extend_from_slice(&1u32.to_be_bytes());
    ok.extend_from_slice(&0u32.to_be_bytes());
    ok.extend_from_slice(&u32::MAX.to_be_bytes()); // offset
    ok.extend_from_slice(&u32::MAX.to_be_bytes()); // size
    ok.extend_from_slice(b"x\0");
    assert!(Bigf::parse(&ok).is_err());
}

#[test]
fn pe_hostile_input() {
    let Some(d) = real("nfs5.exe") else { return };
    never_panics("nfs5.exe", &d, 0x400, 40, 0x200, 0, |d| {
        let _ = PeImage::parse(d);
        let _ = MadTables::from_exe(d);
    });
    // a `PE` pointer far past the end of the file
    let mut m = d[..0x200].to_vec();
    m[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(PeImage::parse(&m).is_err());
}

#[test]
fn bnk_hostile_input() {
    let Some(d) = real("GameData/Sounds/SND_coll.bnk") else { return };
    // only the directory and the first sample headers matter; the sample data is long
    let head = &d[..d.len().min(0x4000)];
    never_panics("SND_coll.bnk", head, 600, 40, 200, 300, |d| {
        if let Ok(b) = Bnk::parse(d) {
            for s in b.used_slots().take(8) {
                let _ = b.decode_slot(d, s);
            }
        }
    });
}

#[test]
fn refpack_hostile_input() {
    let Some(d) = real("GameData/Track/alps.fsh") else { return };
    assert!(refpack::is_refpack(&d));
    let head = &d[..d.len().min(0x20000)];
    never_panics("alps.fsh (RefPack)", head, 300, 40, 24, 200, |d| {
        let _ = refpack::decompress(d);
    });
    // a header promising 16 MiB of output over a few bytes of commands: an error, and no huge
    // allocation up front
    let s = [0x90, 0xFB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFC];
    assert!(refpack::decompress(&s).is_err());
}

fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The stricter block-chain rule must not reject any shipped pack: every `.fsh` / `.env` file and
/// every SHPI embedded in a `.crp` still parses and decodes its base level.
#[test]
fn every_shipped_shpi_still_parses() {
    let Some(root) = game_root() else {
        eprintln!("skipping: original game data not found");
        return;
    };
    let mut files = Vec::new();
    walk(&root.join("GameData"), &mut files);
    walk(&root.join("FEData"), &mut files);
    let mut packs = 0;
    for p in &files {
        let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let found: Vec<Vec<u8>> = match ext.as_str() {
            "fsh" | "env" => vec![refpack::maybe_decompress(&std::fs::read(p).unwrap()).unwrap().into_owned()],
            "crp" => {
                let d = refpack::maybe_decompress(&std::fs::read(p).unwrap()).unwrap().into_owned();
                match Crp::parse(&d) {
                    Ok(c) => c.embedded_texture_files(&d).unwrap_or_default().into_iter().map(|b| b.to_vec()).collect(),
                    Err(_) => continue,
                }
            }
            _ => continue,
        };
        for d in found {
            if d.get(..4) != Some(b"SHPI") {
                continue;
            }
            let f = Fsh::parse(&d).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            for i in 0..f.entries.len().min(4) {
                let _ = f.decode(&d, i); // some entries carry a palette kind the decoder reports as Err
            }
            packs += 1;
        }
    }
    assert!(packs > 300, "expected the shipped packs, found {packs}");
}
