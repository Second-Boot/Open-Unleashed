//! The port's own string ids and texts (the id ranges, the level captions, the install language, `with_extras`).
use super::*;

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
        // UPSCALE-SOFT: a temporal backend chosen with E7 off
        (C::TemporalNeedsHdr, false) => format!("inactive: the temporal {up} works on the HDR image: needs '{}' (or MetalFX 'simple')", name("E7")),
        (C::TemporalNeedsHdr, true) => format!("не действует: временному {up} нужно HDR-изображение: нужен «{}» (или «простой» MetalFX)", name("E7")),
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

