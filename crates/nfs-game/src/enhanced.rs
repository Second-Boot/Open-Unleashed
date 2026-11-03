//! **The enhanced game's own front-end pieces** (not the original):
//! storage of the switches, the «Улучшения» sub-tab's generated rows and its string table.
//! `RE/notes/REMASTER-PLAN.md` stage 1, `ENHANCEMENTS.md`.
//!
//! Nothing here reads the original's binary: the panel is a port-own layout
//! (`enhanced/layouts/enhancements.lay`) built from widget types the original's layouts use, read
//! by the same layout engine from a second directory ([`layouts_dir`]). It is the **third
//! sub-tab of Options › графика**: `graphicstabs_entry.lay` (one `NFS5.TABBUTTON`) is spliced into
//! `GraphicsTabs.lay` and `graphics_entry.lay` (the panel the button names, `ID_ENHPANEL`, and the
//! tab strip's third separator) into `graphics.lay`, at load (`layout::load_one`).
//!
//! **Hard original.** `NFS_ENH=off` is the test mode "this is the original": every switch off
//! *and* none of this module's front-end additions present (no third graphics sub-tab,
//! no extra strings, no F12, nothing saved) — what the FE goldens compare against `master`'s. The
//! menu's «как в оригинале» action turns every switch off but leaves the tab, so the player can
//! come back.

use nfs_assets::paths::GamePaths;
use nfs_formats::locale::{Strings, typed_char};
use nfs_renderer::{ENHANCEMENT_REGISTRY, EnhancementStored, EnhancementSwitch, Enhancements, MAX_ENHANCEMENT_LEVELS};
use nfs_renderer::{E1, E2, E3, E4, E5, E6, E7, E8, E9, E10, E11, E12, E13, E14, E15, E16, E17, E18, E19, E20, E21, E22, E23, E24, E25, E26, E27, E28, E29, E31, E32, E33, E34, E35, E36, E37, E38, E39, E40, E41, E42, E43, E44};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

/// First id of the port's own string table (the original's ids stop below 20 000).
pub const STRING_BASE: u32 = 900_000;
pub const STR_TITLE: u32 = STRING_BASE + 1;
pub const STR_RESET: u32 = STRING_BASE + 2;
pub const STR_NONE_YET: u32 = STRING_BASE + 3;
pub const STR_HINT: u32 = STRING_BASE + 4;
/// The sub-tab's caption.
pub const STR_ENTRY: u32 = STRING_BASE + 5;
pub const STR_DEFAULTS: u32 = STRING_BASE + 6;
/// The next-page button of a panel with more rows than [`MAX_ROWS`].
pub const STR_MORE: u32 = STRING_BASE + 7;
/// The last page's button: back to the first page.
pub const STR_FIRST: u32 = STRING_BASE + 8;
/// ENH-LITE: the «лёгкий» (light preset) button's caption.
pub const STR_LITE: u32 = STRING_BASE + 9;
/// Row `i`'s caption is `STR_NAME + i`, its help `STR_HELP + i`.
pub const STR_NAME: u32 = STRING_BASE + 100;
pub const STR_HELP: u32 = STRING_BASE + 200;
/// Row `i`'s level `l` caption is `STR_LEVEL + i * MAX_ENHANCEMENT_LEVELS + l` (the combo's rows).
pub const STR_LEVEL: u32 = STRING_BASE + 300;
/// ENHANCEMENT ENH-UPSCALER: the virtual rows (MetalFX mode / scale, FSR scale: [`VRow`]): caption `STR_VROW + v`, help `+ 10`, level
/// `l` caption `+ 100 + v * 10 + l`.
pub const STR_VROW: u32 = STRING_BASE + 3000;
/// ENHANCEMENT ENH-UPSCALER: the group header `g` of [`GROUPS`].
pub const STR_GROUP: u32 = STRING_BASE + 3300;
/// ENHANCEMENT ENH-UPSCALER: the hint of an inactive row: `STR_REASON + dep * 3 + by` (`dep` a row of `ENHANCEMENT_DEPENDS`, `by` the
/// upscaler backend that overrides it, 0 for a plain "needs").
pub const STR_REASON: u32 = STRING_BASE + 3400;
// `STRING_BASE + 2000..` held E30's photo-mode lines until 2026-10-27; photo mode is master's now (PORT-DEVIATION D46,
// `nfs_game::photo`, its strings are not in the table) and its depth-of-field lines are [`PHOTO_DOF_STRINGS`]

/// the id range `[start, end)` of every kind above at its full capacity — every registry row the
/// [`Enhancements`] set can hold with every level an entry can have, every virtual row and level, group header and
/// `DEPENDS` reason. [`ranges_disjoint`] checks at compile time that none overlaps another, so adding entries, levels, groups or
/// dependencies cannot make two strings share an id unseen. (The ids themselves are unchanged: `the_string_ids_are_pinned`.)
const ID_RANGES: &[(&str, u32, u32)] = &[
    ("fixed", STR_TITLE, STR_LITE + 1),
    ("name", STR_NAME, STR_NAME + nfs_renderer::MAX_ENHANCEMENTS as u32),
    ("help", STR_HELP, STR_HELP + nfs_renderer::MAX_ENHANCEMENTS as u32),
    ("level", STR_LEVEL, STR_LEVEL + (nfs_renderer::MAX_ENHANCEMENTS * MAX_ENHANCEMENT_LEVELS) as u32),
    // a virtual row's caption `+ v`, help `+ 10 + v`, levels `+ 100 + v * 10 + l`: v and l below 10 (`VROW_FITS`)
    ("vrow", STR_VROW, STR_VROW + 200),
    ("group", STR_GROUP, STR_GROUP + GROUPS.len() as u32),
    ("reason", STR_REASON, STR_REASON + nfs_renderer::ENHANCEMENT_DEPENDS.len() as u32 * 3),
];

/// No two of `r`'s ranges overlap, every range is non-empty and above the original's ids (below 20 000).
const fn ranges_disjoint(r: &[(&str, u32, u32)]) -> bool {
    let mut i = 0;
    while i < r.len() {
        if r[i].1 >= r[i].2 || r[i].1 < 20_000 {
            return false;
        }
        let mut j = i + 1;
        while j < r.len() {
            if r[i].1 < r[j].2 && r[j].1 < r[i].2 {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

/// The virtual rows' ids stay inside their `vrow` range: fewer than 10 rows, fewer than 10 levels each.
const VROW_FITS: bool = {
    let mut ok = VRow::ALL.len() <= 10;
    let mut i = 0;
    while i < VROW_LEVELS.len() {
        ok = ok && VROW_LEVELS[i].1.len() <= 10;
        i += 1;
    }
    ok
};

/// Every registry entry fits the level ids' stride (`STR_LEVEL + i * MAX_ENHANCEMENT_LEVELS + l`).
const LEVELS_FIT: bool = {
    let mut ok = ENHANCEMENT_REGISTRY.len() <= nfs_renderer::MAX_ENHANCEMENTS;
    let mut i = 0;
    while i < ENHANCEMENT_REGISTRY.len() {
        ok = ok && ENHANCEMENT_REGISTRY[i].levels.len() <= MAX_ENHANCEMENT_LEVELS;
        i += 1;
    }
    ok
};

const _: () = assert!(ranges_disjoint(ID_RANGES), "two kinds of the port's string ids overlap (ID_RANGES)");
const _: () = assert!(VROW_FITS, "a virtual row's ids leave the vrow range");
const _: () = assert!(LEVELS_FIT, "a registry entry has more levels than the level ids reserve");

/// The accessor name a menu row binds: `Enh.E1`.
pub const ACCESSOR_PREFIX: &str = "Enh.";
/// The callback of the «как в оригинале» button.
pub const CB_ORIGINAL: &str = "ENHORIGINAL";
/// The callback of the «по умолчанию» button (every switch to its default level).
pub const CB_DEFAULTS: &str = "ENHDEFAULTS";
/// ENH-LITE: the callback of the «лёгкий» button (the light preset, `Enhancements::lite`).
pub const CB_LITE: &str = "ENHLITE";
/// The callback prefix of the page button: `ENHPAGE<n>` shows page `n` (1-based) in the panel.
pub const CB_PAGE: &str = "ENHPAGE";
/// The layout file (and the name of page 0) of the sub-tab's panel.
pub const SCREEN: &str = "enhancements";
/// The `NFS5.PANEL` of `graphics.lay` the third sub-tab names.
pub const PANEL: &str = "ID_ENHPANEL";

/// How the switches were chosen at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// the player's own (`enhancements.json`, or an `NFS_ENH` list / `all`)
    Normal,
    /// `NFS_ENH=off`: the original, with none of the front-end additions (see the module docs)
    HardOriginal,
}

static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();

/// Pin the mode for the run (the game's start-up, once). Until then — a library user, the engine's
/// own tests — the front-end additions are absent, i.e. the original.
pub fn init(mode: Mode) {
    let _ = MODE.set(mode);
}

/// Whether the front-end additions (the entry button, the screen, the strings) exist in this run.
pub fn enabled() -> bool {
    MODE.get() == Some(&Mode::Normal)
}

/// The combo rows of registry entry `i`: the string ids of its level captions, level 0 (off) first.
pub fn level_ids(i: usize) -> &'static [u32] {
    static IDS: std::sync::OnceLock<Vec<Vec<u32>>> = std::sync::OnceLock::new();
    let all = IDS.get_or_init(|| {
        ENHANCEMENT_REGISTRY
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let id = |l: usize| STR_LEVEL + (i * MAX_ENHANCEMENT_LEVELS + l) as u32;
                // ENHANCEMENT ENH-UPSCALER: the upscaler row lists the backends this platform has (no MetalFX off macOS)
                if i == nfs_renderer::E38.0 as usize { nfs_renderer::upscaler_choices().iter().map(|&l| id(l as usize)).collect() } else { (0..e.levels.len()).map(id).collect() }
            })
            .collect()
    });
    all.get(i).map(|v| v.as_slice()).unwrap_or(&[])
}

/// ENHANCEMENT ENH-UPSCALER: the string ids of a virtual row's level captions, in order.
pub fn vrow_ids(v: VRow) -> &'static [u32] {
    static IDS: std::sync::OnceLock<Vec<Vec<u32>>> = std::sync::OnceLock::new();
    let all = IDS.get_or_init(|| VROW_LEVELS.iter().map(|(v, levels)| (0..levels.len()).map(|l| v.level_id(l)).collect()).collect());
    all.get(VROW_LEVELS.iter().position(|(x, _)| *x == v).unwrap_or(0)).map_or(&[][..], |ids| ids.as_slice())
}

/// The switches at start and how they were chosen: `NFS_ENH` over the stored file over the default
/// (every entry at its default level).
pub fn load() -> (Enhancements, Mode) {
    let env = std::env::var("NFS_ENH").ok();
    let file = read_file();
    let r = load_with(env.as_deref(), file.as_deref());
    // ENH-LITE: neither a list in the environment nor a stored file = the first run (or a player who never
    // touched the panel): the set is still the full defaults here, the GPU class is not known yet; the
    // renderer's start-up decides ([`auto_pending`], `app::session`). Nothing is written.
    let env_chosen = env.as_deref().is_some_and(|v| !v.trim().is_empty());
    if !env_chosen && file.is_none() {
        AUTO_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    r
}

static AUTO_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ENH-LITE: whether this run's set is the "no choice made yet" default, to be matched to the GPU
/// (a weak one gets the light preset, `Enhancements::lite`) once the adapter is known; taken once.
pub fn auto_pending() -> bool {
    AUTO_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// ENH-LITE: the set for a first run on an adapter of this class — the full defaults, or the light
/// preset for a weak GPU. `NFS_GPU_CLASS=weak|strong` forces the class (test glue: the Mac is strong).
pub fn auto_set(class: nfs_renderer::GpuClass) -> (Enhancements, nfs_renderer::GpuClass) {
    let class = match std::env::var("NFS_GPU_CLASS").ok().as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("weak") | Some("light") | Some("lite") => nfs_renderer::GpuClass::Weak,
        Some("strong") | Some("full") => nfs_renderer::GpuClass::Strong,
        _ => class,
    };
    (if class == nfs_renderer::GpuClass::Weak { Enhancements::lite() } else { Enhancements::all() }, class)
}

pub(crate) fn load_with(env: Option<&str>, file: Option<&str>) -> (Enhancements, Mode) {
    if let Some(v) = env {
        let mut unknown = Vec::new();
        match Enhancements::parse_override(v, &mut unknown) {
            Some(e) => {
                for u in &unknown {
                    tracing::warn!(token = %u, "NFS_ENH: no such enhancement");
                }
                let hard = v.trim().eq_ignore_ascii_case("off") || v.trim().eq_ignore_ascii_case("none");
                return (e, if hard { Mode::HardOriginal } else { Mode::Normal });
            }
            None => tracing::warn!(value = %v, "NFS_ENH: empty value ignored"),
        }
    }
    (file.map(parse_file).unwrap_or_else(Enhancements::all), Mode::Normal)
}

/// `enhancements.json` — in `NFS5_SAVE_DIR` when that is set (a scripted run owns its directory
/// and must not read the player's file), else the port's data dir
/// (`nfs_assets::platform::data_dir`). Never inside the `.sav`.
pub fn settings_path() -> PathBuf {
    let dir = std::env::var_os("NFS5_SAVE_DIR").map(PathBuf::from).unwrap_or_else(nfs_assets::platform::data_dir);
    dir.join("enhancements.json")
}

fn read_file() -> Option<String> {
    std::fs::read_to_string(settings_path()).ok()
}

/// The set a stored file gives; unreadable → the default (every entry at its default level). The
/// file is `{"version": 2, "levels": {"E1": 2, …}}`; the first format `{"version": 1, "switches":
/// {"E1": true, …}}` reads too (`true` = the entry's default level, `false` = off), and so does a
/// boolean in `levels`. A token the file does not name is at its default.
pub fn parse_file(text: &str) -> Enhancements {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Enhancements::all() };
    let Some(map) = v.get("levels").or_else(|| v.get("switches")).and_then(|s| s.as_object()) else { return Enhancements::all() };
    let stored = |v: &serde_json::Value| match v {
        serde_json::Value::Bool(b) => Some(EnhancementStored::Bool(*b)),
        serde_json::Value::Number(n) => n.as_u64().map(|l| EnhancementStored::Level(l.min(u8::MAX as u64) as u8)),
        _ => None,
    };
    // ENHANCEMENT E34 (temporal levels, file version 3): the old spatial levels 1..3 (quality / balanced / performance) are now 5..7 - the
    // temporal ones took 1..4 - so a file written before keeps its meaning
    let version = v.get("version").and_then(|n| n.as_u64()).unwrap_or(1);
    let mut set = Enhancements::from_stored_in(
        ENHANCEMENT_REGISTRY,
        map.iter().filter_map(|(k, v)| {
            let s = stored(v)?;
            Some((k.as_str(), match s {
                EnhancementStored::Level(l @ 1..=3) if k == "E34" && version < 3 => EnhancementStored::Level(l + 4),
                // ENHANCEMENT ENH-COAT (file version 5): E8's levels are the lacquer's now (1 new, 2 normal, 3 worn); the old ones were the
                // faces' size - 1 low (128 px) and 2 high (256 px). Low becomes normal, high becomes new (a sharper coat, the old high's
                // detail)
                EnhancementStored::Level(1) if k == "E8" && version < 5 => EnhancementStored::Level(2),
                EnhancementStored::Level(2) if k == "E8" && version < 5 => EnhancementStored::Level(1),
                // ENHANCEMENT E41 (file version 7): the textures are off / on now; every older level above 0 (x2, x4, the first «новые») is on
                EnhancementStored::Level(l @ 2..) if k == "E41" && version < 7 => EnhancementStored::Level(l.min(1)),
                s => s,
            }))
        }),
    );
    // ENHANCEMENT ENH-UPSCALER (file version 4): the upscaler choice E38 and FSR's sharpness E39 are new. A file of an older version has
    // E34 (MetalFX) or E37 (FSR) above 0 = that backend was on: the choice becomes it, with the stored level kept as the backend's
    // mode / scale and E39 at its default (E37 always sharpened by 0.3); with neither on the choice stays off.
    if version < 4 {
        set.infer_upscaler(false);
    }
    set
}

/// The stored form: `{"version": 7, "levels": {"E1": 2, …}}` (3: E34's levels renumbered; 4: the upscaler choice E38, see `parse_file`;
/// 5: E8's levels are the clear coat's; 6: E41's levels were original / x2 / x4; 7: E41 is off / on).
pub fn file_text(e: &Enhancements) -> String {
    let levels: serde_json::Map<String, serde_json::Value> = e.pairs_in(ENHANCEMENT_REGISTRY).into_iter().map(|(k, l)| (k.to_string(), l.into())).collect();
    let v = serde_json::json!({ "version": 7, "levels": levels });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// Save on change. Best effort: a read-only data dir loses the setting, not the game.
pub fn save(e: &Enhancements) {
    let path = settings_path();
    if let Err(err) = save_to(&path, e) {
        tracing::warn!(path = %path.display(), error = %err, "enhancements.json not saved");
    }
}

/// [`save`] at a given path (the directory is created).
pub fn save_to(path: &std::path::Path, e: &Enhancements) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, file_text(e))
}

/// The debug A/B key (F12): the player's set <-> every switch off (or, with the player's own set already all
/// off, every default). Returns the set to show and the player's set to bring back by the next press (`None`
/// = this press is the way back). **The A/B is a view, never stored**: before ENH-SAVE the key saved whatever
/// it showed, so two presses (off, then "all") overwrote the player's levels with the registry defaults —
/// «everything is reset» (2026-10-04).
pub fn ab_toggle(current: Enhancements, stash: Option<Enhancements>) -> (Enhancements, Option<Enhancements>) {
    match stash {
        Some(player) => (player, None),
        None => (if current.any_on() { Enhancements::ORIGINAL } else { Enhancements::all() }, Some(current)),
    }
}

/// Where the port-own layouts live: `$NFS5_REMASTER_DIR`, `enhanced/layouts` in the game root
/// (the packaged data, `packaging/stage_data.sh`), beside the executable / in a macOS bundle's Resources,
/// then — a development build — the repository's own `enhanced/layouts`. `remaster/` (the folder's
/// earlier name) is accepted after `enhanced/` at every place.
pub fn layouts_dir(paths: &GamePaths) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("NFS5_REMASTER_DIR") {
        c.push(PathBuf::from(d).join("layouts"));
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
    for folder in nfs_assets::hdpack::ENHANCED_FOLDERS {
        c.push(paths.root.join(folder).join("layouts"));
        if let Some(d) = &exe_dir {
            c.push(d.join(folder).join("layouts"));
            c.push(d.join("../Resources").join(folder).join("layouts"));
        }
        c.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(folder).join("layouts"));
    }
    c.into_iter().find(|d| d.is_dir())
}

/// The language of the port's own strings: Russian when the install's own text is (the original's
/// «выкл» is Windows-1251 bytes), English otherwise — master's rule (`nfs_game::photo::is_russian`, which also keeps an
/// accented German / French / ... line English), so the photo mode and the enhancement rows agree.
pub fn is_russian(strings: &Strings) -> bool {
    crate::photo::is_russian(strings)
}

fn text_row(en: &str, ru: &str, russian: bool) -> String {
    let s = if russian { ru } else { en };
    // the strings the game draws are byte strings of the install's code page (Windows-1251 for
    // Russian), carried as `cp1252` chars (`nfs_formats::locale`)
    s.chars().map(|c| typed_char(c).unwrap_or('?')).collect()
}

/// ENHANCEMENTS E7 / E31 on top of master's photo mode (PORT-DEVIATION D46, `nfs_game::photo`): the depth-of-field lines of
/// its hint, English and Russian, in the order the application's `photo.rs` numbers them (the keys, the pad's keys, then the
/// words of the status line). Shown only while the depth of field can act (the float scene, E7, on).
pub const PHOTO_DOF_STRINGS: [[&str; 2]; 5] = [
    ["F depth of field, 1 / 2 focus, 3 / 4 aperture, 5 focus on the car", "F глубина резкости, 1 / 2 фокус, 3 / 4 диафрагма, 5 фокус на машине"],
    ["pad: X depth of field, d-pad left / right focus", "геймпад: X глубина резкости, крестовина влево / вправо фокус"],
    ["depth of field", "глубина резкости"],
    ["off", "выкл"],
    ["focus", "фокус"],
];

/// Line `k` of [`PHOTO_DOF_STRINGS`] in the install's language (as master's photo strings: English unless Russian).
pub fn photo_dof_text(strings: &Strings, k: usize) -> String {
    let ru = is_russian(strings);
    PHOTO_DOF_STRINGS.get(k).map_or_else(String::new, |t| text_row(t[0], t[1], ru))
}

/// ENHANCEMENT ENH-UPSCALER: the hint of a row that has no effect now (shown as its hover help line): which setting disables it.
/// `by` names the upscaler backend for the upscaler rules (1 MetalFX, 2 FSR 3.1), 0 otherwise.
pub fn reason_text(dep: &nfs_renderer::EnhancementDependency, by: u8, russian: bool) -> String {
    use nfs_renderer::EnhancementCond as C;
    let up = match (by, russian) {
        (1, _) => "MetalFX",
        (2, _) => "FSR 3.1",
        (_, false) => "upscaler",
        (_, true) => "апскейлер",
    };
    let name = |tok: &'static str| ENHANCEMENT_REGISTRY.iter().find(|e| e.token == tok).map_or(tok, |e| e.name[russian as usize]);
    match (dep.cond, russian) {
        (C::Needs(t), false) => format!("inactive: needs '{}'", name(t)),
        (C::Needs(t), true) => format!("не действует: нужен «{}»", name(t)),
        (C::UpscalerOn, false) => format!("inactive: the upscaler ({up}) sets the render size itself"),
        (C::UpscalerOn, true) => format!("не действует: включен апскейлер ({up}), он сам задает размер"),
        (C::TemporalUpscaler, false) => format!("inactive: a temporal upscaler ({up}) replaces the anti-aliasing"),
        (C::TemporalUpscaler, true) => format!("не действует: временной апскейлер ({up}) заменяет сглаживание"),
        (C::NoHdPack, false) => "not available: no hd_textures folder".to_string(),
        (C::NoHdPack, true) => "недоступно: нет папки hd_textures".to_string(),
    }
}

/// The original's strings plus the port's own ([`STRING_BASE`] and up), in the install's language.
pub fn with_extras(mut s: Strings) -> Strings {
    let ru = is_russian(&s);
    let mut put = |id: u32, en: &str, rus: &str| {
        let t = text_row(en, rus, ru);
        s.fe_by_id.insert(id, t.clone());
        s.by_id.insert(id, t);
    };
    put(STR_TITLE, "enhancements", "улучшения");
    put(STR_RESET, "as in the original", "как в оригинале");
    put(STR_NONE_YET, "no enhancements yet", "улучшений пока нет");
    put(STR_HINT, "switched live", "действуют сразу");
    put(STR_ENTRY, "enhancements", "улучшения");
    put(STR_DEFAULTS, "defaults", "по умолчанию");
    put(STR_LITE, "light", "легкий"); // no "ё" in the front end's font (it drew as ","), so the button reads «легкий»
    put(STR_MORE, "more...", "дальше...");
    put(STR_FIRST, "first page...", "сначала...");
    // ENHANCEMENT ENH-UPSCALER: the group headers, the virtual rows, the hints of inactive rows
    for (g, (name, _)) in GROUPS.iter().enumerate() {
        put(STR_GROUP + g as u32, name[0], name[1]);
    }
    for (v, name, help) in VROW_TEXTS {
        put(v.name_id(), name[0], name[1]);
        put(v.help_id(), help[0], help[1]);
    }
    for (v, levels) in VROW_LEVELS {
        for (l, cap) in levels.iter().enumerate() {
            put(v.level_id(l), cap[0], cap[1]);
        }
    }
    for (k, dep) in nfs_renderer::ENHANCEMENT_DEPENDS.iter().enumerate() {
        for by in 0..3u8 {
            put(STR_REASON + k as u32 * 3 + by as u32, &reason_text(dep, by, false), &reason_text(dep, by, true));
        }
    }
    for (i, e) in ENHANCEMENT_REGISTRY.iter().enumerate() {
        put(STR_NAME + i as u32, e.name[0], e.name[1]);
        put(STR_HELP + i as u32, e.help[0], e.help[1]);
        // the row's captions follow `level_ids` (the upscaler row lists only this platform's backends: off macOS it has
        // no MetalFX, so its k-th id is the level `upscaler_choices()[k]`, not `k` — iterating `e.levels` overran it there)
        for (k, &id) in level_ids(i).iter().enumerate() {
            let l = if i == nfs_renderer::E38.0 as usize { nfs_renderer::upscaler_choices()[k] as usize } else { k };
            put(id, e.levels[l][0], e.levels[l][1]);
        }
    }
    s
}

/// Rows per column of the panel: from y 124 at 18 px, the last ends at y 409 (the panel's band is
/// y 116…420, below the sub-tab strip — the third tab ends at y 110 — and above the buttons at 424).
pub const COL_ROWS: usize = 16;
/// Columns of the panel (the captions are long: one column of 16 rows, pages for more).
pub const COLS: usize = 1;
/// Rows per page (a page is one panel content; more entries than this get further pages).
pub const MAX_ROWS: usize = COL_ROWS * COLS;
const ROW_Y0: i32 = 124;
const ROW_STEP: i32 = 18;
/// Where each column sits: the shift of `display.lay`'s row geometry (bar 63, combo 193…343), here
/// the combo at x 330 (190 wide, the long level captions fit), the caption right-aligned at 326 over a
/// 230 px fade bar — one column centred on the panel.
const COL_DX: [i32; COLS] = [137];
/// The buttons under the rows.
const BTN_Y: i32 = 424;
/// Four buttons (defaults, light, as in the original, the page button), 126 wide, in the glinty lines' 57..583.
const BTN_XS: [i32; 4] = [57, 189, 321, 453];
/// A group header's colour (ARGB), and an inactive row's caption colour.
const HEADER_COLOR: &str = "0xff7a2a00";
/// The gap between a caption's right end and the end of the fade bar behind it .
const CAPTION_PAD: i32 = 6;
const INACTIVE_COLOR: &str = "0xff8a8a8a";

// ---- ENHANCEMENT ENH-UPSCALER / ENH-PANEL: the panel's content is built from the CURRENT set ----

/// The panel's groups in display order: `[English, Russian]` header, then the registry switches in row order. The display order is
/// independent of the registry's (the Switch indices never move). After E38 the panel inserts the settings rows of the chosen upscaler
/// (MetalFX: mode + scale, FSR: scale + sharpness). E34 / E37 / E39 have no row of their own: [`SETTINGS_OF_E38`].
/// A group stays on one page when it fits on one (`paginate`).
pub const GROUPS: &[([&str; 2], &[EnhancementSwitch])] = &[
    (["image", "Изображение"], &[E6, E22, E1, E2, E3, E35, E4, E5, E38, E33]),
    (["light and shadows", "Свет и тени"], &[E7, E9, E43, E25, E14, E32, E16, E15, E24, E23, E20]),
    (["surfaces", "Поверхности"], &[E8, E17, E26, E40, E41, E42]),
    (["atmosphere and effects", "Атмосфера и эффекты"], &[E10, E11, E13, E19, E36, E44, E21, E18, E12]),
    (["camera", "Камера"], &[E27, E28, E29, E31]),
];

/// The registry entries the upscaler choice (E38) carries as settings rows, not rows of their own.
pub const SETTINGS_OF_E38: &[EnhancementSwitch] = &[E34, E37, E39];

/// A row that edits a part of a stored level instead of a whole entry (the `Enh.<name>` accessors below).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VRow {
    /// MetalFX «режим»: temporal / simple (E34's level 1..=4 / 5..=7)
    MfxMode,
    /// MetalFX «масштаб»: 1:1 (temporal only) / 67 / 59 / 50 % (E34)
    MfxScale,
    /// FSR «масштаб»: 1:1 / 1.5x / 1.7x / 2x (E37 levels 1..=4)
    FsrScale,
}

impl VRow {
    pub const ALL: [VRow; 3] = [VRow::MfxMode, VRow::MfxScale, VRow::FsrScale];

    /// The accessor the row's combo binds (after [`ACCESSOR_PREFIX`]).
    pub fn accessor(self) -> &'static str {
        match self {
            VRow::MfxMode => "Enh.MFXMODE",
            VRow::MfxScale => "Enh.MFXSCALE",
            VRow::FsrScale => "Enh.FSRSCALE",
        }
    }

    pub fn from_accessor(name: &str) -> Option<VRow> {
        VRow::ALL.into_iter().find(|v| v.accessor() == name)
    }

    fn index(self) -> u32 {
        self as u32
    }

    /// The row's caption / help string ids and its level captions (the first level's id; they are contiguous).
    pub fn name_id(self) -> u32 {
        STR_VROW + self.index()
    }
    pub fn help_id(self) -> u32 {
        STR_VROW + 10 + self.index()
    }
    pub fn level_id(self, level: usize) -> u32 {
        STR_VROW + 100 + self.index() * 10 + level as u32
    }
}

/// The captions of the virtual rows' levels `[English, Russian]`; MfxScale's first entry (1:1) is temporal only, so the simple mode
/// shows the slice from the second.
pub const VROW_LEVELS: [(VRow, &[[&str; 2]]); 3] = [
    (VRow::MfxMode, &[["temporal", "временной"], ["simple", "простой"]]),
    (VRow::MfxScale, &[["1:1 (anti-aliasing only)", "1:1 (только сглаживание)"], ["67 % (quality)", "67 % (качество)"], ["59 % (balanced)", "59 % (баланс)"], ["50 % (performance)", "50 % (производительность)"]]),
    (VRow::FsrScale, &[["1:1 (anti-aliasing only)", "1:1 (только сглаживание)"], ["1.5x (quality)", "1.5x (качество)"], ["1.7x (balanced)", "1.7x (баланс)"], ["2x (performance)", "2x (производительность)"]]),
];

/// The captions and helps of the virtual rows `[English, Russian]`.
pub const VROW_TEXTS: [(VRow, [&str; 2], [&str; 2]); 3] = [
    (
        VRow::MfxMode,
        ["mode", "режим"],
        [
            "temporal: the scene is drawn jittered with motion vectors and MetalFX builds the picture from several frames (sharper, smooth edges; replaces MSAA and SMAA); simple: a plain enlargement of one frame (faster, a little softer)",
            "временной: сцена рисуется со сдвигом и векторами движения, а MetalFX собирает картинку из нескольких кадров (четче, гладкие края; заменяет MSAA и SMAA); простой: обычное растяжение одного кадра (быстрее, чуть мягче)",
        ],
    ),
    (
        VRow::MfxScale,
        ["scale", "масштаб"],
        [
            "the share of the window's size the scene is drawn at: 1:1 only smooths the edges (temporal mode), 67 / 59 / 50 % draw less and save GPU work. The render scale setting is not used while an upscaler is on",
            "доля размера окна, в которой рисуется сцена: 1:1 только сглаживает края (временной режим), 67 / 59 / 50 % рисуют меньше и экономят работу видеокарты. Масштаб рендера не действует, пока включен апскейлер",
        ],
    ),
    (
        VRow::FsrScale,
        ["scale", "масштаб"],
        [
            "the ratio of the window's size to the size the scene is drawn at: 1:1 only smooths the edges, 1.5x / 1.7x / 2x draw less and save GPU work. The render scale setting is not used while an upscaler is on",
            "во сколько раз размер окна больше размера, в котором рисуется сцена: 1:1 только сглаживает края, 1.5x / 1.7x / 2x рисуют меньше и экономят работу видеокарты. Масштаб рендера не действует, пока включен апскейлер",
        ],
    ),
];

/// One line of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// the header of group `GROUPS[g]`
    Header(usize),
    /// registry entry `i`
    Entry(usize),
    /// an upscaler setting that edits part of a stored level
    Virtual(VRow),
}

/// The lines of every group for a set, header first. The upscaler's settings rows follow E38 and only those of the CHOSEN backend
/// (MetalFX exists on macOS only); entries the platform has no backend for are not listed.
pub fn group_items(state: &Enhancements) -> Vec<Vec<Item>> {
    GROUPS
        .iter()
        .enumerate()
        .map(|(g, (_, switches))| {
            let mut v = vec![Item::Header(g)];
            for sw in *switches {
                let i = sw.0 as usize;
                if !ENHANCEMENT_REGISTRY[i].available() {
                    continue;
                }
                v.push(Item::Entry(i));
                if i == nfs_renderer::E38.0 as usize {
                    match state.level(nfs_renderer::E38) {
                        nfs_renderer::UP_METALFX if cfg!(target_os = "macos") => v.extend([Item::Virtual(VRow::MfxMode), Item::Virtual(VRow::MfxScale)]),
                        nfs_renderer::UP_FSR => v.extend([Item::Virtual(VRow::FsrScale), Item::Entry(nfs_renderer::E39.0 as usize)]),
                        _ => {}
                    }
                }
            }
            v
        })
        .collect()
}

/// Pages of at most `cap` lines: groups in order, a group goes whole onto the current page when it fits there, else a new page starts
/// (a group longer than a page is split). Never an empty page, at least one page.
pub fn paginate(groups: &[Vec<Item>], cap: usize) -> Vec<Vec<Item>> {
    let mut pages: Vec<Vec<Item>> = vec![Vec::new()];
    for g in groups {
        if pages.last().is_some_and(|p| !p.is_empty() && p.len() + g.len() > cap) {
            pages.push(Vec::new());
        }
        for chunk in g.chunks(cap) {
            if pages.last().is_some_and(|p| !p.is_empty() && p.len() + chunk.len() > cap) {
                pages.push(Vec::new());
            }
            pages.last_mut().expect("a page").extend_from_slice(chunk);
        }
    }
    pages
}

/// The pages of the panel for a set.
pub fn panel_pages(state: &Enhancements) -> Vec<Vec<Item>> {
    paginate(&group_items(state), MAX_ROWS)
}

/// How a line is drawn: caption id, help id (the reason while inactive), accessor, whether it is inactive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInfo {
    pub name_id: u32,
    pub help_id: u32,
    pub accessor: String,
    pub inactive: bool,
}

/// The drawing info of a (non-header) item for a set; `None` for a header.
pub fn line_info(item: Item, state: &Enhancements) -> Option<LineInfo> {
    match item {
        Item::Header(_) => None,
        Item::Virtual(v) => Some(LineInfo { name_id: v.name_id(), help_id: v.help_id(), accessor: v.accessor().to_string(), inactive: false }),
        Item::Entry(i) => {
            let e = &ENHANCEMENT_REGISTRY[i];
            let blocked = state.blocked(e.token);
            Some(LineInfo {
                name_id: STR_NAME + i as u32,
                help_id: blocked.map_or(STR_HELP + i as u32, |b| STR_REASON + b.dep as u32 * 3 + b.by as u32),
                accessor: format!("{ACCESSOR_PREFIX}{}", e.token),
                inactive: blocked.is_some(),
            })
        }
    }
}

/// Everything that decides how the panel looks for a set: its pages' lines with their drawing info. Two sets with an equal
/// signature give the same panel (no rebuild needed).
pub fn panel_signature(state: &Enhancements) -> Vec<Vec<(Item, Option<LineInfo>)>> {
    panel_pages(state).into_iter().map(|p| p.into_iter().map(|it| (it, line_info(it, state))).collect()).collect()
}

static PANEL_STATE: Mutex<Option<Enhancements>> = Mutex::new(None);
static CUR_PAGE: AtomicUsize = AtomicUsize::new(0);

/// The set the panel is generated for (a page load reads it): the player's stored set, kept by the application.
pub fn panel_state() -> Enhancements {
    PANEL_STATE.lock().ok().and_then(|g| *g).unwrap_or_else(Enhancements::all)
}

/// The application's set changed: remember it for the next page load; `true` when the panel's lines (rows, inactive flags,
/// hints, pages) differ from before, i.e. a panel that is on screen has to be rebuilt.
pub fn set_panel_state(e: Enhancements) -> bool {
    let old = panel_state();
    if let Ok(mut g) = PANEL_STATE.lock() {
        *g = Some(e);
    }
    panel_signature(&old) != panel_signature(&e)
}

/// The page (0-based) the panel last generated (clamped to the pages of the current set): the one to rebuild.
pub fn current_page() -> usize {
    CUR_PAGE.load(Ordering::Relaxed).min(pages().saturating_sub(1))
}

/// How many pages `n` lines need at [`MAX_ROWS`] each, at least one (plain count, no groups).
pub fn pages_for(n: usize) -> usize {
    n.div_ceil(MAX_ROWS).max(1)
}

/// How many pages the panel has for the current set.
pub fn pages() -> usize {
    panel_pages(&panel_state()).len()
}

/// The layout name of page `page` (0-based): `enhancements`, `enhancements2`, ... All pages are
/// the one file `enhancements.lay` with different generated rows; the page button swaps the
/// panel's content (`LayoutScreen::swap_panel`).
pub fn page_name(page: usize) -> String {
    if page == 0 { SCREEN.to_string() } else { format!("{SCREEN}{}", page + 1) }
}

/// Which page a layout name is (0-based), `None` when it is not the port's own panel or the
/// page does not exist.
pub fn page_of(name: &str) -> Option<usize> {
    let n = name.trim_end_matches(".lay").trim_end_matches(".LAY").to_ascii_lowercase();
    let rest = n.strip_prefix(SCREEN)?;
    let page = if rest.is_empty() { 0 } else { rest.parse::<usize>().ok().filter(|p| *p >= 2)? - 1 };
    (page < pages()).then_some(page)
}

/// The panel file's text with a page's generated rows right after its screen section (section 0). `Window::hitTest` 0x528C00
/// gives the pointer to the **first** section whose box holds it (the draw walks backwards, so the first is also the topmost),
/// and the file's two background boxes cover the rows: after them the combos took no click. The original's panels put their
/// backgrounds last for the same reason (`graphicsettings.lay` §36 / §37). A file with no section 1 gets the rows at its end.
pub fn with_rows(file: &str, rows: &str) -> String {
    match file.find("[section.1]") {
        Some(i) => format!("{}{rows}\n{}", &file[..i], &file[i..]),
        None => format!("{file}{rows}"),
    }
}

/// The generated sections for one page of the panel for the current set — see [`generated_rows_in`].
pub fn generated_rows(page: usize) -> String {
    CUR_PAGE.store(page, Ordering::Relaxed);
    generated_rows_in(&panel_state(), page)
}

/// The sections inserted into `enhancements.lay` ([`with_rows`]) for one page of the panel: for each line a caption label and a level combo bound to
/// its accessor (`Enh.<token>`, or a virtual row's), in the geometry `display.lay` uses for its rows (combo width 190, the label
/// right-aligned 4 px left of it, a fade bar behind the label), one column; a group header is a coloured label alone. An inactive
/// row (`state=disabled`, the engine's own disabled state: not clickable) has a grey caption and its hover help is the reason. The combo's
/// `helptext` is the entry's help (the original's tooltip, `FeWidget` 0x4C7FE2). Under the rows the page button (`ENHPAGE<n>`, on the
/// last page back to the first; absent with one page); the three reset buttons are in the file. With nothing to list, one
/// placeholder label. Generated so that adding an `Entry` to a group adds a row.
pub fn generated_rows_in(state: &Enhancements, page: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pages = panel_pages(state);
    let lines: &[Item] = pages.get(page).map_or(&[], |p| p.as_slice());
    if lines.is_empty() {
        let _ = write!(
            out,
            "\n[section.900]\nid=REAL.LABEL\nname=ID_ENHNONE\nstate=normal\npos=320,{ROW_Y0}\nfontPathName=univers8b.ffn\nforeColor=0xff000000\nbackColor=0x00000000\nstringID={STR_NONE_YET}\nalign=1\ntranson=FADE\ntransoff=SAME\n"
        );
        return out;
    }
    let dx = COL_DX[0];
    let y_of = |k: usize| ROW_Y0 + ROW_STEP * k as i32;
    let rows: Vec<(usize, Item, Option<LineInfo>)> = lines.iter().enumerate().map(|(k, it)| (k, *it, line_info(*it, state))).collect();
    // the order display.lay uses: every combo, then every caption, then the fade bars over them
    let mut s = 900;
    for (k, _, info) in &rows {
        let Some(info) = info else { continue };
        let _ = write!(
            out,
            "\n[section.{s}]\nid=NFS5.ComboBoxSlider\nname=ID_COMBOBOX\nstate={st}\npos={x},{y}\ndestination=\nstemleftheight=0\nstemrightheight=0\noffset=0\ncallback=\naccessor={acc}\nwidth=190\nheight=15\nselectionboxwidth=\nselectionboxslidedirection=\nselectionboxmaxheight=160\nportpad=4\nhelptext={help}\ntranson=FADE\ntransoff=SAME\nnolabel=1\nusesmallfont=1\nGraphicsSpecialCaseWidth=\ndropshadow=1\nglintybox=2\nstringID=0\n",
            st = if info.inactive { "disabled" } else { "normal" },
            x = 193 + dx,
            y = y_of(*k),
            acc = info.accessor,
            help = info.help_id
        );
        s += 1;
    }
    for (k, it, info) in &rows {
        let (sid, color, align, x) = match (it, info) {
            (Item::Header(g), _) => (STR_GROUP + *g as u32, HEADER_COLOR, 2, 189 + dx - CAPTION_PAD),
            (_, Some(i)) => (i.name_id, if i.inactive { INACTIVE_COLOR } else { "0xff000000" }, 2, 189 + dx - CAPTION_PAD),
            _ => continue,
        };
        let _ = write!(
            out,
            "\n[section.{s}]\nid=REAL.LABEL\nname=ID_LABEL\nstate=normal\npos={x},{ly}\nfontPathName=univers8b.ffn\nforeColor={color}\nbackColor=0x00000000\nstringID={sid}\nalign={align}\ntranson=FADE\ntransoff=SAME\n",
            ly = y_of(*k) + 2
        );
        s += 1;
    }
    for (k, _, info) in &rows {
        if info.is_none() {
            continue;
        }
        let _ = write!(
            out,
            "\n[section.{s}]\nid=NFS5.GOURAUDBOX\nname=ID_GOURAUDBOX\nstate=normal\npos={x},{by}\nsize=230,13\ncUL=0x00ffffff\ncUR=0x80ffffff\ncBL=0x00ffffff\ncBR=0x80ffffff\ntranson=FADE\ntransoff=SAME\nULoffX=0\nULoffY=0\nURoffX=0\nURoffY=0\nBLoffX=0\nBLoffy=0\nBRoffX=0\nBRoffY=0\nswrenderoff=0\n",
            x = 96 + dx - 137,
            by = y_of(*k) + 1
        );
        s += 1;
    }
    let n_pages = pages.len();
    if n_pages > 1 {
        let (next, caption) = if page + 1 < n_pages { (page + 2, STR_MORE) } else { (1, STR_FIRST) };
        let _ = write!(
            out,
            "\n[section.{s}]\nid=NFS5.GOTOBUTTON\nname=ID_ENHMOREBUTTON\nstate=normal\npos={x},{BTN_Y}\nstringID={caption}\ndestination=\nstemleftheight=0\nstemrightheight=0\noffset=0\ncallback={CB_PAGE}{next}\ntranson=FADE\ntransoff=SAME\nwidth=126\nheight=16\nmedium=4\ndropshadow=2\nmaincolor=0x00000000\nhovercolor=0x00000000\ndisabledcolor=0x30FF0000\nglintybox=1\n",
            x = BTN_XS[3]
        );
    }
    out
}

/// Whether a layout name is the port's own panel.
pub fn is_screen(name: &str) -> bool {
    page_of(name).is_some()
}

fn is_named(name: &str, want: &str) -> bool {
    name.trim_end_matches(".lay").trim_end_matches(".LAY").eq_ignore_ascii_case(want)
}

/// Whether a layout name is `GraphicsTabs.lay` (the graphics tab's sub-tab strip, which gets the third button).
pub fn is_graphics_tabs(name: &str) -> bool {
    is_named(name, "graphicstabs")
}

/// Whether a layout name is `graphics.lay` (the graphics tab, which gets the panel).
pub fn is_graphics(name: &str) -> bool {
    is_named(name, "graphics")
}

/// The splice file a load adds to the original layout `name`, if any: the sub-tab's button into
/// `GraphicsTabs.lay`, its panel into `graphics.lay` (read from [`layouts_dir`]).
pub fn splice_file(name: &str) -> Option<&'static str> {
    if is_graphics_tabs(name) {
        Some("graphicstabs_entry.lay")
    } else if is_graphics(name) {
        Some("graphics_entry.lay")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ENHANCEMENT E41: with no pack found (nothing sets the flag in a test run) the row is disabled, its hint the reason, its level reads off.
    #[test]
    fn the_textures_row_is_grey_without_a_pack() {
        let e = Enhancements::all();
        let i = nfs_renderer::E41.0 as usize;
        assert_eq!(e.level(nfs_renderer::E41), 1);
        assert_eq!(e.shown_level(nfs_renderer::E41), 0);
        let info = line_info(Item::Entry(i), &e).unwrap();
        assert!(info.inactive);
        let b = e.blocked("E41").unwrap();
        assert_eq!(info.help_id, STR_REASON + b.dep as u32 * 3 + b.by as u32);
        assert!(reason_text(&nfs_renderer::ENHANCEMENT_DEPENDS[b.dep], b.by, true).contains("hd_textures"));
    }

    #[test]
    fn an_old_file_with_the_photo_switch_still_loads() {
        // 2026-10-27: E30 left the registry and every later index moved; the file names tokens, so the other levels
        // load as saved and "E30" is ignored
        let e = parse_file(r#"{"version":6,"levels":{"E7":2,"E29":1,"E30":1,"E31":1,"E40":0,"E44":0}}"#);
        assert_eq!((e.level(E7), e.level(E29), e.level(E31), e.level(E40), e.level(E44)), (2, 1, 1, 0, 0));
        assert_eq!(e.level(E32), ENHANCEMENT_REGISTRY[E32.0 as usize].default_level);
        assert!(!file_text(&e).contains("\"E30\""));
    }

    /// the ids the layouts name literally and a sample of every computed kind, as they were before the
    /// compile-time range checks (a saved layout or a string table must not move).
    #[test]
    fn the_string_ids_are_pinned() {
        assert_eq!([STR_TITLE, STR_RESET, STR_NONE_YET, STR_HINT, STR_ENTRY, STR_DEFAULTS, STR_MORE, STR_FIRST, STR_LITE], [900_001, 900_002, 900_003, 900_004, 900_005, 900_006, 900_007, 900_008, 900_009]);
        assert_eq!((STR_NAME + 3, STR_HELP + 43), (900_103, 900_243));
        assert_eq!(level_ids(0), &[900_300, 900_301, 900_302, 900_303]);
        assert_eq!(level_ids(2), &[900_316, 900_317, 900_318, 900_319, 900_320, 900_321]);
        // the per-row ids follow the registry index (no layout names them): E38 is 36 since E30 left the registry
        // (2026-10-27; it was 37 with ids 900_596..)
        let e38 = nfs_renderer::E38.0 as usize;
        assert_eq!(e38, 36);
        let want: &[u32] = if cfg!(target_os = "macos") { &[900_588, 900_589, 900_590] } else { &[900_588, 900_590] };
        assert_eq!(level_ids(e38), want);
        assert_eq!((VRow::MfxMode.name_id(), VRow::FsrScale.help_id(), VRow::FsrScale.level_id(2)), (903_000, 903_012, 903_122));
        assert_eq!((STR_GROUP + 1, STR_REASON + 5 * 3 + 2), (903_301, 903_417));
    }

    #[test]
    fn env_over_file_over_default() {
        // empty registry: every set is the same set, but the mode tells which rule fired
        assert_eq!(load_with(None, None).1, Mode::Normal);
        assert_eq!(load_with(Some("off"), None), (Enhancements::ORIGINAL, Mode::HardOriginal));
        assert_eq!(load_with(Some("all"), None).1, Mode::Normal);
        assert_eq!(load_with(Some("E1,E3"), None).1, Mode::Normal);
        assert_eq!(load_with(Some(""), None).1, Mode::Normal);
    }

    /// A file written while E45 (normal maps, removed 2026-10-05) existed still loads: its token is unknown now and ignored, every
    /// other entry keeps its stored level.
    #[test]
    fn a_stored_removed_token_is_ignored() {
        let with = parse_file(r#"{"version": 6, "levels": {"E41": 2, "E45": 3, "E42": 0}}"#);
        let without = parse_file(r#"{"version": 6, "levels": {"E41": 2, "E42": 0}}"#);
        assert_eq!(with, without);
        assert_eq!(with.level(nfs_renderer::E41), 1);
        assert_eq!(with.level(nfs_renderer::E42), 0);
    }

    /// ENH-SAVE: changed levels survive a save and a fresh load through a real file — every entry (every panel
    /// page, the platform-only ones where available) at its highest level, then each at 0, then a mixed set.
    #[test]
    fn changed_levels_survive_save_and_load() {
        let dir = std::env::temp_dir().join(format!("nfs-enh-save-{}", std::process::id()));
        let path = dir.join("enhancements.json");
        let mut top = Enhancements::all();
        let mut zero = Enhancements::all();
        let mut mixed = Enhancements::all();
        for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate() {
            let s = nfs_renderer::EnhancementSwitch(i as u8);
            if en.available() {
                top.set_level(s, en.levels.len() as u8 - 1);
                zero.set_level(s, 0);
                // every second entry off, every third at its top: differs from the defaults in both directions
                mixed.set_level(s, if i % 2 == 0 { 0 } else if i % 3 == 0 { en.levels.len() as u8 - 1 } else { en.default_level });
            }
        }
        for e in [top, zero, mixed] {
            save_to(&path, &e).unwrap();
            let (back, mode) = load_with(None, std::fs::read_to_string(&path).ok().as_deref());
            assert_eq!((back, mode), (e, Mode::Normal));
            // a second save of what was loaded is the same file (no drift to the defaults)
            save_to(&path, &back).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), file_text(&e));
        }
        // every entry is in the file (none falls back to its default for want of a key)
        let text = file_text(&Enhancements::all());
        for en in ENHANCEMENT_REGISTRY {
            assert!(text.contains(&format!("\"{}\"", en.token)), "{} missing from the file", en.token);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENH-SAVE: F12 is a view — off, then back to the player's own levels, not to the defaults; and from an all-off
    /// player set it shows the defaults and returns to off.
    #[test]
    fn the_ab_key_returns_the_players_set() {
        let mut player = Enhancements::all();
        player.set_level(nfs_renderer::E1, 3);
        player.set_level(nfs_renderer::E9, 0);
        let (shown, stash) = ab_toggle(player, None);
        assert_eq!(shown, Enhancements::ORIGINAL);
        let (shown, stash) = ab_toggle(shown, stash);
        assert_eq!((shown, stash), (player, None));
        let (shown, stash) = ab_toggle(Enhancements::ORIGINAL, None);
        assert_eq!(shown, Enhancements::all());
        assert_eq!(ab_toggle(shown, stash), (Enhancements::ORIGINAL, None));
    }

    #[test]
    fn e34_old_spatial_levels_are_renumbered() {
        // ENHANCEMENT E34: a file written before the temporal levels (version 2) has the spatial 1..3; they are 5..7 now
        let get = |text: &str| parse_file(text).level(nfs_renderer::E34);
        let mac = cfg!(target_os = "macos");
        for (old, new) in [(0u8, 0u8), (1, 5), (2, 6), (3, 7)] {
            assert_eq!(get(&format!(r#"{{"version":2,"levels":{{"E34":{old}}}}}"#)), if mac { new } else { 0 });
            assert_eq!(get(&format!(r#"{{"levels":{{"E34":{old}}}}}"#)), if mac { new } else { 0 });
        }
        // version 3 is stored as it is (1 = the temporal smoothing-only level)
        assert_eq!(get(r#"{"version":3,"levels":{"E34":1}}"#), if mac { 1 } else { 0 });
        assert_eq!(get(r#"{"version":3,"levels":{"E34":7}}"#), if mac { 7 } else { 0 });
        // the writer is version 7
        assert!(file_text(&Enhancements::all()).contains("\"version\": 7"));
        // ENHANCEMENT E41: a file written before off / on (version <= 6): any level above 0 is on, 0 stays off
        let e41 = |t: &str| parse_file(t).level(nfs_renderer::E41);
        assert_eq!(e41(r#"{"version":5,"levels":{"E41":1}}"#), 1);
        assert_eq!(e41(r#"{"version":5,"levels":{"E41":0}}"#), 0);
        assert_eq!(e41(r#"{"version":6,"levels":{"E41":1}}"#), 1);
        assert_eq!(e41(r#"{"version":6,"levels":{"E41":2}}"#), 1);
        assert_eq!(e41(r#"{"version":7,"levels":{"E41":0}}"#), 0);
        assert_eq!(e41(r#"{"version":5,"levels":{"E1":1}}"#), 1); // not named: the default (on)
        // ENHANCEMENT ENH-COAT: E8's stored levels of a file written before the clear coat (version <= 4): low (1) -> normal (2), high (2) -> new (1);
        // 0 stays off, a version-5 file is read as it is
        let e8 = |text: &str| parse_file(text).level(nfs_renderer::E8);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":1}}"#), 2);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":2}}"#), 1);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":0}}"#), 0);
        assert_eq!(e8(r#"{"version":3,"levels":{"E8":2}}"#), 1);
        assert_eq!(e8(r#"{"version":5,"levels":{"E8":1}}"#), 1);
        assert_eq!(e8(r#"{"version":5,"levels":{"E8":3}}"#), 3);
        // a file with no E8 is at the default (normal)
        assert_eq!(e8(r#"{"version":4,"levels":{"E1":1}}"#), 2);
    }

    #[test]
    fn the_file_round_trips() {
        let e = Enhancements::all();
        assert_eq!(parse_file(&file_text(&e)), e);
        assert_eq!(parse_file("not json"), Enhancements::all());
        assert_eq!(parse_file("{}"), Enhancements::all());
        // every level of every entry survives
        // (an entry this platform has no backend for - E33 / E34 off macOS, E37 on macOS - cannot be set: it stays 0, see `Entry::available`)
        for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate().filter(|(_, e)| e.available()) {
            for l in 0..en.levels.len() as u8 {
                let mut x = Enhancements::all();
                x.set_level(nfs_renderer::EnhancementSwitch(i as u8), l);
                assert_eq!(parse_file(&file_text(&x)), x);
            }
        }
        // an absent token is at its default
        let d = parse_file(r#"{"version":2,"levels":{"E1":0}}"#);
        let mut want = Enhancements::all();
        want.set_level(nfs_renderer::E1, 0);
        assert_eq!(d, want);
        // the first (boolean) format: true = default level, false = off
        let old = parse_file(r#"{"version":1,"switches":{"E1":false,"E2":true,"E3":true}}"#);
        assert_eq!(old.level(nfs_renderer::E1), 0);
        assert_eq!(old.level(nfs_renderer::E2), ENHANCEMENT_REGISTRY[1].default_level);
        assert_eq!(old.level(nfs_renderer::E3), ENHANCEMENT_REGISTRY[2].default_level);
        assert_eq!(old.level(nfs_renderer::E4), ENHANCEMENT_REGISTRY[3].default_level);
        // a level past the choices is clamped
        assert_eq!(parse_file(r#"{"levels":{"E1":99}}"#).level(nfs_renderer::E1) as usize, ENHANCEMENT_REGISTRY[0].levels.len() - 1);
    }

    #[test]
    fn nfs_enh_override_forms() {
        use nfs_renderer::{E1, E3};
        assert_eq!(load_with(Some("all"), None).0, Enhancements::all());
        let (e, m) = load_with(Some("E1,E3"), Some(r#"{"levels":{"E2":1}}"#));
        assert_eq!(m, Mode::Normal);
        assert_eq!((e.level(E1), e.level(E3)), (ENHANCEMENT_REGISTRY[0].default_level, ENHANCEMENT_REGISTRY[2].default_level));
        assert!(!e.is_on(nfs_renderer::E2)); // the file is not read under an override
        let (e, _) = load_with(Some("E1=3,E3=0"), None);
        assert_eq!((e.level(E1), e.level(E3)), (3, 0));
        // no override: the file, else the defaults
        assert_eq!(load_with(None, Some(r#"{"levels":{"E1":1}}"#)).0.level(E1), 1);
        assert_eq!(load_with(None, None).0, Enhancements::all());
    }

    #[test]
    fn combo_rows_are_the_level_captions() {
        for (i, e) in ENHANCEMENT_REGISTRY.iter().enumerate() {
            // the upscaler row lists only this platform's backends (no MetalFX off macOS)
            let want = if i == nfs_renderer::E38.0 as usize { nfs_renderer::upscaler_choices().len() } else { e.levels.len() };
            assert_eq!(level_ids(i).len(), want);
        }
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        assert_eq!(s.get(level_ids(0)[0]), Some("off"));
        assert_eq!(s.get(level_ids(0)[2]), Some("4x"));
        assert!(s.get(STR_DEFAULTS).is_some());
    }

    /// The sets the panel is checked for: every upscaler choice (MetalFX only where the platform has it), with HDR off too.
    fn panel_sets() -> Vec<Enhancements> {
        let mut v = Vec::new();
        for &up in nfs_renderer::upscaler_choices() {
            for hdr in [1u8, 0] {
                let mut e = Enhancements::all();
                e.set_level(nfs_renderer::E38, up);
                e.set_level(nfs_renderer::E7, hdr);
                v.push(e);
            }
        }
        v
    }

    fn lines_of(pages: &[Vec<Item>]) -> Vec<Item> {
        pages.iter().flatten().copied().collect()
    }

    #[test]
    fn every_entry_has_a_row_or_is_an_upscaler_setting() {
        let mut seen: Vec<&str> = GROUPS.iter().flat_map(|(_, t)| t.iter().map(|s| s.token())).chain(SETTINGS_OF_E38.iter().map(|s| s.token())).collect();
        for e in ENHANCEMENT_REGISTRY {
            assert_eq!(seen.iter().filter(|t| **t == e.token).count(), 1, "{} is in the panel exactly once", e.token);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ENHANCEMENT_REGISTRY.len(), "a token of the panel that is not in the registry");
    }

    #[test]
    fn only_the_chosen_upscaler_has_settings_rows() {
        let rows = |up: u8| {
            let mut e = Enhancements::all();
            e.set_level(nfs_renderer::E38, up);
            lines_of(&panel_pages(&e))
        };
        let has = |l: &[Item], it: Item| l.contains(&it);
        let e39 = Item::Entry(nfs_renderer::E39.0 as usize);
        // off: neither backend's settings; E34 / E37 / E39 never have a row of their own
        let off = rows(nfs_renderer::UP_OFF);
        assert!(!has(&off, Item::Virtual(VRow::MfxMode)) && !has(&off, Item::Virtual(VRow::MfxScale)) && !has(&off, Item::Virtual(VRow::FsrScale)) && !has(&off, e39));
        for k in [nfs_renderer::E34, nfs_renderer::E37] {
            assert!(!off.contains(&Item::Entry(k.0 as usize)));
        }
        // FSR: its scale and sharpness, not MetalFX's
        let fsr = rows(nfs_renderer::UP_FSR);
        assert!(has(&fsr, Item::Virtual(VRow::FsrScale)) && has(&fsr, e39) && !has(&fsr, Item::Virtual(VRow::MfxMode)));
        // the settings follow the choice row at once
        let at = |l: &[Item], it: Item| l.iter().position(|x| *x == it).unwrap();
        let up = Item::Entry(nfs_renderer::E38.0 as usize);
        assert_eq!((at(&fsr, Item::Virtual(VRow::FsrScale)), at(&fsr, e39)), (at(&fsr, up) + 1, at(&fsr, up) + 2));
        if cfg!(target_os = "macos") {
            let mfx = rows(nfs_renderer::UP_METALFX);
            assert!(has(&mfx, Item::Virtual(VRow::MfxMode)) && has(&mfx, Item::Virtual(VRow::MfxScale)) && !has(&mfx, Item::Virtual(VRow::FsrScale)) && !has(&mfx, e39));
            assert_eq!(at(&mfx, Item::Virtual(VRow::MfxScale)), at(&mfx, up) + 2);
        } else {
            // no MetalFX off macOS: a stored choice 1 shows no settings
            assert!(!has(&rows(nfs_renderer::UP_METALFX), Item::Virtual(VRow::MfxMode)));
        }
    }

    /// Every available entry shows once, a group is never split over two pages while it fits on one, no page is over the limit.
    #[test]
    fn pages_keep_groups_whole_and_cover_every_row_once() {
        for e in panel_sets() {
            let groups = group_items(&e);
            let pages = panel_pages(&e);
            assert!(pages.iter().all(|p| !p.is_empty() && p.len() <= MAX_ROWS));
            // every line once, in group order
            assert_eq!(lines_of(&pages), groups.iter().flatten().copied().collect::<Vec<_>>());
            for g in &groups {
                if g.len() <= MAX_ROWS {
                    let on: Vec<usize> = pages.iter().enumerate().filter(|(_, p)| p.contains(&g[0])).map(|(i, _)| i).collect();
                    assert_eq!(on.len(), 1);
                    assert!(g.iter().all(|it| pages[on[0]].contains(it)), "group {:?} is split", g[0]);
                }
            }
            // every available entry that has a row appears; the headers are the groups'
            let lines = lines_of(&pages);
            for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate().filter(|(_, e)| e.available() && !SETTINGS_OF_E38.iter().any(|s| s.token() == e.token)) {
                assert_eq!(lines.iter().filter(|it| **it == Item::Entry(i)).count(), 1, "{}", en.token);
            }
            assert_eq!(lines.iter().filter(|it| matches!(it, Item::Header(_))).count(), GROUPS.len());
        }
        // a group longer than a page is split, a short one starts a new page instead
        let long: Vec<Item> = (0..20).map(Item::Entry).collect();
        let short: Vec<Item> = (0..5).map(Item::Entry).collect();
        assert_eq!(paginate(&[long.clone()], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![16, 4]);
        assert_eq!(paginate(&[short.clone(), short.clone(), short.clone(), short.clone()], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![15, 5]);
        assert_eq!(paginate(&[short.clone(), long], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![5, 16, 4]);
        assert_eq!(paginate(&[], 16), vec![Vec::<Item>::new()]);
        assert_eq!(pages_for(MAX_ROWS * 2 + 6), 3);
        assert_eq!(pages_for(0), 1);
    }

    #[test]
    fn the_rows_go_ahead_of_the_backgrounds() {
        let file = "[section.0]\nid=REAL.BLACK_640x480\n\n[section.1]\nid=NFS5.GOURAUDBOX\n";
        let t = with_rows(file, "\n[section.900]\nid=NFS5.ComboBoxSlider\n");
        assert!(t.find("ComboBoxSlider").unwrap() < t.find("GOURAUDBOX").unwrap());
        assert!(t.starts_with("[section.0]"));
    }

    #[test]
    fn generated_rows_have_the_lines_of_the_page() {
        for e in panel_sets() {
            let pages = panel_pages(&e);
            let mut combos = 0;
            for (page, lines) in pages.iter().enumerate() {
                let rows = generated_rows_in(&e, page);
                let n = lines.iter().filter(|it| !matches!(it, Item::Header(_))).count();
                assert_eq!(rows.matches("NFS5.ComboBoxSlider").count(), n);
                assert_eq!(rows.matches("helptext=").count(), n);
                // a caption label per line (headers too), a fade bar per combo
                assert_eq!(rows.matches("id=REAL.LABEL").count(), lines.len());
                assert_eq!(rows.matches("NFS5.GOURAUDBOX").count(), n);
                // the page button exactly when there is more than one page, walking on and back to the first
                assert_eq!(rows.contains("ID_ENHMOREBUTTON"), pages.len() > 1);
                if pages.len() > 1 {
                    assert!(rows.contains(&format!("callback={CB_PAGE}{}", if page + 1 < pages.len() { page + 2 } else { 1 })));
                }
                combos += n;
            }
            assert!(combos > 0);
        }
        // the accessors: entries `Enh.<token>`, the settings rows their own
        let mut e = Enhancements::all();
        e.set_level(nfs_renderer::E38, nfs_renderer::UP_FSR);
        let all: String = (0..panel_pages(&e).len()).map(|p| generated_rows_in(&e, p)).collect();
        assert!(all.contains("accessor=Enh.FSRSCALE") && all.contains("accessor=Enh.E39") && all.contains("accessor=Enh.E38") && !all.contains("accessor=Enh.E34") && !all.contains("accessor=Enh.E37"));
    }

    /// An inactive row is the engine's disabled state with the reason as its hover help; its stored level is kept and it comes back.
    #[test]
    fn inactive_rows_say_why_and_keep_their_level() {
        let mut e = Enhancements::all();
        let bloom = ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E10").unwrap();
        let info = |e: &Enhancements| line_info(Item::Entry(bloom), e).unwrap();
        assert!(!info(&e).inactive);
        assert_eq!(info(&e).help_id, STR_HELP + bloom as u32);
        let level = e.level(nfs_renderer::E10);
        e.set_level(nfs_renderer::E7, 0);
        let i = info(&e);
        assert!(i.inactive);
        assert!(i.help_id >= STR_REASON && i.help_id != STR_HELP + bloom as u32);
        assert_eq!(e.level(nfs_renderer::E10), level, "the stored level is kept");
        // the page shows it disabled, grey, with the reason id as its help
        let page: String = (0..panel_pages(&e).len()).map(|p| generated_rows_in(&e, p)).collect();
        assert!(page.contains("state=disabled") && page.contains(INACTIVE_COLOR) && page.contains(&format!("helptext={}", i.help_id)));
        // switching the cause back restores the row
        e.set_level(nfs_renderer::E7, 1);
        assert!(!info(&e).inactive);
        // the hint names the setting that disables it
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        e.set_level(nfs_renderer::E7, 0);
        assert!(s.get(info(&e).help_id).is_some_and(|t| t.contains("HDR and tone mapping")), "{:?}", s.get(info(&e).help_id));
        // the upscaler overrides name the backend
        let (render_scale, msaa) = (ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E3").unwrap(), ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E1").unwrap());
        let mut f = Enhancements::all();
        f.set_level(nfs_renderer::E38, nfs_renderer::UP_FSR);
        assert!(line_info(Item::Entry(render_scale), &f).unwrap().inactive && line_info(Item::Entry(msaa), &f).unwrap().inactive);
        assert!(s.get(line_info(Item::Entry(render_scale), &f).unwrap().help_id).is_some_and(|t| t.contains("FSR 3.1")));
        // the lines change with the set (a rebuild is needed) exactly when they differ
        assert_ne!(panel_signature(&Enhancements::all()), panel_signature(&f));
        assert_eq!(panel_signature(&f), panel_signature(&f));
    }

    #[test]
    fn every_reason_and_virtual_row_has_a_string() {
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        for k in 0..nfs_renderer::ENHANCEMENT_DEPENDS.len() as u32 * 3 {
            assert!(s.get(STR_REASON + k).is_some_and(|t| !t.is_empty()), "reason {k}");
        }
        for v in VRow::ALL {
            assert!(s.get(v.name_id()).is_some() && s.get(v.help_id()).is_some());
            assert!(vrow_ids(v).iter().all(|id| s.get(*id).is_some()));
        }
        for g in 0..GROUPS.len() as u32 {
            assert!(s.get(STR_GROUP + g).is_some());
        }
        // the upscaler row lists this platform's backends, off first
        let ids = level_ids(nfs_renderer::E38.0 as usize);
        assert_eq!(ids.len(), nfs_renderer::upscaler_choices().len());
        assert_eq!(s.get(ids[0]), Some("off"));
        assert_eq!(s.get(*ids.last().unwrap()), Some("FSR 3.1"));
        // the simple MetalFX mode lists the three scales after 1:1
        assert_eq!(vrow_ids(VRow::MfxScale).len(), 4);
        assert_eq!(vrow_ids(VRow::MfxMode).len(), 2);
    }

    /// The settings file: a file written before the upscaler choice (version <= 3) turns its stored backend on; version 4 is read as it is.
    #[test]
    fn old_files_choose_their_upscaler() {
        use nfs_renderer::{E34, E37, E38, E39, UP_FSR, UP_METALFX, UP_OFF};
        let mac = cfg!(target_os = "macos");
        // MetalFX stored on (v3: temporal 59 %)
        let m = parse_file(r#"{"version":3,"levels":{"E34":3}}"#);
        assert_eq!((m.level(E38), m.level(E34)), if mac { (UP_METALFX, 3) } else { (UP_OFF, 0) });
        assert_eq!(m.effective().level(E34), if mac { 3 } else { 0 });
        // the first spatial levels (v2: 1..3 = 5..7) and the choice
        let sp = parse_file(r#"{"version":2,"levels":{"E34":2}}"#);
        assert_eq!((sp.level(E38), sp.level(E34)), if mac { (UP_METALFX, 6) } else { (UP_OFF, 0) });
        // FSR stored on (v3): the choice is FSR, the scale stays, sharpness = the old constant
        let f = parse_file(r#"{"version":3,"levels":{"E37":4}}"#);
        assert_eq!((f.level(E38), f.level(E37), f.level(E39)), (UP_FSR, 4, 2));
        assert_eq!(nfs_renderer::fsr_sharpness(f.level(E39)), Some(0.3));
        // neither on: the choice stays off
        let off = parse_file(r#"{"version":3,"levels":{"E34":0,"E37":0}}"#);
        assert_eq!(off.level(E38), UP_OFF);
        // version 4: E38 as stored, hidden settings kept (FSR chosen, E34 left over)
        let v4 = parse_file(r#"{"version":4,"levels":{"E38":2,"E34":3,"E37":3,"E39":4}}"#);
        assert_eq!((v4.level(E38), v4.level(E37), v4.level(E39)), (UP_FSR, 3, 4));
        assert_eq!(v4.level(E34), if mac { 3 } else { 0 });
        assert_eq!((v4.effective().level(E37), v4.effective().level(E34)), (3, 0));
        let off4 = parse_file(r#"{"version":4,"levels":{"E38":0,"E37":3}}"#);
        assert_eq!((off4.level(E38), off4.level(E37), off4.effective().level(E37)), (UP_OFF, 3, 0));
        // a version-4 file written from a set round-trips exactly (E38 / E39 in it)
        let mut e = Enhancements::all();
        e.set_level(E38, UP_FSR);
        e.set_level(E37, 3);
        e.set_level(E39, 3);
        assert_eq!(parse_file(&file_text(&e)), e);
    }

    #[test]
    fn the_strings_follow_the_install_language() {
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "выкл".chars().map(|c| typed_char(c).unwrap()).collect());
        assert!(is_russian(&s));
        let s = with_extras(s);
        assert_eq!(s.get(STR_RESET).map(|t| t.chars().count()), Some("как в оригинале".chars().count()));
        // Windows-1251 bytes carried as cp1252 chars: «к» is 0xEA
        assert_eq!(s.get(STR_RESET).and_then(|t| t.chars().next()), Some(nfs_formats::locale::cp1252(0xEA)));
        let mut e = Strings::default();
        e.fe_by_id.insert(1500, "off".into());
        assert!(!is_russian(&e));
        assert_eq!(with_extras(e).get(STR_RESET), Some("as in the original"));
    }

    #[test]
    fn screen_names() {
        assert!(is_screen("Enhancements") && is_screen("enhancements.lay") && !is_screen("options"));
        assert_eq!(splice_file("GraphicsTabs"), Some("graphicstabs_entry.lay"));
        assert_eq!(splice_file("graphics.lay"), Some("graphics_entry.lay"));
        assert_eq!(splice_file("graphicsettings"), None);
        assert_eq!(splice_file("options"), None);
    }
}
