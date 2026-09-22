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

    /// A **mirrored** race moves the driver to the other side of the car: `RCar::load` tests
    /// `[0x65742C] > 0` at 0x441E16 and negates the **x** of both triples in place —
    /// `car+0x10E8` at 0x441E2D and `car+0x10F4` at 0x441E41. It happens once, at load, so the
    /// eye point and its aim are mirrored for the whole race.
    ///
    /// The position's x is plainly the driver's side. The *rotation's* x is what `car-model.md`
    /// reads as the pitch, and negating a pitch in a mirror is odd — but that is what the two
    /// `fchs` do, and both fields are treated the same way, so it is carried as read.
    pub fn mirrored(mut self) -> Self {
        self.position.x = -self.position.x;
        self.rotation.x = -self.rotation.x;
        self
    }
}

impl CarStyle {
    pub fn from_tpg(tpg: &Ini, index: usize) -> Self {
        let mut st = Self::default();
        if let Some(sec) = tpg.section(&format!("style{index}")) {
            st.name = sec.get("name").unwrap_or("").to_string();
            let count = sec.get_i64("count").unwrap_or(0);
            for k in 1..=count {
                // **`geometry` wins, and the two are exclusive.** `applyStyle` 0x437B00 reads
                // `geometry%d` with a default of **−1** (0x437B97 `push -1`) and only when that
                // comes back negative (0x437BA3 `jge`) does it read `texture%d` and offset the
                // slot by **60** (0x437BC8 `add esi, 0x3C`) — so one entry fills one slot of the
                // 120-dword array at `car+0x56C`, never both. No shipped `[styleN]` entry carries
                // both keys, so this is the rule rather than a change in behaviour.
                let variant = sec.get_i64(&format!("type{k}")).unwrap_or(0) as u32;
                match sec.get_i64(&format!("geometry{k}")) {
                    Some(g) if g >= 0 => {
                        st.geometry.insert(g as u32, variant);
                    }
                    _ => {
                        st.texture.insert(sec.get_i64(&format!("texture{k}")).unwrap_or(0) as u32, variant);
                    }
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
    /// The render overlay `R3DCar` slot 19 lays over the `.tpg` style at model load
    /// (0x43AEA5…0x43AF6B). It is not a separate mechanism — it writes the **same** array, so
    /// these are style slots:
    ///
    /// | slot | `Base+68` | what it selects |
    /// |---|---|---|
    /// | 9 | 0x09 | `DriverBody1…10` |
    /// | 43 | 0x2B | `LeftArm`, `RightArm`, `Legs` — four variants |
    /// | 56 | 0x38 | `LeftHand`, `RightHand` — two |
    ///
    /// ```text
    /// 0x43AEAB  slot9  = 0x5CD5C4[detail]                ; {2,3,4,5,10,8,9,7,1}
    /// 0x43AED4  car+0x52C & 0x10   -> slot9 = 0, done    ; no driver at all
    /// 0x43AEEA  detail >= 8        -> slot9 = 1, slot43 = 1, and slot56 is left alone
    /// 0x43AF0B  GameType 9, detail >= 4 -> 6, 3, 1       ; a factory-driver mission
    /// 0x43AF2B  GameType 9, else        -> 1, 1, 2
    /// 0x43AF51  otherwise -> slot43 = 2·(detail/4) + 2, slot56 = detail/4 + 1
    /// ```
    ///
    /// The port renders at the best detail the table has, so it takes the `detail >= 8` arm.
    /// Slot 19's roof line in a race (`[0x65B928] == 0`, 0x43AE7A…0x43AE9F): `slot41 = 2·slot37`,
    /// one less when the car's `.CABRIODOWN` option `[carinfo+0x50C]` (id 0x24B) is **0**, and
    /// then `slot37 = 0`. With slot 41's pairs (`CabrioDown` 1 / `CabrioUp` 2, …) a 0 picks the
    /// odd article of the family. A car with no roof family ends on 41 = −1 or 0, which no
    /// article carries.
    /// Slot 19's tail, **0x43B0B7…0x43B0DC**, in both branches: every one of the 120 dwords of
    /// `carinfo+0x32C` that is not negative replaces the style's own — the geometry slots 0…59
    /// and the texture slots 60…119, after every write slot 19 made. The port's array holds only
    /// what the parts wrote; the rest is taken as −1 (what the launch leaves there is not read).
    pub fn apply_overlay(&mut self, swaps: &[[i16; 3]]) {
        for &[group, slot, value] in swaps {
            if value < 0 || !(0..60).contains(&slot) {
                continue;
            }
            match group {
                0 => {
                    self.geometry.insert(slot as u32, value as u32);
                }
                1 => {
                    self.texture.insert(slot as u32, value as u32);
                }
                _ => {}
            }
        }
    }

    pub fn apply_race_roof(&mut self, cabrio_down: u32) {
        let family = self.geometry.get(&37).copied().unwrap_or(0);
        let v = (2 * family).wrapping_sub(u32::from(cabrio_down == 0));
        self.geometry.insert(41, v);
        self.geometry.insert(37, 0);
    }

    pub fn apply_driver_detail(&mut self, detail: u32, no_driver: bool, factory_driver: bool) {
        if no_driver {
            self.geometry.insert(9, 0);
            return;
        }
        if detail >= 8 {
            self.geometry.insert(9, 1);
            self.geometry.insert(43, 1);
            return;
        }
        if factory_driver {
            let (b, l, h) = if detail >= 4 { (6, 3, 1) } else { (1, 1, 2) };
            self.geometry.insert(9, b);
            self.geometry.insert(43, l);
            self.geometry.insert(56, h);
            return;
        }
        const DRIVER_BY_DETAIL: [u32; 9] = [2, 3, 4, 5, 10, 8, 9, 7, 1];
        self.geometry.insert(9, DRIVER_BY_DETAIL[detail.min(8) as usize]);
        self.geometry.insert(43, 2 * (detail / 4) + 2);
        self.geometry.insert(56, detail / 4 + 1);
    }

    /// Whether an article of `Base+68` type id and `+69` variant is shown.
    ///
    /// **Type 0 is not a wildcard.** `applyStyle` 0x437B00 `memset`s the whole 120-dword array at
    /// `car+0x56C` to zero (0x437B1B, `push 0x1E0`) before it reads the `[styleN]` section, so an
    /// unnamed slot — slot 0 included — holds 0, and the loader's test `carinfo[0x56C + 4·type]
    /// == variant` (0x4420B9) fails for a type-0 article whose variant is not 0. This used to
    /// short-circuit `type_id == 0` to *true*, which drew `DecalHood1` — type 0, variant **1** —
    /// on top of `DecalHood`, type 0 variant 0, at the same pivot. Both exist on the 993, the GT2
    /// and the two 993 police cars, no `[styleN]` on any of them sets slot 0, and across all 44
    /// shipped models and every style this correction drops that one article and nothing else.
    pub fn article_enabled(&self, type_id: u8, variant: u8) -> bool {
        self.geometry.get(&(type_id as u32)).copied().unwrap_or(0) == variant as u32
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
    /// Which **body style** of this model the car is, **0-based** — `nfs5.car +0x1C0`
    /// (`fedata::CarCatalogEntry::variant`). `Car_init` copies it out of the car-info record at
    /// `+0x328` into `car+0x568` (0x410F5C…0x410F62) and `applyStyle` 0x437B00 looks up
    /// `"style%d"` with it **plus one** (0x437B33 `inc esi`). The `drivercamposstyle<N>` keys use
    /// the raw 0-based number instead — `550.tpg` has a `drivercamposstyle0`.
    ///
    /// Zero until somebody sets it, which is the right default: `style1` is what a model with one
    /// style has.
    pub style_index: usize,
    /// The catalogue record's convertible flag `nfs5.car +0x1BF` (see [`Self::with_style`]).
    pub cabrio: bool,
    /// `carinfo+0x32C`, the style overlay a race car's installed parts write: `{group, slot,
    /// value}` from each part's `+0x8E` swaps (0x49C5F7…0x49C635), group 0 a geometry slot and 1 a
    /// texture slot (`slot + 60` in the 120-dword array)
    pub style_overlay: Vec<[i16; 3]>,
    /// `.TINYCAR` (`carinfo+0x518`), which slot 19's tail copies into style slot 58 (0x43B0E5)
    pub tiny: bool,
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
            style_index: 0,
            cabrio: false,
            style_overlay: Vec::new(),
            tiny: false,
        })
    }

    /// Set the body style from a catalogue entry's `nfs5.car +0x1C0`, clamping a negative one to
    /// style 1 the way an unset `car+0x568` would.
    pub fn with_style(mut self, variant: i16) -> Self {
        self.style_index = variant.max(0) as usize;
        self
    }

    /// Carry the installed parts' geometry swaps (`Part::geometry_swaps`).
    pub fn with_style_overlay(mut self, swaps: Vec<[i16; 3]>) -> Self {
        self.style_overlay = swaps;
        self
    }

    /// Carry the catalogue record's convertible flag `+0x1BF`.
    pub fn with_cabrio(mut self, cabrio: bool) -> Self {
        self.cabrio = cabrio;
        self
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
    /// A car's `.sim`, falling back to **`default.sim`** when the named one is not there.
    ///
    /// That fallback is the original's, not a convenience: 0x49C7B6 formats `"%s%s.sim"` and
    /// opens it, and when the handle comes back null (`test edi, edi; jne` at 0x49C7D4) it
    /// formats `"%sdefault.sim"` (0x49C7E0) and opens that instead. It matters for exactly one
    /// shipped car — the **'96 911 Carrera 4S Coupé (993)**, whose catalogue record names
    /// `993coupeS436` while the file on disk is `993coupe4S36.sim`. A typo in the data, and the
    /// only one of the 87 catalogue cars with a missing `.sim`; without the fallback, picking
    /// that car fails to start a race.
    pub fn load_sim(paths: &GamePaths, variant: &str) -> Result<CarSim, AssetError> {
        let dir = paths.car_data_dir();
        let named = GamePaths::find_file(&dir, &format!("{variant}.sim"));
        let p = match named {
            Some(p) => p,
            None => {
                tracing::warn!(variant, "no such .sim; falling back to default.sim (0x49C7D4)");
                let n = "default.sim".to_string();
                GamePaths::find_file(&dir, &n).ok_or_else(|| AssetError::MissingFile { dir: dir.clone(), name: n })?
            }
        };
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
            image: TextureImage { name: e.name.clone(), width: img.width, height: img.height, mips: vec![img.rgba], has_alpha: true, page: b.misc[0] as usize, x: (b.misc[2] & 0xFFF) as u32, y: (b.misc[3] & 0xFFF) as u32, place: [b.misc[0] as i16 as i32, b.misc[1] as i16 as i32], text: None },
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

#[cfg(test)]
mod wheel_channel_tests {
    use super::*;

    /// Slot 9 copies channels 1…4 into 18…21 with `.x` cleared (0x438669…0x438726), so a
    /// wheel-typed article on 18…21 takes the steer but never the spin. `CarModel::draw_items`
    /// relies on every wheel article of every shipped car being on one of those two runs.
    #[test]
    fn wheel_articles_are_on_the_spinning_or_the_still_channels() {
        let Ok(paths) = GamePaths::locate() else { return };
        let dir = paths.car_model_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut seen = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("crp") {
                continue;
            }
            let Some(model) = p.file_stem().and_then(|x| x.to_str()) else { continue };
            let Ok(car) = CarAssets::load(&paths, model) else { continue };
            let Ok(arts) = car.crp.articles(&car.crp_data) else { continue };
            for a in &arts {
                let Ok(g) = car.crp.article_geometry(&car.crp_data, a) else { continue };
                let Some(b) = g.base.as_ref() else { continue };
                if !matches!(b.tag[0], 0x0D | 0x0E) {
                    continue;
                }
                seen += 1;
                let c = b.rot_channel;
                assert!(
                    (1..=4).contains(&c) || (18..=21).contains(&c),
                    "{model}: a wheel article on channel {c}"
                );
            }
        }
        assert!(seen > 100, "only {seen} wheel articles seen");
    }
}
