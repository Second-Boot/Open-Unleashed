//! Car asset bundle: model CRP, day/night texture packs, texture-page layout, simulation data.

use crate::{fmt_err, read, texture::{decode_pack, TextureImage}, AssetError, GamePaths};
use nfs_formats::{crp::Crp, fsh::Fsh, ini::Ini, refpack, sim::CarSim};
use std::path::PathBuf;

/// One assembled texture page (`.tpg` `[tpageN.details]`), RGBA8.
#[derive(Debug, Clone)]
pub struct TexturePage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// `window=1` page (glass) keeps its alpha; other pages are forced opaque.
    pub is_window: bool,
    /// Page fed from a global file with alpha content (`Shadow.fsh`, `INTGLASS.FSH`): keeps alpha, blended.
    pub is_alpha: bool,
}

/// Variant selection from a `.tpg` `[styleN]` section: `geometryK=<type id> typeK=<variant>` and
/// `textureK=<texture id> typeK=<variant>`; ids not listed use variant 0. (RE/notes/car-model.md §2.2/§3, MEDIUM)
#[derive(Debug, Clone, Default)]
pub struct CarStyle {
    pub name: String,
    pub geometry: std::collections::HashMap<u32, u32>,
    pub texture: std::collections::HashMap<u32, u32>,
}

impl CarStyle {
    pub fn from_tpg(tpg: &Ini, index: usize) -> Self {
        let mut st = Self::default();
        if let Some(sec) = tpg.section(&format!("style{index}")) {
            st.name = sec.get("name").unwrap_or("").to_string();
            let count = sec.get_i64("count").unwrap_or(0);
            for k in 1..=count {
                let variant = sec.get_i64(&format!("type{k}")).unwrap_or(0) as u32;
                if let Some(g) = sec.get_i64(&format!("geometry{k}")) {
                    st.geometry.insert(g as u32, variant);
                }
                if let Some(t) = sec.get_i64(&format!("texture{k}")) {
                    st.texture.insert(t as u32, variant);
                }
            }
        }
        st
    }
    /// Whether an entry/article section with `textureK/geometryK` + `typeK` pairs is enabled.
    pub fn entry_enabled(&self, sec: &nfs_formats::ini::IniSection) -> bool {
        if sec.get_i64("frontend") == Some(1) {
            return false;
        }
        let count = sec.get_i64("count").unwrap_or(1);
        for k in 1..=count {
            let want = sec.get_i64(&format!("type{k}")).unwrap_or(0) as u32;
            if let Some(t) = sec.get_i64(&format!("texture{k}")) {
                if self.texture.get(&(t as u32)).copied().unwrap_or(0) != want {
                    return false;
                }
            }
            if let Some(g) = sec.get_i64(&format!("geometry{k}")) {
                if self.geometry.get(&(g as u32)).copied().unwrap_or(0) != want {
                    return false;
                }
            }
        }
        true
    }
    /// Whether an article of `Base+68` type id and `+69` variant is shown.
    pub fn article_enabled(&self, type_id: u8, variant: u8) -> bool {
        type_id == 0 || self.geometry.get(&(type_id as u32)).copied().unwrap_or(0) == variant as u32
    }
}

#[derive(Debug)]
pub struct CarAssets {
    /// Model name, e.g. `993` (matches `CarModel/<model>.crp`).
    pub model: String,
    pub crp_data: Vec<u8>,
    pub crp: Crp,
    pub textures_day: Vec<TextureImage>,
    pub textures_night: Vec<TextureImage>,
    pub tpg: Ini,
}

impl CarAssets {
    pub fn load(paths: &GamePaths, model: &str) -> Result<Self, AssetError> {
        let dir = paths.car_model_dir();
        let file = |suffix: &str| -> Result<PathBuf, AssetError> {
            let n = format!("{model}{suffix}");
            GamePaths::find_file(&dir, &n).ok_or_else(|| AssetError::MissingFile { dir: dir.clone(), name: n })
        };
        let crp_path = file(".crp")?;
        let crp_data = refpack::maybe_decompress(&read(&crp_path)?).map_err(fmt_err(&crp_path))?.into_owned();
        let crp = Crp::parse(&crp_data).map_err(fmt_err(&crp_path))?;
        let pack = |suffix: &str| -> Result<Vec<TextureImage>, AssetError> {
            let p = file(suffix)?;
            let d = refpack::maybe_decompress(&read(&p)?).map_err(fmt_err(&p))?.into_owned();
            decode_pack(&p, &d)
        };
        let tpg_path = file(".tpg")?;
        Ok(Self {
            model: model.to_string(),
            crp_data,
            crp,
            textures_day: pack("D.fsh")?,
            textures_night: pack("L.fsh")?,
            tpg: Ini::parse(&String::from_utf8_lossy(&read(&tpg_path)?)),
        })
    }

    /// Build the texture pages the car materials index (`RE/notes/car-model.md` §2.2, CONFIRMED):
    /// each embedded `sf#k` SHPI corresponds to `.tpg` `file(k+1)`; `[file(k+1).details] tpage=N`
    /// selects the page (1-based), entries are blitted at `(misc[2] & 0xFFF, misc[3] & 0xFFF)` plus
    /// `offsetx/y`; `global=K` files come from `CarModel/{Shadow,INTGLASS,Suit,Head,Cabrio}.fsh`.
    /// Alternates (`[fileN.<entry>]` with `typeK != 0`, `frontend=1`) are skipped (coupe defaults).
    /// Paint: greyscale body texels with alpha in the tint slots get `tint × grey` (§5); `paint` is
    /// used for every slot (APPROXIMATION: the 18-slot table from `nfs5.car` is not wired yet).
    pub fn build_pages(&self, paths: &GamePaths, paint: [u8; 3], style: &CarStyle) -> Result<Vec<TexturePage>, AssetError> {
        let crp_path = paths.car_model_dir().join(format!("{}.crp", self.model));
        let hdr = self.tpg.section("header");
        let numfiles = hdr.and_then(|h| h.get_i64("numfiles")).unwrap_or(0) as usize;
        let numpages = hdr.and_then(|h| h.get_i64("numtpages")).unwrap_or(0) as usize;
        let mut pages: Vec<TexturePage> = (1..=numpages)
            .map(|n| {
                let d = self.tpg.section(&format!("tpage{n}.details"));
                let w = d.and_then(|d| d.get_i64("width")).unwrap_or(256) as u32;
                let h = d.and_then(|d| d.get_i64("height")).unwrap_or(256) as u32;
                let is_window = d.and_then(|d| d.get_i64("window")).unwrap_or(0) == 1;
                TexturePage { width: w, height: h, rgba: vec![0; (w * h * 4) as usize], is_window, is_alpha: false }
            })
            .collect();
        let embedded = self.crp.embedded_texture_files(&self.crp_data).map_err(fmt_err(&crp_path))?;
        let sf_indices: Vec<u16> = self.crp.material_entries.iter().filter(|e| e.is_part(b"sf")).map(|e| e.part_index()).collect();
        const GLOBALS: [&str; 5] = ["Shadow.fsh", "INTGLASS.FSH", "Suit.fsh", "Head.fsh", "Cabrio.fsh"];
        let mut owned_globals: Vec<Vec<u8>> = Vec::new();
        for file_no in 1..=numfiles {
            let Some(det) = self.tpg.section(&format!("file{file_no}.details")) else { continue };
            let Some(page_no) = det.get_i64("tpage") else { continue };
            let page_no = page_no as usize;
            if page_no == 0 || page_no > pages.len() {
                continue;
            }
            let offx = det.get_i64("offsetx").unwrap_or(0) as i64;
            let offy = det.get_i64("offsety").unwrap_or(0) as i64;
            let mirror = det.get_i64("mirror").unwrap_or(0) == 1;
            let mirror_page = self.tpg.section(&format!("tpage{page_no}.details")).and_then(|d| d.get_i64("mirrortpage")).map(|m| m as usize);
            let (moffx, moffy) = (det.get_i64("mirroroffsetx").unwrap_or(0) as i64, det.get_i64("mirroroffsety").unwrap_or(0) as i64);
            // source SHPI bytes
            let data: &[u8] = if let Some(g) = det.get_i64("global") {
                if g == 1 || g == 2 {
                    pages[page_no - 1].is_alpha = true;
                }
                let name = GLOBALS.get((g as usize).wrapping_sub(1)).copied();
                let Some(name) = name else { continue };
                let Some(p) = GamePaths::find_file(&paths.car_model_dir(), name) else { continue };
                let d = refpack::maybe_decompress(&read(&p)?).map_err(fmt_err(&p))?.into_owned();
                owned_globals.push(d);
                owned_globals.last().unwrap()
            } else {
                // embedded: sf#k with k = file_no - 1 (or by position when indices are sequential)
                match sf_indices.iter().position(|&i| i as usize + 1 == file_no) {
                    Some(k) => embedded[k],
                    None => continue,
                }
            };
            let fsh = match Fsh::parse(data) {
                Ok(f) => f,
                Err(e) => return Err(AssetError::Format { path: crp_path.clone(), source: e }),
            };
            for (ei, entry) in fsh.entries.iter().enumerate() {
                let ename = entry.name.trim_end().to_string();
                if let Some(sec) = self.tpg.section(&format!("file{file_no}.{ename}"))
                    && !style.entry_enabled(sec)
                {
                    continue;
                }
                let Ok(img) = fsh.decode(data, ei) else { continue };
                let b = &entry.blocks[0];
                let x = ((b.misc[2] & 0xFFF) as i64) as i64;
                let y = ((b.misc[3] & 0xFFF) as i64) as i64;
                let tint = self.tpg.section(&format!("file{file_no}.{}", ename.to_lowercase())).and_then(|s| s.get_i64("tint")).unwrap_or(1) != 0;
                blit(&mut pages[page_no - 1], &img.rgba, img.width, img.height, x + offx, y + offy, tint, paint);
                if mirror && let Some(mp) = mirror_page && mp >= 1 && mp <= pages.len() {
                    blit(&mut pages[mp - 1], &img.rgba, img.width, img.height, x + moffx, y + moffy, tint, paint);
                }
            }
        }
        Ok(pages)
    }

    /// Load `GameData/Simulation/CarData/<variant>.sim`.
    pub fn load_sim(paths: &GamePaths, variant: &str) -> Result<CarSim, AssetError> {
        let dir = paths.car_data_dir();
        let n = format!("{variant}.sim");
        let p = GamePaths::find_file(&dir, &n).ok_or_else(|| AssetError::MissingFile { dir: dir.clone(), name: n })?;
        CarSim::parse(&read(&p)?).map_err(fmt_err(&p))
    }
}

/// Copy `src` into the page at (x, y), applying the paint-slot rule (RE/notes/car-model.md §5):
/// `slot = 20 − ((a+1)·20 >> 8)`; slots 0/10/20 are copied unchanged, others become `tint × grey`.
#[allow(clippy::too_many_arguments)]
fn blit(page: &mut TexturePage, src: &[u8], w: u32, h: u32, x: i64, y: i64, tint: bool, paint: [u8; 3]) {
    for row in 0..h as i64 {
        let py = y + row;
        if py < 0 || py >= page.height as i64 {
            continue;
        }
        for col in 0..w as i64 {
            let px = x + col;
            if px < 0 || px >= page.width as i64 {
                continue;
            }
            let s = &src[((row as u32 * w + col as u32) * 4) as usize..][..4];
            let d = &mut page.rgba[((py as u32 * page.width + px as u32) * 4) as usize..][..4];
            let a = s[3] as u32;
            let slot = 20 - ((a + 1) * 20 >> 8);
            if tint && slot != 0 && slot != 10 && slot != 20 {
                let grey = s[0] as u32;
                d[0] = (paint[0] as u32 * grey >> 8) as u8;
                d[1] = (paint[1] as u32 * grey >> 8) as u8;
                d[2] = (paint[2] as u32 * grey >> 8) as u8;
                d[3] = 255;
            } else {
                d.copy_from_slice(s);
                if !page.is_window && !page.is_alpha {
                    d[3] = 255;
                }
            }
        }
    }
}
