//! PORT-DEVIATION D47 (`RE/notes/PORT-DEVIATIONS.md`): the port's display options and their sub-tab. **Not the original's**:
//! `nfs5.exe` has no such screen. The options are the third sub-tab of Options > graphics, «дополнительно» / «advanced»: how the
//! frame is put on the screen (frame output, frame-rate limit) and how the 3D picture is sampled (multisampling, anisotropic
//! filtering, render scale). Every option's level 0 is the original's look and the port's previous behaviour, so a missing
//! settings file (`display.json`) is the game as it was. The original's own graphics screens (`graphicsettings`, `advgraphics`)
//! are untouched.
//!
//! Nothing here reads the original's binary. The panel is a port-own layout (`layouts/displayopts.lay` + the rows generated from
//! [`REGISTRY`]) built from widget types the original's layouts use, embedded in the binary and read by the same layout engine
//! (`layout::load_one`); the tab button and the panel are spliced into `GraphicsTabs.lay` / `graphics.lay` at load
//! ([`splice_file`]). The strings are the port's own table (ids from [`STRING_BASE`]), English and Russian by the install's language.
//!
//! The options are render-side and pacing-side only: nothing reaches the simulation, a replay or the network.

use nfs_formats::locale::Strings;
use nfs_renderer::{Enhancements, EnhancementSwitch};
use std::path::PathBuf;

/// One option's row.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// the key in `display.json`
    pub token: &'static str,
    /// the row's caption, `[English, Russian]`
    pub name: [&'static str; 2],
    /// the one-line help of the row (the combo's hover text), `[English, Russian]`
    pub help: [&'static str; 2],
    /// the choices, `[English, Russian]` captions in the combo's order; index 0 is the original's behaviour
    pub levels: &'static [[&'static str; 2]],
    /// the level a missing file gives (0 everywhere: the original's look)
    pub default_level: u8,
}

/// Index of the options in [`REGISTRY`] and in [`Settings`].
pub const FRAME_OUTPUT: usize = 0;
pub const FPS_LIMIT: usize = 1;
pub const MSAA: usize = 2;
pub const ANISO: usize = 3;
pub const RENDER_SCALE: usize = 4;

/// The options in menu order.
pub const REGISTRY: &[Entry] = &[
    Entry {
        token: "frame_output",
        name: ["frame output (vsync)", "вывод кадров (vsync)"],
        help: [
            "mailbox: latest frame, short queue, no tearing (not offered by Metal on macOS: then = off); immediate: no vsync, tearing, the front end is capped at 120 fps",
            "mailbox: свежий кадр, короткая очередь, без разрывов (на macOS Metal его нет: тогда = выкл); immediate: без vsync, возможны разрывы, меню не быстрее 120 кадров/с",
        ],
        levels: &[["off (as now)", "выкл (как сейчас)"], ["mailbox (low lag)", "mailbox (мал. задержка)"], ["immediate (no vsync)", "immediate (без vsync)"]],
        default_level: 0,
    },
    Entry {
        token: "fps_limit",
        name: ["frame rate limit", "ограничение FPS"],
        help: [
            "holds the game to at most this many frames a second (frame pacing only: the simulation keeps its 64 Hz tick); \"display\" is the screen's own refresh rate (with the frame output on «off» the display's sync already does that); \"auto\" is 60 on the battery and \"display\" on mains power. The NFS_FPS_CAP variable still wins",
            "ограничивает игру этим числом кадров в секунду (только темп кадров: симуляция остаётся на своих 64 Гц); «по экрану» - частота обновления самого экрана (при выводе кадров «выкл» это уже делает синхронизация экрана); «авто» - 60 от батареи и «по экрану» от сети. Переменная NFS_FPS_CAP главнее",
        ],
        levels: &[["off", "выкл"], ["30", "30"], ["60", "60"], ["120", "120"], ["240", "240"], ["display", "по экрану"], ["auto (60 on battery)", "авто (60 от батареи)"]],
        default_level: 0,
    },
    Entry {
        token: "msaa",
        name: ["anti-aliasing (MSAA)", "сглаживание (MSAA)"],
        help: [
            "smooths jagged edges: the 3D picture is drawn with several samples a pixel (the menu and the HUD are not touched); costs speed, 8x the most. A count the graphics card does not support falls back to the next lower",
            "сглаживает рваные края: 3D-картинка рисуется с несколькими выборками на пиксель (меню и индикаторы не затрагиваются); замедляет, 8x сильнее всего. Если видеокарта не поддерживает выбранное число, берётся ближайшее меньшее",
        ],
        levels: &[["off", "выкл"], ["2x", "2x"], ["4x", "4x"], ["8x", "8x"]],
        default_level: 0,
    },
    Entry {
        token: "aniso",
        name: ["anisotropic filtering", "анизотропная фильтрация"],
        help: [
            "sharper textures at a slant (the road and the walls far ahead): trilinear filtering with up to 4x / 8x / 16x anisotropy, and a generated mip chain for the textures the original has one level of (the cars' pages, the sprites); off is the original's filtering",
            "чёткие текстуры под углом (дорога и стены вдали): трилинейная фильтрация с анизотропией до 4x / 8x / 16x и созданные уровни mip для текстур, у которых в оригинале один уровень (текстуры машин, спрайты); «выкл» - фильтрация оригинала",
        ],
        levels: &[["off", "выкл"], ["4x", "4x"], ["8x", "8x"], ["16x", "16x"]],
        default_level: 0,
    },
    Entry {
        token: "render_scale",
        name: ["render scale", "масштаб рендера"],
        help: [
            "the 3D picture is drawn at this share of the window's size and scaled to it (the menu and the HUD stay sharp): below 100 % faster and softer, above 100 % supersampled (sharper, slower)",
            "3D-картинка рисуется в этой доле размера окна и растягивается до него (меню и индикаторы остаются чёткими): меньше 100 % быстрее и мягче, больше 100 % суперсэмплинг (чётче, медленнее)",
        ],
        levels: &[["off (100 %)", "выкл (100 %)"], ["50 %", "50 %"], ["75 %", "75 %"], ["125 %", "125 %"], ["150 %", "150 %"], ["200 %", "200 %"]],
        default_level: 0,
    },
];

/// The most levels an option can have (the string ids reserve this many per row).
pub const MAX_LEVELS: usize = 8;

/// The chosen level of each option, in [`REGISTRY`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    levels: [u8; 8],
}

impl Default for Settings {
    /// Every option at its default level (the original's look).
    fn default() -> Self {
        let mut levels = [0u8; 8];
        for (i, e) in REGISTRY.iter().enumerate() {
            levels[i] = e.default_level;
        }
        Self { levels }
    }
}

impl Settings {
    /// The level of option `i`.
    pub fn level(&self, i: usize) -> u8 {
        self.levels.get(i).copied().unwrap_or(0)
    }

    /// Set option `i` to `level`, clamped to the option's levels.
    pub fn set_level(&mut self, i: usize, level: u8) {
        if let (Some(slot), Some(e)) = (self.levels.get_mut(i), REGISTRY.get(i)) {
            *slot = level.min(e.levels.len() as u8 - 1);
        }
    }

    /// Whether every option is at the original's level.
    pub fn is_original(&self) -> bool {
        (0..REGISTRY.len()).all(|i| self.level(i) == 0)
    }

    /// ENHANCED GAME (branch `remaster`): the set the enhanced game starts from when `display.json` is missing - the five rows at the
    /// levels of their enhancement entries' defaults (`Enhancements::all()`: MSAA 4x, anisotropic 16x, frame limit «auto», ...).
    pub fn enhanced_default() -> Self {
        Self::from_enhancements(&Enhancements::all())
    }

    /// ENHANCED GAME: the five rows' levels out of an enhancement set (the rows live in the enhancements panel there).
    pub fn from_enhancements(e: &Enhancements) -> Self {
        let mut s = Self::default();
        for (i, sw) in ENH_SWITCHES.iter().enumerate() {
            s.set_level(i, e.level(*sw));
        }
        s
    }

    /// ENHANCED GAME: put the five rows' levels into an enhancement set (the renderer and the pacer read them there).
    pub fn apply_to(&self, e: &mut Enhancements) {
        for (i, sw) in ENH_SWITCHES.iter().enumerate() {
            e.set_level(*sw, self.level(i));
        }
    }
}

/// ENHANCED GAME: the enhancement entry that is the row of each option in the enhancements panel (E6 frame output, E22 frame-rate
/// limit, E1 MSAA, E2 anisotropic filtering, E3 render scale), in [`REGISTRY`] order. Their levels are the options' levels one to one
/// (a test pins the captions); the values live in `display.json`, the entries are the rows' labels.
pub const ENH_SWITCHES: [EnhancementSwitch; 5] = [nfs_renderer::E6, nfs_renderer::E22, nfs_renderer::E1, nfs_renderer::E2, nfs_renderer::E3];

/// ENHANCED GAME: the display options of the enhanced game's start. `NFS_ENH` set (an explicit list or `off`) = that set decides and
/// `display.json` is not read. Otherwise `display.json` decides the five rows when it exists; when it does not, the five come from
/// `enhancements.json` if that exists (migrated once into a new `display.json`: the levels the player chose there are kept) or stay at
/// the enhanced game's defaults (nothing written). `set` is the set `remaster::load` gave. Returns whether `display.json` was read.
pub fn adopt(set: &mut Enhancements, env_chosen: bool, enhancements_file_exists: bool) -> bool {
    if env_chosen {
        return false;
    }
    match std::fs::read_to_string(settings_path()) {
        Ok(t) => {
            parse_file_over(&t, Settings::enhanced_default()).apply_to(set);
            true
        }
        Err(_) => {
            if enhancements_file_exists {
                let s = Settings::from_enhancements(set);
                tracing::info!(path = %settings_path().display(), "display.json is new: the five display rows migrated from enhancements.json");
                save(&s);
            }
            false
        }
    }
}

/// ENHANCED GAME: after a set was stored (`remaster::save`): `display.json` follows the five rows when they changed.
pub fn follow(set: &Enhancements) {
    let s = Settings::from_enhancements(set);
    let stored = std::fs::read_to_string(settings_path()).map(|t| parse_file_over(&t, Settings::enhanced_default())).ok();
    if stored != Some(s) {
        save(&s);
    }
}

// ---- storage ----

/// `display.json` - in `NFS5_SAVE_DIR` when that is set (a scripted run owns its directory and must not read the player's file),
/// else the port's data dir (`nfs_assets::platform::data_dir`). Never inside the `.sav`.
pub fn settings_path() -> PathBuf {
    let dir = std::env::var_os("NFS5_SAVE_DIR").map(PathBuf::from).unwrap_or_else(nfs_assets::platform::data_dir);
    dir.join("display.json")
}

/// The stored set; a missing or unreadable file is the defaults.
pub fn load() -> Settings {
    std::fs::read_to_string(settings_path()).map(|t| parse_file(&t)).unwrap_or_default()
}

/// The set a stored file gives, `{"version": 1, "levels": {"frame_output": 1, ...}}`: an option the file does not name is at its
/// default, a level out of range is clamped, an unknown key is ignored.
pub fn parse_file(text: &str) -> Settings {
    parse_file_over(text, Settings::default())
}

/// [`parse_file`] with the options the file does not name taken from `base` (the enhanced game's defaults are not level 0).
pub fn parse_file_over(text: &str, base: Settings) -> Settings {
    let mut s = base;
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return s };
    let Some(map) = v.get("levels").and_then(|l| l.as_object()) else { return s };
    for (i, e) in REGISTRY.iter().enumerate() {
        if let Some(l) = map.get(e.token).and_then(|l| l.as_u64()) {
            s.set_level(i, l.min(u8::MAX as u64) as u8);
        }
    }
    s
}

/// The stored form.
pub fn file_text(s: &Settings) -> String {
    let levels: serde_json::Map<String, serde_json::Value> = REGISTRY.iter().enumerate().map(|(i, e)| (e.token.to_string(), s.level(i).into())).collect();
    serde_json::to_string_pretty(&serde_json::json!({ "version": 1, "levels": levels })).unwrap_or_default() + "\n"
}

/// Save on change. Best effort: a read-only data dir loses the setting, not the game.
pub fn save(s: &Settings) {
    let path = settings_path();
    if let Err(err) = save_to(&path, s) {
        tracing::warn!(path = %path.display(), error = %err, "display.json not saved");
    }
}

/// [`save`] at a given path (the directory is created).
pub fn save_to(path: &std::path::Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, file_text(s))
}

// ---- the sub-tab: strings, layouts ----

/// First id of the port's own display-options strings (the original's ids stop below 20 000).
pub const STRING_BASE: u32 = 950_000;
/// The sub-tab's caption.
pub const STR_TAB: u32 = STRING_BASE + 1;
/// The «по умолчанию» button.
pub const STR_DEFAULTS: u32 = STRING_BASE + 2;
/// Row `i`'s caption is `STR_NAME + i`, its help `STR_HELP + i`.
pub const STR_NAME: u32 = STRING_BASE + 100;
pub const STR_HELP: u32 = STRING_BASE + 200;
/// Row `i`'s level `l` caption is `STR_LEVEL + i * MAX_LEVELS + l` (the combo's rows).
pub const STR_LEVEL: u32 = STRING_BASE + 300;

/// The accessor prefix of the rows: `PortDisplay.<token>`.
pub const ACCESSOR_PREFIX: &str = "PortDisplay.";
/// The callback of the «по умолчанию» button.
pub const CB_DEFAULTS: &str = "DSPDEFAULTS";
/// The callback prefix every button of the panel has (the application's hit path tests it).
pub const CB_PREFIX: &str = "DSP";
/// The panel's layout name (`filename=` of the `NFS5.PANEL` in `display_graphics.lay`) and its `NFS5.PANEL` name.
pub const SCREEN: &str = "displayopts";
pub const PANEL: &str = "ID_DISPLAYPANEL";

const TAB_ENTRY: &str = include_str!("../layouts/display_tab.lay");
const GRAPHICS_ENTRY: &str = include_str!("../layouts/display_graphics.lay");
const PANEL_BASE: &str = include_str!("../layouts/displayopts.lay");

/// The combo rows of option `i`: the string ids of its level captions, level 0 first.
pub fn level_ids(i: usize) -> Vec<u32> {
    let n = REGISTRY.get(i).map_or(0, |e| e.levels.len());
    (0..n).map(|l| STR_LEVEL + (i * MAX_LEVELS + l) as u32).collect()
}

/// The registry index of an `PortDisplay.<token>` accessor.
pub fn index_of(accessor: &str) -> Option<usize> {
    let tok = accessor.strip_prefix(ACCESSOR_PREFIX)?;
    REGISTRY.iter().position(|e| e.token == tok)
}

/// The install's strings with the port's display-options table added (Russian when the install's own text is, English otherwise:
/// `photo::is_russian`). The front end reads them by id like its own.
pub fn with_extras(mut s: Strings) -> Strings {
    let ru = crate::photo::is_russian(&s);
    let mut put = |id: u32, en: &str, rus: &str| {
        let t = crate::photo::as_game_text(if ru { rus } else { en });
        s.fe_by_id.insert(id, t.clone());
        s.by_id.insert(id, t);
    };
    put(STR_TAB, "advanced", "дополнительно");
    put(STR_DEFAULTS, "defaults", "по умолчанию");
    for (i, e) in REGISTRY.iter().enumerate() {
        put(STR_NAME + i as u32, e.name[0], e.name[1]);
        put(STR_HELP + i as u32, e.help[0], e.help[1]);
        for (l, id) in level_ids(i).into_iter().enumerate() {
            put(id, e.levels[l][0], e.levels[l][1]);
        }
    }
    s
}

// the rows' geometry: display.lay's (bar 63, combo 193..343) shifted to a centred column of 190-wide combos
const ROW_Y0: i32 = 124;
const ROW_STEP: i32 = 18;
const COL_DX: i32 = 137;

/// The sections appended to `displayopts.lay`: for each option a caption label and a level combo bound to its accessor, in the
/// geometry `display.lay` uses for its rows (combo width 190, the label right-aligned 4 px left of it, a fade bar behind the
/// label). The combo's `helptext` is the option's help (the original's tooltip, `FeWidget` 0x4C7FE2). Generated so that adding an
/// [`Entry`] adds a row.
pub fn generated_rows() -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let y_of = |k: usize| ROW_Y0 + ROW_STEP * k as i32;
    let mut s = 900;
    // the order display.lay uses: every combo, then every caption, then the fade bars over them
    for (k, e) in REGISTRY.iter().enumerate() {
        let _ = write!(
            out,
            "\n[section.{s}]\nid=NFS5.ComboBoxSlider\nname=ID_COMBOBOX\nstate=normal\npos={x},{y}\ndestination=\nstemleftheight=0\nstemrightheight=0\noffset=0\ncallback=\naccessor={ACCESSOR_PREFIX}{tok}\nwidth=190\nheight=15\nselectionboxwidth=\nselectionboxslidedirection=\nselectionboxmaxheight=160\nportpad=4\nhelptext={help}\ntranson=FADE\ntransoff=SAME\nnolabel=1\nusesmallfont=1\nGraphicsSpecialCaseWidth=\ndropshadow=1\nglintybox=2\nstringID=0\n",
            x = 193 + COL_DX,
            y = y_of(k),
            tok = e.token,
            help = STR_HELP + k as u32
        );
        s += 1;
    }
    for k in 0..REGISTRY.len() {
        let _ = write!(
            out,
            "\n[section.{s}]\nid=REAL.LABEL\nname=ID_LABEL\nstate=normal\npos={x},{ly}\nfontPathName=univers8b.ffn\nforeColor=0xff000000\nbackColor=0x00000000\nstringID={sid}\nalign=2\ntranson=FADE\ntransoff=SAME\n",
            x = 189 + COL_DX,
            ly = y_of(k) + 2,
            sid = STR_NAME + k as u32
        );
        s += 1;
    }
    for k in 0..REGISTRY.len() {
        let _ = write!(
            out,
            "\n[section.{s}]\nid=NFS5.GOURAUDBOX\nname=ID_GOURAUDBOX\nstate=normal\npos={x},{by}\nsize=230,13\ncUL=0x00ffffff\ncUR=0x80ffffff\ncBL=0x00ffffff\ncBR=0x80ffffff\ntranson=FADE\ntransoff=SAME\nULoffX=0\nULoffY=0\nURoffX=0\nURoffY=0\nBLoffX=0\nBLoffy=0\nBRoffX=0\nBRoffY=0\nswrenderoff=0\n",
            x = 96,
            by = y_of(k) + 1
        );
        s += 1;
    }
    out
}

fn is_named(name: &str, want: &str) -> bool {
    name.trim_end_matches(".lay").trim_end_matches(".LAY").eq_ignore_ascii_case(want)
}

/// Whether a layout name is the port's own panel.
pub fn is_screen(name: &str) -> bool {
    is_named(name, SCREEN)
}

/// The text of the port's own panel layout: the embedded base plus the generated rows.
pub fn screen_text() -> String {
    format!("{PANEL_BASE}\n{}", generated_rows())
}

/// The splice a load adds to the original layout `name`, if any: the sub-tab's button into `GraphicsTabs.lay`, its panel into
/// `graphics.lay` (embedded).
pub fn splice_file(name: &str) -> Option<&'static str> {
    if is_named(name, "graphicstabs") {
        Some(TAB_ENTRY)
    } else if is_named(name, "graphics") {
        Some(GRAPHICS_ENTRY)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_originals_look() {
        let d = Settings::default();
        assert!((0..REGISTRY.len()).all(|i| d.level(i) == 0));
        assert!(d.is_original());
        assert!(REGISTRY.iter().all(|e| e.default_level == 0 && e.levels.len() >= 2 && e.levels.len() <= MAX_LEVELS));
    }

    /// ENHANCED GAME: the five rows are the enhancement entries' rows one to one (same captions, same level count), and the enhanced
    /// default is the entries' defaults.
    #[test]
    fn the_rows_are_the_enhancement_entries() {
        for (i, e) in REGISTRY.iter().enumerate() {
            let en = &nfs_renderer::ENHANCEMENT_REGISTRY[ENH_SWITCHES[i].0 as usize];
            assert_eq!(en.levels, e.levels, "{}", e.token);
            assert_eq!(en.name, e.name, "{}", e.token);
        }
        let d = Settings::enhanced_default();
        assert_eq!((d.level(MSAA), d.level(ANISO), d.level(RENDER_SCALE), d.level(FRAME_OUTPUT), d.level(FPS_LIMIT)), (2, 3, 0, 0, 6));
        let mut e = Enhancements::ORIGINAL;
        d.apply_to(&mut e);
        assert_eq!(Settings::from_enhancements(&e), d);
        assert_eq!(e.level(nfs_renderer::E1), 2);
        assert!(Settings::from_enhancements(&Enhancements::ORIGINAL).is_original());
        // an option a stored file does not name is at the enhanced game's default, not at level 0
        let p = parse_file_over("{\"levels\": {\"msaa\": 1}}", d);
        assert_eq!((p.level(MSAA), p.level(ANISO), p.level(FPS_LIMIT)), (1, 3, 6));
    }

    #[test]
    fn the_file_round_trips_and_a_missing_one_is_the_defaults() {
        let mut s = Settings::default();
        s.set_level(FRAME_OUTPUT, 2);
        s.set_level(FPS_LIMIT, 3);
        assert_eq!(parse_file(&file_text(&s)), s);
        assert_eq!(parse_file(""), Settings::default());
        assert_eq!(parse_file("{\"levels\": {\"nope\": 1}}"), Settings::default());
        // a level out of range is clamped
        assert_eq!(parse_file("{\"levels\": {\"frame_output\": 99}}").level(FRAME_OUTPUT), 2);
        let dir = std::env::temp_dir().join(format!("display_opts_test_{}", std::process::id()));
        let path = dir.join("display.json");
        save_to(&path, &s).unwrap();
        assert_eq!(parse_file(&std::fs::read_to_string(&path).unwrap()), s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_string_ids_are_distinct_and_the_rows_have_text() {
        let mut ids = vec![STR_TAB, STR_DEFAULTS];
        for (i, e) in REGISTRY.iter().enumerate() {
            ids.push(STR_NAME + i as u32);
            ids.push(STR_HELP + i as u32);
            ids.extend(level_ids(i));
            assert_eq!(level_ids(i).len(), e.levels.len());
        }
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        let s = with_extras(Strings::default());
        assert!(ids.iter().all(|id| s.get(*id).is_some()));
        // English install: the English captions
        assert_eq!(s.get(STR_TAB), Some("advanced"));
    }

    #[test]
    fn the_panel_text_has_a_combo_per_option() {
        let t = screen_text();
        for e in REGISTRY {
            assert!(t.contains(&format!("accessor={ACCESSOR_PREFIX}{}", e.token)));
        }
        assert_eq!(index_of("PortDisplay.fps_limit"), Some(FPS_LIMIT));
        assert_eq!(index_of("Display.Speed"), None);
        assert!(splice_file("GraphicsTabs.lay").is_some() && splice_file("graphics").is_some() && splice_file("audio").is_none());
        assert!(is_screen("displayopts") && !is_screen("graphics"));
    }
}
