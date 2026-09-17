//! Front-end screens from `FEData/Layouts/*.lay`.
//!
//! The `.lay` file is the screen's *appearance*: a list of widgets with a class, a position and size in
//! the original 640×480 design space, colours, and for the picture widgets an `.fsh` in `FEData/Art`
//! plus the entry index inside it. What a widget *does* — and, for a few of them, where it is placed —
//! lives in the executable and is not in the file.
//!
//! This module draws the widgets the file fully describes: the screen background, the solid, outlined
//! and gouraud boxes, the glinty lines and rectangles, the pictures, and every text widget that
//! carries its own `pos` — each in its own `fontPathName` font, its own `foreColor` and its own
//! `align`. Only `NFS5.HEADER` really has no geometry in the file (it carries `id`, `name`, `state`
//! and `stringID` and nothing else); it, and the widgets whose *content* comes from the executable
//! rather than the file (`NFS5.3DCAR`, `NFS5.ListBox`, `NFS5.CarLogo`, and an `NFS5.AccessorWatch`
//! whose accessor the port cannot resolve), are counted and skipped rather than guessed at.
//! `unrendered()` reports them.
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
use nfs_formats::lay::Layout;
use nfs_formats::locale::Strings;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};
use std::collections::HashMap;

use crate::text::BitmapFont;

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

fn tint(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// Classes the file does not describe well enough to draw: `NFS5.HEADER` carries no geometry at
/// all, and the rest draw content the executable supplies (a 3D car, a list of rows, a car logo, a
/// controller map).
fn code_placed(class: &str) -> bool {
    matches!(class, "NFS5.HEADER" | "NFS5.SUBHEADER" | "NFS5.ListBox" | "NFS5.3DCAR" | "NFS5.CarLogo" | "NFS5.CONTROLLERSETUP")
}

impl LayoutScreen {
    /// Load `FEData/Layouts/<name>.lay`, the background screen it names, and every `.fsh` entry
    /// they reference.
    pub fn load(r: &mut Renderer, paths: &GamePaths, name: &str) -> anyhow::Result<Self> {
        let mut screen = Self::load_one(r, paths, name)?;
        if let Some(bg) = screen.layout.screen().and_then(|s| s.get("screen")).and_then(background_layout) {
            match Self::load_one(r, paths, bg) {
                Ok(b) => screen.background = Some(Box::new(b)),
                Err(e) => tracing::debug!(background = bg, error = %e, "background screen unavailable"),
            }
        }
        Ok(screen)
    }

    fn load_one(r: &mut Renderer, paths: &GamePaths, name: &str) -> anyhow::Result<Self> {
        let dir = paths.fe_data().join("Layouts");
        let path = GamePaths::find_file(&dir, &format!("{name}.lay")).ok_or_else(|| anyhow::anyhow!("no layout {name}.lay in {}", dir.display()))?;
        let bytes = std::fs::read(&path)?;
        let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let layout = Layout::parse(&text);

        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        let art = paths.fe_data().join("Art");
        let mut shapes: HashMap<(String, usize), (TextureId, f32, f32)> = HashMap::new();
        let mut packs: HashMap<String, Vec<nfs_assets::texture::TextureImage>> = HashMap::new();
        for e in &layout.elements {
            let Some((file, index)) = e.shape() else { continue };
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

        // every `fontPathName` the file names, plus the FE default
        let mut fonts: HashMap<String, BitmapFont> = HashMap::new();
        let mut want: Vec<String> = vec![DEFAULT_FONT.to_string()];
        for e in &layout.elements {
            if let Some(f) = e.get("fontPathName").filter(|f| !f.is_empty()) {
                want.push(f.to_lowercase());
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
        Ok(Self { layout, background: None, shapes, fonts, white, unrendered })
    }

    pub fn unrendered(&self) -> &[(String, usize)] {
        &self.unrendered
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

    fn font_for(&self, e: &nfs_formats::lay::Element) -> Option<&BitmapFont> {
        let named = e.get("fontPathName").filter(|f| !f.is_empty()).map(|f| f.to_lowercase());
        named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))
    }

    /// Draw one line of text inside the widget box `(x, y, w, h)` with the original's alignment
    /// rule (0x5232AD → 0x4DA3D7): the anchor is the box's left edge / midpoint / right edge and the
    /// string is placed at the anchor, half a width back, or a full width back, rounded to pixels.
    /// Glyphs are drawn at their native size in the 640×480 design space.
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
        for g in &glyphs {
            let (pos, size) = self.place(aspect, left + g.x, top + g.y, g.w, g.h);
            out.push(Sprite { texture: f.texture(), pos, size, uv_min: g.uv_min, uv_max: g.uv_max, color, rotation: 0.0, pivot: [0.0, 0.0] });
        }
    }

    /// Every sprite of the screen, back to front in file order. `accessor` resolves an
    /// `NFS5.AccessorWatch`'s `accessor` name to the text it shows; a watch it cannot resolve is
    /// counted as unrendered instead of being filled with something invented.
    pub fn sprites(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        let mut out = Vec::new();
        if let Some(bg) = &self.background {
            out.extend(bg.sprites(aspect, strings, None, accessor));
        }
        for (i, e) in self.layout.elements.iter().enumerate() {
            let (x, y) = e.position();
            let (w, h) = e.size();
            match e.class.as_str() {
                // the screen itself: a full-screen fill
                c if c.starts_with("REAL.BLACK") => {
                    // The screen's own `color` is not the fill (the shipped files carry
                    // 0xff000000, 0xff0000ff and 0xff004080 without looking any different), so the
                    // class name is taken at its word — black. Only the bottom-most screen fills:
                    // a screen laid over a background must not paint over it. APPROXIMATION.
                    if self.background.is_none() {
                        out.push(self.quad(aspect, 0.0, 0.0, DESIGN.0, DESIGN.1, [0.0, 0.0, 0.0, 1.0]));
                    }
                }
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
                "NFS5.Rectangle" | "NFS5.GlintyRectangle" | "NFS5.ColorBox" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.PANEL" => {
                    if let Some(c) = e.color("color") {
                        out.push(self.quad(aspect, x, y, w.max(1.0), h.max(1.0), tint(c)));
                    }
                }
                "NFS5.GLINTYLINE" => {
                    let c = e.color("color").map(tint).unwrap_or([1.0, 1.0, 1.0, 0.5]);
                    out.push(self.quad(aspect, x, y, w.max(1.0), h.max(1.0), c));
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
                        continue;
                    }
                    let Some(f) = self.font_for(e) else { continue };
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
                    let text = if e.class.contains("AccessorWatch") {
                        match e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
                            Some(v) => v,
                            None => continue,
                        }
                    } else {
                        match e.string_id().and_then(|id| strings.get(id)) {
                            Some(t) if !t.is_empty() => t.to_string(),
                            _ => continue,
                        }
                    };
                    // the highlighted button uses its own `hovercolor`, as the original does
                    let color = if selected == Some(i) {
                        // a widget with no `hovercolor` of its own is highlighted anyway, so the
                        // player can see where the selection is — APPROXIMATION
                        e.color("hovercolor").or_else(|| e.color("maincolor")).map(tint).unwrap_or([1.0, 0.85, 0.2, 1.0])
                    } else {
                        e.color("foreColor").or_else(|| e.color("maincolor")).or_else(|| e.color("color")).map(tint).unwrap_or([1.0, 1.0, 1.0, 1.0])
                    };
                    // a widget with no box of its own anchors on its own text (align 0 then works
                    // out to the position in the file, which is what the button classes carry)
                    let box_w = if w > 0.0 { w } else { 0.0 };
                    let line_h = f.line_px();
                    let y = if h > line_h { y + ((h - line_h) * 0.5).round() } else { y };
                    self.text(&mut out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
                }
            }
        }
        out
    }
}
