//! Front-end screens from `FEData/Layouts/*.lay`.
//!
//! The `.lay` file is the screen's *appearance*: a list of widgets with a class, a position and size in
//! the original 640×480 design space, colours, and for the picture widgets an `.fsh` in `FEData/Art`
//! plus the entry index inside it. What a widget *does* — and, for a few of them, where it is placed —
//! lives in the executable and is not in the file.
//!
//! This module draws the widgets the file fully describes: the screen background, the solid, outlined
//! and gouraud boxes, the glinty lines and rectangles, the pictures, and every text widget — each in
//! its own `fontPathName` font, its own `foreColor` and its own `align`. `NFS5.HEADER` carries no
//! `pos`, but the `pos` attribute is registered with a zero default (0x524735), so the class default
//! *is* the screen origin and the header draws there like any other label. A widget whose `stringID`
//! is 0 draws nothing: that id is the shipped "Hello, World!" placeholder. The widgets whose
//! *content* comes from the executable (`NFS5.3DCAR`, `NFS5.ListBox`, `NFS5.CONTROLLERSETUP`, and
//! an `NFS5.AccessorWatch` whose accessor the port cannot resolve) are counted and skipped rather
//! than guessed at; `unrendered()` reports them. `NFS5.CarLogo` *is* drawn once the caller binds a
//! car to it with [`LayoutScreen::set_car_logo`]: the pack and the entry pair both come from the
//! car's own `nfs5.car` record (+0x147 and +0x1F6).
//!
//! **Alignment** is the original's (`FeLabel::draw` 0x5232AD and `FeText_draw` 0x4DA3D7, CONFIRMED):
//! `align` 0/1/2 anchors the text at the widget box's left edge / midpoint / right edge, and the
//! string is then drawn at `anchor`, `anchor − width/2` or `anchor − width`, with the result rounded
//! to whole pixels. The front end's default font is `Univers8b.ffn` (loaded at FE start-up,
//! 0x4C72B1).
//!
//! The design space is 4:3, so the screen is letterboxed inside the window rather than stretched.

use nfs_assets::GamePaths;
use nfs_assets::texture::decode_pack;
use nfs_formats::ctr::ControlLibrary;
use nfs_formats::lay::Layout;
use nfs_formats::locale::Strings;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::text::BitmapFont;

/// Read `FEData/Controls/*.ctr` — the widget class templates that hold every attribute's default
/// and the pieces each class is made of (`RE/notes/controls.md`).
pub fn load_controls(paths: &GamePaths) -> ControlLibrary {
    let dir = paths.fe_data().join("Controls");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        tracing::debug!(dir = %dir.display(), "no control templates");
        return ControlLibrary::default();
    };
    let files = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("ctr"))).filter_map(|e| {
        let bytes = std::fs::read(e.path()).ok()?;
        Some(bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect::<String>())
    });
    let lib = ControlLibrary::from_files(files);
    tracing::debug!(classes = lib.len(), "control templates loaded");
    lib
}

/// A widget the player can operate: a button that leads somewhere or runs a callback, or a combo
/// slider that edits the accessor it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// index into `layout.elements`
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub destination: Option<String>,
    pub callback: Option<String>,
    pub accessor: Option<String>,
    pub string_id: Option<u32>,
}

/// The design space every `.lay` is authored in.
pub const DESIGN: (f32, f32) = (640.0, 480.0);

pub struct LayoutScreen {
    pub layout: Layout,
    /// The background screen this one is laid over (`screen=` → [`background_layout`]).
    background: Option<Box<LayoutScreen>>,
    /// `("Extras.fsh", 3)` → texture and the entry's own pixel size
    shapes: HashMap<(String, usize), (TextureId, f32, f32)>,
    /// `fontPathName` (lower-cased) → font, plus the front end's default
    fonts: HashMap<String, BitmapFont>,
    white: TextureId,
    /// widget classes the file does not place, with how many of each were skipped
    unrendered: Vec<(String, usize)>,
    /// element index of an `NFS5.CarLogo` → the `.fsh` pack and the first of its two entries
    logos: HashMap<usize, (String, usize)>,
    /// `NFS5.RADIOGROUP` element index → the layout its `filename` names, loaded and drawn inside
    /// it. Four screens embed a tab strip this way: `Controllers` takes `controllertabs`, `Options`
    /// `optiontabs`, `graphics` `graphicstabs` and `showcase` `showcasetabs`. `Radio Group.Ctr`
    /// gives the class one attribute of its own, `filename`, and nothing drawable.
    groups: HashMap<usize, Box<LayoutScreen>>,
    /// `NFS5.GouraudIndicatorBar` element index → the 0..100 value its `othersource` resolved to.
    /// A bar with no entry here draws nothing, the way an unresolved `AccessorWatch` shows nothing.
    bars: HashMap<usize, i32>,
    /// element index of an `NFS5.ListBox` → its rows and the accessor's current row
    lists: HashMap<usize, (Vec<ListRow>, Option<usize>)>,
    /// element index of an `NFS5.3DCAR` → the offscreen texture its car was rendered into, drawn
    /// full-screen in the widget's own place in the drawing order
    car_views: HashMap<usize, TextureId>,
    /// the widget class templates (`FEData/Controls/*.ctr`), shared with the background screen
    controls: Arc<ControlLibrary>,
    /// where this screen is in its transition
    transition: Transition,
    /// drawn over another screen (the bottom bar, 0x4D39D3) — an overlay must not paint its own fill
    overlay: bool,
}

/// The bits a `transon` / `transoff` name maps to (`FE_transitionBits` 0x4C88C0 — the name is
/// lower-cased first, and anything unrecognised is 0, "no transition").
pub mod trans {
    pub const SAME: u32 = 1;
    pub const LEFT: u32 = 2;
    pub const RIGHT: u32 = 4;
    pub const TOP: u32 = 8;
    pub const BOTTOM: u32 = 0x10;
    pub const FADE: u32 = 0x20;
}

/// `transon` / `transoff` → bits. CONFIRMED (0x4C88C0).
pub fn transition_bits(name: &str) -> u32 {
    match name.trim().to_lowercase().as_str() {
        "same" => trans::SAME,
        "left" => trans::LEFT,
        "right" => trans::RIGHT,
        "top" => trans::TOP,
        "bottom" => trans::BOTTOM,
        "fade" => trans::FADE,
        _ => 0,
    }
}

/// Progress runs from 0 — the widget in place — to 512, a whole screen away and fully faded
/// ([0x5B6B2C], 0x4C87CD). CONFIRMED.
pub const TRANS_FULL: i32 = 512;
/// The sweep takes half of the timer's ticks-per-second ([0x5DEB48] / 2, 0x4E0A36); the timer is
/// the 1 kHz `timeSetEvent` set up at 0x564F2D, so half a second. CONFIRMED.
pub const TRANS_MS: u32 = 500;
/// …after three frames of nothing (`this+0x124` starts at 3, 0x4E09A0 / 0x4E09E8). CONFIRMED.
pub const TRANS_DELAY_FRAMES: u32 = 3;

/// Where a screen is in its transition. `In` is the original's state 1 (the progress falls to 0 and
/// the screen settles), `Out` its state 3 (the progress climbs to 512 and the screen is gone),
/// `Idle` its state 2 (0x4E0A3F…0x4E0AB4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Idle,
    In { frames: u32, ms: u32 },
    Out { frames: u32, ms: u32 },
}

impl Transition {
    /// 0 = in place, 512 = a whole screen away.
    pub fn progress(self) -> i32 {
        let t = |frames: u32, ms: u32| if frames > 0 { 0 } else { (ms as i64 * TRANS_FULL as i64 / TRANS_MS as i64) as i32 };
        match self {
            Transition::Idle => 0,
            Transition::In { frames, ms } => (TRANS_FULL - t(frames, ms)).max(0),
            Transition::Out { frames, ms } => t(frames, ms).min(TRANS_FULL),
        }
    }

    pub fn done(self) -> bool {
        match self {
            Transition::Idle => true,
            Transition::In { .. } => self.progress() == 0,
            Transition::Out { .. } => self.progress() == TRANS_FULL,
        }
    }

    /// One frame of the transition clock: the first [`TRANS_DELAY_FRAMES`] frames only count down,
    /// and the timer starts after them (0x4E09E8…0x4E0A16).
    pub fn advance(self, dt_ms: u32) -> Self {
        let step = |frames: u32, ms: u32| if frames > 0 { (frames - 1, ms) } else { (0, ms + dt_ms) };
        match self {
            Transition::Idle => Transition::Idle,
            Transition::In { frames, ms } => {
                let (frames, ms) = step(frames, ms);
                let next = Transition::In { frames, ms };
                if next.done() { Transition::Idle } else { next }
            }
            Transition::Out { frames, ms } => {
                let (frames, ms) = step(frames, ms);
                Transition::Out { frames, ms }
            }
        }
    }
}

/// The height of one list-box row and the width of its scrollbar, both constants in the original:
/// the accessor base class answers 14 to "how tall is row n" (0x4BC700) and the scrollbar is 16
/// wide (0x4DB030).
pub const LIST_ROW_H: f32 = 14.0;
pub const LIST_SCROLLBAR_W: f32 = 16.0;

/// An `NFS5.ListBox` widget and the accessor whose rows fill it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListBoxWidget {
    pub index: usize,
    pub accessor: Option<String>,
}

/// One piece of a column-laid-out list row: a string whose x comes from **another widget of the
/// same screen**, found by name, plus the accessor's own offset from it. `Track.Records` draws its
/// four columns that way (0x4FB2A0, anchors `ID_RECORDPLAYERPOS` −10 and +4, `ID_RECORDCARPOS` +4,
/// `ID_RECORDTIMEPOS` +4).
#[derive(Debug, Clone, PartialEq)]
pub struct ListCell {
    /// the `name` of the widget whose x this column sits at
    pub anchor: String,
    pub dx: f32,
    pub text: String,
}

/// The height of a parts list's category heading (0x518EA0) and of one of its rows (0x515FD4).
pub const PART_HEADER_H: f32 = 13.0;
pub const PART_ROW_H: f32 = 13.0;

/// A list-box row. The base accessor draws the whole row as one string (0x4BC350); the accessors
/// that override the row draw lay it out in columns instead, and the parts lists put a category
/// heading above the first row of each group.
#[derive(Debug, Clone, PartialEq)]
pub enum ListRow {
    Text(String),
    Columns(Vec<ListCell>),
    /// A parts list's category heading: a black rule above and below and the name centred between
    /// them (`drawPartsHeading` 0x518DE0), 13 px tall.
    Header(String),
}

impl ListRow {
    /// How tall this row is. The base accessor answers 14 for every row (0x4BC700); a parts list's
    /// rows are 13 and its headings another 13 (0x516080).
    pub fn height(&self) -> f32 {
        match self {
            ListRow::Text(_) => LIST_ROW_H,
            ListRow::Columns(_) => LIST_ROW_H,
            ListRow::Header(_) => PART_HEADER_H,
        }
    }
}

impl From<String> for ListRow {
    fn from(s: String) -> Self {
        ListRow::Text(s)
    }
}

/// An `NFS5.3DCAR` widget: which car it shows and where it puts it. `positiontype` 0 means the
/// widget has no car of its own and the screen's 3D room owns it (0x4BFEEC); 1, 2 and 3 place a car
/// of their own at a fixed spot in front of the camera (`RE/notes/frontend-3d.md` §4).
#[derive(Debug, Clone, PartialEq)]
pub struct Car3dWidget {
    pub index: usize,
    pub accessor: Option<String>,
    pub othersource: Option<String>,
    pub position_type: i64,
    /// the widget's `damage` key: draw the car with its damage state
    pub damage: bool,
}

impl Car3dWidget {
    /// Where `positiontype` puts the car, camera-relative (0x4BFF28…0x4BFF75). `positiontype` 0
    /// has none of its own.
    pub fn position(&self) -> Option<[f32; 3]> {
        match self.position_type {
            1 => Some([2.4, 0.2, 8.1]),
            2 => Some([-2.4, 0.2, 8.1]),
            3 => Some([2.4, -1.2, 6.2]),
            _ => None,
        }
    }
}

/// An `NFS5.CarLogo` widget and the car source it names. The original resolves the car in this
/// order (0x4C14E0, CONFIRMED): `othersource` `FactoryDriver` → the `FactoryDriver.Car` accessor,
/// `EndOfEra` → `PrizeCars.Car`, `FactoryDriverPrize` → the factory-drive prize car (0x4F4360),
/// then the widget's own `accessor`, then `CarCompare1` → the compare screen's left car (0x50E6B0).
/// `othersource=Chron` is handled before all of that (0x4C1660) and takes its pack and entry from
/// the chronicle record instead of from `nfs5.car`.
#[derive(Debug, Clone, PartialEq)]
pub struct CarLogoWidget {
    pub index: usize,
    pub accessor: Option<String>,
    pub othersource: Option<String>,
}

/// The front end's default font, loaded at FE start-up (0x4C72B1).
pub const DEFAULT_FONT: &str = "univers8b.ffn";

/// The screen block's `screen=` id → the layout drawn underneath it (`FE_loadBackground` 0x4E1470,
/// CONFIRMED). The original has a second, reduced table — `ID_FULLOVAL` → `screenfulloval` and
/// everything else → `screenveryempty` — behind a memory/option gate (`[0x5D9D38]`, set at 0x4E3584
/// only when more than 4 MB are free and `[0x657E5C]` is set), and swaps `screenoptionoval` for
/// `screenoptionovallite` on a screen with `canplaygame=1` when memory is short. A port with memory
/// to spare always takes the full table below.
pub fn background_layout(screen_id: &str) -> Option<&'static str> {
    Some(match screen_id {
        "ID_SCREEN" => "screenempty",
        "ID_DARKOVAL" => "screendarkoval",
        "ID_FULLOVAL" => "screenfulloval",
        "ID_MOVIEOVAL" => "screenmovieoval",
        "ID_OPTIONOVAL" => "screenoptionoval",
        _ => return None,
    })
}

/// An `0xAARRGGBB` string from a `.ctr` template as `[r, g, b, a]`.
fn parse_color(v: &str) -> Option<[u8; 4]> {
    let v = v.trim();
    let n = u64::from_str_radix(v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"))?, 16).ok()?;
    Some([((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8, ((n >> 24) & 0xFF) as u8])
}

fn tint(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// The tag the chronicle's picture tiles carry, which the control's own name repeats at 0x5DBBFE.
const CHRONICLE_PICTURE_TAG: &str = "1111";

/// Classes whose content comes from the executable and which the port does not draw. `NFS5.3DCAR`
/// is not one of them any more: the car it shows belongs to the screen's 3D room
/// (`RE/notes/frontend-3d.md`), which the port renders behind the screen.
fn code_placed(_class: &str) -> bool {
    false
}

/// The source a widget names, for the blank-widget report.
fn source_of(e: &nfs_formats::lay::Element) -> String {
    for k in ["accessor", "othersource", "listaccessor"] {
        if let Some(v) = e.get(k).filter(|v| !v.is_empty()) {
            return format!("{k}={v}");
        }
    }
    e.string_id().map(|id| format!("stringID={id}")).unwrap_or_else(|| "(none)".to_string())
}

impl LayoutScreen {
    /// Load `FEData/Layouts/<name>.lay`, the background screen it names, and every `.fsh` entry
    /// they reference.
    pub fn load(r: &mut Renderer, paths: &GamePaths, name: &str) -> anyhow::Result<Self> {
        let controls = Arc::new(load_controls(paths));
        let mut screen = Self::load_one(r, paths, name, controls.clone())?;
        // `NFS_FE_BG=<layout>` puts a background under a **page** — one of the 28 layouts whose
        // `screen=` is empty because in the game it is drawn over whatever opened it. Without it
        // `--fe=<page>` shows the page over black, which is `RE/AUDIT.md` F2.
        // …and only for the screen being opened: `bottombar` is a page too, and the front end draws
        // it *after* the screen, so giving it a background put a second copy of that background
        // over everything. That artefact cost four passes of `RE/AUDIT.md` F2.
        let forced = std::env::var("NFS_FE_BG")
            .ok()
            .filter(|_| !name.eq_ignore_ascii_case("bottombar"))
            .filter(|_| screen.layout.screen().and_then(|s| s.get("screen")).is_none_or(str::is_empty));
        if let Some(bg) = forced.as_deref().or_else(|| screen.layout.screen().and_then(|s| s.get("screen")).and_then(background_layout)) {
            match Self::load_one(r, paths, bg, controls) {
                Ok(b) => screen.background = Some(Box::new(b)),
                Err(e) => tracing::debug!(background = bg, error = %e, "background screen unavailable"),
            }
        }
        // the tab strips an `NFS5.RADIOGROUP` embeds
        let want: Vec<(usize, String)> = screen
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.RADIOGROUP")
            .filter_map(|(i, e)| e.get("filename").filter(|f| !f.is_empty()).map(|f| (i, f.to_string())))
            .collect();
        for (i, file) in want {
            match Self::load_one(r, paths, &file, screen.controls.clone()) {
                Ok(g) => {
                    screen.groups.insert(i, Box::new(g));
                }
                Err(e) => tracing::debug!(file, error = %e, "radio group layout unavailable"),
            }
        }
        Ok(screen)
    }

    fn load_one(r: &mut Renderer, paths: &GamePaths, name: &str, controls: Arc<ControlLibrary>) -> anyhow::Result<Self> {
        let dir = paths.fe_data().join("Layouts");
        let path = GamePaths::find_file(&dir, &format!("{name}.lay")).ok_or_else(|| anyhow::anyhow!("no layout {name}.lay in {}", dir.display()))?;
        let bytes = std::fs::read(&path)?;
        let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let layout = Layout::parse(&text);

        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        let art = paths.fe_data().join("Art");
        let mut shapes: HashMap<(String, usize), (TextureId, f32, f32)> = HashMap::new();
        let mut packs: HashMap<String, Vec<nfs_assets::texture::TextureImage>> = HashMap::new();
        // the widget's own `shapePathName`/`shapeIndex`, and every `SHAPE` part its class template
        // draws — the second is where `NFS5.CARSETTINGBUTTON` and its like keep all of their art
        let mut wanted: Vec<(String, usize)> = Vec::new();
        for e in &layout.elements {
            if let Some((file, index)) = e.shape() {
                wanted.push((file.to_string(), index));
            }
            let Some(c) = controls.get(&e.class) else { continue };
            for p in c.state_parts(e.get("state").unwrap_or("normal")).iter().filter_map(|n| c.part(n)) {
                if p.kind() != nfs_formats::ctr::PartKind::Shape {
                    continue;
                }
                if e.get("shapePathName").is_some_and(|v| !v.is_empty()) {
                    continue;
                }
                let Some(file) = p.value("shapePathName").filter(|v| !v.is_empty()) else { continue };
                let index = match p.value("shapeIndex") {
                    Some(v) => match v.trim().strip_prefix("data.") {
                        Some(k) => match e.int(k) {
                            Some(n) if n >= 0 => n as usize,
                            _ => continue,
                        },
                        None => match v.trim().parse::<i64>() {
                            Ok(n) if n >= 0 => n as usize,
                            _ => continue,
                        },
                    },
                    None => 0,
                };
                wanted.push((file.to_string(), index));
            }
        }
        for (file, index) in wanted {
            let file = file.as_str();
            let key = (file.to_string(), index);
            if shapes.contains_key(&key) {
                continue;
            }
            let entry = packs.entry(file.to_string()).or_insert_with(|| {
                GamePaths::find_file(&art, file)
                    .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                    .and_then(|(p, d)| decode_pack(&p, &d).ok())
                    .unwrap_or_default()
            });
            let Some(img) = entry.get(index) else {
                tracing::debug!(file, index, "layout art entry missing");
                continue;
            };
            let Some(level0) = img.mips.first() else { continue };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, level0.clone()));
            shapes.insert(key, (id, img.width as f32, img.height as f32));
        }

        // every `fontPathName` the file names, every one its widgets' class templates name, plus
        // the FE default
        let mut fonts: HashMap<String, BitmapFont> = HashMap::new();
        let mut want: Vec<String> = vec![DEFAULT_FONT.to_string()];
        for e in &layout.elements {
            if let Some(f) = e.get("fontPathName").filter(|f| !f.is_empty()) {
                want.push(f.to_lowercase());
            }
            if let Some(c) = controls.get(&e.class) {
                for part in c.parts.values() {
                    if let Some(f) = part.value("fontPathName").filter(|f| !f.is_empty()) {
                        want.push(f.to_lowercase());
                    }
                }
                if let Some(f) = c.default_of("fontpath").filter(|f| !f.is_empty()) {
                    want.push(f.to_lowercase());
                }
            }
        }
        want.sort();
        want.dedup();
        for name in want {
            let Some(path) = GamePaths::find_file(&art, &name) else {
                tracing::debug!(name, "layout font missing");
                continue;
            };
            match std::fs::read(&path).ok().and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()) {
                Some(f) => {
                    fonts.insert(name, BitmapFont::upload(r, f));
                }
                None => tracing::debug!(name, "layout font unreadable"),
            }
        }

        let mut counts: HashMap<String, usize> = HashMap::new();
        for e in &layout.elements {
            if code_placed(&e.class) {
                *counts.entry(e.class.clone()).or_default() += 1;
            }
        }
        let mut unrendered: Vec<(String, usize)> = counts.into_iter().collect();
        unrendered.sort();
        tracing::info!(name, elements = layout.elements.len(), art = shapes.len(), fonts = fonts.len(), skipped = unrendered.len(), "layout loaded");
        Ok(Self { layout, background: None, shapes, fonts, white, unrendered, logos: HashMap::new(), groups: HashMap::new(), bars: HashMap::new(), lists: HashMap::new(), car_views: HashMap::new(), controls, transition: Transition::Idle, overlay: false })
    }

    /// The widgets the screen does not draw: the classes whose content comes from the executable,
    /// plus any `NFS5.CarLogo` the caller has not bound a car to yet.
    pub fn unrendered(&self) -> Vec<(String, usize)> {
        let mut out = self.unrendered.clone();
        for (class, bound) in [("NFS5.CarLogo", &self.logos.keys().copied().collect::<Vec<_>>()), ("NFS5.ListBox", &self.lists.keys().copied().collect::<Vec<_>>())] {
            let n = self.layout.elements.iter().enumerate().filter(|(i, e)| e.class == class && !bound.contains(i)).count();
            if n > 0 {
                out.push((class.to_string(), n));
            }
        }
        out.sort();
        out
    }

    /// The same, but naming each widget's own `accessor` / `othersource` so a sweep can tell an
    /// unimplemented source from a list that is simply empty in the current state.
    pub fn unrendered_sources(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (class, bound) in [("NFS5.CarLogo", self.logos.keys().copied().collect::<Vec<_>>()), ("NFS5.ListBox", self.lists.keys().copied().collect::<Vec<_>>())] {
            for (i, e) in self.layout.elements.iter().enumerate() {
                if e.class == class && !bound.contains(&i) {
                    let src = self
                        .attr(e, "accessor")
                        .filter(|v| !v.is_empty())
                        .or_else(|| self.attr(e, "othersource").filter(|v| !v.is_empty()))
                        .unwrap_or("(none)")
                        .to_string();
                    out.push((class.to_string(), src));
                }
            }
        }
        out.sort();
        out
    }

    /// The value of a widget attribute: the `.lay` block's own key when it carries one, otherwise
    /// the default in the class's `.ctr` template (`RE/notes/controls.md` §2). A key the block
    /// leaves empty counts as absent, which is how the shipped files spell "use the default"
    /// (`selectmode=`, `disablesound=`).
    pub fn attr<'a>(&'a self, e: &'a nfs_formats::lay::Element, key: &str) -> Option<&'a str> {
        if let Some(v) = e.get(key).filter(|v| !v.is_empty()) {
            return Some(v);
        }
        self.controls.get(&e.class)?.default_of(key).filter(|v| !v.is_empty())
    }

    fn attr_num(&self, e: &nfs_formats::lay::Element, key: &str) -> Option<f64> {
        let v = self.attr(e, key)?.trim();
        match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            Some(h) => u64::from_str_radix(h, 16).ok().map(|n| n as f64),
            None => v.parse().ok(),
        }
    }

    /// Resolve one `.ctr` expression reference for `e`: `data.<key>` is the widget's own attribute,
    /// `control.size.width` its box, and `<part>.<key>` another piece of the same template. A
    /// reference the port cannot answer — a text part's width, which depends on the string — is
    /// left unresolved so the whole expression falls back rather than silently reading zero.
    fn reference(&self, e: &nfs_formats::lay::Element, name: &str, depth: u32) -> Option<f64> {
        let (head, rest) = name.split_once('.')?;
        if head == "data" {
            return self.attr_num(e, rest);
        }
        let (bw, bh) = self.box_of(e, depth + 1);
        if head == "control" {
            return match rest {
                "size.width" => Some(bw as f64),
                "size.height" => Some(bh as f64),
                _ => None,
            };
        }
        if depth > 4 {
            return None;
        }
        let part = self.controls.get(&e.class)?.part(head)?;
        let (key, field) = rest.split_once('.')?;
        let v = part.value(key)?;
        let pair = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1))?;
        match field {
            "width" | "x" => Some(pair.0),
            "height" | "y" => Some(pair.1),
            _ => None,
        }
    }

    /// The widget's box. The `.lay` block's `size` wins; failing that the class template's own
    /// `size.value`, which for most classes is an expression over the block's keys
    /// (`data.width,data.height` for a button, `data.width,data.selectionboxmaxheight` for a list).
    fn box_of(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        if let Some(p) = e.pair("size") {
            return p;
        }
        if depth <= 4
            && let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("size")
            && let Some((w, h)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1))
        {
            return (w as f32, h as f32);
        }
        (e.float("width").unwrap_or(0.0), e.float("height").unwrap_or(0.0))
    }

    /// The widget's position: the block's own `pos`, or the class template's `[control] pos.value`
    /// — `NFS5.HEADER` carries no `pos` at all and `Header.ctr` puts it at (6, 4).
    fn position_of(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        if let Some(p) = e.pair("pos") {
            return p;
        }
        if let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("pos")
            && let Some((x, y)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0))
        {
            return (x as f32, y as f32);
        }
        (0.0, 0.0)
    }

    /// The `RECT` parts of the widget's class template that its `state` draws, each resolved to a
    /// rectangle and a colour (`RE/notes/controls.md` §1). This is how a class that is nothing but
    /// boxes — `NFS5.CONTROLLERSETUP` is a filled box with a one-pixel border — draws without the
    /// port knowing anything about it.
    fn rect_parts(&self, e: &nfs_formats::lay::Element) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let state = e.get("state").unwrap_or("normal");
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| p.value(key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let Some((x, y)) = at("pos") else { continue };
            let Some((w, h)) = at("size") else { continue };
            // a colour may be a literal or a reference to one of the `data` attributes
            let colour = p
                .value("color")
                .and_then(|v| parse_color(v).or_else(|| v.split_once('.').and_then(|(_, key)| self.attr(e, key)).and_then(parse_color)));
            let Some(colour) = colour else { continue };
            out.push((x as f32, y as f32, w as f32, h as f32, colour));
        }
        out
    }

    /// The `SHAPE` parts of the widget's class template that its `state` draws, each resolved to a
    /// place and a `.fsh` pack entry (`RE/notes/controls.md` §1).
    ///
    /// This is the other half of [`rect_parts`](Self::rect_parts): a class whose art is entirely in
    /// its template draws through here without the port knowing anything about it.
    /// `NFS5.CARSETTINGBUTTON` is the plainest case — `CarSettingButton.ctr` gives its `picnonsel`
    /// part `shapePathName=weeicons.fsh` and `shapeIndex=data.nonselindex`, so the widget's own
    /// `nonselindex` picks the arrow. The same shape covers `NFS5.POPUPCIRCLE`,
    /// `NFS5.ExtraCircle`, the tab buttons and the rest of the art-from-template classes.
    fn shape_parts(&self, e: &nfs_formats::lay::Element) -> Vec<(f32, f32, f32, f32, String, usize)> {
        // `NFS5.POPUPCIRCLE` is the frame of a **modal popup**, not part of the screen behind it:
        // `PopUp.ctr` pins it at `pos=113,100` (`HIDDEN|READONLY`) and builds a 414×350 frame out of
        // one 207×175 quarter mirrored four ways (`p1`…`p4`, `flip` 0…3), and its states are
        // `normal` and `hidden`. Nineteen screens carry one — `repair`, `VictoryMessage`,
        // `accountlogin`, `createplayer` — and all of them are screens that can raise a dialog.
        //
        // This port never raises one, so drawing the frame leaves it standing over the screen for
        // ever, which is what it did on `repair.lay`. The widget is skipped until the port has
        // popup state. **Not** because of `swrenderoff`: that key reaches `[widget+0x13C]` at
        // 0x4C7733 and 0x4C7750 and a scan of the whole `.text` for the displacement finds those
        // two writes and **no read at all**, so it is dead in this build (CONFIRMED by exhaustion).
        if e.class == "NFS5.POPUPCIRCLE" {
            return Vec::new();
        }
        // a part's `shapePathName.value` is that part's **default**; a widget that names its own
        // overrides it, and then the widget's own pair is what gets drawn (the `e.shape()` path
        // below). Without this, every `NFS5.PICTURE` would also draw `Picture.ctr`'s placeholder —
        // `pointer.fsh` entry 0, an arrow — on top of its real art.
        if e.get("shapePathName").is_some_and(|v| !v.is_empty()) {
            return Vec::new();
        }
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let state = e.get("state").unwrap_or("normal");
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Shape {
                continue;
            }
            let Some(file) = p.value("shapePathName").filter(|v| !v.is_empty()) else { continue };
            // `shapeIndex` is a literal or a `data.<key>` reference to the widget's own attribute
            let index = match p.value("shapeIndex") {
                Some(v) => match nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0)) {
                    Some(n) if n >= 0.0 => n as usize,
                    _ => continue,
                },
                None => 0,
            };
            let at = |key: &str| p.value(key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (px, py) = at("pos").unwrap_or((0.0, 0.0));
            let (pw, ph) = at("size").unwrap_or((0.0, 0.0));
            out.push((px as f32, py as f32, pw as f32, ph as f32, file.to_string(), index));
        }
        out
    }

    /// The first drawable part of the widget's class template that carries `key`, e.g. the
    /// `fontPathName` of a header's `label` part.
    fn part_value<'a>(&'a self, e: &'a nfs_formats::lay::Element, key: &str) -> Option<&'a str> {
        let c = self.controls.get(&e.class)?;
        let state = e.get("state").unwrap_or("normal");
        c.state_parts(state).iter().filter_map(|n| c.part(n)).find_map(|p| p.value(key)).filter(|v| !v.is_empty())
    }

    /// The black bars that keep the 4:3 design area square in a wider window. The 2D screen is
    /// letterboxed by [`place`], but a 3D room behind it fills the whole window, so the bars have to
    /// be drawn over it. APPROXIMATION — the original's front end is always 4:3.
    pub fn letterbox_bars(&self, aspect: f32) -> Vec<Sprite> {
        let design = DESIGN.0 / DESIGN.1;
        if aspect <= design + 1e-3 {
            return Vec::new();
        }
        let w = (1.0 - design / aspect) * 0.5;
        let bar = |x: f32| Sprite {
            texture: self.white,
            pos: [x, 0.0],
            size: [w * aspect, 1.0],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [0.0, 0.0, 0.0, 1.0],
            rotation: 0.0,
            pivot: [0.5, 0.5],
        };
        vec![bar(0.0), bar(1.0 - w)]
    }

    /// Mark this screen as drawn over something else — the bottom bar the front end lays over
    /// every screen (0x4D39D3), or any screen with a 3D room behind it
    /// (`RE/notes/frontend-3d.md`). Such a screen draws its widgets but not the screen fill, which
    /// would otherwise paint over what is behind. The background screen is marked with it.
    pub fn set_overlay(&mut self, overlay: bool) {
        self.overlay = overlay;
        if let Some(b) = self.background.as_mut() {
            b.set_overlay(overlay);
        }
    }

    /// Start this screen sliding and fading in, the way the original does when a screen is pushed
    /// (state 1, 0x4E09A0). The background screen transitions with it.
    pub fn transition_in(&mut self) {
        self.transition = Transition::In { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_in();
        }
    }

    /// Start it going out (state 3, 0x4E09C0).
    pub fn transition_out(&mut self) {
        self.transition = Transition::Out { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_out();
        }
    }

    pub fn transition(&self) -> Transition {
        self.transition
    }

    /// Advance the transition clock by one frame; `true` once the screen has settled or gone.
    pub fn advance_transition(&mut self, dt_ms: u32) -> bool {
        self.transition = self.transition.advance(dt_ms);
        if let Some(b) = self.background.as_mut() {
            b.advance_transition(dt_ms);
        }
        self.transition.done()
    }

    /// The offset and the alpha a widget draws with at this point in the transition
    /// (`FeWidget::transitionOffset` 0x4C8760 and `::transitionAlpha` 0x4C8840): the slide is the
    /// screen size scaled by the progress, the fade is the progress itself, and the bits come from
    /// `transon` while the screen comes in and from `transoff` while it goes out.
    fn transition_of(&self, e: &nfs_formats::lay::Element) -> ((f32, f32), f32) {
        let p = self.transition.progress();
        if p == 0 {
            return ((0.0, 0.0), 1.0);
        }
        let out = matches!(self.transition, Transition::Out { .. });
        let (key, other) = if out { ("transoff", "transon") } else { ("transon", "transoff") };
        // `SAME` means "use the other one", and both `SAME` means no transition at all
        // (0x4C75D4…0x4C7601)
        let (mut bits, ob) = (transition_bits(self.attr(e, key).unwrap_or("")), transition_bits(self.attr(e, other).unwrap_or("")));
        if bits & trans::SAME != 0 {
            bits = if ob & trans::SAME != 0 { 0 } else { ob };
        }
        let f = p as f32 / TRANS_FULL as f32;
        let mut d = (0.0f32, 0.0f32);
        if bits & trans::LEFT != 0 {
            d.0 -= DESIGN.0 * f;
        }
        if bits & trans::RIGHT != 0 {
            d.0 += DESIGN.0 * f;
        }
        if bits & trans::TOP != 0 {
            d.1 -= DESIGN.1 * f;
        }
        if bits & trans::BOTTOM != 0 {
            d.1 += DESIGN.1 * f;
        }
        let alpha = if bits & trans::FADE != 0 { 1.0 - (p * 255 / TRANS_FULL) as f32 / 255.0 } else { 1.0 };
        (d, alpha)
    }

    /// The x of the widget named `name` on this screen — the column anchor an accessor's row draw
    /// looks up (`FE_widgetPos` 0x4D6410 searches the current screen's widget list from the end).
    fn anchor_x(&self, name: &str) -> Option<f32> {
        let e = self.layout.elements.iter().rev().find(|e| e.name.eq_ignore_ascii_case(name))?;
        Some(self.position_of(e).0)
    }

    /// The `NFS5.ListBox` widgets on this screen, with the accessor each one lists.
    pub fn list_boxes(&self) -> Vec<ListBoxWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.ListBox")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect()
    }

    /// Fill a list box with the rows its accessor would hand out, and say which row the accessor
    /// has selected (`current` — the row the original paints with `highlightcolorselected`).
    pub fn set_list_rows(&mut self, index: usize, rows: Vec<ListRow>, current: Option<usize>) {
        if rows.is_empty() {
            self.lists.remove(&index);
        } else {
            self.lists.insert(index, (rows, current));
        }
    }

    /// Hand an `NFS5.3DCAR` the offscreen view its car was rendered into. The view covers the whole
    /// screen — the car's place in it comes from the camera, not from the widget's rectangle — and
    /// is drawn in the widget's own place in the layout order, which is what puts it over the art
    /// behind it and under the art in front.
    pub fn set_car_view(&mut self, index: usize, view: Option<TextureId>) {
        match view {
            Some(t) => self.car_views.insert(index, t),
            None => self.car_views.remove(&index),
        };
    }

    /// The offscreen view already handed to this widget, so the caller can render into it again.
    pub fn car_view(&self, index: usize) -> Option<TextureId> {
        self.car_views.get(&index).copied()
    }

    /// The `NFS5.3DCAR` widgets on this screen.
    pub fn car_3d(&self) -> Vec<Car3dWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.3DCAR")
            .map(|(index, e)| Car3dWidget {
                index,
                accessor: self.attr(e, "accessor").map(str::to_string),
                othersource: self.attr(e, "othersource").map(str::to_string),
                position_type: self.attr_num(e, "positiontype").unwrap_or(0.0) as i64,
                damage: self.attr_num(e, "damage").unwrap_or(0.0) != 0.0,
            })
            .collect()
    }

    /// The `NFS5.CarLogo` widgets on this screen, with the car source each one names.
    pub fn car_logos(&self) -> Vec<CarLogoWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CarLogo")
            .map(|(index, e)| CarLogoWidget {
                index,
                accessor: e.get("accessor").filter(|a| !a.is_empty()).map(str::to_string),
                othersource: e.get("othersource").filter(|a| !a.is_empty()).map(str::to_string),
            })
            .collect()
    }

    /// Every `NFS5.GouraudIndicatorBar` and the source it reads. `FEGouraudIndicatorBar`'s
    /// constructor takes both keys (0x4CA24F `data.othersource`, 0x4CA27E `data.accessor`) and its
    /// value getter tries the `othersource` names first, falling through to the accessor at
    /// 0x4CA974. The flag says which of the two the name came from.
    pub fn indicator_bars(&self) -> Vec<(usize, String, bool)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.GouraudIndicatorBar")
            .filter_map(|(index, e)| {
                match e.get("othersource").filter(|a| !a.is_empty()) {
                    Some(a) => Some((index, a.to_string(), true)),
                    None => e.get("accessor").filter(|a| !a.is_empty()).map(|a| (index, a.to_string(), false)),
                }
            })
            .collect()
    }

    /// Give one indicator bar its value, or `None` to leave it blank.
    pub fn set_bar(&mut self, index: usize, value: Option<i32>) {
        match value {
            Some(v) => self.bars.insert(index, v),
            None => self.bars.remove(&index),
        };
    }

    /// The texture of any font this screen loaded, for the sprite self-test.
    pub fn a_font_texture(&self) -> Option<TextureId> {
        self.fonts.values().next().map(|f| f.texture())
    }

    /// The 1×1 white texture this screen uploaded, for callers that want to draw a plain quad.
    pub fn white_texture(&self) -> TextureId {
        self.white
    }

    /// Every widget of `class` and the `number` it carries, in element order.
    pub fn numbered_widgets(&self, class: &str) -> Vec<(usize, i64)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == class)
            .map(|(index, e)| (index, e.int("number").unwrap_or(0)))
            .collect()
    }

    /// Draw one entry of a `.fsh` pack from `dir` at the widget's box.
    ///
    /// This is how the classes that are only a **place** get their picture. Two of them say so in
    /// their own template: `ChroniclePictureIcon.ctr`'s header is "Simply used for the x,y and to
    /// grab input events", and `ShowcasePicture.ctr` has no drawable part either. What goes there
    /// is picked from code, and in both cases the `.lay` block's `number` is the entry:
    ///
    /// * the chronicle packs open with two 41×25 entries, `bi` and `ci`, which is exactly the
    ///   icon widget's `size=41,25`;
    /// * a showcase pack holds eight, and `general.lay` and `glancestats.lay` between them give
    ///   `number` 0…7 to widgets whose `size` matches entry `number`'s dimensions on all eight
    ///   (128×90, 127×90, 128×91, 127×91, twice). CONFIRMED by that match.
    pub fn set_pack_picture(&mut self, r: &mut Renderer, paths: &GamePaths, dir: &str, index: usize, file: &str, entry: usize) {
        if file.is_empty() {
            self.logos.remove(&index);
            return;
        }
        let key = (file.to_string(), entry);
        if !self.shapes.contains_key(&key) {
            let dir = paths.fe_data().join(dir);
            let entries = GamePaths::find_file(&dir, file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            let Some(img) = entries.get(entry) else {
                self.logos.remove(&index);
                return;
            };
            let Some(px) = img.mips.first() else { return };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
            self.shapes.insert(key.clone(), (id, img.width as f32, img.height as f32));
        }
        self.logos.insert(index, key);
    }

    /// Point a `NFS5.CarLogo` at a **chronicle** picture instead of a car logo.
    ///
    /// The chronicle's own art is not in `FEData/Art` and is not a logo pair: `0x50757F` builds the
    /// name with `"%s%c%c%02d%d%c.fsh"` — the directory, `'M'` for a picture (`'p'` is the other
    /// kind), the model-group letter from the table at **`0x5B8B18` = `"CBADE"`** indexed by the
    /// group at `[0x5DBA44]`, the year mod 100, a 1-based index within the year, and a variant
    /// `'a' + n`. `.mad` is the same name for the movie. So one whole image, drawn on its own.
    pub fn set_chronicle_picture(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, file: &str) {
        if file.is_empty() {
            self.logos.remove(&index);
            return;
        }
        if self.logos.get(&index).is_some_and(|(f, b)| f == file && *b == 0) {
            return;
        }
        if !self.shapes.contains_key(&(file.to_string(), 0)) {
            let dir = paths.fe_data().join("Chron");
            let entries = GamePaths::find_file(&dir, file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            // the picture is the run of tiles tagged `1111` — the same tag the control's own name
            // carries at 0x5DBBFE — laid side by side; `bi` and `ci` are its 41×25 icons and the
            // `Laye` entries a second layer.
            //
            // **APPROXIMATION**: the tiles are joined in pack order, left to right. Where each one
            // actually belongs is in the `.fsh` entry's own attachment record, which has not been
            // read, so a picture whose tiles are not stored in reading order comes out shuffled.
            let tiles: Vec<&nfs_assets::texture::TextureImage> = entries.iter().filter(|t| t.name.trim() == CHRONICLE_PICTURE_TAG).collect();
            let (w, h) = (tiles.iter().map(|t| t.width).sum::<u32>(), tiles.iter().map(|t| t.height).max().unwrap_or(0));
            if w == 0 || h == 0 {
                tracing::debug!(file, entries = entries.len(), "chronicle picture has no `1111` tiles");
                self.logos.remove(&index);
                return;
            }
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            let mut x0 = 0u32;
            for t in &tiles {
                let Some(px) = t.mips.first() else { continue };
                for y in 0..t.height {
                    let src = (y * t.width * 4) as usize;
                    let dst = ((y * w + x0) * 4) as usize;
                    let n = (t.width * 4) as usize;
                    if src + n <= px.len() && dst + n <= rgba.len() {
                        rgba[dst..dst + n].copy_from_slice(&px[src..src + n]);
                    }
                }
                x0 += t.width;
            }
            let id = r.upload_texture(&TextureData::single(w, h, rgba));
            self.shapes.insert((file.to_string(), 0), (id, w as f32, h as f32));
        }
        self.logos.insert(index, (file.to_string(), 0));
    }

    /// Point a `NFS5.CarLogo` at a car's logo: `pack` is the car record's +0x147 (`finallogos1`)
    /// and `pair` its +0x1F6. The widget draws entries `2*pair` and `2*pair + 1` of
    /// `FEData/Art/<pack>.fsh` (`NFS5.CarLogo::update` 0x4C1A63 — the index is the pair doubled,
    /// and the pair past the end of the pack draws nothing, 0x4C1AF6). CONFIRMED.
    pub fn set_car_logo(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, pack: &str, pair: i16) {
        if pack.is_empty() || pair < 0 {
            self.logos.remove(&index);
            return;
        }
        let file = format!("{pack}.fsh");
        let base = pair as usize * 2;
        if self.logos.get(&index).is_some_and(|(f, b)| f == &file && *b == base) {
            return;
        }
        if !self.shapes.contains_key(&(file.clone(), base)) {
            let art = paths.fe_data().join("Art");
            let images = GamePaths::find_file(&art, &file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            for entry in [base, base + 1] {
                let Some(img) = images.get(entry) else {
                    tracing::debug!(file, entry, "car logo entry missing");
                    continue;
                };
                let Some(level0) = img.mips.first() else { continue };
                let id = r.upload_texture(&TextureData::single(img.width, img.height, level0.clone()));
                self.shapes.insert((file.clone(), entry), (id, img.width as f32, img.height as f32));
            }
        }
        // the original draws nothing unless both entries of the pair resolved (0x4C1B76)
        if self.shapes.contains_key(&(file.clone(), base)) && self.shapes.contains_key(&(file.clone(), base + 1)) {
            self.logos.insert(index, (file, base));
        } else {
            self.logos.remove(&index);
        }
    }

    /// `canplaygame` on the screen block: the bottom bar's PLAYGAME button is live on this screen.
    /// (The shipped files carry 0, 1, 2 or nothing; 0x4D3410 hands the value to the background
    /// chooser, so it is per-screen state, HIGH.)
    pub fn can_play(&self) -> bool {
        self.layout.screen().and_then(|s| s.int("canplaygame")).unwrap_or(0) != 0
    }

    /// The widgets a player can operate, in reading order: the buttons that lead somewhere and the
    /// combo sliders, each with the accessor it edits.
    pub fn controls(&self) -> Vec<Control> {
        let mut out: Vec<Control> = Vec::new();
        for (i, e) in self.layout.elements.iter().enumerate() {
            let (x, y) = e.position();
            let dest = e.get("destination").filter(|d| !d.is_empty()).map(str::to_string);
            let callback = e.get("callback").filter(|c| !c.is_empty()).map(str::to_string);
            let accessor = e.get("accessor").filter(|a| !a.is_empty()).map(str::to_string);
            let is_button = e.class.contains("BUTTON");
            if !(is_button || e.class == "NFS5.ComboBoxSlider") {
                continue;
            }
            if dest.is_none() && callback.is_none() && accessor.is_none() {
                continue;
            }
            out.push(Control { index: i, x, y, destination: dest, callback, accessor, string_id: e.string_id() });
        }
        out.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// The elements that name another screen in `destination` — the screen's navigation.
    pub fn buttons(&self) -> Vec<usize> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.get("destination").is_some_and(|d| !d.is_empty()) && e.string_id().is_some())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn destination(&self, index: usize) -> Option<&str> {
        self.layout.elements.get(index)?.get("destination")
    }

    pub fn label(&self, index: usize, strings: &Strings) -> String {
        self.layout.elements.get(index).and_then(|e| e.string_id()).and_then(|id| strings.get(id)).unwrap_or("").to_string()
    }

    /// The screen's music pattern (`musicfile=menu*`), for `MusicPlayer::play_pattern`.
    pub fn music_pattern(&self) -> Option<&str> {
        self.layout.screen().and_then(|s| s.get("musicfile")).filter(|s| !s.is_empty())
    }

    /// Map a design-space rectangle into sprite coordinates, letterboxing the 4:3 design inside a
    /// window of aspect `aspect`.
    fn place(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32) -> ([f32; 2], [f32; 2]) {
        let design_aspect = DESIGN.0 / DESIGN.1;
        let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
        ([x0 + x / DESIGN.0 * fw, y / DESIGN.1], [w / DESIGN.1, h / DESIGN.1])
    }

    fn quad(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Sprite {
        let (pos, size) = self.place(aspect, x, y, w, h);
        Sprite { texture: self.white, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5] }
    }

    /// The font a widget draws its text in: its own `fontPathName`, or the one its class template
    /// gives the part it draws (a header's is `Univers17b.ffn`), or the front end's default.
    fn font_for(&self, e: &nfs_formats::lay::Element) -> Option<&BitmapFont> {
        let named = e
            .get("fontPathName")
            .filter(|f| !f.is_empty())
            .or_else(|| self.part_value(e, "fontPathName"))
            .map(|f| f.to_lowercase());
        named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))
    }

    /// Draw one line of text inside the widget box `(x, y, w, h)` with the original's alignment
    /// rule (0x5232AD → 0x4DA3D7): the anchor is the box's left edge / midpoint / right edge and the
    /// string is placed at the anchor, half a width back, or a full width back, rounded to pixels.
    /// Glyphs are drawn at their native size in the 640×480 design space.
    /// Lay `s` out inside `w`, breaking between words, and draw the lines `line_px` apart. Used by
    /// `NFS5.WORDWRAPACCESSORWATCH` — the class the front end uses where a value is a sentence
    /// rather than a figure: a tournament's description, a part's, a mission briefing.
    ///
    /// **APPROXIMATION**: breaking on spaces, keeping a word that does not fit on its own line, is
    /// the obvious rule but is not one that was read out of the original.
    #[allow(clippy::too_many_arguments)]
    fn wrapped_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        if w <= 0.0 {
            self.text(out, aspect, f, s, x, y, w, align, color);
            return;
        }
        let mut line = String::new();
        let mut ly = y;
        for word in s.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if f.place(&candidate).1 > w && !line.is_empty() {
                self.text(out, aspect, f, &line, x, ly, w, align, color);
                ly += f.line_px();
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            self.text(out, aspect, f, &line, x, ly, w, align, color);
        }
    }

    fn text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        let (glyphs, width) = f.place(s);
        let anchor = match align {
            1 => x + w * 0.5,
            2 => x + w,
            _ => x,
        };
        let left = match align {
            1 => anchor - width * 0.5,
            2 => anchor - width,
            _ => anchor,
        }
        .round();
        let top = y.round();
        // `NFS_TEXT_PROBE=1` draws the same glyphs in white — the A/B that characterised the
        // faint-text fault in `RE/AUDIT.md` F2.
        let probe = std::env::var("NFS_TEXT_PROBE").is_ok();
        let color = if probe { [1.0, 1.0, 1.0, 1.0] } else { color };
        let solid = std::env::var("NFS_TEXT_SOLID").is_ok();
        for g in &glyphs {
            let (pos, size) = self.place(aspect, left + g.x, top + g.y, g.w, g.h);
            // `NFS_TEXT_FULLUV=1` opens the glyph's UV window to the whole sheet — with
            // `NFS_TEXT_BIG` it shows the sheet, without it nothing (`AUDIT.md` F2)
            let full = std::env::var("NFS_TEXT_FULLUV").is_ok();
            let (uv0, uv1) = if full {
                ([0.0, 0.0], [1.0, 1.0])
            } else if solid {
                let c = [(g.uv_min[0] + g.uv_max[0]) * 0.5, (g.uv_min[1] + g.uv_max[1]) * 0.5];
                ([c[0] - 0.0001, c[1] - 0.0001], [c[0] + 0.0001, c[1] + 0.0001])
            } else {
                (g.uv_min, g.uv_max)
            };
            // `NFS_TEXT_SOLID=1` swaps the atlas for the 1×1 white texture and collapses the UV
            // window to one texel, so a glyph quad becomes a plain filled rectangle — the probe
            // that showed the quads composite at about 8 % whatever they sample (`AUDIT.md` F2).
            // `NFS_TEXT_BIG=1` draws the quads four times the size
            let size = if std::env::var("NFS_TEXT_BIG").is_ok() { [size[0] * 4.0, size[1] * 4.0] } else { size };
            let tex = if solid { self.white } else { f.texture() };
            out.push(Sprite { texture: tex, pos, size, uv_min: uv0, uv_max: uv1, color, rotation: 0.0, pivot: [0.0, 0.0] });
            // `NFS_TEXT_AB=1` repeats each glyph one line lower **in red**, same texture, same UV
            // window, same size — so one image carries the widget's text twice and the only
            // difference between the two is `color.rgb`. The red copy draws and the black original
            // does not (`AUDIT.md` F2).
            if std::env::var("NFS_TEXT_AB").is_ok() {
                out.push(Sprite { texture: f.texture(), pos: [pos[0], pos[1] + size[1] * 1.2], size, uv_min: uv0, uv_max: uv1, color: [1.0, 0.0, 0.0, 1.0], rotation: 0.0, pivot: [0.0, 0.0] });
            }
        }
    }

    /// Every sprite of the screen, back to front in file order. `accessor` resolves an
    /// `NFS5.AccessorWatch`'s `accessor` name to the text it shows; a watch it cannot resolve is
    /// counted as unrendered instead of being filled with something invented.
    pub fn sprites(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.sprites_reporting(aspect, strings, selected, accessor, &mut Vec::new())
    }

    /// Every widget that produced **no sprite at all** — its class and the source it names. This is
    /// the port's own measure of what it does not draw: a widget whose content the original places
    /// from code, and for which the file carries neither a shape nor a string, leaves nothing
    /// behind here. `examples/layout_sweep.rs` runs it over every `.lay` in the game.
    pub fn blank_widgets(&self, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
        let mut blank = Vec::new();
        self.sprites_reporting(aspect, strings, None, accessor, &mut blank);
        blank
    }

    fn sprites_reporting(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>) -> Vec<Sprite> {
        let mut out = Vec::new();
        if let Some(bg) = &self.background {
            out.extend(bg.sprites_reporting(aspect, strings, None, accessor, blank));
        }
        let bg_end = out.len();
        // `NFS_REVERSE_ORDER=1` draws the widgets back to front instead of file order (probe for
        // `RE/AUDIT.md` F2). The screen block, element 0, is metadata and is never drawn.
        let order: Vec<usize> = if std::env::var("NFS_REVERSE_ORDER").is_ok() {
            (1..self.layout.elements.len()).rev().collect()
        } else {
            (0..self.layout.elements.len()).collect()
        };
        for i in order {
            let e = &self.layout.elements[i];
            let ((tdx, tdy), talpha) = self.transition_of(e);
            let (x, y) = self.position_of(e);
            let (x, y) = (x + tdx, y + tdy);
            let (w, h) = self.box_of(e, 0);
            let first = out.len();
            'elem: {
            match e.class.as_str() {
                // the screen itself: a full-screen fill
                c if c.starts_with("REAL.BLACK") => {
                    // The screen's own `color` is not the fill (the shipped files carry
                    // 0xff000000, 0xff0000ff and 0xff004080 without looking any different), so the
                    // class name is taken at its word — black. Only the bottom-most screen fills,
                    // and only when there is nothing behind it: a screen with a 3D room behind it
                    // (`RE/notes/frontend-3d.md`) would otherwise paint the room out.
                    // APPROXIMATION.
                    if self.background.is_none() && !self.overlay {
                        out.push(self.quad(aspect, 0.0, 0.0, DESIGN.0, DESIGN.1, [0.0, 0.0, 0.0, 1.0]));
                    }
                }
                // `NFS_NO_GOURAUD=1` skips them — the probe for which sprites bury the text
                "NFS5.GOURAUDBOX" if std::env::var("NFS_NO_GOURAUD").is_ok() => {}
                "NFS5.GOURAUDBOX" => {
                    // the four corner colours are `cUL cUR cBL cBR`; the sprite pipeline has one colour
                    // per quad, so the box is drawn as a grid of bilinearly interpolated cells
                    // (APPROXIMATION — the original draws one gouraud-shaded quad). The grid is only
                    // subdivided along the axes the colour actually varies on.
                    let c = |k: &str| e.color(k).map(tint).unwrap_or([0.0, 0.0, 0.0, 0.0]);
                    let (ul, ur, bl, br) = (c("cUL"), c("cUR"), c("cBL"), c("cBR"));
                    let differs = |a: [f32; 4], b: [f32; 4]| (0..4).any(|j| (a[j] - b[j]).abs() > 1.0 / 255.0);
                    let nx = if differs(ul, ur) || differs(bl, br) { 16 } else { 1 };
                    let ny = if differs(ul, bl) || differs(ur, br) { 16 } else { 1 };
                    for gy in 0..ny {
                        for gx in 0..nx {
                            let (u, v) = ((gx as f32 + 0.5) / nx as f32, (gy as f32 + 0.5) / ny as f32);
                            let mut col = [0.0f32; 4];
                            for j in 0..4 {
                                let top = ul[j] + (ur[j] - ul[j]) * u;
                                let bot = bl[j] + (br[j] - bl[j]) * u;
                                col[j] = top + (bot - top) * v;
                            }
                            let (cw, ch) = (w / nx as f32, h / ny as f32);
                            out.push(self.quad(aspect, x + gx as f32 * cw, y + gy as f32 * ch, cw, ch, col));
                        }
                    }
                }
                // `FEGouraudIndicatorBar::draw` (0x4CA690) asks its source for a number, then at
                // 0x4CA71A picks the span it paints: with `goingleft` clear it is `0 ..= value`,
                // and with it set `100 - value ..= 100`, so the mirrored column of the car-compare
                // screen fills from its right edge. The value is a per cent because the widget's
                // own colour stops are named `c0`, `c50` and `c100`.
                "NFS5.GouraudIndicatorBar" => {
                    let Some(&value) = self.bars.get(&i) else { break 'elem };
                    let value = value.clamp(0, 100) as f32;
                    let going_left = e.get("goingleft").is_some_and(|v| v.trim() != "0");
                    let (lo, hi) = if going_left { (100.0 - value, 100.0) } else { (0.0, value) };
                    if hi <= lo {
                        break 'elem;
                    }
                    // the three stops are the colour at 0, 50 and 100 per cent of the **box**, not
                    // of the filled part; the sprite pipeline has one colour per quad, so the span
                    // is cut into cells of one per cent each — APPROXIMATION, as for `NFS5.GOURAUDBOX`
                    let stop = |k: &str, d: [f32; 4]| e.color(k).map(tint).unwrap_or(d);
                    let (c0, c50, c100) = (stop("c0", [1.0; 4]), stop("c50", [1.0; 4]), stop("c100", [1.0; 4]));
                    let at = |p: f32| -> [f32; 4] {
                        let (a, b, t) = if p < 50.0 { (c0, c50, p / 50.0) } else { (c50, c100, (p - 50.0) / 50.0) };
                        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
                    };
                    let cell = w / 100.0;
                    for p in lo as u32..hi as u32 {
                        out.push(self.quad(aspect, x + p as f32 * cell, y, cell, h.max(1.0), at(p as f32 + 0.5)));
                    }
                }
                // `FEGlintyRectangle`'s constructor (0x522530) reads **`width`, `height`, `col` and
                // `animate`** — not `size` and `color`, which is why one of these draws nothing when
                // it is read like a plain rectangle. It hands them to 0x51FDA0 in that order, and
                // the draw (0x51FDE0) passes the widget's own position and that width and height to
                // **0x4D9200, which is an outline**: four calls to the one-bar primitive 0x4D9280
                // with a thickness of `1.0f` — top, bottom, left and right. So this is a one-pixel
                // frame, which is what makes it a frame around each bar of `repair.lay` (92×8 over
                // a 90×6 bar) and around the whole list (261×163 over the panel), rather than a
                // fill that would bury them.
                //
                // Not drawn: the glint. With `animate` non-zero the draw arms a timer
                // (0x51FE39…0x51FE80) that waits `rand() % 32 + 64` and then plays one of the
                // flourishes — with `animate == 5` a random one of four (0x51FE8E). Its shapes were
                // not traced, so it is left out rather than guessed at, exactly as the `NFS5.CarLogo`
                // glint is.
                "NFS5.GlintyRectangle" => {
                    let Some(c) = e.color("col").or_else(|| e.color("color")) else { break 'elem };
                    let gw = e.int("width").map(|v| v as f32).filter(|v| *v > 0.0).unwrap_or(w).max(1.0);
                    let gh = e.int("height").map(|v| v as f32).filter(|v| *v > 0.0).unwrap_or(h).max(1.0);
                    let (c, t) = (tint(c), 1.0);
                    out.push(self.quad(aspect, x, y, gw, t, c));
                    out.push(self.quad(aspect, x, y + gh - t, gw, t, c));
                    out.push(self.quad(aspect, x, y, t, gh, c));
                    out.push(self.quad(aspect, x + gw - t, y, t, gh, c));
                }
                "NFS5.Rectangle" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.PANEL" => {
                    if let Some(c) = e.color("color") {
                        out.push(self.quad(aspect, x, y, w.max(1.0), h.max(1.0), tint(c)));
                    }
                }
                "NFS5.GLINTYLINE" => {
                    let c = e.color("color").map(tint).unwrap_or([1.0, 1.0, 1.0, 0.5]);
                    out.push(self.quad(aspect, x, y, w.max(1.0), h.max(1.0), c));
                }
                // `NFS5.3DCAR` is drawn after the screen, below — its car is a 3D view, not a
                // sprite in the layout's own order
                "NFS5.3DCAR" => {}
                // The text boxes: `InputBox.ctr`'s `state.normal` is `unselbkgd` — a
                // `data.width`×14 rect filled `0xFFBC0000`, the same dark red `ControllerSetup.ctr`
                // gives its rows as `regcolor` — four one-pixel black `bgoutline` rects, and
                // `untext`, whose text is the widget's accessor. The chat boxes are built the same
                // way. Their `state.focused` swaps in `selbkgd`/`hitext` and a `cursor`, which this
                // port has no focus or caret for.
                "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHATDISPLAYBOX"
                | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP" => {
                    for (px, py, pw, ph, c) in self.rect_parts(e) {
                        out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(c)));
                    }
                    let Some(f) = self.font_for(e) else { break 'elem };
                    let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) else { break 'elem };
                    self.text(&mut out, aspect, f, &v, x, y, w, e.int("align").unwrap_or(0), [1.0, 1.0, 1.0, 1.0]);
                }
                "NFS5.ListBox" => {
                    // `FeListBox::draw` 0x4C64FE…0x4C691B with `ListBox.ctr`. The panel is the
                    // template's `selectionbox` part — `pos=0,1`, `size=data.width,
                    // data.selectionboxmaxheight` — filled with 0x10FFFFFF (0x4C6636), and its
                    // height is clamped to the content: `min(rows·14, selectionboxmaxheight)`
                    // (0x4C53A0). When it clamps, a scrollbar appears and takes 16 px off the
                    // width (0x4C5421 with 0x4DB030). Rows are the accessor's own: 14 px each
                    // (0x4BC700), the string at (+3, +2) over a bar of the row's colour
                    // (0x4BC350). The original keeps drawing while the accumulated height is still
                    // within the panel and scissors the overflow; the port draws only the rows
                    // that fit whole (APPROXIMATION — the sprite pipeline has no scissor).
                    let Some((rows, current)) = self.lists.get(&i) else { break 'elem };
                    let Some(f) = self.font_for(e) else { break 'elem };
                    let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
                    let full: f32 = rows.iter().map(|r| r.height()).sum();
                    let (panel_h, scrolls) = if full < max_h { (full, false) } else { (max_h, true) };
                    let mut panel_w = self.attr_num(e, "width").unwrap_or(0.0) as f32;
                    if scrolls {
                        panel_w -= LIST_SCROLLBAR_W;
                    }
                    let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
                        Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
                        None => (x, y),
                    };
                    out.push(self.quad(aspect, px, py, panel_w, panel_h, [1.0, 1.0, 1.0, 16.0 / 255.0]));
                    // `textcolor` is black and the two highlights amber and dark red, all three
                    // straight out of `ListBox.ctr`; a highlighted row's own text turns white
                    // (0x4C67A6 → 0x4D87D0).
                    let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
                    let sel_c = self.attr(e, "highlightcolorselected").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0xBC, 0, 0, 255]);
                    let mut ry = py;
                    for (n, row) in rows.iter().enumerate() {
                        if ry + row.height() > py + panel_h {
                            break;
                        }
                        let (bar, fg) = if *current == Some(n) { (Some(sel_c), [1.0, 1.0, 1.0, 1.0]) } else { (None, tint(text_c)) };
                        match row {
                            ListRow::Text(t) => {
                                if let Some(c) = bar {
                                    out.push(self.quad(aspect, px, ry, panel_w, LIST_ROW_H, tint(c)));
                                }
                                self.text(&mut out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg);
                            }
                            // a columned row draws no bar of its own — `Track.Records` reduces the
                            // colour it is handed to its alpha alone (0x4FB328) and places each
                            // column at another widget's x, at the row's own y (no +2)
                            ListRow::Columns(cells) => {
                                if let Some(c) = bar {
                                    out.push(self.quad(aspect, px, ry, panel_w, row.height(), tint(c)));
                                }
                                for cell in cells {
                                    // a cell with no anchor of its own sits at the panel's left edge
                                    let ax = if cell.anchor.is_empty() { px } else { self.anchor_x(&cell.anchor).unwrap_or(px) };
                                    let align = if cell.dx < 0.0 { 2 } else { 0 };
                                    self.text(&mut out, aspect, f, &cell.text, ax + cell.dx, ry, 0.0, align, tint(text_c));
                                }
                            }
                            // `drawPartsHeading` 0x518DE0: a black rule at the top and another 13
                            // pixels down, with the name centred two pixels below the first
                            ListRow::Header(t) => {
                                let black = [0.0, 0.0, 0.0, 1.0];
                                out.push(self.quad(aspect, px, ry, panel_w, 1.0, black));
                                out.push(self.quad(aspect, px, ry + PART_HEADER_H, panel_w, 1.0, black));
                                self.text(&mut out, aspect, f, t, px, ry + 2.0, panel_w, 1, tint(text_c));
                            }
                        }
                        ry += row.height();
                    }
                }
                // a widget that is only a place, with its picture put there from code: one entry
                // of a `.fsh` pack — see [`set_pack_picture`](Self::set_pack_picture)
                // the tab strip this group embeds, drawn at the group's own place. Its `pos` is
                // 0,0 on all four screens that use one, so the strip lands in the screen's own
                // coordinates.
                "NFS5.RADIOGROUP" => {
                    let Some(g) = self.groups.get(&i) else { break 'elem };
                    let mut inner = g.sprites_reporting(aspect, strings, None, accessor, blank);
                    if x != 0.0 || y != 0.0 {
                        let (dx, dy) = (x / DESIGN.0, y / DESIGN.1);
                        for sp in &mut inner {
                            sp.pos[0] += dx;
                            sp.pos[1] += dy;
                        }
                    }
                    out.extend(inner);
                }
                "NFS5.ChroniclePictureIcon" | "NFS5.ShowcasePicture" => {
                    let Some(key) = self.logos.get(&i) else { break 'elem };
                    let Some(&(tex, aw, ah)) = self.shapes.get(key) else { break 'elem };
                    let (dw, dh) = (if w > 0.0 { w } else { aw }, if h > 0.0 { h } else { ah });
                    let (pos, size) = self.place(aspect, x, y, dw, dh);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                "NFS5.CarLogo" => {
                    // `NFS5.CarLogo::draw` 0x4C1B70, CONFIRMED: the pair is drawn side by side —
                    // the first entry at the anchor, the second immediately to its right — and the
                    // whole thing is vertically centred in the widget's own box. `align` moves the
                    // anchor the same way the text rule does: 0 keeps `pos`, 1 pulls back half the
                    // combined width, 2 the full width (0x4C1BE9…0x4C1C22). A second entry only one
                    // pixel high is a placeholder: the original zeroes its width and skips it
                    // (0x4C1BDC). Nothing is drawn until the port has bound a car to the widget.
                    let Some((file, base)) = self.logos.get(&i) else { break 'elem };
                    let Some(&(ta, aw, ah)) = self.shapes.get(&(file.clone(), *base)) else { break 'elem };
                    let b = self.shapes.get(&(file.clone(), base + 1)).copied();
                    let (bw, bh, tb) = match b {
                        Some((t, w, h)) if h != 1.0 => (w, h, Some(t)),
                        _ => (0.0, 0.0, None),
                    };
                    let left = match e.int("align").unwrap_or(0) {
                        1 => x - ((aw + bw) * 0.5).trunc(),
                        2 => x - (aw + bw),
                        _ => x,
                    };
                    let top = y + ((h - ah) * 0.5).trunc();
                    let (pos, size) = self.place(aspect, left, top, aw, ah);
                    out.push(Sprite { texture: ta, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    if let Some(tb) = tb {
                        let (pos, size) = self.place(aspect, left + aw, top, bw, bh);
                        out.push(Sprite { texture: tb, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    // Not drawn: the glint that sweeps over the logo (the `GLNT` entry of
                    // `weeicons.fsh`, 0x4C1D26…0x4C1DB2) — its sweep is driven by the same global
                    // the front end sets to 640 at 0x4D35B4, but the shape's own travel was not
                    // traced, so it is left out rather than guessed at.
                }
                "NFS5.OutlineRectangle" => {
                    let c = e.color("color").map(tint).unwrap_or([1.0, 1.0, 1.0, 0.5]);
                    let t = 1.0;
                    out.push(self.quad(aspect, x, y, w, t, c));
                    out.push(self.quad(aspect, x, y + h - t, w, t, c));
                    out.push(self.quad(aspect, x, y, t, h, c));
                    out.push(self.quad(aspect, x + w - t, y, t, h, c));
                }
                _ => {
                    // Not drawn here: the template's `RECT` parts. They are the widget's regions,
                    // not its paint — `GoToButton.ctr`'s `bg` is `data.maincolor` over the whole
                    // box, which is the colour the button's *text* uses, and filling it puts a
                    // solid red bar behind every label on `QuickRace.lay`. A class that really is
                    // a box draws its rects from its own arm, as `NFS5.CONTROLLERSETUP` does.
                    // the art the widget's class template carries, at the places the template puts
                    // it — `NFS5.CARSETTINGBUTTON`'s arrows, the popup circles, the tab buttons
                    for (px, py, pw, ph, file, index) in self.shape_parts(e) {
                        let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
                        let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
                        let (pos, size) = self.place(aspect, x + px, y + py, dw, dh);
                        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    // anything with a picture draws it; anything with a string draws that
                    if let Some((file, index)) = e.shape()
                        && let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), index))
                    {
                        // A picture is drawn at its art's own size: every piece of the oval frames
                        // carries `size=50,150` in the file while the `.fsh` entries are 256×256,
                        // 89×82 … and each entry's own signed offset is exactly minus the widget's
                        // `pos` (`CnstLyr.fsh` entry 4 is at (−131, −120), its widget at 131,120).
                        // HIGH, from the data.
                        let (pos, size) = self.place(aspect, x, y, aw, ah);
                        // `flip` mirrors the art: 1 in x, 2 in y, 3 in both. HIGH — the attribute is
                        // an int on the picture widget (0x52AA4E) whose consumer was not traced, but
                        // it is what makes the oval frames of the background screens close up: the
                        // four corners of `screenempty` are the same `.fsh` entry with flip 0/1/2/3.
                        let flip = e.int("flip").unwrap_or(0).max(0);
                        let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    if code_placed(&e.class) {
                        break 'elem;
                    }
                    let Some(f) = self.font_for(e) else { break 'elem };
                    // A combo slider carries both: its `stringID` is the label and its `accessor`
                    // holds the value. The original places the value with code; the port fills the
                    // widget's own box — label left, value right — which is the only geometry the
                    // file gives. APPROXIMATION.
                    if e.class == "NFS5.ComboBoxSlider"
                        && let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor)
                    {
                        let colour = if selected == Some(i) { [1.0, 0.85, 0.2, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
                        let box_w = if w > 0.0 { w } else { 120.0 };
                        self.text(&mut out, aspect, f, &v, x, y, box_w, 2, colour);
                    }
                    // an accessor watch shows a value, everything else its own `stringID`
                    // case-insensitively: the class is `NFS5.AccessorWatch` on most screens but
                    // `NFS5.WORDWRAPACCESSORWATCH` where the text has to wrap, and a `contains`
                    // that respected case never matched the second one
                    let text = if e.class.to_ascii_lowercase().contains("accessorwatch") {
                        match e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
                            Some(v) => v,
                            None => break 'elem,
                        }
                    } else if let Some(t) = e.get("text").filter(|t| !t.is_empty()) {
                        // a literal string in the file rather than a `FEStrings` id.
                        // `Non-Localized Label.ctr` is nothing but a `TEXT` part whose `text.value`
                        // is the placeholder "Put Text Here", which the widget's own `text` key
                        // replaces — `audioreal.lay`'s per-cent signs are `text=%`.
                        t.to_string()
                    } else {
                        // `stringID=0` is the shipped "Hello, World!" placeholder (marked DO NOT
                        // TRANSLATE in `FEStrings.csv`): those widgets get their text from code.
                        match e.string_id().filter(|id| *id != 0).and_then(|id| strings.get(id)) {
                            Some(t) if !t.is_empty() => t.to_string(),
                            _ => break 'elem,
                        }
                    };
                    // the highlighted button uses its own `hovercolor`, as the original does
                    let color = if selected == Some(i) {
                        // a widget with no `hovercolor` of its own is highlighted anyway, so the
                        // player can see where the selection is — APPROXIMATION
                        e.color("hovercolor").or_else(|| e.color("maincolor")).map(tint).unwrap_or([1.0, 0.85, 0.2, 1.0])
                    } else {
                        e.color("foreColor")
                            .or_else(|| e.color("maincolor"))
                            .or_else(|| e.color("color"))
                            .or_else(|| self.part_value(e, "foreColor").and_then(parse_color))
                            .map(tint)
                            .unwrap_or([1.0, 1.0, 1.0, 1.0])
                    };
                    // a widget with no box of its own anchors on its own text (align 0 then works
                    // out to the position in the file, which is what the button classes carry)
                    let box_w = if w > 0.0 { w } else { 0.0 };
                    // The text sits at the widget's own `pos`, not centred in its box. The files
                    // say so: on `tournament.lay` every value and its label share a `y` — the
                    // `REAL.LABEL` at 299,114 and the `NFS5.AccessorWatch` at 302,114, and so on
                    // down the panel — and the labels carry no `size` at all. Centring a watch in
                    // its `size=100,30` box pushed every value a row below its label, while
                    // `PRIZEMONEY`, whose box is `43,11` and so too short to centre in, lined up.
                    let _ = h;
                    if e.class.to_ascii_lowercase().contains("wordwrap") {
                        self.wrapped_text(&mut out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
                    } else {
                        self.text(&mut out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
                    }
                }
            }
            }
            // every sprite this widget produced fades with it (0x4C8840)
            if talpha < 1.0 {
                for s in &mut out[first..] {
                    s.color[3] *= talpha;
                }
            }
            // the first block is the screen's own descriptor, not a widget: it is the one that
            // carries `screen=`, `room=`, `musicfile=` and `callbacklist=`, and its class
            // (`REAL.BLACK_640x480`) brings a `color` and a `size` with it. Painting that colour
            // would cover the background layout the same block names, which every screen visibly
            // has, so it is metadata and not counted as undrawn.
            // `NFS_WIDGET_DUMP=1` prints, per widget, how many sprites it produced, their extent
            // and the transition alpha they carry — the diagnostic that found the faint-text bug
            // recorded in `RE/AUDIT.md`
            if std::env::var("NFS_WIDGET_DUMP").is_ok() {
                let n = out.len() - first;
                let b = out[first..].iter().fold((9.0f32, 9.0f32, -9.0f32, -9.0f32), |a, s| (a.0.min(s.pos[0]), a.1.min(s.pos[1]), a.2.max(s.pos[0]), a.3.max(s.pos[1])));
                let f0: Vec<(String, [f32; 4])> = { let mut v: Vec<(String, [f32; 4])> = out[first..].iter().map(|s| (format!("{:?}", s.texture), s.color)).collect(); v.dedup(); v };
                println!("  #{i:3} {:<26} pos=({x:.0},{y:.0}) sprites={n} box=({:.3},{:.3})-({:.3},{:.3}) alpha={talpha:.2} first={f0:?}", e.class, b.0, b.1, b.2, b.3);
            }
            if out.len() == first && i > 0 {
                blank.push((e.class.clone(), source_of(e)));
            }
        }
        if std::env::var("NFS_LAYER_DUMP").is_ok() {
            println!("layer: background {} sprites, own {} sprites", bg_end, out.len() - bg_end);
        }
        // The cars of the `NFS5.3DCAR` widgets that have a `positiontype` of their own, each
        // already rendered into an offscreen view the size of the screen — its place in that view
        // comes from the camera, not from the widget's rectangle. The original draws each one in
        // its widget's own place in the layout order, but two of the three screens that use them
        // (`carcompare`) then put `DarkOval.fsh` over the middle, which would hide them, so the
        // port draws them over the finished screen instead. APPROXIMATION.
        for (&i, &tex) in &self.car_views {
            let alpha = self.transition_of(&self.layout.elements[i]).1;
            let design = DESIGN.0 / DESIGN.1;
            let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
            out.push(Sprite { texture: tex, pos: [x0, 0.0], size: [fw * aspect, 1.0], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], rotation: 0.0, pivot: [0.5, 0.5] });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FE_transitionBits` 0x4C88C0 — the six names, lower-cased, and 0 for anything else.
    #[test]
    fn transition_names_map_to_their_bits() {
        assert_eq!(transition_bits("SAME"), trans::SAME);
        assert_eq!(transition_bits("LEFT"), trans::LEFT);
        assert_eq!(transition_bits("RIGHT"), trans::RIGHT);
        assert_eq!(transition_bits("TOP"), trans::TOP);
        assert_eq!(transition_bits("BOTTOM"), trans::BOTTOM);
        assert_eq!(transition_bits("FADE"), trans::FADE);
        assert_eq!(transition_bits("fade"), trans::FADE);
        assert_eq!(transition_bits(""), 0);
    }

    /// The clock: three dead frames, then 0..512 over half a second (0x4E09E8…0x4E0AB4).
    #[test]
    fn the_transition_clock_runs_for_half_a_second_after_three_frames() {
        let mut t = Transition::In { frames: TRANS_DELAY_FRAMES, ms: 0 };
        for _ in 0..TRANS_DELAY_FRAMES {
            assert_eq!(t.progress(), TRANS_FULL, "the widget is a whole screen away while it waits");
            t = t.advance(16);
        }
        assert_eq!(t.progress(), TRANS_FULL);
        t = t.advance(250);
        assert_eq!(t.progress(), TRANS_FULL / 2, "half the time, half the way in");
        t = t.advance(250);
        assert_eq!(t, Transition::Idle, "and it settles exactly at the half second");
        assert!(t.done());

        // going out climbs the other way and stops at 512
        let mut t = Transition::Out { frames: 0, ms: 0 };
        assert_eq!(t.progress(), 0);
        t = t.advance(500);
        assert_eq!(t.progress(), TRANS_FULL);
        assert!(t.done());
        assert_eq!(t.advance(500).progress(), TRANS_FULL, "and stays gone");
    }
}
