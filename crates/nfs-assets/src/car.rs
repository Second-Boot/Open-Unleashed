//! Car asset bundle: model CRP, day/night texture packs, texture-page layout, simulation data.

use crate::{fmt_err, read, texture::{decode_pack, TextureImage}, AssetError, GamePaths};
use nfs_formats::{crp::Crp, fsh::Fsh, ini::Ini, refpack, sim::CarSim};
use std::path::{Path, PathBuf};

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

/// Where the driver's view sits and how it is aimed, from the car's `.tpg`
/// (`RE/notes/car-model.md` §3): `drivercampos` / `drivercamrot`, or the body style's own
/// `drivercamposstyle<N>` / `drivercamrotstyle<N>` when it has them (0x441A10 reads both).
///
/// Loading a car copies the pair into the camera globals (0x46427F: `carInfo+0x10E8` →
/// `0x5CF620`, the driver entry of the view-offset table, and `carInfo+0x10F4` → `0x627ECC`), so
/// the position is an offset in the same body frame as the rest of that table
/// (`nfs_game::camera`): +x right, +y up, +z forward, from the car's physics position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriverCam {
    /// the eye point in the car body frame: +x right, +y up, +z forward
    pub position: glam::Vec3,
    /// pitch, yaw and roll in **degrees**, applied to the camera matrix in that order
    /// (0x461B48 multiplies by `angle/360` turns and calls the x/y/z rotations 0x5563D0 /
    /// 0x5564A0 / 0x556560). A positive pitch tilts the forward row towards −up, i.e. **down**:
    /// 0x5563D0 leaves the x row alone and does `up' = cos·up + sin·fwd`,
    /// `fwd' = cos·fwd − sin·up` (`0x532800` fills sin then cos). HIGH.
    pub rotation: glam::Vec3,
}

impl DriverCam {
    /// Read it from a `.tpg`, preferring the body style's own keys.
    ///
    /// The file's axes are the body frame's, not the model's: the left-hand-drive cars put the
    /// driver at `x = −0.38`, the right-hand-drive 935 and GT1 at `x = +0.31`, so +x is the car's
    /// right (the model has +x on the *left*, `car-model.md` §7); and the front-engined 928 sits
    /// at `z = −0.53` while the GT1, whose cockpit is far forward, sits at `z = +0.24`, so +z is
    /// forward. HIGH.
    pub fn from_tpg(tpg: &Ini, style: usize) -> Option<Self> {
        let h = tpg.section("header")?;
        let triple = |key: &str| -> Option<glam::Vec3> {
            let v = h.get_f32_list(key)?;
            (v.len() >= 3).then(|| glam::Vec3::new(v[0], v[1], v[2]))
        };
        let pos = triple(&format!("drivercamposstyle{style}")).or_else(|| triple("drivercampos"))?;
        let rot = triple(&format!("drivercamrotstyle{style}")).or_else(|| triple("drivercamrot")).unwrap_or(glam::Vec3::ZERO);
        Some(Self { position: pos, rotation: rot })
    }
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
            if let Some(t) = sec.get_i64(&format!("texture{k}"))
                && self.texture.get(&(t as u32)).copied().unwrap_or(0) != want {
                    return false;
                }
            if let Some(g) = sec.get_i64(&format!("geometry{k}"))
                && self.geometry.get(&(g as u32)).copied().unwrap_or(0) != want {
                    return false;
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
        Self::load_in(&paths.car_model_dir(), model)
    }

    /// The same model files from any directory. The front end's 3D rooms are `Car`-kind `.crp`s in
    /// `FEData/Models` (`garage`, `dealer2a…c`, `factory`, `victory`, `trackwall`, `track`) that
    /// carry their textures inside the `.crp` and ship **no** `D.fsh` / `L.fsh` beside them, so the
    /// two external packs are optional here (`RE/notes/frontend-3d.md` §1).
    pub fn load_in(dir: &Path, model: &str) -> Result<Self, AssetError> {
        let dir = dir.to_path_buf();
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
        // A few traffic cars ship only one texture pack (the Tbus `eas3` has `eas3L.fsh` and no
        // `eas3D.fsh`), so each set falls back to the other rather than failing the whole car.
        let (day, night) = (pack("D.fsh"), pack("L.fsh"));
        let (textures_day, textures_night) = match (day, night) {
            (Ok(d), Ok(n)) => (d, n),
            (Ok(d), Err(_)) => (d.clone(), d),
            (Err(_), Ok(n)) => (n.clone(), n),
            // a model whose textures are all embedded in the `.crp` has neither pack
            (Err(_), Err(_)) => (Vec::new(), Vec::new()),
        };
        Ok(Self {
            model: model.to_string(),
            crp_data,
            crp,
            textures_day,
            textures_night,
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
            let offx = det.get_i64("offsetx").unwrap_or(0);
            let offy = det.get_i64("offsety").unwrap_or(0);
            let mirror = det.get_i64("mirror").unwrap_or(0) == 1;
            let mirror_page = self.tpg.section(&format!("tpage{page_no}.details")).and_then(|d| d.get_i64("mirrortpage")).map(|m| m as usize);
            let (moffx, moffy) = (det.get_i64("mirroroffsetx").unwrap_or(0), det.get_i64("mirroroffsety").unwrap_or(0));
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
                let x = (b.misc[2] & 0xFFF) as i64;
                let y = (b.misc[3] & 0xFFF) as i64;
                let tint = self.tpg.section(&format!("file{file_no}.{}", ename.to_lowercase())).and_then(|s| s.get_i64("tint")).unwrap_or(1) != 0;
                blit(&mut pages[page_no - 1], &img.rgba, img.width, img.height, x + offx, y + offy, tint, paint);
                if mirror && let Some(mp) = mirror_page && mp >= 1 && mp <= pages.len() {
                    blit(&mut pages[mp - 1], &img.rgba, img.width, img.height, x + moffx, y + moffy, tint, paint);
                }
            }
        }
        Ok(pages)
    }

    /// Load `GameData/Simulation/AICarData/<family>.ais`; tries the model name (e.g. `993`, `356_1` → `356`)
    /// then `default.ais` (the exe keys the file by the car-info family string — HYPOTHESIS on the mapping).
    pub fn load_ais(paths: &GamePaths, model: &str) -> Result<nfs_formats::ais::AiCarData, AssetError> {
        let dir = paths.game_data().join("Simulation").join("AICarData");
        let base: String = model.chars().take_while(|c| *c != '_').collect();
        let candidates = [format!("{model}.ais"), format!("{base}.ais"), format!("{}.ais", base.to_lowercase()), "default.ais".into()];
        for n in &candidates {
            if let Some(p) = GamePaths::find_file(&dir, n) {
                return nfs_formats::ais::AiCarData::parse(&read(&p)?).map_err(fmt_err(&p));
            }
        }
        Err(AssetError::MissingFile { dir, name: format!("{model}.ais") })
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
            let slot = 20 - (((a + 1) * 20) >> 8);
            if tint && slot != 0 && slot != 10 && slot != 20 {
                let grey = s[0] as u32;
                d[0] = ((paint[0] as u32 * grey) >> 8) as u8;
                d[1] = ((paint[1] as u32 * grey) >> 8) as u8;
                d[2] = ((paint[2] as u32 * grey) >> 8) as u8;
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

/// One entry of a global car texture file (`CarModel/{Shadow,INTGLASS,Suit,Head,Cabrio}.fsh`) by
/// name — the packs the `.tpg` pulls page pieces from, which also hold standalone textures such as
/// `INTGLASS`'s `glai` windscreen glass.
pub fn global_texture(paths: &GamePaths, file: &str, entry: &str) -> Result<Option<TextureImage>, AssetError> {
    let Some(p) = GamePaths::find_file(&paths.car_model_dir(), file) else { return Ok(None) };
    let data = refpack::maybe_decompress(&read(&p)?).map_err(fmt_err(&p))?.into_owned();
    Ok(decode_pack(&p, &data)?.into_iter().find(|t| t.name.trim().eq_ignore_ascii_case(entry)))
}

/// One patch of a `<model>D.fsh` / `<model>L.fsh` damage pack (`RE/notes/car-model.md` §11).
#[derive(Debug, Clone)]
pub struct DamagePatch {
    /// Last digit of the entry name = the damage zone it belongs to (`car+0x4E4 + 4·zone`).
    pub zone: u8,
    /// `misc[0]`: which texture page the patch is drawn onto.
    pub page: usize,
    pub x: u32,
    pub y: u32,
    /// `misc[1]` as a float. Above zero the original blits the patch opaque once the zone's damage
    /// passes it; no shipped patch does, so in practice every patch takes the blended path.
    pub threshold: f32,
    pub image: TextureImage,
}

/// Read the damage decals beside a car's `.crp`. `night` picks `<model>L.fsh` over `<model>D.fsh`,
/// as `car+0xAA <= 0x50` does in the original (0x442011 / 0x44204E).
pub fn damage_patches(paths: &GamePaths, model: &str, night: bool) -> Result<Vec<DamagePatch>, AssetError> {
    let suffix = if night { "L" } else { "D" };
    let Some(p) = GamePaths::find_file(&paths.car_model_dir(), &format!("{model}{suffix}.fsh")) else { return Ok(Vec::new()) };
    let data = refpack::maybe_decompress(&read(&p)?).map_err(fmt_err(&p))?.into_owned();
    let fsh = Fsh::parse(&data).map_err(fmt_err(&p))?;
    let mut out = Vec::new();
    for (i, e) in fsh.entries.iter().enumerate() {
        // the four-digit name: the applier keeps the entry when carInfo[0x56C][first two] equals the
        // third digit (0 == 0 for every shipped pack) and uses the fourth as the damage zone
        let n = e.name.trim();
        let digits: Vec<u32> = n.chars().filter_map(|c| c.to_digit(10)).collect();
        if digits.len() != 4 || n.len() != 4 {
            continue;
        }
        let (group, variant, zone) = (digits[0] * 10 + digits[1], digits[2], digits[3] as u8);
        if group != 0 || variant != 0 || zone as usize >= 10 {
            continue;
        }
        let Ok(img) = fsh.decode_mip(&data, i, 0) else { continue };
        let b = &e.blocks[0];
        out.push(DamagePatch {
            zone,
            page: b.misc[0] as usize,
            x: (b.misc[2] & 0xFFF) as u32,
            y: (b.misc[3] & 0xFFF) as u32,
            threshold: b.misc[1] as f32,
            image: TextureImage { name: e.name.clone(), width: img.width, height: img.height, mips: vec![img.rgba], has_alpha: true, page: b.misc[0] as usize, x: (b.misc[2] & 0xFFF) as u32, y: (b.misc[3] & 0xFFF) as u32 },
        });
    }
    Ok(out)
}

/// Blend one damage patch onto a page, as 0x448110 does: skipped while the zone is undamaged,
/// otherwise drawn with a constant `alpha = min(damage · 3.5, 255)` on top of the patch's own
/// per-pixel alpha.
pub fn blend_patch(page: &mut TexturePage, patch: &DamagePatch, damage: f32) {
    if patch.threshold > 0.0 {
        if damage <= patch.threshold {
            return;
        }
    } else if damage <= 0.0 {
        return;
    }
    let level = if patch.threshold > 0.0 { 255.0 } else { (damage * 3.5).min(255.0) };
    let src = &patch.image.mips[0];
    for row in 0..patch.image.height {
        let dy = patch.y + row;
        if dy >= page.height {
            break;
        }
        for col in 0..patch.image.width {
            let dx = patch.x + col;
            if dx >= page.width {
                break;
            }
            let s = ((row * patch.image.width + col) * 4) as usize;
            let a = src[s + 3] as f32 / 255.0 * (level / 255.0);
            if a <= 0.0 {
                continue;
            }
            let d = ((dy * page.width + dx) * 4) as usize;
            for k in 0..3 {
                let out = page.rgba[d + k] as f32 * (1.0 - a) + src[s + k] as f32 * a;
                page.rgba[d + k] = out.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}
