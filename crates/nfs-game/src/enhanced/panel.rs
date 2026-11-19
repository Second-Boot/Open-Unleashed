//! The generated front-end panel: groups, virtual rows, pages, the rows text, the layout splices.
use super::*;

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
pub(super) const INACTIVE_COLOR: &str = "0xff8a8a8a";

// ---- ENHANCEMENT ENH-UPSCALER / ENH-PANEL: the panel's content is built from the CURRENT set ----

/// The panel's groups in display order: `[English, Russian]` header, then the registry switches in row order. The display order is
/// independent of the registry's (the Switch indices never move). After E38 the panel inserts the settings rows of the chosen upscaler
/// (MetalFX: mode + scale + sharpness, FSR: scale + sharpness). E34 / E37 / E39 have no row of their own: [`SETTINGS_OF_E38`].
/// A group stays on one page when it fits on one (`paginate`).
pub const GROUPS: &[([&str; 2], &[EnhancementSwitch])] = &[
    (["image", "Изображение"], &[E6, E22, E1, E2, E3, E35, E4, E5, E38, E33]),
    (["light and shadows", "Свет и тени"], &[E7, E9, E51, E43, E50, E14, E32, E16, E15, E24, E23, E47]),
    (["surfaces", "Поверхности"], &[E8, E49, E17, E26, E40, E41, E42]),
    (["atmosphere and effects", "Атмосфера и эффекты"], &[E10, E11, E13, E19, E36, E44, E21, E18, E12]),
    (["camera", "Камера"], &[E27, E28, E29, E48, E31]),
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
                        // UPSCALE-SOFT: the sharpness row is MetalFX's too (the port's RCAS pass after its scalers)
                        nfs_renderer::UP_METALFX if cfg!(target_os = "macos") => v.extend([Item::Virtual(VRow::MfxMode), Item::Virtual(VRow::MfxScale), Item::Entry(nfs_renderer::E39.0 as usize)]),
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

