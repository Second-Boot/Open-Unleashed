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
    /// `window=1` page (glass).
    pub is_window: bool,
    /// Page fed from a global file with alpha content (`Shadow.fsh`, `INTGLASS.FSH`): keeps alpha, blended.
    pub is_alpha: bool,
    /// The page's `.tpg` `bpp` — 32, 1555 (the default, 0x613 at 0x442F5F), 4444 or 565 — the
    /// surface format 0x535950 makes it in, which decides what is left of a texel's alpha.
    pub bpp: i64,
}

/// A car's four colours and its stripe package — the car-info `+0x218` exterior, `+0x21C` stripe 1,
/// `+0x220` stripe 2 and `+0x224` interior, and `+0x514` the package — which 0x443DB0 turns into
/// the 18-slot tint table (`RE/notes/car-model.md` §5.0a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paint {
    pub exterior: [u8; 3],
    pub stripe1: [u8; 3],
    pub stripe2: [u8; 3],
    pub interior: [u8; 3],
    /// `PackageNum`, 0-based over `packages.ini`'s `package1…` — 0 for any car nobody owns
    /// (0x508E70, `controls.md` §4.137)
    pub package: u32,
}

impl From<[u8; 3]> for Paint {
    /// One colour everywhere — what the port did before the table was read; kept for callers that
    /// have no catalogue record (examples, a car without paint sets).
    fn from(c: [u8; 3]) -> Self {
        Self { exterior: c, stripe1: c, stripe2: c, interior: c, package: 0 }
    }
}

impl Paint {
    /// A catalogue car in paint set `set` — the catalogue accessor's slots 29…33
    /// (`controls.md` §4.137): the chosen exterior, interior entry 0 (`rec+0x530`) while none was
    /// chosen, stripes 1 / 2 from `rec+0x3F0` / `rec+0x3F4` (paint set 0's, whichever set is
    /// chosen) and package a flat 0 (0x508E70).
    pub fn from_catalogue(entry: &nfs_formats::fedata::CarCatalogEntry, set: usize) -> Self {
        let Some(p) = entry.paint_sets.get(set % entry.paint_sets.len().max(1)) else { return [0xD4, 0x10, 0x10].into() };
        let first = &entry.paint_sets[0];
        Self { exterior: p.colours[0], stripe1: first.colours[1], stripe2: first.colours[2], interior: entry.interior_colours.first().copied().unwrap_or(p.colours[0]), package: 0 }
    }

    /// A garage car: paint set `set`'s exterior, interior row `interior`, stripe 1 from paint set
    /// `stripe1`'s colour 1 and stripe 2 from `stripe2`'s colour 2 — the rows the paint shop's
    /// `OwnedCar.*Colour` lists offer (`savegame.md` §4.7a) — and `package` (`OwnedCar.PackageNum`).
    pub fn owned(entry: &nfs_formats::fedata::CarCatalogEntry, set: usize, interior: usize, stripe1: usize, stripe2: usize, package: u32) -> Self {
        let mut p = Self::from_catalogue(entry, set);
        let n = entry.paint_sets.len().max(1);
        if let Some(s) = entry.paint_sets.get(stripe1 % n) {
            p.stripe1 = s.colours[1];
        }
        if let Some(s) = entry.paint_sets.get(stripe2 % n) {
            p.stripe2 = s.colours[2];
        }
        if let Some(c) = entry.interior_colours.get(interior) {
            p.interior = *c;
        }
        p.package = package;
        p
    }

    /// The paint set of `entry` whose exterior is `colour` (the port keeps the chosen paint as its
    /// exterior colour), or `colour` everywhere when none is.
    pub fn matching(entry: &nfs_formats::fedata::CarCatalogEntry, colour: [u8; 3]) -> Self {
        match entry.paint_sets.iter().position(|p| p.colours[0] == colour) {
            Some(i) => Self::from_catalogue(entry, i),
            None => colour.into(),
        }
    }

    /// The 18-slot table 0x443DB0 fills: `tint[k] = colour[band_k]` with the bands from
    /// `CarModel/packages.ini` as 0x443A60 reads them — `[package<n+1>] band0…band17`, default 0,
    /// `band14` / `band15` default 3 — and the colours exterior, stripe 1, stripe 2, interior.
    /// A package past the file's `count` leaves the table as it was (0x443DD5); the port then
    /// paints every slot the exterior (APPROXIMATION of an unwritten table).
    pub fn tint_table(&self, paths: &GamePaths) -> [[u8; 3]; 18] {
        let colours = [self.exterior, self.stripe1, self.stripe2, self.interior];
        let ini = GamePaths::find_file(&paths.car_model_dir(), "packages.ini").and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Ini::parse(&t));
        let count = ini.as_ref().and_then(|i| i.section("header")).and_then(|h| h.get_i64("count")).unwrap_or(0);
        let Some(ini) = ini.filter(|_| (self.package as i64) < count) else { return [self.exterior; 18] };
        let sec = ini.section(&format!("package{}", self.package + 1));
        std::array::from_fn(|k| {
            let default = if k == 14 || k == 15 { 3 } else { 0 };
            let band = sec.and_then(|s| s.get_i64(&format!("band{k}"))).unwrap_or(default);
            colours[band.clamp(0, 3) as usize]
        })
    }
}

/// Variant selection from a `.tpg` `[styleN]` section: `geometryK=<type id> typeK=<variant>` and
/// `textureK=<texture id> typeK=<variant>`; ids not listed use variant 0. (RE/notes/car-model.md §2.2/§3, MEDIUM)
#[derive(Debug, Clone, Default)]
pub struct CarStyle {
    pub name: String,
    pub geometry: std::collections::HashMap<u32, u32>,
    pub texture: std::collections::HashMap<u32, u32>,
    /// the page builder's context for 0x439C60: whether this is the front end (`[0x65B928] != 0`)
    pub front_end: bool,
    /// the track's night flag `word[0x628A28]`, which a race entry's `night` key must equal
    pub night: bool,
    /// on `skidpad`, whether the game is a Factory Driver mission (`[0x6573E8] == 9`), which an
    /// entry's `testdrive` key must equal there; `None` off `skidpad`
    pub skidpad: Option<bool>,
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
    /// pitch, yaw and roll in **degrees**, each pre-multiplying the camera matrix in that order
    /// (so `car · Rx · Ry · Rz` on column vectors)
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
    /// Whether an entry/article section with `geometryK`/`textureK` + `typeK` pairs is enabled —
    /// 0x439C60: enabled by default; with a `count` above 0 it is enabled only when **any** of the
    /// `count` pairs matches, `carinfo[0x56C + 4·slot] == typeK` with the slot `geometryK`, or
    /// `textureK + 0x3C` when `geometryK` is absent (0x439D07…0x439DA0: the first match sets the
    /// flag and leaves the loop); then the `frontend`, `night` and `testdrive` keys below. *(Until 2026-09-25 the pairs had
    /// to match all at once, which a section listing two values of one slot — the 993's `top`,
    /// `geometry1=7 type1=0`, `geometry2=7 type2=1` — never could, and its texels stayed black.)*
    pub fn entry_enabled(&self, sec: &nfs_formats::ini::IniSection) -> bool {
        let count = sec.get_i64("count").unwrap_or(0);
        let pairs = count <= 0
            || (1..=count).any(|k| {
            let want = sec.get_i64(&format!("type{k}")).unwrap_or(0) as u32;
            match sec.get_i64(&format!("geometry{k}")) {
                Some(g) if g >= 0 => self.geometry.get(&(g as u32)).copied().unwrap_or(0) == want,
                _ => self.texture.get(&(sec.get_i64(&format!("texture{k}")).unwrap_or(0) as u32)).copied().unwrap_or(0) == want,
            }
        });
        if !pairs {
            return false;
        }
        // `frontend` must say whether this is the front end (0x439DD9…0x439DED)
        if let Some(f) = sec.get_i64("frontend")
            && (f != 0) != self.front_end
        {
            return false;
        }
        // in a race, `night` must match the track (0x439DF8…0x439E07)
        if !self.front_end
            && let Some(n) = sec.get_i64("night")
            && (n != 0) != self.night
        {
            return false;
        }
        // `testdrive` (0x439E75…0x439EC1): on `skidpad` it must equal `[0x6573E8] == 9` (a
        // Factory Driver mission); elsewhere `testdrive = 1` rules the entry out (`cmp ebp, 1`),
        // any other value passes. Until 2026-09-25 skidpad's own rule was not carried.
        match (sec.get_i64("testdrive"), self.skidpad) {
            (None, _) => true,
            (Some(t), Some(mission)) => t == i64::from(mission),
            (Some(t), None) => t != 1,
        }
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
    /// A front-end room model (`FEData/Models`, `RE/notes/frontend-3d.md`) rather than a car: the
    /// port keeps the car's draw states (`nfs_renderer::MaterialDesc::device_states`, the `Rs`
    /// cull, the `Base+70` order) to cars — APPROXIMATION for the rooms, whose page formats and
    /// draw class were not read (their spotlight layers turned stepped under the car's states).
    pub front_end_room: bool,
    /// `carinfo+0x32C`, the style overlay a race car's installed parts write: `{group, slot,
    /// value}` from each part's `+0x8E` swaps (0x49C5F7…0x49C635), group 0 a geometry slot and 1 a
    /// texture slot (`slot + 60` in the 120-dword array)
    pub style_overlay: Vec<[i16; 3]>,
    /// `.TINYCAR` (`carinfo+0x518`), which slot 19's tail copies into style slot 58 (0x43B0E5)
    pub tiny: bool,
    /// The car-info strings the page builder writes into `license` / `racedecal` entries —
    /// `None` leaves those entries' texels as they are (`car-model.md` §5.3).
    pub page_text: Option<PageText>,
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
            front_end_room: false,
            page_text: None,
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
    /// Paint: greyscale body texels with alpha in the tint slots get `tint × grey` (§5), `tint` the
    /// 18-slot table `Paint::tint_table` builds as 0x443DB0 does (§5.0a).
    pub fn build_pages(&self, paths: &GamePaths, paint: impl Into<Paint>, style: &CarStyle) -> Result<Vec<TexturePage>, AssetError> {
        let paint: Paint = paint.into();
        let tints = paint.tint_table(paths);
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
                let bpp = d.and_then(|d| d.get_i64("bpp")).unwrap_or(1555);
                TexturePage { width: w, height: h, rgba: vec![0; (w * h * 4) as usize], is_window, is_alpha: false, bpp }
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
                // In a race `RCar` tints nothing (vtable +0x30 = 0x444120 answers 0); the port's
                // paint on the embedded body pages stands in for the race's own, unread paint path
                // (APPROXIMATION, `car-model.md` §5), and stops there: a global file's entry — the
                // `Shadow.fsh` wheel shadows above all, whose alpha is a gradient — keeps its texels
                // in a race. *(Until 2026-09-25 it was tinted too and drew opaque painted bands
                // over the tops of the wheels.)*
                //
                // Which texels the front end's builder tints (0x443F60, handed the page's `.tpg`
                // `bpp` — default 0x613 = 1555 at 0x442F5F — by 0x44300C): an 8-bit source through
                // its palette (0x444130), a 32-bit one texel by texel onto a 32-bit page (0x444494)
                // or a 1555 one (0x444602); a 4444 page — the wheel shadows' — takes no tint at all.
                let page_bpp = self.tpg.section(&format!("tpage{page_no}.details")).and_then(|d| d.get_i64("bpp")).unwrap_or(1555);
                let tint = self.tpg.section(&format!("file{file_no}.{}", ename.to_lowercase())).and_then(|s| s.get_i64("tint")).unwrap_or(1) != 0
                    && (style.front_end || det.get_i64("global").is_none())
                    && match img.source_format {
                        nfs_formats::fsh::PixelFormat::Indexed8 => true,
                        nfs_formats::fsh::PixelFormat::Argb8888 => page_bpp == 32 || page_bpp == 1555,
                        _ => false,
                    };
                blit(&mut pages[page_no - 1], &img.rgba, img.width, img.height, x + offx, y + offy, tint, &tints);
                // 0x444880, right after the entry's blit: the licence / race number / flyer text
                if let (Some(text), Some(sec)) = (&self.page_text, self.tpg.section(&format!("file{file_no}.{}", ename.to_lowercase()))) {
                    let flag = |k: &str| sec.get_i64(k).unwrap_or(0) != 0;
                    let (sx, sy) = (sign12(b.misc[2]), sign12(b.misc[3]));
                    let rect = (sx, sy, entry.blocks[0].width as i64, entry.blocks[0].height as i64);
                    let page = &mut pages[page_no - 1];
                    if flag("license") {
                        let s: String = text.license.chars().take(8).collect();
                        page_text_draw(page, paths, "license.ffn", &s, rect, [0xFF00_0000, 0, 0xFF00_0000], false);
                    }
                    if flag("racedecal") {
                        let n = text.race_number;
                        let s = if n < 10 { format!("0{n}") } else { format!("{n}") };
                        // `this+0xAA`, the exterior's HSV value `max(r, g, b)` (0x443CA0 on car-info
                        // `+0x218`, 0x443E18)
                        let v = paint.exterior.into_iter().max().unwrap_or(0);
                        let (fill, edge) = if v >= 0xBE { (0xFF00_0000, 0xFFFF_FFFF) } else { (0xFFFF_FFFF, 0xFF00_0000) };
                        page_text_draw(page, paths, "racedecal.ffn", &s, rect, [fill, 0, edge], false);
                    }
                    if flag("flyer") {
                        page_text_draw(page, paths, "license.ffn", "456787", rect, [0xFF00_0000, 0, 0xFF00_0000], true);
                    }
                }
                if mirror && let Some(mp) = mirror_page && mp >= 1 && mp <= pages.len() {
                    blit(&mut pages[mp - 1], &img.rgba, img.width, img.height, x + moffx, y + moffy, tint, &tints);
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

/// What 0x444880 writes into a page (`car-model.md` §5.3): the car-info `.LICENSE` string
/// (`+0x1C0`, at most 8 bytes) and `.RACENUMBER` (`+0x510`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageText {
    pub license: String,
    pub race_number: i32,
}

/// A 12-bit SHPI position field, sign-extended as 0x4449FA / 0x444A0D read it.
fn sign12(v: u16) -> i64 {
    (((v as i32) << 20) >> 20) as i64
}

/// 0x594A30: `(c1·w1 + c2·w2) / (w1 + w2)` per byte of two ARGB colours, clamped to 0…255.
fn font_mix(c1: u32, w1: i32, c2: u32, w2: i32) -> u32 {
    [24u32, 16, 8, 0].into_iter().fold(0, |out, sh| {
        let (a, b) = (((c1 >> sh) & 0xFF) as i32, ((c2 >> sh) & 0xFF) as i32);
        out | ((((a * w1 + b * w2) / (w1 + w2)).clamp(0, 255) as u32) << sh)
    })
}

/// The 16 colours of a font's 4-bit texels (0x594980) from properties 8 / 9 / 0xA (`abc`), then
/// premultiplied by their alpha (0x53CD50: `(c·a + 128) / 255`), which is the palette both the
/// 32-bit and the 1555 writers take (`[desc+0x54]`, 0x53CFFD).
fn font_palette(flags: u32, abc: [u32; 3]) -> [u32; 16] {
    let [a, b, c] = abc;
    let mut p = [0u32; 16];
    if flags & 2 != 0 {
        // 0x594B50: fill A, background B (0 when its alpha is 0), edge C
        let b = if b & 0xFF00_0000 != 0 { b } else { 0 };
        p[0] = b;
        p[1] = c;
        for k in 1..=3 {
            p[1 + k] = font_mix(c, 4 - k as i32, b, k as i32);
        }
        for i in 5..=8 {
            p[i] = font_mix(b, 9 - i as i32, a, i as i32 - 4);
        }
        p[9] = a;
        for i in 10..=13 {
            p[i] = font_mix(a, 14 - i as i32, c, i as i32 - 9);
        }
    } else {
        // 0x5949E0 (flag 4's four-colour 0x594C30 is on no car font): A over B, B taking A's
        // RGB when its alpha is 0
        let b = if b & 0xFF00_0000 != 0 { b } else { a & 0x00FF_FFFF };
        for (i, e) in p.iter_mut().enumerate() {
            *e = font_mix(a, i as i32, b, 15 - i as i32);
        }
    }
    p.map(|c| {
        let al = c >> 24;
        let m = |sh: u32| (((c >> sh) & 0xFF) * al + 0x80) / 255;
        al << 24 | m(16) << 16 | m(8) << 8 | m(0)
    })
}

/// One palette colour onto a page texel, by the page's format: 0x53C950 on a 32-bit page,
/// 0x573FAF on a 1555 one (the page kept as 8-bit channels, read and written at 5 bits).
fn font_write(page: &mut TexturePage, px: usize, s: u32) {
    let d = &mut page.rgba[px..px + 4];
    if page.bpp == 32 {
        let (sa, da) = (s >> 24, d[3] as u32);
        let out = if sa == 0xFF || da == 0 {
            s
        } else if sa == 0 {
            return;
        } else {
            let k = 0xFF - sa;
            let ch = |dc: u8, sh: u32| ((dc as u32 * k) >> 8) + ((s >> sh) & 0xFF);
            // the channels are OR'd, not clamped (0x53C9A7…0x53C9DF)
            (ch(d[1], 8) << 8) | (ch(d[0], 16) << 16) | ch(d[2], 0) | ((0xFF - (0xFF - da) * k / 255) << 24)
        };
        d.copy_from_slice(&[(out >> 16) as u8, (out >> 8) as u8, out as u8, (out >> 24) as u8]);
        return;
    }
    if s < 0x1000_0000 {
        return;
    }
    let mut e = s;
    if s < 0xFC00_0000 {
        let (r5, g5, b5) = ((d[0] >> 3) as u32, (d[1] >> 3) as u32, (d[2] >> 3) as u32);
        let packed = (r5 << 27).wrapping_add(g5 << 14).wrapping_add(b5);
        let prod = packed as u64 * (0xFF ^ (s >> 24)) as u64;
        let (hi, lo) = ((prod >> 32) as u32, prod as u32);
        e = e.wrapping_add(hi << 16).wrapping_add((lo >> 5) & 0xFF).wrapping_add((lo >> 11) & 0xFF00);
    }
    let x5 = |v: u32| ((v << 3) | (v >> 2)) as u8;
    let (r, g, b) = ((e & 0xF8_0000) >> 19, (e & 0xF800) >> 11, (e >> 3) & 0x1F);
    d.copy_from_slice(&[x5(r), x5(g), x5(b), 0xFF]);
}

/// 0x53D9C0 into a page: `text` in `font` (a `CarModel` `.ffn`), centred in the entry's `rect`
/// by the box of its glyph quads (0x53ECE0 over 0x53E360) — `flyer` with half the height
/// (0x444BF0) — with the colours `abc` (properties 8 / 9 / 0xA).
///
/// Layout as 0x53DAB0 lays it: the pen starts at the font's `origin`, a glyph missing from the
/// font is tried in the other case (0x53E270) and else skipped without moving the pen, a space
/// is only moved over while the background is clear (0x53DB36), and each quad sits at the pen
/// plus the glyph's offset. Each quad's corner is rounded as `fistp` rounds (to even), and its
/// texels go down two at a time from atlas byte `v·(W/2) + u/2`, high nibble first, `(w+1)/2`
/// bytes a row (0x53CFE0) — an odd `u` or `w` takes a neighbouring nibble along, as there.
fn page_text_draw(page: &mut TexturePage, paths: &GamePaths, font_file: &str, text: &str, rect: (i64, i64, i64, i64), abc: [u32; 3], half: bool) {
    let Some(font) = GamePaths::find_file(&paths.car_model_dir(), font_file)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
    else {
        return;
    };
    let mut pen = font.origin[0] as i32;
    let mut quads = Vec::new();
    for ch in text.chars() {
        let c = ch as u32;
        if c == 0x20 && abc[1] & 0xFF00_0000 == 0 {
            pen += font.glyph(' ').map_or(0, |g| g.advance as i32);
            continue;
        }
        let swapped = match c {
            0x41..=0x5A | 0xC0..=0xD6 | 0xD8..=0xDE => c + 0x20,
            0x61..=0x7A | 0xE0..=0xF6 => c - 0x20,
            _ => c,
        };
        let Some(g) = font.glyph(ch).or_else(|| char::from_u32(swapped).and_then(|s| font.glyph(s))) else { continue };
        quads.push((pen + g.offset_x as i32, font.origin[1] as i32 + g.offset_y as i32, *g));
        pen += g.advance as i32;
    }
    if quads.is_empty() {
        return;
    }
    let min_x = quads.iter().map(|q| q.0).min().unwrap_or(0);
    let max_x = quads.iter().map(|q| q.0 + q.2.width as i32).max().unwrap_or(0);
    let min_y = quads.iter().map(|q| q.1).min().unwrap_or(0);
    let max_y = quads.iter().map(|q| q.1 + q.2.height as i32).max().unwrap_or(0);
    let (x0, y0, w, h) = rect;
    let h = if half { (h as i16 >> 1) as i64 } else { h };
    let ox = x0 as f32 + (w as f32 - (max_x - min_x) as f32) * 0.5;
    let oy = y0 as f32 + (h as f32 - (max_y - min_y) as f32) * 0.5;
    let pal = font_palette(font.flags, abc);
    let row_bytes = font.atlas_width as usize / 2;
    for (qx, qy, g) in quads {
        let dx = (ox + qx as f32).round_ties_even() as i64;
        let dy = (oy + qy as f32).round_ties_even() as i64;
        let bytes = (g.width as usize).div_ceil(2);
        for row in 0..g.height as usize {
            let src = (g.y as usize + row) * row_bytes + g.x as usize / 2;
            for bi in 0..bytes {
                let Some(&byte) = font.atlas.get(src + bi) else { break };
                for (k, nib) in [byte >> 4, byte & 0xF].into_iter().enumerate() {
                    let (px, py) = (dx + (bi * 2 + k) as i64, dy + row as i64);
                    if px < 0 || py < 0 || px >= page.width as i64 || py >= page.height as i64 {
                        continue;
                    }
                    let at = (py as usize * page.width as usize + px as usize) * 4;
                    font_write(page, at, pal[nib as usize]);
                }
            }
        }
    }
}

/// Copy `src` into the page at (x, y), applying the paint-slot rule (RE/notes/car-model.md §5):
/// `slot = 20 − ((a+1)·20 >> 8)`; slots 0/10/20 are copied unchanged, others become `tint × grey`.
#[allow(clippy::too_many_arguments)]
fn blit(page: &mut TexturePage, src: &[u8], w: u32, h: u32, x: i64, y: i64, tint: bool, tints: &[[u8; 3]; 18]) {
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
                // `idx = slot − 1 − (slot > 10)`, the band (0x4441AA…0x4441B4)
                let paint = tints[(slot - 1 - u32::from(slot > 10)) as usize];
                let grey = s[0] as u32;
                d[0] = ((paint[0] as u32 * grey) >> 8) as u8;
                d[1] = ((paint[1] as u32 * grey) >> 8) as u8;
                d[2] = ((paint[2] as u32 * grey) >> 8) as u8;
                d[3] = 255;
            } else {
                // copied unchanged, alpha and all (0x444130's slots 0 / 10 / 20, and every texel
                // of an untinted page): a wheel face's corners outside the tyre keep their 0 and
                // the blended `CarWheel` pass leaves them out. *(Until 2026-09-25 the port forced
                // 255 on every page but the window and alpha ones, its own rule, which drew the
                // corners of the spinning face square as black wedges past the tyre.)*
                d.copy_from_slice(s);
            }
            // what the page's surface keeps of the alpha: none in 565, one bit in 1555 — set for
            // any alpha above 0, as the 1555 tint path computes it (0x4446DD…0x4446F3; taken for
            // the untinted texels too, APPROXIMATION) — and all eight bits in 32 (4444's four are
            // kept as eight, APPROXIMATION)
            match page.bpp {
                565 => d[3] = 255,
                1555 => d[3] = if d[3] > 0 { 255 } else { 0 },
                _ => {}
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

#[cfg(test)]
mod entry_tests {
    use super::*;

    /// 0x439E75…0x439EC1: on `skidpad` `testdrive` must equal "a Factory Driver mission";
    /// elsewhere only `testdrive = 1` rules an entry out.
    #[test]
    fn testdrive_follows_skidpad_and_the_game_type() {
        let ini = nfs_formats::ini::Ini::parse("[a]\ntestdrive=1\n[b]\ntestdrive=0\n[c]\nx=1\n");
        let (a, b, c) = (ini.section("a").unwrap(), ini.section("b").unwrap(), ini.section("c").unwrap());
        let mut st = CarStyle { front_end: false, ..Default::default() };
        // off skidpad
        assert!(!st.entry_enabled(a) && st.entry_enabled(b) && st.entry_enabled(c));
        // skidpad, a mission: the test-drive faces
        st.skidpad = Some(true);
        assert!(st.entry_enabled(a) && !st.entry_enabled(b) && st.entry_enabled(c));
        // skidpad, anything else
        st.skidpad = Some(false);
        assert!(!st.entry_enabled(a) && st.entry_enabled(b));
    }
}
