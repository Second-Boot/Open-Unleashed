//! ENHANCEMENT E41: the HD texture pack («улучшенные текстуры»).
//!
//! A pack is a directory of images, one per texture, named by a **key** (below), made from the
//! original's textures by `asset-extractor hd-export` and upscaled x4 by a model (`tools/hd-textures`). The game finds it
//! beside itself ([`HdPack::locate`]): `<hd_textures>/x4/` or a flat `<hd_textures>/` with `pack.txt` (`scale=4`). Without one the
//! switch's row is grey and cannot change. Nothing here is the original's: with E41 off (`NFS_ENH=off`) no pack file is read.
//!
//! ## Keys
//!
//! `<class>/<scope>/<name>`, every component lower-case and made filesystem-safe by
//! [`safe`] (`[a-z0-9_-]` kept, any other byte `~XX` hex, an empty name `~`); the file is
//! `<pack>/<key>.ktx2` (GPU-compressed BC7 with mips, [`crate::ktx2`], what ships) or `<pack>/<key>.png` (the
//! uncompressed form the upscaler writes; looked for second), through [`FORMATS`]:
//!
//! | key | what |
//! |---|---|
//! | `track/<track>/<entry>` | an entry of `Track/<track>.fsh` (pack key 0, the 3D world's textures) |
//! | `car/<model>/f<N>/<entry>` | an entry of the car's embedded `sf` pack for `.tpg` file N (the body, wheels, interior ...) |
//! | `carglobal/<file>/<entry>` | an entry of `CarModel/{Shadow,INTGLASS,Suit,Head,Cabrio}.fsh`, shared by every car |
//! | `car/<model>/damage_<d or l>/<NNN>_<entry>` | a damage decal of `<model>D.fsh` / `<model>L.fsh`, NNN the entry's index in the pack (the names repeat) |
//! | `sky/<track>/<entry>` | a GPU texture of `Track/Sky/<track>.fsh`: the horizon mountains (`horz`, `hor0`..) and the cloud layers; UVs are normalized fractions of the strip (`sky.rs`) |
//!
//! An image must be exactly `scale x` the original's size (`pack.txt`'s `scale=4`, default 4);
//! another size is refused (logged) and the original is used. The pack covers only the
//! textures sampled with normalized UVs in the 3D world; everything whose pixel size matters stays
//! original (the audit in `RE/notes/ENHANCEMENTS.md`, E41).
//!
//! ## Alpha
//!
//! The original's alpha semantics are kept by [`AlphaPolicy`], taken from the ORIGINAL texels:
//! no alpha stays opaque, a 1-bit alpha is re-thresholded at 128, a small set of alpha values (the
//! cars' paint-slot selectors) snaps to the nearest member, a smooth alpha is kept.

/// The enhanced game's own data folder (`layouts/`, `hd_textures/`) in the game root, beside the executable or in a macOS
/// bundle's Resources: the current name first, then `remaster` (its earlier name, still accepted).
pub const ENHANCED_FOLDERS: [&str; 2] = ["enhanced", "remaster"];

use crate::GamePaths;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Pack scale when `pack.txt` has none.
pub const DEFAULT_SCALE: u32 = 4;

/// The pack scale of a level of the switch E41: 0 off, none; 1 on, the x4 pack.
pub fn scale_of_level(level: u8) -> Option<u32> {
    (level > 0).then_some(DEFAULT_SCALE)
}

/// Where a pack root may be, in order: `hd_textures` (then `enhanced/hd_textures`, `remaster/hd_textures`) beside the executable, beside a macOS
/// `.app`, in the game root and in the bundle's Resources.
fn candidates(exe: Option<&Path>, game_root: &Path) -> Vec<PathBuf> {
    let exe_dir = exe.and_then(Path::parent);
    let contents = exe_dir.and_then(Path::parent);
    let bundle = contents.and_then(Path::parent).filter(|b| b.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")));
    let mut bases: Vec<PathBuf> = exe_dir.map(Path::to_path_buf).into_iter().collect();
    bases.extend(bundle.and_then(Path::parent).map(Path::to_path_buf));
    bases.push(game_root.to_path_buf());
    bases.extend(bundle.and(contents).map(|c| c.join("Resources")));
    let mut c = Vec::new();
    for base in bases {
        c.push(base.join("hd_textures"));
        c.extend(ENHANCED_FOLDERS.iter().map(|f| base.join(f).join("hd_textures")));
    }
    c
}

type Decoder = fn(&Path) -> Option<(u32, u32, Vec<u8>)>;

/// The file formats a key is looked up in, in order: extension, decoder to RGBA8 (`w`, `h`, base level pixels).
pub const FORMATS: &[(&str, Decoder)] = &[(KTX2, decode_ktx2), ("png", decode_png)];
/// The GPU-compressed form's extension (the first of [`FORMATS`]): [`HdPack::load_any`] hands its BC7 blocks on as they are.
const KTX2: &str = "ktx2";

fn decode_ktx2(p: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let img = crate::ktx2::read_bc7(&std::fs::read(p).ok()?)?;
    Some((img.width, img.height, crate::ktx2::decode_level(img.levels.first()?, img.width, img.height)))
}

fn decode_png(p: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::open(p).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

/// A lower-case, filesystem-safe path component: `[a-z0-9_-]` kept, every other byte `~XX`.
pub fn safe(name: &str) -> String {
    let t = name.trim().to_lowercase();
    if t.is_empty() {
        return "~".to_string();
    }
    let mut out = String::with_capacity(t.len());
    for b in t.bytes() {
        if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("~{b:02x}"));
        }
    }
    out
}

/// `track/<track>/<entry>`
pub fn key_track(track: &str, entry: &str) -> String {
    format!("track/{}/{}", safe(track), safe(entry))
}
/// `sky/<track>/<entry>`: an entry of `Track/Sky/<track>.fsh` (the horizon mountains, the cloud layers)
pub fn key_sky(track: &str, entry: &str) -> String {
    format!("sky/{}/{}", safe(track), safe(entry))
}
/// `car/<model>/f<file>/<entry>`, `file` the `.tpg` file number the embedded pack belongs to
pub fn key_car(model: &str, file: usize, entry: &str) -> String {
    format!("car/{}/f{file}/{}", safe(model), safe(entry))
}
/// `carglobal/<file>/<entry>`, `file` the global pack's name without extension
pub fn key_car_global(file: &str, entry: &str) -> String {
    format!("carglobal/{}/{}", safe(file), safe(entry))
}
/// `car/<model>/damage_<d|l>/<index>_<entry>`: the decal packs repeat their entry names (several `0000`), so the
/// entry's 3-digit index in the pack is part of the key
pub fn key_car_damage(model: &str, light: bool, index: usize, entry: &str) -> String {
    format!("car/{}/damage_{}/{index:03}_{}", safe(model), if light { 'l' } else { 'd' }, safe(entry))
}

/// How the HD image's alpha is made to behave like the original's (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlphaPolicy {
    /// every original texel opaque: the HD alpha is forced to 255
    Opaque,
    /// the original's alpha is 0 / 255 only: the HD alpha is thresholded at 128
    OneBit,
    /// a few distinct values (paint-slot selectors, 4444 levels): snapped to the nearest
    Levels(Vec<u8>),
    /// a smooth alpha: kept as the model made it
    Smooth,
}

impl AlphaPolicy {
    /// The policy of an original RGBA8 image.
    pub fn of(rgba: &[u8]) -> Self {
        let mut seen = [false; 256];
        for p in rgba.chunks_exact(4) {
            seen[p[3] as usize] = true;
        }
        let vals: Vec<u8> = (0..=255u8).filter(|&a| seen[a as usize]).collect();
        match vals.as_slice() {
            [] | [255] => Self::Opaque,
            v if v.iter().all(|&a| a == 0 || a == 255) => Self::OneBit,
            v if v.len() <= 24 => Self::Levels(v.to_vec()),
            _ => Self::Smooth,
        }
    }

    /// Apply to an HD image's pixels.
    pub fn apply(&self, rgba: &mut [u8]) {
        match self {
            Self::Opaque => rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255),
            Self::OneBit => rgba.chunks_exact_mut(4).for_each(|p| p[3] = if p[3] >= 128 { 255 } else { 0 }),
            Self::Levels(v) => {
                // a lookup table: 256 entries once, not a search per texel
                let lut: Vec<u8> = (0..=255u8).map(|a| *v.iter().min_by_key(|&&m| (m as i32 - a as i32).abs()).unwrap_or(&a)).collect();
                rgba.chunks_exact_mut(4).for_each(|p| p[3] = lut[p[3] as usize]);
            }
            Self::Smooth => {}
        }
    }
}

/// One HD texture ready for the GPU: `width x height` RGBA8, the alpha policy applied.
#[derive(Debug, Clone)]
pub struct HdImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// An opened pack directory.
#[derive(Debug)]
pub struct HdPack {
    dir: PathBuf,
    scale: u32,
    /// PERF: the pack's files and folders, read once at [`HdPack::open`]
    listing: Listing,
}

/// How deep [`Listing::read`] descends below the pack folder (the deepest key, `car/<model>/damage_<d|l>/<name>`, is 4).
const MAX_LISTING_DEPTH: usize = 8;

/// PERF: every file and every folder under a pack, relative to it with `/` separators, as
/// `std::fs::metadata` saw them (symbolic links followed, as `is_file` / `is_dir` do). A key's lookup is then a set probe
/// instead of one `stat` per format per texture (thousands at a track's load). The pack is not re-read: a file added
/// while the game runs is not seen (the pack is a shipped, read-only folder). A name that is not UTF-8 cannot be a key
/// ([`safe`] makes ASCII) and is left out.
#[derive(Debug, Default)]
struct Listing {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

impl Listing {
    fn read(dir: &Path) -> Self {
        let mut l = Listing::default();
        let mut stack = vec![(dir.to_path_buf(), String::new(), 0usize)];
        while let Some((d, rel, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
                let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                let p = e.path();
                match std::fs::metadata(&p) {
                    Ok(m) if m.is_dir() => {
                        if depth + 1 < MAX_LISTING_DEPTH {
                            stack.push((p, r.clone(), depth + 1));
                        }
                        l.dirs.insert(r);
                    }
                    Ok(m) if m.is_file() => {
                        l.files.insert(r);
                    }
                    _ => {}
                }
            }
        }
        l
    }
}

/// What a key's file holds, for the track's textures (which can take the GPU-compressed form as it is).
#[derive(Debug)]
pub enum HdFile {
    /// RGBA8 base level (a PNG, or a KTX2 decoded on the CPU), the alpha policy applied
    Rgba(HdImage),
    /// BC7 blocks with their mip chain (KTX2): `levels[0]` the base level. The alpha policy was applied by the pack tool
    Bc7(crate::ktx2::Bc7Image),
    /// a KTX2 file decoded on the CPU, every level (a device without BC7): RGBA8 mips, the pack tool's alpha policy and chain
    Chain { width: u32, height: u32, mips: Vec<Vec<u8>> },
}

impl HdPack {
    /// Open a pack directory (`None` when it is not a directory). `pack.txt` may say `scale=N`.
    pub fn open(dir: &Path) -> Option<Self> {
        if !dir.is_dir() {
            return None;
        }
        let scale = std::fs::read_to_string(dir.join("pack.txt"))
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("scale=").and_then(|v| v.trim().parse::<u32>().ok())))
            .filter(|s| (1..=16).contains(s))
            .unwrap_or(DEFAULT_SCALE);
        Some(Self { dir: dir.to_path_buf(), scale, listing: Listing::read(dir) })
    }

    /// The pack inside a pack root `root`: its `x4/`, or `root` itself when it has a `pack.txt` of that scale (a flat pack, e.g. the
    /// upscaler's output dir). A folder with no image is no pack.
    pub fn open_in_root(root: &Path) -> Option<Self> {
        let pack = Self::open(&root.join("x4")).map(|p| Self { scale: DEFAULT_SCALE, ..p }).or_else(|| {
            let flat = root.join("pack.txt").is_file().then(|| Self::open(root)).flatten()?;
            (flat.scale == DEFAULT_SCALE).then_some(flat)
        })?;
        pack.listing.files.iter().any(|f| FORMATS.iter().any(|(ext, _)| f.ends_with(&format!(".{ext}")))).then_some(pack)
    }

    /// Where the pack lives: `$NFS_HD_TEX_DIR` (a pack root; alone when set), else [`candidates`], and in a development build the
    /// repository's `enhanced/hd_textures`.
    pub fn locate(paths: &GamePaths) -> Option<Self> {
        // an explicit override is the only candidate: a wrong path must not fall back silently
        if let Some(d) = std::env::var_os("NFS_HD_TEX_DIR").filter(|v| !v.is_empty()) {
            return Self::open_in_root(Path::new(&d));
        }
        let exe = std::env::current_exe().ok();
        let mut c = candidates(exe.as_deref(), &paths.root);
        if cfg!(debug_assertions) {
            c.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../enhanced/hd_textures"));
        }
        c.into_iter().find_map(|d| Self::open_in_root(&d))
    }

    /// The pack of this run, looked for once. `None`: there is none (the original is used, one info line).
    pub fn global(paths: &GamePaths) -> Option<&'static HdPack> {
        static PACK: OnceLock<Option<HdPack>> = OnceLock::new();
        PACK.get_or_init(|| {
            let p = Self::locate(paths);
            match &p {
                Some(p) => tracing::info!(dir = %p.dir.display(), scale = p.scale, "HD texture pack (ENHANCEMENT E41)"),
                None => tracing::info!("no HD texture pack beside the game (ENHANCEMENT E41): the original textures"),
            }
            p
        })
        .as_ref()
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The first of [`FORMATS`] the pack has a file of `key` in: its path, extension and decoder.
    fn file(&self, key: &str) -> Option<(PathBuf, &'static str, Decoder)> {
        FORMATS.iter().find_map(|&(ext, dec)| {
            let name = format!("{key}.{ext}");
            self.listing.files.contains(&name).then(|| (self.dir.join(name), ext, dec))
        })
    }

    /// Whether the pack has the directory `rel` (a key prefix such as `car/993`): a cheap test that a car has
    /// anything in the pack at all.
    pub fn has_dir(&self, rel: &str) -> bool {
        rel.is_empty() || self.listing.dirs.contains(rel)
    }

    /// Whether the pack holds an image for `key`.
    pub fn has(&self, key: &str) -> bool {
        self.file(key).is_some()
    }

    /// The HD image of `key` for an original of `ow x oh` texels (`orig` its RGBA8 level 0): the
    /// file decoded, its size checked against `scale x`, the original's alpha semantics applied.
    /// `None` when there is no file, it cannot be read, or it has the wrong size (logged).
    pub fn load(&self, key: &str, orig: &[u8], ow: u32, oh: u32) -> Option<HdImage> {
        let (path, _, dec) = self.file(key)?;
        self.load_file(key, &path, dec, orig, ow, oh)
    }

    /// [`Self::load`] of the file [`Self::file`] found.
    fn load_file(&self, key: &str, path: &Path, dec: Decoder, orig: &[u8], ow: u32, oh: u32) -> Option<HdImage> {
        let Some((w, h, mut rgba)) = dec(path) else {
            tracing::warn!(key, "HD texture unreadable: the original is used (ENHANCEMENT E41)");
            return None;
        };
        if (w, h) != (ow * self.scale, oh * self.scale) || rgba.len() != (w * h * 4) as usize {
            tracing::warn!(key, got = ?(w, h), want = ?(ow * self.scale, oh * self.scale), "HD texture has the wrong size: the original is used (ENHANCEMENT E41)");
            return None;
        }
        AlphaPolicy::of(orig).apply(&mut rgba);
        Some(HdImage { width: w, height: h, rgba })
    }

    /// [`Self::load`] for the track's textures: a KTX2 file comes back as its BC7 blocks and mip chain when `bc7` (the device takes
    /// BC7), else decoded to RGBA8 like a PNG. The size check is the same; a BC7 file's alpha policy was applied by the pack tool
    /// (`hd-pack bc7`, from the exporter's `alpha` column), a PNG's here.
    pub fn load_any(&self, key: &str, orig: &[u8], ow: u32, oh: u32, bc7: bool) -> Option<HdFile> {
        // the first format the pack has: a KTX2 file is taken as it is, else (a PNG) the RGBA8 load
        let (p, ext, dec) = self.file(key)?;
        if ext == KTX2 {
            let Some(img) = std::fs::read(&p).ok().and_then(|b| crate::ktx2::read_bc7(&b)) else {
                tracing::warn!(key, "HD texture unreadable: the original is used (ENHANCEMENT E41)");
                return None;
            };
            if (img.width, img.height) != (ow * self.scale, oh * self.scale) {
                tracing::warn!(key, got = ?(img.width, img.height), want = ?(ow * self.scale, oh * self.scale), "HD texture has the wrong size: the original is used (ENHANCEMENT E41)");
                return None;
            }
            if bc7 {
                return Some(HdFile::Bc7(img));
            }
            return Some(HdFile::Chain { width: img.width, height: img.height, mips: decode_chain(&img) });
        }
        self.load_file(key, &p, dec, orig, ow, oh).map(HdFile::Rgba)
    }
}

/// The whole mip chain of a decoded KTX2 BC7 image as RGBA8 levels (a device without BC7 support gets the levels the pack tool made).
pub fn decode_chain(img: &crate::ktx2::Bc7Image) -> Vec<Vec<u8>> {
    let (mut w, mut h) = (img.width, img.height);
    img.levels
        .iter()
        .map(|l| {
            let px = crate::ktx2::decode_level(l, w, h);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            px
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pack is looked for beside the executable, beside a macOS `.app`, in the game root; the bundle's Resources only for a bundle.
    #[test]
    fn the_pack_is_looked_for_beside_the_game() {
        let c = candidates(Some(Path::new("/g/Open.app/Contents/MacOS/Open")), Path::new("/data"));
        for p in ["/g/Open.app/Contents/MacOS/hd_textures", "/g/hd_textures", "/g/enhanced/hd_textures", "/data/hd_textures", "/data/enhanced/hd_textures", "/g/Open.app/Contents/Resources/hd_textures"] {
            assert!(c.contains(&PathBuf::from(p)), "{p}");
        }
        assert_eq!(c[0], PathBuf::from("/g/Open.app/Contents/MacOS/hd_textures"));
        let c = candidates(Some(Path::new("/g/open-unleashed")), Path::new("/g/game"));
        assert_eq!(c[0], PathBuf::from("/g/hd_textures"));
        assert!(c.contains(&PathBuf::from("/g/game/hd_textures")));
        assert!(!c.iter().any(|p| p.to_string_lossy().contains("Resources")));
    }

    /// A pack root: `x4/` or a flat x4 pack; an x2 pack and a folder with no image are none.
    #[test]
    fn a_pack_root_is_x4_with_images() {
        let dir = std::env::temp_dir().join(format!("nfs_hdpack_root_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x4/track")).unwrap();
        assert!(HdPack::open_in_root(&dir).is_none(), "an empty x4");
        std::fs::write(dir.join("x4/track/a.ktx2"), b"x").unwrap();
        assert_eq!(HdPack::open_in_root(&dir).map(|p| p.scale()), Some(4));
        let flat = dir.join("flat");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join("a.png"), b"x").unwrap();
        std::fs::write(flat.join("pack.txt"), b"scale=4\n").unwrap();
        assert!(HdPack::open_in_root(&flat).is_some());
        std::fs::write(flat.join("pack.txt"), b"scale=2\n").unwrap();
        assert!(HdPack::open_in_root(&flat).is_none(), "an x2 pack is not read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_safe_and_stable() {
        assert_eq!(key_track("Alps", "TR12 "), "track/alps/tr12");
        assert_eq!(key_track("alps", "a/b.c"), "track/alps/a~2fb~2ec");
        assert_eq!(key_track("alps", "  "), "track/alps/~");
        assert_eq!(key_car("993", 3, "Hood"), "car/993/f3/hood");
        assert_eq!(key_car_global("INTGLASS", "x"), "carglobal/intglass/x");
        assert_eq!(key_car_damage("993", true, 7, "0001"), "car/993/damage_l/007_0001");
    }

    /// the listing read at `open` picks the same file for every key as the old `stat` per format did
    /// (`dir/<key>.<ext>` `is_file`, FORMATS in order), and `has_dir` answers as `is_dir` did — on a pack made in a temp dir:
    /// a key with both formats, one with each, one with none, nested keys, a folder named like a file, a file named like a
    /// folder.
    #[test]
    fn the_listing_picks_the_file_the_stats_did() {
        let dir = std::env::temp_dir().join(format!("nfs_hdpack_p9_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["track/alps", "car/993/f3", "car/993/damage_l", "carglobal/intglass", "track/alps/dir.png"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["pack.txt", "track/alps/both.ktx2", "track/alps/both.png", "track/alps/png.png", "track/alps/ktx.ktx2", "car/993/f3/hood.png", "car/993/damage_l/007_0001.ktx2", "carglobal/intglass/x.png", "track/alps/odd.jpg", "car/993/f3/file_dir"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("pack.txt"), b"scale=2\n").unwrap();
        let pack = HdPack::open(&dir).expect("pack");
        assert_eq!(pack.scale(), 2);
        // the lookup before P9, verbatim
        let old_file = |key: &str| FORMATS.iter().find_map(|&(ext, _)| {
            let p = dir.join(format!("{key}.{ext}"));
            p.is_file().then_some(p)
        });
        let keys = ["track/alps/both", "track/alps/png", "track/alps/ktx", "track/alps/none", "track/alps/odd", "track/alps/dir", "car/993/f3/hood", "car/993/damage_l/007_0001", "carglobal/intglass/x", "car/993/f3/file_dir", "car/993", "pack", "track/alps/both.png"];
        for key in keys {
            assert_eq!(pack.file(key).map(|(p, _, _)| p), old_file(key), "{key}");
            assert_eq!(pack.has(key), old_file(key).is_some(), "{key}");
        }
        assert_eq!(pack.file("track/alps/both").map(|(_, ext, _)| ext), Some("ktx2"));
        for rel in ["car", "car/993", "car/993/f3", "car/993/damage_l", "carglobal", "carglobal/intglass", "track/alps/dir.png", "car/993/f3/file_dir", "car/911", "track/alps/both.png", "nope", ""] {
            assert_eq!(pack.has_dir(rel), dir.join(rel).is_dir(), "{rel:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn alpha_policies() {
        let opaque = [1u8, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(AlphaPolicy::of(&opaque), AlphaPolicy::Opaque);
        let one = [0u8, 0, 0, 0, 9, 9, 9, 255];
        let p = AlphaPolicy::of(&one);
        assert_eq!(p, AlphaPolicy::OneBit);
        let mut hd = [1u8, 1, 1, 127, 2, 2, 2, 128];
        p.apply(&mut hd);
        assert_eq!([hd[3], hd[7]], [0, 255]);
        let slots = [0u8, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 255];
        let p = AlphaPolicy::of(&slots);
        assert_eq!(p, AlphaPolicy::Levels(vec![0, 100, 255]));
        let mut hd = [0u8, 0, 0, 70, 0, 0, 0, 200];
        p.apply(&mut hd);
        assert_eq!([hd[3], hd[7]], [100, 255]);
        let mut hd = [0u8, 0, 0, 5];
        AlphaPolicy::Opaque.apply(&mut hd);
        assert_eq!(hd[3], 255);
    }
}
