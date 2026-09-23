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

mod dialog_draw;
mod end_of_era_draw;
mod widgets;
use widgets::WidgetDraw;

/// Read `FEData/Controls/*.ctr` — the widget class templates that hold every attribute's default
/// and the pieces each class is made of (`RE/notes/controls.md`).
/// The front end's string table, for the one geometry rule that needs it: a `TEXT` or
/// `LOCALIZEDTEXT` part's `size` is the **extent of the string it draws**, which several templates
/// then position other parts from (`BottomBarButton.ctr`'s own `pos.value` is
/// `(data.align&1)?data.posx-(label.size.width+picture.size.width+data.space):data.posx`).
pub fn load_strings(paths: &GamePaths) -> Strings {
    let dir = paths.fe_data().join("Locale");
    match GamePaths::find_file(&dir, "FEStrings.csv").and_then(|p| std::fs::read(p).ok()) {
        Some(b) => Strings::parse(&b, 0),
        None => Strings::default(),
    }
}

pub fn load_controls(paths: &GamePaths) -> ControlLibrary {
    let dir = paths.fe_data().join("Controls");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        tracing::debug!(dir = %dir.display(), "no control templates");
        return ControlLibrary::default();
    };
    // `.ctr` is a control template and **`.scr` a screen one** — the directory ships exactly one,
    // `Black Screen 640x480.scr`, which is the template for `REAL.BLACK_640x480`, the id every
    // layout's `[section.0]` carries. It has the same shape as a `.ctr` and the port used to skip
    // it, so the one class that every screen is built on had no template at all here.
    let files = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("ctr") || x.eq_ignore_ascii_case("scr"))).filter_map(|e| {
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
    /// `data.offset`, which is **what the button does to the screen stack**, not a position:
    /// `NFS5.GOTOBUTTON`'s bind takes it into `[this+0x144]` (0x4C8F55) and its slot 9 hands the
    /// attribute's own int, `[+0x38]`, to `0x4D2100(destination, offset, 1)` as the second
    /// argument (0x4C94BD…0x4C94CF). `RE/notes/layouts.md`.
    pub offset: i32,
    /// `data.step` — what `NFS5.CARSETTINGBUTTON` adds to its accessor's index when it is pressed.
    /// Its bind takes it into `[this+0x144]` (0x4C1ED7) and its **slot 27**, 0x4C1F20, hands it to
    /// `accessor->slot19(step)`; slot 5 (0x4C1F6C) takes focus and starts 0x529220's 150 ms
    /// repeat, and slot 10 (0x4C1F80) runs slot 27 again on every tick of it. Seventeen of the 34
    /// shipped buttons carry **1** and seventeen **−1** — the left and right arrow of each pair —
    /// so the sign is the whole point. §4.84.
    pub step: i32,
}

/// The design space every `.lay` is authored in.
pub const DESIGN: (f32, f32) = (640.0, 480.0);

/// One event of the tournament a `NFS5.TrackSmall2dSpline` watches, as the widget needs it.
///
/// `FETrackSmall2dSpline::update` (0x4FCB40) asks its `Tournament.Tournament` accessor which
/// tournament is selected (0x4FC9A6 registers it, 0x4FC9B9 keeps the object), takes the event
/// count from `trn+0x39C` (0x503370) and, for event `i`, resolves `trn+0x3A0+i*0xA4` — the track
/// id — into the `nfs5.trk` record (0x5033D0 → 0x4FBF30). From that record it takes **`+0x58`**,
/// the small art name, and loads `<TrackArt>/<name>.fsh` (0x4FCC7E, `"%s%s.fsh"`), keeping its
/// entries **0, 1 and 2** (0x4FCCA5, 0x4FCCB4, 0x4FCCC0).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMapEvent {
    /// the `nfs5.trk` record's `+0x58` — `alpsSpSm` and the like, without the extension
    pub art: String,
    /// the event's `+4`: with it set the widget draws entry **2** (`f`) instead of entry 1 (`s`),
    /// because a point-to-point track driven backwards starts where it normally finishes
    /// (0x4FCDFC…0x4FCE0F). The shipped `nfs5.trn` sets it on 39 of its 130 events.
    pub backwards: bool,
    /// the event's `+5`, handed to the blit as its fifth argument: when set the marker's x is
    /// mirrored across the map, `map.x + map.w − marker.x − marker.w` (0x4FCE9F…0x4FCEC8). No
    /// shipped event sets it.
    pub mirror: bool,
    /// the track's name in caps — `FEStrings[trk+4 + 20]`, which is what 0x4FCFAD…0x4FCFBE looks
    /// up for the list beside the map
    pub name: String,
}

/// A `.mad` playing inside a `NFS5.MOVIEBOX`.
///
/// The frame period is the video header's `+0x0C` as **16.16 fixed point**: 0x4DCB01 masks its low
/// sixteen bits off as the fraction and 0x4DCB06 shifts the rest down to whole units, which it adds
/// to the player's own clock. Every shipped movie carries 4369067, so 66.67 ms a frame — fifteen a
/// second.
struct MoviePlayer {
    /// the whole file, and where its video chunks are
    data: Vec<u8>,
    chunks: Vec<(usize, usize)>,
    decoder: nfs_formats::mad::MadDecoder,
    /// the next chunk to decode
    next: usize,
    period_q16: u64,
    clock_q16: u64,
    tex: TextureId,
    size: (f32, f32),
}

impl MoviePlayer {
    /// The whole file kept, its video chunks indexed, the first frame decoded into a texture.
    fn open(r: &mut Renderer, paths: &GamePaths, dir: &str, file: &str) -> Option<Self> {
        let path = GamePaths::find_file(&paths.fe_data().join(dir), file)?;
        let (Ok(exe), Ok(data)) = (std::fs::read(paths.root.join("nfs5.exe")), std::fs::read(&path)) else { return None };
        let tables = nfs_formats::mad::MadTables::from_exe(&exe).ok()?;
        let chunks: Vec<(usize, usize)> = {
            let mut at = 0usize;
            let mut out = Vec::new();
            for c in nfs_formats::mad::chunks(&data) {
                if c.is_video() {
                    out.push((at, c.data.len()));
                }
                at += c.data.len();
            }
            out
        };
        let &(off, len) = chunks.first()?;
        let header = nfs_formats::mad::MadHeader::parse(&data[off..off + len]).ok()?;
        let mut decoder = nfs_formats::mad::MadDecoder::new(tables, &header);
        let f = decoder.decode_frame(&data[off..off + len]);
        if f.width == 0 || f.height == 0 {
            return None;
        }
        let tex = r.upload_texture(&TextureData::single(f.width as u32, f.height as u32, f.rgba));
        Some(MoviePlayer { data, chunks, decoder, next: 1, period_q16: header.period.max(1) as u64, clock_q16: 0, tex, size: (f.width as f32, f.height as f32) })
    }

    /// Step by `dt_ms`, decoding and uploading the frames the period calls for; a movie that has
    /// run out starts again.
    fn advance(&mut self, r: &mut Renderer, dt_ms: u32) {
        self.clock_q16 += (dt_ms as u64) << 16;
        let mut drawn = false;
        while self.clock_q16 >= self.period_q16 {
            self.clock_q16 -= self.period_q16;
            if self.next >= self.chunks.len() {
                self.next = 0;
            }
            let (off, len) = self.chunks[self.next];
            self.next += 1;
            let _ = self.decoder.decode_chunk(&self.data[off..off + len]);
            drawn = true;
        }
        if drawn {
            let (w, h) = (self.size.0 as u32, self.size.1 as u32);
            r.update_texture(self.tex, &TextureData::single(w, h, self.decoder.rgba()));
        }
    }
}

/// A resolved [`TrackMapEvent`]: the art, already uploaded, at the place its own header gives.
struct TrackMapArt {
    /// texture, width, height, x, y
    map: Option<(TextureId, f32, f32, f32, f32)>,
    marker: Option<(TextureId, f32, f32, f32, f32)>,
    name: String,
}

pub struct LayoutScreen {
    pub layout: Layout,
    /// The background screen this one is laid over (`screen=` → [`background_layout`]).
    background: Option<Box<LayoutScreen>>,
    /// `("Extras.fsh", 3)` → texture and the entry's own pixel size
    shapes: HashMap<(String, usize), (TextureId, f32, f32)>,
    /// `fontPathName` (lower-cased) → font, plus the front end's default
    fonts: HashMap<String, BitmapFont>,
    white: TextureId,
    /// how long the cursor has been animating, for the frame of a multi-frame shape (§4.152).
    /// `u32::MAX` marks a screen that is somebody's background and draws no pointer.
    cursor_ms: u32,
    /// `[0x690794]` and `[0x690790]`: the shape the clock belongs to and the millisecond it was
    /// first requested, restamped when the request changes (0x4E1CCD…0x4E1CE9)
    cursor_shape_at: std::cell::Cell<(usize, u32)>,
    /// the stamp the two arms of 0x4E1D3E keep for each other: the button state and the
    /// millisecond it last changed
    cursor_btn_at: std::cell::Cell<(bool, u32)>,
    /// `[0x69CAE8]` itself — whether a mouse button is down (0x5299C0)
    cursor_btn: bool,
    /// where the 3D room's turntable stands, in turns — what `FECar3DControl`'s zone test reads
    /// out of the room through `[[0x65B928]+0xE8]`'s slot 0xB4 (§4.153)
    car_spin: f32,
    /// each `NFS5.3DCAR`'s opening parts — `FECar3DControl`'s slot 9 and its step
    /// (`crate::carparts`, §4.153), by widget index
    car_parts: HashMap<usize, crate::carparts::CarParts>,
    /// the `NFS5.3DCAR` widgets whose car has a roof animation (`set_car_roof`)
    car_roof: std::collections::HashSet<usize>,
    /// `[screen+0x261]`, "covered" — set by the manager's 0x4D2000 while a dialog is up, and by
    /// 0x4D1E70 on every layer but the top; a covered screen's widgets answer `isDisabled` and draw
    /// `[state.disabled]` where their template has it (§4.147)
    covered: bool,
    /// `(combo element, row)` — the open drop-down's hovered row, kept from the last frame the
    /// pointer was actually on the list. The original keeps it in `[this+0x1A8]` with
    /// `[this+0x1A4]` as "there is one", and **`FEComboBox`'s leave handler 0x4C6F00 does not
    /// clear either when the pointer has gone onto the list's own scrollbar** — 0x4C6F21 asks the
    /// scrollbar at `[this+0x1B8]` whether it has the pointer now and returns before the
    /// `mov byte [esi+0x1A4], 0` at 0x4C6F31. So the row stays lit while you reach for the bar.
    combo_hover: std::cell::Cell<Option<(usize, usize)>>,
    /// widget classes the file does not place, with how many of each were skipped
    unrendered: Vec<(String, usize)>,
    /// element index of an `NFS5.CarLogo` → the `.fsh` pack and the first of its two entries
    logos: HashMap<usize, (String, usize)>,
    /// `NFS5.PANEL` element indices the screen deliberately leaves **closed** — the tabs that are
    /// not the selected one, and the CD player. A closed panel draws nothing (0x4CEBC0 returns at
    /// once when `[panel+0x14C]` is null), so it is not a gap; a panel whose file simply would not
    /// load is not in here and is still counted.
    closed: std::collections::HashSet<usize>,
    /// `NFS5.RADIOGROUP` or `NFS5.PANEL` element index → the layout its `filename` names, loaded
    /// and drawn inside it (`RE/notes/controls.md` §4.1). Four screens embed a tab strip this way:
    /// `Controllers` takes `controllertabs`, `Options` `optiontabs`, `graphics` `graphicstabs` and
    /// `showcase` `showcasetabs`. `Radio Group.Ctr` gives the class one attribute of its own,
    /// `filename`, and nothing drawable; a panel is the container those strips switch between.
    groups: HashMap<usize, Box<LayoutScreen>>,
    /// element index of a `NFS5.CheckBox` → whether its `boolAccessor` is true. A box with no
    /// entry is **unticked**, which is what the original's own registration says: a bool accessor
    /// is a two-entry list (0x4B8720 hands 0x4BC1F0 a count of 2) and every `QUICKFIX.*` is built
    /// with a default index of 0 (0x4F2584).
    checks: HashMap<usize, bool>,
    /// element index of a `NFS5.CarSettingSlider` → the accessor's index and its list's length,
    /// which is what puts the knob along the track (0x4D0C0C…0x4D0C68)
    knobs: HashMap<usize, (i32, i32)>,
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
    /// the string table, shared the same way — a text part's `size` is its string's extent
    fe_strings: Arc<Strings>,
    /// element index of a `NFS5.TrackSmall2dSpline` → the events of the tournament it watches
    track_maps: HashMap<usize, Vec<TrackMapArt>>,
    /// element index of a `NFS5.MOVIEBOX` → the `.mad` it is playing
    movies: HashMap<usize, MoviePlayer>,
    /// element index of a `NFS5.KNOCKOUTTRACKLIST` → how many rows it has and which one is current
    ko_strip: HashMap<usize, (usize, usize)>,
    /// element index of a `NFS5.BADGE` → the player's name, the rank line under it and the colour
    /// the name is printed in
    badges: HashMap<usize, (String, String, [f32; 4])>,
    /// milliseconds this screen has been up, for the map widget's own cycle (0x4FCB50)
    clock_ms: u32,
    /// each `NFS5.MOVIECIRCLE`'s overlay (0x4CDC80), made on its first draw
    movie_overlays: std::cell::RefCell<HashMap<usize, crate::movie_overlay::MovieOverlay>>,
    /// the credits roll the movie circle draws on the credits screen (0x4CE0F0)
    credits_roll: std::cell::RefCell<Option<crate::movie_overlay::CreditsRoll>>,
    /// `FEEndOfEraCallBackList`'s show, when this is the end-of-era screen (`set_end_of_era`)
    end_of_era: Option<end_of_era_draw::EndOfEraScreen>,
    /// `[0x65B940]`, the name-box codes the player has typed (`nfs_game::cheats`) — bit 6 is read
    /// by the credits roll
    pub cheat_bits: u32,
    /// the `weeicons.fsh` entry tagged `GLNT`, when this screen has a `NFS5.HEADER` to sweep it
    /// across — `RE/notes/controls.md` §4.25
    glint: Option<usize>,
    /// the `weeicons.fsh` entry tagged `COLB`, the one sprite `NFS5.ColorBox`'s picker draws
    colb: Option<usize>,
    /// the `weeicons.fsh` entries tagged `arc0`…`arc7`, which `NFS5.CONTROLLERSETUP` looks up by
    /// name (`"arc%d"` 0x51C9DA → 0x4D7B20 over the `"%sweeicons.fsh"` its constructor 0x51BFEE
    /// names) — `RE/notes/controls.md` §4.117
    arcs: Option<[usize; 8]>,
    /// where the pointer is, in design coordinates, when it is over the window at all
    /// (`RE/notes/controls.md` §4.39)
    cursor: Option<(f32, f32)>,
    /// whether the left button is held (`RE/notes/controls.md` §4.40 — type 4 down, type 8 up)
    pressed: bool,
    /// the widget that holds the mouse, if one does — `[window+0x120]`, which a widget takes when
    /// it gains focus and gives back when it loses it (`RE/notes/controls.md` §4.57)
    capture: Option<usize>,
    /// how long the pointer has rested on one widget, and which — the tooltip of §4.52 needs
    /// `[widget+0x12C]`, the tick the hover started
    hover_ms: u32,
    hover_of: Option<usize>,
    /// the first row each list draws — `[this+0x1BC]->slot10()` in the original
    /// (`RE/notes/controls.md` §4.45)
    scroll: HashMap<usize, usize>,
    /// the combo slider whose drop-down list is open, by this screen's own element index
    /// (`RE/notes/controls.md` §4.43 — a combo's list is open exactly while it holds focus)
    open_combo: Option<usize>,
    /// `NFS5.ColorBox`'s picker state — `RE/notes/controls.md` §4.66
    picker: Picker,
    /// the analogue devices the front end can see, in enumeration order — the port's own stand-in
    /// for the record table at `[0x69C9EC]` (`RE/notes/controls.md` §4.17)
    pads: Vec<FePad>,
    /// which pad the `Controller.AllPads` accessor is on (`[accessor+0x28 + pad*4]` selects the
    /// stored value inside every one of the sixteen dead-zone accessors)
    pad_sel: usize,
    /// the sixteen `FEControllerDeadZoneAccessor` values, `[min, max]` per axis per pad — range
    /// 0…0x80, and **0 / 0x80 out of the factory** (0x51E546, 0x51E550)
    deadzone: Vec<[[i32; 2]; 8]>,
    /// the bar and handle a press grabbed — `[this+0x140]`, which slot 5 sets and slot 9 clears
    dz_grab: Option<(usize, u32)>,
    /// `[this+0x144]` — the handle the pointer is **over**, which slot 8 writes whenever nothing
    /// is held (0x51BD02). The draw paints that one amber and the rest dark red.
    dz_hover: Option<(usize, u32)>,
    /// `Camera.Current` — which of the four camera slots is on, for `NFS5.CAMERASELECT` (§4.89)
    camera_current: usize,
    /// which of an open drop-down's scrollbar regions the pointer is on, and whether it is
    /// held — 1 up arrow, 2 down, 3 knob, 4 trough, the numbers 0x4DB2F0 writes (§4.101)
    combo_scroll: Option<(usize, u32, bool)>,
    /// The input boxes that have taken focus — `[widget+0x10C] & 0x40` (§4.109). A set, not one
    /// index, because that is what the binary does: `FeWidget::setFocused` 0x4C7D70 sets its own
    /// flag and calls 0x4C82A0, which is `setState`, and neither it nor anything on that path
    /// clears a sibling's. Cleared when the screen is rebuilt, as the widgets are.
    focused: std::collections::BTreeSet<usize>,
    /// What has been typed into each input box, before it is committed. The original keeps it in
    /// the **accessor** — `FEStringAccessor`'s own `char*` buffer, sized `maxLen + 1` by
    /// `0x59F020` in its ctor 0x4BC8B0 — and the widget only ever calls `vt[0x30]` to insert,
    /// `vt[0x38]` to erase and `vt[0x0C]` to commit. Keeping it beside the widget here is the same
    /// state in a different place: nothing else in this port reads a half-typed value.
    edits: HashMap<usize, String>,
    /// The widgets carrying the **`selected`** flag, `[widget+0x10C] & 8` (§4.109, §4.111). The
    /// front end sets it by name through the helper 0x50D9A0, not from any pointer state.
    selected_flags: std::collections::BTreeSet<usize>,
    /// The widgets the screen has **hidden** — `[widget+0x10C] & 0x20` (§4.112). A hidden widget
    /// draws `[state.hidden]`, whose part list in every shipped template is the non-drawable
    /// `data` alone, so it puts nothing on the screen.
    hidden_flags: std::collections::BTreeSet<usize>,
    /// The widgets the screen has **disabled** — `[widget+0x10C] & 0x10` (§4.113), which
    /// 0x4C831E turns into `[state.disabled]`.
    disabled_flags: std::collections::BTreeSet<usize>,
    /// `NFS5.ChroniclePicture`'s pan: where each widget's picture has been dragged to
    /// (`[widget+0x14C]`, `[+0x150]`), and, while a drag is on, which widget it is, where the
    /// pointer took hold (`[+0x144]`, `[+0x148]`) and the offset it had then (`[+0x154]`,
    /// `[+0x158]`). §4.119.
    picture_pan: std::collections::BTreeMap<usize, (f32, f32)>,
    picture_drag: Option<(usize, f32, f32, f32, f32)>,
    /// The five year-and-variant columns the chronicle's timeline is showing — what 0x507B4F's
    /// walk settles on and writes into `[this+0x160]` and `[this+0x174]` (§4.118, §4.122).
    chronicle_columns: Vec<(u32, usize)>,
    /// The timeline's icon pack, and the entry each (column, row) cell shows in its two states —
    /// `0x507340(year, row, variant, hot)`'s two slots (§4.118, §4.123).
    chronicle_icons: (String, std::collections::BTreeMap<(usize, usize), (usize, usize)>),
    /// the six `WeeIcons.fsh` entries the combo scrollbar's two arrows cycle (§4.90, §4.101)
    combo_arrows: Option<[usize; 6]>,
    /// a list's scrollbar knob parked at the pointer by a press on the **track**, until the first
    /// move puts the model somewhere — `RE/notes/controls.md` §4.46, the tail of 0x4D0995
    knob_at: Option<(usize, f32)>,
    /// where this screen is in its transition
    transition: Transition,
    /// drawn over another screen (the bottom bar, 0x4D39D3) — an overlay must not paint its own fill
    overlay: bool,
}

/// One analogue device as the front end sees it — this port's stand-in for a record of the
/// 0x104-byte table at `[0x69C9EC]`, which 0x51EDEA caches from `0x532E10(i, 6)`.
///
/// `analogue` is `[rec+0] == 4` and the info block's `[+4] & 1`; `axes` is that block's `[+8]`,
/// the **axis bitmask** `0x51F6F0` tests bit by bit; `values` are `[rec+4 + 4n]`, which
/// `0x51F7A0(device, 0x24 + n, &out)` answers. Axis `n` is bit `n`, in the order the sixteen
/// accessors are registered at 0x51E4C0 — `XAxis`, `YAxis`, `ZAxis`, `XRot`, `YRot`, `ZRot`,
/// `SliderA`, `SliderB`, which is `DIJOYSTATE`'s own field order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FePad {
    pub analogue: bool,
    pub axes: u8,
    pub values: [i32; 8],
}

/// `FeListScrollBar::hitTest` 0x4DB2F0 over the combo scrollbar's own rectangles, which the
/// constructor 0x4DAF10 lays out from the bar's `(x, y, 16, h)` (§4.90): the up arrow `(x, y, 16,
/// 13)`, the down arrow `(x, y + h - 13, 16, 13)`, the knob `(x + 1, ky, 14, knob_h)` and the
/// trough itself. The order is the binary's, and each test is inclusive at both ends — 0x4DB331
/// takes `x <= px` from `test ah, 1` after `fcompp` and 0x4DB346 takes `px <= x + w` from
/// `test ah, 0x41`. Returns the number 0x4DB2F0 writes to `[this+0xF0]`, 0 for a miss.
pub fn scrollbar_region(x: f32, y: f32, h: f32, rows: usize, first: usize, px: f32, py: f32) -> u32 {
    const ARROW_H: f32 = 13.0;
    let knob_h = (h - 2.0 * ARROW_H - rows as f32).max(16.0);
    let travel = (h - 2.0 * ARROW_H - knob_h).max(0.0);
    let steps = rows.saturating_sub(1).max(1) as f32;
    let ky = y + ARROW_H + travel * (first as f32 / steps).min(1.0);
    for (n, (rx, ry, rw, rh)) in [
        (x, y, LIST_SCROLLBAR_W, ARROW_H),
        (x, y + h - ARROW_H, LIST_SCROLLBAR_W, ARROW_H),
        (x + 1.0, ky, 14.0, knob_h),
        (x, y, LIST_SCROLLBAR_W, h),
    ]
    .into_iter()
    .enumerate()
    {
        if px >= rx && px <= rx + rw && py >= ry && py <= ry + rh {
            return n as u32 + 1;
        }
    }
    0
}


/// Where `NFS5.DEADZONE`'s live marker sits, relative to the box's left edge — 0x51B7FF…0x51B84B:
///
/// ```text
/// 0x51B7FF  eax = box.w - 10
/// 0x51B802  eax *= axis          ; the device's own reading, 0…65535
/// 0x51B819  fmul [0x5B25C0]      ; 1/65536
/// 0x51B84B  x = box.l + that
/// ```
///
/// The image sets no `DIPROP_RANGE` — 0x56FD70's only `SetProperty` passes GUID **1**,
/// `DIPROP_BUFFERSIZE` — so an absolute axis keeps DirectInput's default 0…65535, and the
/// ten-pixel marker lands exactly inside the box at either end.
pub fn deadzone_marker(width: i32, axis: i32) -> f32 {
    ((width - 10) * axis.clamp(0, 0xFFFF)) as f32 / 65536.0
}

/// The four handles' centres, relative to the box's left edge, **in the order the draw visits
/// them** — 1, 3, 2, 4 (0x51B883, 0x51B924, 0x51B9C3, 0x51BA64). `half` is `(box.w) / 2`
/// (0x51B712) and a value's pixel offset is `v * half / 128` (0x51B717, 0x51B732), so the `.Min`
/// pair rides either side of the centre and the `.Max` pair further out. Each bar is five wide
/// and starts **two** left of its centre (0x51B8B0).
pub fn deadzone_handles(width: i32, dz: [i32; 2]) -> [(u32, f32); 4] {
    let half = (width / 2).max(1);
    let pix = |v: i32| (v * half / 128) as f32;
    let c = half as f32;
    [(1, c - pix(dz[0])), (3, c + pix(dz[0])), (2, c - pix(dz[1])), (4, c + pix(dz[1]))]
}

/// One handle's new `[min, max]` — the two arms of **0x51BD10**, which keep the pair **five**
/// units apart and inside the accessors' own 0…0x80:
///
/// ```text
/// 0x51BD84  regions 1 and 3:  min = clamp(v, 0, max - 5)
/// 0x51BDC9  regions 2 and 4:  max = clamp(v, min + 5, 0x80)
/// ```
///
/// The jump table at 0x51BE14 is what pairs the four handles down to those two arms.
pub fn deadzone_clamp(region: u32, v: i32, dz: [i32; 2]) -> [i32; 2] {
    match region {
        1 | 3 => [v.min(dz[1] - 5).max(0), dz[1]],
        2 | 4 => [dz[0], v.max(dz[0] + 5).min(0x80)],
        _ => dz,
    }
}

/// The sixteen dead-zone accessors out of the factory: every `.Min` 0 and every `.Max` 0x80
/// (0x51E546, 0x51E550).
pub const DEADZONE_DEFAULT: [[i32; 2]; 8] = [[0, 0x80]; 8];

/// `NFS5.ColorBox`'s custom colour editor, as state — `RE/notes/controls.md` §4.47, §4.48, §4.66.
///
/// The class keeps three floats and an index in the widget, and the eight colours it edits in a
/// global array. 0x4D8900, which the mouse slots pack with, reads the three floats as **hue,
/// saturation and luminance**, not red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picker {
    /// `[this+0x288]`, `[this+0x28C]`, `[this+0x290]` — hue, saturation, luminance, each
    /// **0.5 out of the factory** (0x4C38B9)
    pub hsl: (f32, f32, f32),
    /// `[this+0x284]`, the well the editor writes — **0** out of the factory (0x4C38B3)
    pub well: usize,
    /// `[0x691D98 + n·4]`, read by `0x5019B0(n)` and written by `0x5019C0(n, v)` — eight
    /// `0xAARRGGBB` words, **all zero in the image**
    pub wells: [u32; 8],
    /// `[this+0x2A0]`…, the copy the draw takes of all eight the first frame the picker is up
    /// (0x4C3CCE…0x4C3CFA, guarded by `[this+0x2C0]`, which the closed draw clears at 0x4C3D03)
    /// — what **cancel** puts back (0x4C49E6)
    saved: [u32; 8],
}

impl Default for Picker {
    fn default() -> Self {
        Self { hsl: (0.5, 0.5, 0.5), well: 0, wells: [0; 8], saved: [0; 8] }
    }
}

/// The picker's six rects, `(x0, y0, x1, y1)` each, from the widget's own box — §4.47's table,
/// built by the bind 0x4C3A10. Indexed by [`rect`].
pub fn picker_rects(l: f32, t: f32, r: f32, b: f32) -> [(f32, f32, f32, f32); 6] {
    let _ = t;
    let half = ((r - l) / 2.0).trunc();
    [
        (l + 6.0, b + 6.0, l + 0x79 as f32, b + 0xE as f32),      // A, the luminance ramp
        (l + 0x7D as f32, b + 5.0, r - 5.0, b + 0xF as f32),      // B, the current colour
        (l + 6.0, b + 0x15 as f32, r - 6.0, b + 0x74 as f32),     // C, the hue/saturation field
        (l + 5.0, b + 0x7A as f32, r - 5.0, b + 0x84 as f32),     // D, the eight wells
        (l + 5.0, b + 0x89 as f32, l + half - 3.0, b + 0x9B as f32), // E left, "ok"
        (r - half + 3.0, b + 0x89 as f32, r - 5.0, b + 0x9B as f32), // E right, "cancel"
    ]
}

/// Names for [`picker_rects`]' six.
pub mod rect {
    pub const A: usize = 0;
    pub const B: usize = 1;
    pub const C: usize = 2;
    pub const D: usize = 3;
    pub const E_LEFT: usize = 4;
    pub const E_RIGHT: usize = 5;
}

/// The picker's hit test, `NFS5.ColorBox`'s own **0x4C4AD0**, which every one of its three mouse
/// slots runs first and leaves in `[this+0x218]`.
///
/// It tries the rects in a fixed order and stops at the first that contains the point —
/// **C → 0**, **A → 1**, E left → **3**, E right → **4**, D → **2** — and answers **5** when none
/// does. Rect **B**, the swatch, is never tested: it is not clickable. Each test is `jl`/`jg` on
/// both axes, so a rect is **inclusive on both edges**, like `Control::containsPoint` (§4.56).
pub fn picker_region(rects: &[(f32, f32, f32, f32); 6], x: f32, y: f32) -> u32 {
    let inside = |r: (f32, f32, f32, f32)| x >= r.0 && x <= r.2 && y >= r.1 && y <= r.3;
    for (i, region) in [(rect::C, 0u32), (rect::A, 1), (rect::E_LEFT, 3), (rect::E_RIGHT, 4), (rect::D, 2)] {
        if inside(rects[i]) {
            return region;
        }
    }
    5
}

/// **0x4D8900** — hue, saturation and luminance to a packed `0xAARRGGBB`, alpha forced to 0xFF.
///
/// The textbook HLS→RGB: `h` is scaled to degrees by **360.0** (0x5B25CC at 0x4D8904), the
/// midpoint pair is `m2 = l·(1+s)` when `l <= 0.5` and `l + s - l·s` otherwise (0x4D8926 /
/// 0x4D8932, the 0.5 at 0x5B23F0), `m1 = 2·l - m2` (0x4D8944), and a saturation of exactly **0**
/// returns `(l, l, l)` (0x4D8956). The per-channel helper is inlined three times, at `h + 120`,
/// `h` and `h - 120` (0x5B3D60), each wrapped into `[0, 360)` and then split at **60**, **180**
/// and **240** (0x5B26F0, 0x5B6938, 0x5B6930) with a slope of **1/60** (0x5B3E48). The three come
/// out as red, green and blue in that order and are packed `(v·255)` truncated toward zero
/// (0x5A0F98), red at bit 16 (0x4D8B91…0x4D8BCB).
pub fn hls_to_rgb(h: f32, s: f32, l: f32) -> u32 {
    let m2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let m1 = 2.0 * l - m2;
    let channel = |mut hue: f32| {
        if s == 0.0 {
            return l;
        }
        while hue > 360.0 {
            hue -= 360.0;
        }
        while hue < 0.0 {
            hue += 360.0;
        }
        if hue < 60.0 {
            m1 + (m2 - m1) * hue / 60.0
        } else if hue < 180.0 {
            m2
        } else if hue < 240.0 {
            m1 + (m2 - m1) * (240.0 - hue) / 60.0
        } else {
            m1
        }
    };
    let deg = h * 360.0;
    let (r, g, b) = (channel(deg + 120.0), channel(deg), channel(deg - 120.0));
    let byte = |v: f32| ((v * 255.0) as i32 & 0xff) as u32;
    0xff00_0000 | byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// **0x4D8BD0** — the inverse, a packed `0xAARRGGBB` back to hue, saturation and luminance.
///
/// Each byte is scaled by **1/255** (0x5B3888), `l` is `(max + min)·0.5` (0x4D8CBC), a grey
/// (`max == min`) gives `h = s = 0` (0x4D8CD3), and otherwise `s` is `delta/(max+min)` below the
/// 0.5 midpoint and `delta/(2-(max+min))` above it (0x4D8D12 / 0x4D8D18). The hue is the usual
/// six-sector form — `(g-b)/delta`, `2 + (b-r)/delta`, `4 + (r-g)/delta` (0x4D8D43, 0x4D8D64,
/// 0x4D8D85) — then **×60** (0x5B6940), `+360` while negative (0x4D8DA4) and **×1/360**
/// (0x5B3E84), so the float the widget keeps is a fraction of a turn.
pub fn rgb_to_hls(c: u32) -> (f32, f32, f32) {
    let (r, g, b) = (((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - (max + min)) };
    let mut h = if r == max {
        (g - b) / d
    } else if g == max {
        2.0 + (b - r) / d
    } else {
        4.0 + (r - g) / d
    } * 60.0;
    while h < 0.0 {
        h += 360.0;
    }
    (h / 360.0, s, l)
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
/// The sweep takes half of the timer's ticks-per-second (`[0x5DEB48] / 2`, 0x4E0A36), and that is
/// **128**, not 1000: `TIMER_init` (0x565030) stores its own frequency argument into `[0x5DEB48]`
/// — defaulting to 100 and range-checked to 1…10000 — and `main` calls it as `TIMER_init(0x80)`
/// (`timing.md`, via 0x4A5410). So the fade is **64 ticks of the 128 Hz `TIMER_getTicks`**, which
/// is half a second.
///
/// *This comment used to say "the timer is the 1 kHz `timeSetEvent` set up at 0x564F2D, so half a
/// second". The number below was right and the reasoning was not: 1000/2 ticks at 1 kHz and 128/2
/// ticks at 128 Hz are both 0.5 s, so the error cancelled. Corrected 2026-09-21, because the next
/// person to touch either half would have had the two disagree.* CONFIRMED.
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

/// `"%sshow.fsh"` — the pack `NFS5.ANIMSHOWCASEPICTURE` cycles, four pictures and four labels
/// (0x4BDBC5, §4.127).
pub const SHOWCASE_ANIM: &str = "show.fsh";

/// `CameraPicture.ctr`'s `shp` part — `optart.fsh`, indexed by `data.picturenum` (§4.129).
pub const CAMERA_PICTURES: &str = "optart.fsh";
/// `VerticalSlider.ctr`'s `[part.knob]` is `decrement_button.size.width,15`
pub const LIST_KNOB_H: f32 = 15.0;

/// A list's scrollbar, as the port hit-tests it — see [`LayoutScreen::list_scrollbar`].
#[derive(Clone, Copy, Debug)]
pub struct ListScrollbar {
    /// the decrement button, region 1 of 0x4D0CC0
    pub up: (f32, f32, f32, f32),
    /// the increment button, region 2
    pub down: (f32, f32, f32, f32),
    /// the knob, region 3
    pub knob: (f32, f32, f32, f32),
    /// where the knob's travel starts
    pub track_top: f32,
    /// how far it travels — track height less the knob's
    pub span: f32,
    /// how many first rows there are
    pub positions: usize,
    /// which one it is on
    pub at: usize,
}

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
    /// The garage list's own grouping heading — two rules **13 px** apart with the name between
    /// them, drawn by `FEOwnedCarAccessor`'s row painter 0x4F5D54…0x4F5DB3 and costing **14** px
    /// of row height. Its height comes out of slot 26 (0x4F60F0): a row whose era differs from
    /// the one above it is **42** — two headings and the row — one whose class differs is **28**,
    /// and any other row is **14**. `RE/notes/controls.md` §4.144.
    GroupHeading(String),
}

impl ListRow {
    /// How tall this row is. The base accessor answers 14 for every row (0x4BC700); a parts list's
    /// rows are 13 and its headings another 13 (0x516080).
    pub fn height(&self) -> f32 {
        match self {
            ListRow::Text(_) => LIST_ROW_H,
            ListRow::Columns(_) => LIST_ROW_H,
            ListRow::Header(_) => PART_HEADER_H,
            ListRow::GroupHeading(_) => LIST_ROW_H,
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

/// The font `FeComboBox` keeps at `[this+0x184]` and hands both its strings when
/// `data.usesmallfont` is set (0x4C4E8E, 0x4C555E) — the same file as [`DEFAULT_FONT`], named
/// separately because the two are the same only by coincidence.
pub const SMALL_FONT: &str = "univers8b.ffn";
/// `NFS5.ColorBox`'s picker keeps two fonts of its own — `[this+0x294]` and `[this+0x298]`
pub const PICKER_BUTTON_FONT: &str = "universc11b.ffn";
/// The dialogs' two fonts, loaded once for all of them at 0x5121EB / 0x512227 into `[0x69A060]`
/// (the text) and `[0x69A064]` (the buttons) — §4.147.
pub const DIALOG_TEXT_FONT: &str = "univers10b.ffn";
pub const DIALOG_BUTTON_FONT: &str = "universc11m.ffn";
pub const PICKER_TITLE_FONT: &str = "univers7b.ffn";

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

/// A front-end colour, as the shader wants it.
///
/// The original writes these bytes into an 8-bit frame buffer with no colour management at all:
/// a `NFS5.GOTOBUTTON`'s `0xFFBC0000` is red **188** on the player's screen. This port's surface
/// is sRGB, so a value handed to the shader is treated as *linear* and encoded on the way out —
/// 188 came back as **223**, and every flat colour in the front end was that much too light.
/// Undoing the encoding here puts the byte back where the file put it. Measured, not assumed:
/// sampling a pill over black gave 223 before and 188 after.
///
/// Only the three colour channels; alpha is not encoded.
fn tint(c: [u8; 4]) -> [f32; 4] {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    [lin(c[0]), lin(c[1]), lin(c[2]), c[3] as f32 / 255.0]
}

/// `NFS5.MOVIECIRCLE`'s flash frames and its two run fonts (0x4CDE99, 0x4CDED3).
const MOVIE_FLASHES: &str = "circles.fsh";
const RUN_FONTS: [&str; 2] = ["univers7b.ffn", "univers11b.ffn"];
/// The credits roll's body `[+0x21C]` and heading `[+0x220]` fonts.
const ROLL_FONTS: [&str; 2] = ["univers11b.ffn", "univers8b.ffn"];

/// The tag the chronicle's picture tiles carry, which the control's own name repeats at 0x5DBBFE.
const CHRONICLE_PICTURE_TAG: &str = "1111";

/// The two fonts `NFS5.BADGE` loads for itself (0x4CF25C, 0x4CF2A0).
const BADGE_NAME_FONT: &str = "univers14b.ffn";
const BADGE_RANK_FONT: &str = "univers10b.ffn";
/// The drop shadow `NFS5.BADGE` lays over the licence card (0x4CF208).
const LICENSE_SHADOW: &str = "LicenseShadow.fsh";

/// The front end draws its own pointer, out of `FEData/Art/Pointer.fsh` — `controls.md` §4.152.
const POINTER: &str = "Pointer.fsh";

/// The ten shapes, as `{firstFrame, lastFrame}` over `Pointer.fsh`'s 34 entries. This is the table
/// at **0x5D9C8C**, read out pair by pair; every pair lands exactly on a tag boundary in the file
/// (`00`…`05`, `watc`, `mags`, `magP`, `pick`, `Lift`×6, `Pan.`×6, `Poin`×6, `grab`×6).
pub const CURSOR_SHAPES: [(usize, usize); 10] =
    [(0, 0), (1, 5), (9, 9), (10, 15), (16, 21), (28, 33), (22, 27), (7, 7), (8, 8), (6, 6)];

/// How the original steps a shape's frames — **0x4E1C90, read 2026-09-21**, which retires the
/// `CURSOR_FRAME_MS` approximation that used to stand here. The clock is `TIMER_getTicks()`
/// (0x53C2E0 → `[0x6B7C7C]`, **128 Hz**; `RE/notes/timing.md`), and there are two rules:
///
/// * **shape 1**, the hovered arrow (0x4E1CFA): `frame = last − ((t − t_shape) / 16 mod
///   (last − first))`. Sixteen ticks is **125 ms**; the count runs **backwards** from `last`; and
///   the span is `last − first`, not `last − first + 1`, so **`first` is never drawn** — shape 1
///   shows 5, 4, 3, 2, 5, 4, … and never 1. That also makes the `jl` at 0x4E1D31, which would
///   clamp to `first`, unreachable.
/// * **every other shape** (0x4E1D3E), gated on 0x5299C0 = `[0x69CAE8]`, which 0x5297E3 sets from
///   whether a mouse button changed to down: while it is **down** the frame ramps *up* from
///   `first` and clamps at `last`; while it is **up** it ramps *down* from `last` and clamps at
///   `first`. Four ticks is **31.25 ms**. Each arm restamps the *other* one's timer with `now`
///   (0x4E1D68, 0x4E1DB3), so the ramp always measures from the moment the button last changed —
///   which is why `grab` closes as it is pressed and opens as it is let go.
///
/// Both divisors are exact in milliseconds: `16 ticks = ms / 125` and `4 ticks = ms × 32 / 1000`,
/// since flooring the tick count before dividing by an integer is the same as dividing the real
/// tick count.
const CURSOR_SLOW_MS: u32 = 125;

/// Whether one row of a combo scrollbar's sliver falls inside 0x4D94A0's clip box — the trough's
/// **interior**, between the two arrows (0x4DB633…0x4DB653; see `combo_scrollbar`). The arrows are
/// `COMBO_ARROW_H` tall at either end of the bar, so the box is `y + h_arrow … y + h − h_arrow`,
/// and a sliver beside a knob at either extreme is clipped away rather than drawn over an arrow.
pub fn combo_sliver_visible(sy: f32, y: f32, h: f32) -> bool {
    sy >= y + COMBO_ARROW_H && sy < y + h - COMBO_ARROW_H
}

/// The height of each of a combo scrollbar's two arrows, from `weeicons.fsh` (§4.90).
pub const COMBO_ARROW_H: f32 = 13.0;

/// The offset applied to **both** of the pointer's coordinates before the cursor is blitted —
/// 0x4E1DF1…0x4E1E42. Only the requested shape **2** is moved, by `[0x5B2460]` = 6.0; there is no
/// per-shape table in the original, and the `jne` arm passes the coordinates through untouched.
pub fn cursor_hot_spot(shape: usize) -> f32 {
    if shape == 2 { -6.0 } else { 0.0 }
}

/// The frame of a cursor shape, 0x4E1C90's two rules — see [`CURSOR_SLOW_MS`] for where each
/// number comes from. `since_shape` and `since_btn` are milliseconds since the shape and the
/// button state last changed, which is what the original's two mutually-restamped timers measure.
pub fn cursor_frame(shape: usize, (first, last): (usize, usize), since_shape: u32, since_btn: u32, btn: bool) -> usize {
    if shape == 1 {
        // 0x4E1CFA: backwards from `last`, modulo `last − first`, so `first` never shows
        let span = last - first;
        if span == 0 { first } else { last - (since_shape / CURSOR_SLOW_MS) as usize % span }
    } else {
        // 0x4E1D3E: up while the button is down, down while it is up, clamped at both ends
        let d = (since_btn as u64 * 32 / 1000) as usize;
        if btn { (first + d).min(last) } else { last.saturating_sub(d).max(first) }
    }
}

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
        let fe_strings = Arc::new(load_strings(paths));
        let mut screen = Self::load_one(r, paths, name, controls.clone(), fe_strings.clone())?;
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
            match Self::load_one(r, paths, bg, controls, fe_strings) {
                Ok(mut b) => {
                    // a background screen never draws the pointer — the screen in front of it does
                    b.cursor_ms = u32::MAX;
                    screen.background = Some(Box::new(b));
                }
                Err(e) => tracing::debug!(background = bg, error = %e, "background screen unavailable"),
            }
        }
        Self::load_children(r, paths, &mut screen, 0);
        Ok(screen)
    }

    /// The layouts a screen's widgets embed, loaded into [`groups`](Self::groups).
    ///
    /// Two classes carry a `filename`. `NFS5.RADIOGROUP` has nothing else — `Radio Group.Ctr`
    /// gives it that one attribute and no drawable part — and `NFS5.PANEL` is the container the
    /// tabbed screens are built from; `Panel.ctr`'s own comment says so ("A lot of the work for
    /// this is done through code. It will load up and display the lay file given").
    ///
    /// The original's panel: **0x4CEA40** reads `data.filename` into `[panel+0x148]`; **0x4CEA90**
    /// calls the layout loader 0x4E1200 with it, keeps the screen in `[panel+0x14C]`, parents it
    /// (`[loaded+0xD0] = panel`) and forces its `moveable` attribute to 1; **0x4CEBC0**, the
    /// per-frame tick, does *nothing at all* when `[panel+0x14C]` is null and otherwise runs the
    /// loaded screen's own tick and carries the parent's transition into it; **0x4CECE0** keeps it
    /// at the panel's position — it moves the panel by the delta (0x525310 / 0x4C8760) and then
    /// hands the panel's new position to the loaded screen (0x525330 and its vtable +0x3C).
    ///
    /// That last one is why the sub-layout is drawn **offset by the panel's `pos`**, and the files
    /// agree: `usedcarparts.lay` is a 448×183 screen whose widgets run 0..444 in x and 0..179 in y,
    /// and `usedcars.lay` §19 places it at (96, 182) with `size=440+8,175+8` — the same box.
    ///
    /// Two levels deep in the shipped data: `Options.lay` embeds `controllers`, which embeds
    /// `controllertabs` and six more. The depth cap is this port's own guard, not a rule of the
    /// original.
    /// Open or close the `NFS5.PANEL` named `name`, here or in any panel this screen holds —
    /// a panel's slot 27 load (0x4CEA90, `[panel+0x14C]` from the layout loader) the first time it
    /// opens, then 0x4CEC60 (show) or 0x4CEC50 (hide). Answers whether the panel was found.
    pub fn set_panel(&mut self, r: &mut Renderer, paths: &GamePaths, name: &str, open: bool) -> bool {
        let found = self.layout.elements.iter().position(|e| e.class == "NFS5.PANEL" && e.get("name").is_some_and(|n| n.eq_ignore_ascii_case(name)));
        if let Some(i) = found {
            if !open {
                self.closed.insert(i);
                return true;
            }
            self.closed.remove(&i);
            if !self.groups.contains_key(&i)
                && let Some(file) = self.layout.elements[i].get("filename").map(str::to_string)
            {
                match Self::load_one(r, paths, &file, self.controls.clone(), self.fe_strings.clone()) {
                    Ok(mut g) => {
                        g.set_overlay(true);
                        Self::load_children(r, paths, &mut g, 1);
                        self.groups.insert(i, Box::new(g));
                    }
                    Err(e) => {
                        tracing::debug!(file, error = %e, "panel layout unavailable");
                        self.closed.insert(i);
                    }
                }
            }
            return true;
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter().any(|k| self.groups.get_mut(&k).is_some_and(|g| g.set_panel(r, paths, name, open)))
    }

    fn load_children(r: &mut Renderer, paths: &GamePaths, screen: &mut Self, depth: usize) {
        if depth >= 4 {
            return;
        }
        let embedded = |class: &str| class == "NFS5.RADIOGROUP" || class == "NFS5.PANEL";
        let want: Vec<(usize, String, String)> = screen
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| embedded(&e.class))
            .filter_map(|(i, e)| e.get("filename").filter(|f| !f.is_empty()).map(|f| (i, e.class.clone(), f.to_string())))
            .collect();

        // the tab strips first: a strip's buttons are what say which panel of *this* screen is open
        for (i, class, file) in want.iter().filter(|(_, c, _)| c == "NFS5.RADIOGROUP") {
            let _ = class;
            match Self::load_one(r, paths, file, screen.controls.clone(), screen.fe_strings.clone()) {
                Ok(mut g) => {
                    g.set_overlay(true);
                    Self::load_children(r, paths, &mut g, depth + 1);
                    screen.groups.insert(*i, Box::new(g));
                }
                Err(e) => tracing::debug!(file, error = %e, "tab strip unavailable"),
            }
        }

        // every `panel=` a tab button carries, here or in a strip this screen embeds, and whether
        // that button is the selected one
        let mut claimed: HashMap<String, bool> = HashMap::new();
        let mut note = |es: &[nfs_formats::lay::Element]| {
            for e in es {
                if let Some(name) = e.get("panel").filter(|n| !n.is_empty()) {
                    let sel = e.get("state") == Some("selected");
                    let slot = claimed.entry(name.to_ascii_uppercase()).or_insert(false);
                    *slot |= sel;
                }
            }
        };
        note(&screen.layout.elements);
        for g in screen.groups.values() {
            note(&g.layout.elements);
        }

        for (i, class, file) in want.iter().filter(|(_, c, _)| c == "NFS5.PANEL") {
            let _ = class;
            let name = screen.layout.elements[*i].get("name").unwrap_or("").to_ascii_uppercase();
            // a panel a tab button owns is up only while that button is the selected one
            // (0x4D02E0: selecting a tab calls its panel's load, vtable +0x6C = 0x4CEA90, and then
            // shows it, 0x4CEC60). Exactly one button per shipped strip carries `state=selected`.
            if claimed.get(&name) == Some(&false) {
                screen.closed.insert(*i);
                continue;
            }
            // and the one panel the *screen* hides on entry: `Audio.lay`'s CD player, hidden at
            // 0x511A85 (`0x4CEC50` on the `ID_PLAYLISTPANEL` it looks up at 0x511A45). Its own
            // `audioreal.lay` carries the `SHOWPLAYLISTPANEL` callback that raises it.
            if name == "ID_PLAYLISTPANEL" {
                screen.closed.insert(*i);
                continue;
            }
            match Self::load_one(r, paths, file, screen.controls.clone(), screen.fe_strings.clone()) {
                Ok(mut g) => {
                    g.set_overlay(true);
                    Self::load_children(r, paths, &mut g, depth + 1);
                    screen.groups.insert(*i, Box::new(g));
                }
                // A panel whose file will not load draws **nothing, correctly**: 0x4CEA90 hands
                // the name to the loader 0x4E1200 and keeps what comes back in `[panel+0x14C]`,
                // and 0x4CEBC0 returns at once when that is null. `Controllers.lay` §6 is the
                // shipped case — `filename=predefkeys` and no `predefkeys.lay` in the install,
                // and no tab button names that panel either, so nothing ever selects it.
                Err(e) => {
                    tracing::debug!(file, error = %e, "panel layout unavailable");
                    screen.closed.insert(*i);
                }
            }
        }
        // a group is loaded after its parent read `NFS_FE_CURSOR`, and it read the same absolute
        // pair for itself — push the parent's pointer down again, now that the offsets are known
        let at = screen.cursor;
        screen.place_cursor(at);
    }

    fn load_one(r: &mut Renderer, paths: &GamePaths, name: &str, controls: Arc<ControlLibrary>, fe_strings: Arc<Strings>) -> anyhow::Result<Self> {
        let dir = paths.fe_data().join("Layouts");
        // 0x4E1281…0x4E12EF: the loader searches the name for `.lay` and appends it only when it
        // is not already there. Both spellings ship — `filename=audio` and `filename=gearratio.lay`.
        let file = if name.to_ascii_lowercase().contains(".lay") { name.to_string() } else { format!("{name}.lay") };
        let path = GamePaths::find_file(&dir, &file).ok_or_else(|| anyhow::anyhow!("no layout {file} in {}", dir.display()))?;
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
            // `NFS5.AnimatingPicture` cycles a whole run of frames rather than drawing one
            // (§4.53), so its pack has to give up all of them
            if e.class == "NFS5.AnimatingPicture"
                && let Some(file) = e.get("shapePathName").filter(|f| !f.is_empty())
            {
                let a = e.int("start_frame").unwrap_or(0).max(0) as usize;
                let b = e.int("end_frame").unwrap_or(0).max(0) as usize;
                for n in a.min(b)..=a.max(b) {
                    wanted.push((file.to_string(), n));
                }
            }
            // and a bottom-bar button has three runs of its own (§4.53)
            if e.class == "NFS5.BOTTOMBARBUTTON"
                && let Some(file) = e.get("shapePathName").filter(|f| !f.is_empty())
            {
                for (s, t) in [("startframe", "endframe"), ("hoverstartframe", "hoverendframe"), ("hovermousedownstartframe", "hovermousedownendframe")] {
                    let a = e.int(s).unwrap_or(0).max(0) as usize;
                    let b = e.int(t).unwrap_or(0).max(0) as usize;
                    for n in a.min(b)..=a.max(b) {
                        wanted.push((file.to_string(), n));
                    }
                }
            }
            if let Some((file, index)) = e.shape() {
                wanted.push((file.to_string(), index));
            }
            // a `NFS5.CheckBox` keeps both of its entries: the template's part names only the
            // ticked one (`data.onIndex`) and the code swaps in `data.offIndex` (0x4C3730)
            if e.class == "NFS5.CheckBox"
                && let Some(file) = e.get("shapePathName").filter(|v| !v.is_empty())
            {
                for key in ["onIndex", "offIndex"] {
                    if let Some(n) = e.int(key).filter(|n| *n >= 0) {
                        wanted.push((file.to_string(), n as usize));
                    }
                }
            }
            let Some(c) = controls.get(&e.class) else { continue };
            // **every** part of the class, not only the ones its current state draws: a
            // template's geometry can measure a part that is not on screen —
            // `Graphic Tab Button.ctr` centres its caption over `selectedbg.size.width`, which
            // only `[state.selected]` lists, so without the art of every state the four unopened
            // tabs could not place their own captions
            for p in c.parts.values() {
                if p.kind() != nfs_formats::ctr::PartKind::Shape {
                    continue;
                }
                // the same resolution `shape_parts` applies: the part attribute's screen key
                // against the `.lay` block, else the template's own value
                let attr = |key: &str| -> Option<&str> {
                    if let Some(k) = p.screen_key(key)
                        && let Some(v) = e.get(&k).filter(|v| !v.is_empty())
                    {
                        return Some(v);
                    }
                    p.value(key)
                };
                // the same `%<part>.<key>%` indirection the draw resolves: without it the
                // loader asked the art packs for a file literally called
                // `"%picnonsel.shapePathName%"` and quietly got nothing
                let file = match attr("shapePathName").filter(|v| !v.is_empty()) {
                    Some(v) => match v.strip_prefix('%').and_then(|v| v.strip_suffix('%')) {
                        Some(inner) => match inner.split_once('.') {
                            Some(("data", k)) => match e.get(k).filter(|v| !v.is_empty()) {
                                Some(v) => v,
                                None => continue,
                            },
                            Some((part, k)) => match c.part(part).and_then(|q| q.value(k)).filter(|v| !v.is_empty()) {
                                Some(v) => v,
                                None => continue,
                            },
                            None => continue,
                        },
                        None => v,
                    },
                    None => continue,
                };
                let index = match attr("shapeIndex") {
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
        // A `NFS5.HEADER` sweeps a glint across the screen (§4.25). Its activate builds
        // `"%sweeicons.fsh"` from the art directory and looks the entry up **by its tag**,
        // `"GLNT"` (0x4CADC0), so the port does the same rather than carry the index.
        let glint = layout.elements.iter().any(|e| matches!(e.class.as_str(), "NFS5.HEADER" | "NFS5.CarLogo")).then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let n = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("GLNT"))?;
            wanted.push(("weeicons.fsh".to_string(), n));
            Some(n)
        }).flatten();
        // The combo's drop-down grows a scrollbar of its own, and it is **not** the list box's
        // `NFS5.VerticalSlider`: 0x4DADB0 builds a class of its own (vtable 0x5B6A64) and looks
        // `WeeIcons.fsh` up **by tag** for six arrows — `ARup`/`ARov`/`ARin` into
        // `[this+0x13C…0x144]` and `A2up`/`A2ov`/`A2in` into `[+0x148…0x150]` (0x4DAE22…0x4DAE91).
        // This port draws the resting pair. §4.90.
        let combo_arrows = layout
            .elements
            .iter()
            .any(|e| matches!(e.class.as_str(), "NFS5.ComboBoxSlider" | "NFS5.ColorBox"))
            .then(|| {
                let p = GamePaths::find_file(&art, "weeicons.fsh")?;
                let d = std::fs::read(p).ok()?;
                let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
                let at = |tag: &str| pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(tag));
                let six = [at("ARup")?, at("ARov")?, at("ARin")?, at("A2up")?, at("A2ov")?, at("A2in")?];
                for n in six {
                    wanted.push(("weeicons.fsh".to_string(), n));
                }
                Some(six)
            })
            .flatten();
        // `NFS5.ColorBox`'s picker draws one sprite, the `weeicons.fsh` entry tagged **`COLB`**
        // that its factory looks up at 0x4C390C — the same by-tag lookup as the glint's
        // (`RE/notes/controls.md` §4.47, §4.48)
        let colb = layout.elements.iter().any(|e| e.class == "NFS5.ColorBox").then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let n = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("COLB"))?;
            wanted.push(("weeicons.fsh".to_string(), n));
            Some(n)
        }).flatten();
        let arcs = layout.elements.iter().any(|e| e.class == "NFS5.CONTROLLERSETUP").then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let mut arcs = [0; 8];
            for (k, a) in arcs.iter_mut().enumerate() {
                let tag = format!("arc{k}");
                *a = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(&tag))?;
                wanted.push(("weeicons.fsh".to_string(), *a));
            }
            Some(arcs)
        }).flatten();
        // A `NFS5.ListBox` that overflows grows a scrollbar of its own — a `NFS5.VerticalSlider`,
        // which is no widget of any layout, so its art has to be asked for here
        if layout.elements.iter().any(|e| e.class == "NFS5.ListBox")
            && let Some(sb) = controls.get("NFS5.VerticalSlider")
        {
            for p in sb.state_parts("normal").iter().filter_map(|n| sb.part(n)) {
                if p.kind() != nfs_formats::ctr::PartKind::Shape {
                    continue;
                }
                let (Some(f), Some(n)) = (p.value("shapePathName"), p.value("shapeIndex").and_then(|v| v.trim().parse::<usize>().ok())) else { continue };
                wanted.push((f.to_string(), n));
            }
        }
        // The pointer belongs to no widget — the front end draws it itself out of
        // `FEData/Art/Pointer.fsh` every frame (§4.152), so its 34 entries have to be asked for
        // here or nothing would ever load them.
        // `NFS5.MOVIECIRCLE`'s flashes are the five entries of `circles.fsh` its constructor loads
        // (0x52069C…0x5206D5)
        if layout.elements.iter().any(|e| e.class == "NFS5.MOVIECIRCLE") {
            for i in 0..5 {
                wanted.push((MOVIE_FLASHES.to_string(), i));
            }
        }
        for i in 0..34 {
            wanted.push((POINTER.to_string(), i));
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
        // the FE default, and the dialogs' two fonts: a dialog can come up over any screen
        let mut want: Vec<String> = vec![DEFAULT_FONT.to_string(), DIALOG_TEXT_FONT.to_string(), DIALOG_BUTTON_FONT.to_string()];
        // `NFS5.BADGE` builds its own two fonts rather than taking one from the file: 0x4CF25C
        // loads `Univers14b.ffn` for the player's name and 0x4CF2A0 `Univers10b.ffn` for the rank
        // and `NFS5.MOVIECIRCLE`'s overlay two for its runs, `Univers7b.ffn` and `Univers11b.ffn`
        // (0x4CDE99, 0x4CDED3)
        if layout.elements.iter().any(|e| e.class == "NFS5.MOVIECIRCLE") {
            want.push(RUN_FONTS[0].to_string());
            want.push(RUN_FONTS[1].to_string());
            // …and the roll's two, `Univers11B` and `Univers8B` (0x4CDF01, 0x4CDF0E)
            want.push(ROLL_FONTS[0].to_string());
            want.push(ROLL_FONTS[1].to_string());
        }
        if layout.elements.iter().any(|e| e.class == "NFS5.BADGE") {
            want.push(BADGE_NAME_FONT.to_string());
            want.push(BADGE_RANK_FONT.to_string());
        }
        // and `NFS5.ColorBox`'s picker builds two of its own: 0x4C3A3A loads `Universc11b.ffn`
        // into `[this+0x294]` for the buttons and 0x4C3A76 `Univers7b.ffn` into `[+0x298]` for the
        // title (`RE/notes/controls.md` §4.48)
        if layout.elements.iter().any(|e| e.class == "NFS5.ColorBox") {
            want.push(PICKER_BUTTON_FONT.to_string());
            want.push(PICKER_TITLE_FONT.to_string());
        }
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
        Ok(Self { layout, background: None, shapes, fonts, white, cursor_ms: 0, cursor_shape_at: std::cell::Cell::new((usize::MAX, 0)), cursor_btn_at: std::cell::Cell::new((false, 0)), cursor_btn: false, car_spin: 0.0, car_parts: HashMap::new(), car_roof: Default::default(), covered: false, combo_hover: std::cell::Cell::new(None), unrendered, logos: HashMap::new(), checks: HashMap::new(), knobs: HashMap::new(), closed: std::collections::HashSet::new(), groups: HashMap::new(), bars: HashMap::new(), lists: HashMap::new(), car_views: HashMap::new(), track_maps: HashMap::new(), movies: HashMap::new(), ko_strip: HashMap::new(), badges: HashMap::new(),
            // `NFS_FE_CLOCK` preloads the screen clock in milliseconds, so a still can be taken
            // part way through an animation the front end would otherwise only show in motion
            clock_ms: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(0), glint, colb, arcs,
            movie_overlays: Default::default(),
            credits_roll: Default::default(),
            end_of_era: None,
            cheat_bits: 0,
            // `NFS_FE_CURSOR=x,y` parks the pointer for a still, the way `NFS_FE_CLOCK` parks the
            // clock — a screenshot has no pointer of its own
            cursor: std::env::var("NFS_FE_CURSOR").ok().and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            }),
            // `NFS_FE_PRESS=1` holds the button down for a still, the pair of `NFS_FE_CURSOR`
            pressed: std::env::var("NFS_FE_PRESS").ok().is_some_and(|v| v != "0"),
            capture: None,
            // `NFS_FE_OPEN=<index>` drops a combo's list open for a still
            open_combo: std::env::var("NFS_FE_OPEN").ok().and_then(|v| v.parse().ok()),
            picker: Picker::default(),
            pads: Vec::new(),
            pad_sel: 0,
            deadzone: Vec::new(),
            dz_grab: None,
            dz_hover: None,
            camera_current: 0,
            combo_scroll: None,
            focused: Default::default(),
            edits: HashMap::new(),
            selected_flags: Default::default(),
            hidden_flags: Default::default(),
            disabled_flags: Default::default(),
            picture_pan: Default::default(),
            picture_drag: None,
            chronicle_columns: Vec::new(),
            chronicle_icons: (String::new(), Default::default()),
            combo_arrows,
            knob_at: None,
            scroll: HashMap::new(),
            hover_ms: 0,
            hover_of: None,
            controls, fe_strings, transition: Transition::Idle, overlay: false })
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
    fn reference(&self, e: &nfs_formats::lay::Element, name: &str, depth: u32, measure: bool) -> Option<f64> {
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
        // A drawable part's `size` is the extent of **what it draws** when the template does not
        // give it one: `Picture.ctr`'s picture part and `BottomBarButton.ctr`'s label both carry
        // `size.flags=HIDDEN|READONLY` and no `size.value` at all, and the bar button's own
        // position is written over `label.size.width + picture.size.width`. HIGH — the rule is
        // what the expressions require, the point at which the original measures was not traced.
        let pair = match part.value(key).filter(|v| !v.trim().is_empty()) {
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1, measure))?,
            // a measured extent is only for an explicit `<part>.size.*` reference, never for the
            // widget's own box: `Label.ctr`'s `size.value` is `label.size.width*2,…`, and letting
            // that resolve would give every label a box twice its text and move it, because this
            // port implements the alignment rule as an anchor in the box rather than as the part
            // offset the template writes (`(0-label.size.width)/2*data.align`, `layouts.md` §2.1)
            None if key == "size" && measure => self.part_extent(e, part)?,
            None => return None,
        };
        match field {
            "width" | "x" => Some(pair.0),
            "height" | "y" => Some(pair.1),
            _ => None,
        }
    }

    /// Every part expression on this screen that does **not** resolve, as
    /// `(class, part, key, expression)`.
    ///
    /// `reference` answers `None` for a name it cannot resolve and every caller falls back to a
    /// zero or to the widget's box, so a template the port cannot evaluate is silent. This is the
    /// measurement of that: `--layout-sweep` with `NFS_EXPR_SWEEP=1` runs it over all 98 layouts.
    pub fn expr_failures(&self) -> Vec<(String, String, String, String)> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter() {
            let Some(c) = self.controls.get(&e.class) else { continue };
            let state = e.get("state").unwrap_or("normal");
            for name in c.state_parts(state) {
                let Some(p) = c.part(&name) else { continue };
                for key in ["pos", "size"] {
                    let Some(v) = self.part_attr(e, p, key).filter(|v| !v.trim().is_empty()) else { continue };
                    if nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)).is_none() {
                        out.push((e.class.clone(), name.clone(), key.to_string(), v.to_string()));
                    }
                }
            }
        }
        out
    }

    /// The shared bar painter **0x521CB0**: the span `lo … hi` of the box, as one cell per per
    /// cent, coloured by `at`. `NFS5.GouraudIndicatorBar` and `NFS5.DEADZONE` both draw through it.
    fn gouraud_bar(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, lo: f32, hi: f32, at: &dyn Fn(f32) -> [f32; 4], shadow: Option<bool>) {
        if hi <= lo {
            return;
        }
        let cell = w / 100.0;
        for p in lo as u32..hi as u32 {
            out.push(self.quad(aspect, x + p as f32 * cell, y, cell, h.max(1.0), at(p as f32 + 0.5)));
        }
        // …and, last, a **drop shadow** over what it has just painted — 0x521FC2…0x52203C, which
        // is what `data.hasshadow` turns on. It is the painter's tenth argument; the eleventh
        // picks the offset, and both arms go through 0x520F00:
        //
        // ```text
        // 0x521FAD  total = [F+0x20] + [F+0x1C], the two segments' widths together
        // 0x521FB1  total <= [0x5B2404] -> nothing
        // 0x521FC2  arg10 clear          -> nothing
        // 0x521FD2  arg11 set            -> 0x520F00(cursor, box.t, total, box.b - box.t, fade, -4, 4)
        // 0x522013  arg11 clear          -> the same with **3, 4**
        // ```
        //
        // `NFS5.GouraudIndicatorBar` passes `data.hasshadow` (`[this+0x160]`, 0x4CA735) for the
        // tenth and `data.goingleft` (`[this+0x15C]`, the same byte that mirrors the span) for the
        // eleventh, so a bar that runs leftwards throws its shadow leftwards. `NFS5.DEADZONE`
        // passes **1** for the tenth on both bands and 1 / 0 for the eleventh (0x51B739,
        // 0x51B7BC), which mirrors the pair about the bar's centre the way §4.17 describes.
        // §4.79.
        if let Some(mirrored) = shadow {
            let (a, b) = if mirrored { (-4.0, 4.0) } else { (3.0, 4.0) };
            self.drop_shadow(out, aspect, x + lo * cell, y, (hi - lo) * cell, h.max(1.0), a, b);
        }
    }

    /// `NFS5.DEADZONE`'s hit test, **0x51BBA0** — which of the bar's four handles the pointer is
    /// on, or 0 for none.
    ///
    /// The handles sit at `x0 + half ∓ minPx` and `x0 + half ∓ maxPx`, each with a **±2**
    /// tolerance, and they are tested in the order 1, 3, 2, 4 with no early exit, so a later hit
    /// wins (0x51BC28…0x51BC90). The jump table at 0x51BE14 then sends **1 and 3** to the `.Min`
    /// arm and **2 and 4** to the `.Max` one.
    fn deadzone_region(&self, e: &nfs_formats::lay::Element, i: usize, px: f32) -> u32 {
        let _ = i;
        let (x0, _) = self.position_of(e);
        let (w, _) = self.box_of(e, 0);
        let half = (w as i32 / 2).max(1);
        let n = self.attr_num(e, "hackindex").unwrap_or(0.0) as usize;
        let dz = self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)[n.min(7)];
        let px = px as i32;
        let pix = |v: i32| v * half / 128;
        let c = x0 as i32 + half;
        let mut hit = 0;
        for (centre, region) in [(c - pix(dz[0]), 1), (c + pix(dz[0]), 3), (c - pix(dz[1]), 2), (c + pix(dz[1]), 4)] {
            if px >= centre - 2 && px <= centre + 2 {
                hit = region;
            }
        }
        hit
    }

    /// `NFS5.DEADZONE`'s setter, **0x51BD10** — the value is **centre-relative**, and the two
    /// handles keep five units apart:
    ///
    /// ```text
    /// 0x51BD4F  half  = (box.r - box.l) / 2
    /// 0x51BD51  v     = |x - box.l - half| * 128 / half
    /// 0x51BD84  region 1 or 3: accessorMin = clamp(v, 0, accessorMax - 5)
    /// 0x51BDC9  region 2 or 4: accessorMax = clamp(v, accessorMin + 5, 0x80)
    /// ```
    fn deadzone_set(&mut self, e: &nfs_formats::lay::Element, region: u32, px: f32) {
        if region == 0 {
            return;
        }
        let (x0, _) = self.position_of(e);
        let (w, _) = self.box_of(e, 0);
        let half = (w as i32 / 2).max(1);
        let v = ((px as i32 - x0 as i32 - half).abs() * 128 / half).clamp(0, 0x80);
        let n = (self.attr_num(e, "hackindex").unwrap_or(0.0) as usize).min(7);
        let pad = self.pad_sel;
        if self.deadzone.len() <= pad {
            self.deadzone.resize(pad + 1, DEADZONE_DEFAULT);
        }
        self.deadzone[pad][n] = deadzone_clamp(region, v, self.deadzone[pad][n]);
    }

    /// The bar the pointer pressed on, and which handle — `FEDeadZone`'s slot 5, **0x51BB60**: it
    /// hit-tests (0x51BBA0), keeps the answer in `[this+0x140]`, takes focus (0x4C7D70) and sets
    /// the value straight away (0x51BD10). Answers whether it took the press.
    pub fn deadzone_press(&mut self, px: f32, py: f32) -> bool {
        let Some((i, region)) = self.deadzone_at(px, py) else { return false };
        self.dz_grab = Some((i, region));
        let e = self.layout.elements[i].clone();
        self.deadzone_set(&e, region, px);
        true
    }

    /// Slot 8, **0x51BCD0** — two arms. While `[this+0x140]` holds a handle it drags it
    /// (0x51BCEB → 0x51BD10); with nothing held it hit-tests instead and keeps the answer in
    /// `[this+0x144]` (0x51BCFD → 0x51BD02), which is the **hover** the draw colours amber.
    pub fn deadzone_drag(&mut self, px: f32, py: f32) {
        if let Some((i, region)) = self.dz_grab {
            let e = self.layout.elements[i].clone();
            self.deadzone_set(&e, region, px);
            return;
        }
        self.dz_hover = self.deadzone_at(px, py);
    }

    /// Slot 9, **0x51BCB0** — the release clears the focus and drops the grab.
    pub fn deadzone_release(&mut self) {
        self.dz_grab = None;
    }

    /// The `NFS5.GouraudIndicatorBar` under the pointer, if it is one the pointer may move —
    /// `FeGouraudIndicatorBar`'s slot 5, **0x4CA470**, and its slot 8, **0x4CA590**, which are the
    /// same routine twice:
    ///
    /// ```text
    /// 0x4CA476  [this+0x150] == 0 -> false        ; data.changeable, the bind at 0x4CA256
    /// 0x4CA49C  0x4E2AB0([this+0x140], ev.x, ev.y) -> false when the pointer misses the part
    /// 0x4CA4B1  box  = 0x4C8670(this)
    /// 0x4CA4E2  left = 0x4E2B20([this+0x140], &box)      ; the part's own left
    /// 0x4CA4F9  n    = accessor->slot9()                 ; how many values it has
    /// 0x4CA512  w    = 0x4E2B90([this+0x140])            ; the part's width
    /// 0x4CA51F  v    = trunc(n * (ev.x - left) / w)      ; 0x5A0F98 truncates
    /// 0x4CA535  v >= n -> v = n - 1
    /// ```
    ///
    /// `GouraudIndicatorBar.ctr`'s only rect, `area`, is `pos=0,0` sized
    /// `control.size.width,control.size.height` `HIDDEN|READONLY`, so the part **is** the widget's
    /// box and `left`/`w` are its own. What comes back here is the accessor's name and the
    /// fraction `(ev.x - x) / w`; the count and the clamp belong to whoever owns the accessor.
    ///
    /// Eleven of the 42 shipped bars carry `changeable=1` — the five volume levels of
    /// `audioreal.lay`, the five of `forcefeedback.lay` and `advgraphics.lay`'s gamma.
    pub fn bar_at(&self, px: f32, py: f32) -> Option<(usize, String, f32)> {
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.class != "NFS5.GouraudIndicatorBar" || self.attr_num(e, "changeable").unwrap_or(0.0) == 0.0 {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_of(e, 0);
            let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
            if !(px >= x && px <= x + w && py >= y && py <= y + h) || w <= 0.0 {
                continue;
            }
            let name = e.get("accessor").or_else(|| e.get("othersource")).filter(|v| !v.is_empty())?;
            return Some((i, name.to_string(), (px - x) / w));
        }
        None
    }

    /// Which `NFS5.DEADZONE` the pointer is inside, and on which handle.
    fn deadzone_at(&self, px: f32, py: f32) -> Option<(usize, u32)> {
        if !self.any_analogue() {
            return None;
        }
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.class != "NFS5.DEADZONE" {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_of(e, 0);
            if !(px >= x && px <= x + w && py >= y && py <= y + h) {
                continue;
            }
            let r = self.deadzone_region(e, i, px);
            if r != 0 {
                return Some((i, r));
            }
        }
        None
    }

    /// Tell the screen which camera slot is on — `Camera.Current`, the accessor
    /// `NFS5.CAMERASELECT` compares against its own `data.accessornum` (§4.89).
    pub fn set_camera_current(&mut self, n: usize) {
        self.camera_current = n;
        for g in self.groups.values_mut() {
            g.set_camera_current(n);
        }
    }

    /// The sixteen dead-zone values for the selected pad, `[min, max]` per axis.
    pub fn deadzone_values(&self) -> [[i32; 2]; 8] {
        self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)
    }

    /// Put them back — what the save block carries (`savegame.md` §472).
    pub fn set_deadzone_values(&mut self, pad: usize, v: [[i32; 2]; 8]) {
        if self.deadzone.len() <= pad {
            self.deadzone.resize(pad + 1, DEADZONE_DEFAULT);
        }
        self.deadzone[pad] = v;
    }

    /// Hand the front end the analogue devices it can see, and which one is selected. Empty —
    /// which is what this port has with no pad attached — makes `NFS5.DEADZONE` draw nothing, the
    /// same answer 0x51B60D gives.
    pub fn set_pads(&mut self, pads: Vec<FePad>, selected: usize) {
        self.deadzone.resize(pads.len().max(1), DEADZONE_DEFAULT);
        self.pad_sel = selected.min(pads.len().saturating_sub(1));
        self.pads = pads.clone();
        for g in self.groups.values_mut() {
            g.set_pads(pads.clone(), selected);
        }
    }

    /// Is any analogue device enumerated — `0x51F2B0`, which walks the device list and answers true
    /// at the first record whose class byte is **4** and whose info block's `[+4] & 1` is set.
    fn any_analogue(&self) -> bool {
        self.pads.iter().any(|p| p.analogue)
    }

    /// The **n-th** analogue device — `0x51F1D0`, which counts only those records and answers the
    /// device index of the one the accessor's pad number names.
    fn nth_analogue(&self, n: usize) -> Option<&FePad> {
        self.pads.iter().filter(|p| p.analogue).nth(n)
    }

    /// `NFS5.DEADZONE`'s eight bars — its own **slot 15**, 0x51B5F0, read end to end
    /// (`RE/notes/controls.md` §4.17).
    ///
    /// ```text
    /// 0x51B606  no analogue device at all -> draw nothing          ; 0x51F2B0
    /// 0x51B622  device = 0x51F1D0(accessorMin->pad)
    /// 0x51B639  hackindex 0…7 -> capability 0x24…0x2B
    /// 0x51B687  0x51F7A0 gates on the axis bit and answers its value; false -> draw nothing
    /// 0x51B70B  half = (r - l) / 2
    /// 0x51B714  minPixels = accessorMin * half / 128
    /// 0x51B72D  maxPixels = accessorMax * half / 128
    /// 0x51B748  minPct = minPixels * 100 / half,  maxPct = maxPixels * 100 / half
    /// 0x51B784  band 1: box (l+1, t, l+1+half, b), span 100-maxPct … 100-minPct,
    ///                   stops 0xFFFDB306, 0xFFFFD76C, 0xFFE60000
    /// 0x51B7D0  band 2: box (l+half-1, t, l+2*half-1, b), span minPct … maxPct,
    ///                   stops 0xFFE60000, 0xFFFFD76C, 0xFFFDB306   — the triple reversed
    /// ```
    ///
    /// so the bar mirrors about its centre, and the factory's 0 / 0x80 lights both halves whole.
    fn deadzone_bars(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, x: f32, y: f32, w: f32, h: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        if !self.any_analogue() {
            return;
        }
        let n = self.attr_num(e, "hackindex").unwrap_or(0.0) as usize;
        if n > 7 {
            return;
        }
        let Some(pad) = self.nth_analogue(self.pad_sel) else { return };
        if pad.axes >> n & 1 == 0 {
            return;
        }
        let half = (w as i32 / 2).max(1);
        let dz = self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)[n];
        let pct = |v: i32| {
            let pixels = v * half / 128;
            (pixels * 100 / half) as f32
        };
        let (lo, hi) = (pct(dz[0]), pct(dz[1]));
        let ramp = |a: [u8; 4], b: [u8; 4], c: [u8; 4]| {
            let (a, b, c) = (tint(a), tint(b), tint(c));
            move |p: f32| -> [f32; 4] {
                let (s, t, u) = if p < 50.0 { (a, b, p / 50.0) } else { (b, c, (p - 50.0) / 50.0) };
                std::array::from_fn(|j| s[j] + (t[j] - s[j]) * u)
            }
        };
        const RED: [u8; 4] = [0xE6, 0x00, 0x00, 0xFF];
        const PALE: [u8; 4] = [0xFF, 0xD7, 0x6C, 0xFF];
        const ORANGE: [u8; 4] = [0xFD, 0xB3, 0x06, 0xFF];
        let hw = half as f32;
        self.gouraud_bar(out, aspect, x + 1.0, y, hw, h, 100.0 - hi, 100.0 - lo, &ramp(ORANGE, PALE, RED), Some(true));
        self.gouraud_bar(out, aspect, x + hw - 1.0, y, hw, h, lo, hi, &ramp(RED, PALE, ORANGE), Some(false));

        // The five bars over the gradient — 0x51B7FA…0x51BAE1. Each is a `0x4D9280` fill with an
        // outline primitive over it, and all five share the box's own height.
        //
        // ```text
        // 0x51B802  marker : x = box.l + (box.w - 10) * axis / 65536   (1/65536 at 0x5B25C0)
        // 0x51B852           0x4D9280(x, y, 10.0, h, 0x4D87C0(fade))   — always amber
        // 0x51B875           0x521810(x, y, 10.0, h, fade)             — its outline
        // 0x51B8B0  handle : x = box.l + half -+ pixels - 2,  5.0 wide
        // 0x51B889           0x4D87C0(fade) amber when [this+0x144] names it, else 0x4D87B0 red
        // 0x51B903           0x521050(x, y, 5.0, h, fade, 3) when [this+0x140] holds it,
        // 0x51B916           0x521250(...) otherwise
        // ```
        //
        // and the four handles are visited **1, 3, 2, 4** (0x51B883, 0x51B924, 0x51B9C3,
        // 0x51BA64) — the min pair either side of the centre first, then the max pair — which is
        // the order the hit test walks them in too.
        //
        // The marker's divisor pins the device's range: the image sets **no** `DIPROP_RANGE`
        // (0x56FD70's only `SetProperty` is `DIPROP_BUFFERSIZE`, GUID 1), so an absolute axis
        // keeps DirectInput's own 0…65535, and `(w - 10) * v / 65536` puts a ten-pixel marker
        // exactly inside the box at either end.
        let mx = x + deadzone_marker(w as i32, pad.values[n]);
        out.push(self.quad(aspect, mx, y, 10.0, h, tint([0xFF, 0xBB, 0x00, 0xFF])));
        // and 0x521810's inset highlight over it (0x51B875)
        self.glass(out, aspect, mx, y, 10.0, h, &tint);
        for (region, centre) in deadzone_handles(w as i32, dz) {
            let hot = self.dz_hover.is_some_and(|(_, r)| r == region);
            let col = if hot { [0xFF, 0xBB, 0x00, 0xFF] } else { [0xBC, 0x00, 0x00, 0xFF] };
            let hx = x + centre - 2.0;
            out.push(self.quad(aspect, hx, y, 5.0, h, tint(col)));
            // the held handle takes 0x521050's sunken bevel and every other one 0x521250's
            // raised outline — §4.77 reads the second out band by band
            if self.dz_grab.is_some_and(|(_, r)| r == region) {
                self.bevel(out, aspect, hx, y, 5.0, h, 3.0);
            } else {
                self.bevel_glow(out, aspect, hx, y, 5.0, h, 3.0);
            }
        }
    }

    /// What one part of a widget actually covers: a `SHAPE`'s `.fsh` entry at its own pixel size,
    /// or a `TEXT`/`LOCALIZEDTEXT`'s string measured in the font that part draws it in.
    fn part_extent(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part) -> Option<(f64, f64)> {
        match p.kind() {
            nfs_formats::ctr::PartKind::Shape => {
                // the same resolution `shape_parts` uses — the part attribute's screen key against
                // the block, else the template's own value. `Graphic Tab Button.ctr`'s
                // `selectedbg` has an **empty** `shapePathName.value` and only the screen key
                // `selectedbgSmallPathName` reaches its pack, so without this the four unopened
                // tabs could not measure the art they centre their captions over.
                let file = self.part_attr(e, p, "shapePathName").filter(|v| !v.is_empty())?;
                let index = match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)).filter(|n| *n >= 0.0)? as usize,
                    None => 0,
                };
                let &(_, w, h) = self.shapes.get(&(file.to_string(), index))?;
                Some((w as f64, h as f64))
            }
            nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText => {
                let text = match e.get("text").filter(|t| !t.is_empty()) {
                    Some(t) => t.to_string(),
                    None => self.fe_strings.get(e.string_id()?).filter(|t| !t.is_empty())?.to_string(),
                };
                let named = e
                    .get("fontPathName")
                    .filter(|f| !f.is_empty())
                    .or_else(|| p.value("fontPathName").filter(|f| !f.is_empty()))
                    .map(|f| f.to_lowercase());
                let f = named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))?;
                let (_, w) = f.place(&text);
                Some((w as f64, f.line_px() as f64))
            }
            _ => None,
        }
    }

    /// The widget's box. The `.lay` block's `size` wins; failing that the class template's own
    /// `size.value`, which for most classes is an expression over the block's keys
    /// (`data.width,data.height` for a button, `data.width,data.selectionboxmaxheight` for a list).
    fn box_of(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        // an autosizing class is as big as what it draws (§4.62), and `NFS_BOX_SWEEP=1` measured
        // where that differs from the template's `size.value`: 411 headers and labels whose
        // expression this port cannot evaluate and so answered (0, 0), 82 car-setting buttons
        // whose art is 15 wide where the template says 13, and four camera pictures the block
        // calls 15x13 and whose parts cover 105x78.
        //
        // **Only at depth 0**, which is to say only for the widget's own box and never for a
        // `control.size.*` term inside a part's expression. §4.64: those two are not the same
        // thing in the original either — 31 of the templates that reference `control.size.width`
        // autosize, and if that term went through the box the union would re-enter itself before
        // `computeBox` sets its valid flag (0x525516) and never come back. The term is the `size`
        // attribute; the box is the union.
        if depth == 0
            && e.pair("size").is_none()
            && self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false)
            && let Some(u) = self.parts_union(e)
        {
            return u;
        }
        self.box_sized(e, depth, false)
    }

    /// The box a widget is **hit-tested** against, which is the template's `size.value` with the
    /// part-relative terms measured — `box_of` leaves those unresolved for the reason it gives, and
    /// the difference only ever shows on a class whose size is written over its parts.
    ///
    /// Two of them have a hover state and so can be hit at all: `BottomBarButton.ctr`'s
    /// `(label.size.width + picture.size.width + data.space), data.height` and
    /// `Graphic Tab Button.ctr`'s `hoverbg.size.width, hoverbg.size.height`. Without this the box
    /// came out (0, 0) and the 27 of them could not be pointed at (§4.53, §4.54).
    ///
    /// **Only where `size.value` is live.** §4.62: `Control::getSize` 0x525360 and
    /// `computeBox` 0x5253C0 both branch on the `autosize` attribute (`[this+0xF0]`, bound at
    /// 0x52498A), and when it is set the box is the **union of the parts the current state
    /// draws**, not the `size` expression at all. Seven of the seventy shipped templates set it to
    /// 0 — `BottomBarButton`, `ComboBox`, `ColorBox`, `GoToButton`, `Graphic Tab Button`,
    /// `MultiAccessor` and `OvalThing` — and only those seven have a `size.value` the original
    /// ever evaluates.
    fn box_hit(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        let live = self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() == "0").unwrap_or(false);
        if !live && let Some(p) = self.parts_union(e) {
            return p;
        }
        self.box_sized(e, 0, live)
    }

    /// Every widget whose two boxes disagree: what the template's `size.value` says and what the
    /// union of its parts says. For an autosizing class the original uses the union (§4.62), and
    /// this port's *drawing* still uses `size.value` — so this is the list of places where that
    /// choice could show. `NFS_BOX_SWEEP=1` prints it.
    pub fn box_disagreements(&self) -> Vec<(String, (f32, f32), (f32, f32))> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter().skip(1) {
            let autosizes = self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false);
            if !autosizes || e.pair("size").is_some() {
                continue;
            }
            let Some(u) = self.parts_union(e) else { continue };
            let b = self.box_of(e, 0);
            if (b.0 - u.0).abs() > 1.0 || (b.1 - u.1).abs() > 1.0 {
                out.push((e.class.clone(), b, u));
            }
        }
        for g in self.groups.values() {
            out.extend(g.box_disagreements());
        }
        out
    }

    /// The box an **autosizing** widget gets: the union of the rects of the parts its state draws,
    /// started at the origin — `Control::computeBox` 0x5253C0, the branch at 0x525446.
    ///
    /// ```text
    /// left = top = right = bottom = 0                      ; 0x5253E9…0x525403
    /// for each name in the state's part list:
    ///     part = lookupPart(this, name)                     ; 0x525620, skipped when < 0
    ///     r    = part->slot2()                              ; the part's own rect
    ///     left = min(left, r.left);  top    = min(top,    r.top)
    ///     right= max(right, r.right); bottom = max(bottom, r.bottom)
    /// ```
    ///
    /// Because it starts at zero the box always contains the widget's own origin, so it can only
    /// grow from there. The port takes each part's `pos` and `size` the way it draws them, falling
    /// back to what the part measures when the template gives it no size (§4.54's `part_extent`).
    fn parts_union(&self, e: &nfs_formats::lay::Element) -> Option<(f32, f32)> {
        let c = self.controls.get(&e.class)?;
        let state = e.get("state").unwrap_or("normal");
        let (mut r, mut b) = (0.0f32, 0.0f32);
        let (mut l, mut t) = (0.0f32, 0.0f32);
        let mut any = false;
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            // only what the state **draws**. A `DATA` part is the widget's attribute block, not a
            // rect, and its `pos` carries a screen key that resolves to the widget's own `pos=` —
            // unioning that would drag the box back to the screen's origin.
            if !matches!(
                p.kind(),
                nfs_formats::ctr::PartKind::Rect
                    | nfs_formats::ctr::PartKind::Shape
                    | nfs_formats::ctr::PartKind::Text
                    | nfs_formats::ctr::PartKind::LocalizedText
            ) {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let (px, py) = at("pos").unwrap_or((0.0, 0.0));
            let (pw, ph) = match at("size") {
                Some(v) => v,
                None => self.part_extent(e, p).unwrap_or((0.0, 0.0)),
            };
            l = l.min(px as f32);
            t = t.min(py as f32);
            r = r.max(px as f32 + pw as f32);
            b = b.max(py as f32 + ph as f32);
            any = true;
        }
        (any && r - l >= 1.0 && b - t >= 1.0).then_some((r - l, b - t))
    }

    fn box_sized(&self, e: &nfs_formats::lay::Element, depth: u32, measure: bool) -> (f32, f32) {
        if let Some(p) = e.pair("size") {
            return p;
        }
        // **A `size` with one usable component overwrites only the width.** 0x4B5936 skips an
        // empty token without emitting a value, so `size=150,` — the three identical
        // `NFS5.Rectangle`s of `location.lay` §12, `locationMP.lay` §16 and `locationQK.lay` §16
        // — parses to the single number 150, and the height stays whatever the template put
        // there: `Rectangle.ctr`'s `size.value=50,50`. §4.116.
        let given = e.parts_of("size");
        if given.len() == 1 {
            let h = self.box_template(e, depth, measure).map(|(_, h)| h).unwrap_or(0.0);
            return (given[0], h);
        }
        if let Some(p) = self.box_template(e, depth, measure) {
            return p;
        }
        (e.float("width").unwrap_or(0.0), e.float("height").unwrap_or(0.0))
    }

    /// The class template's own `size.value`, evaluated.
    fn box_template(&self, e: &nfs_formats::lay::Element, depth: u32, measure: bool) -> Option<(f32, f32)> {
        if depth > 4 {
            return None;
        }
        let c = self.controls.get(&e.class)?;
        let v = c.value("size")?;
        nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1, measure)).map(|(w, h)| (w as f32, h as f32))
    }

    /// The widget's position: the block's own `pos`, or the class template's `[control] pos.value`
    /// — `NFS5.HEADER` carries no `pos` at all and `Header.ctr` puts it at (6, 4).
    fn position_of(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        if let Some(p) = e.pair("pos") {
            return p;
        }
        if let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("pos")
            && let Some((x, y)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true))
        {
            return (x as f32, y as f32);
        }
        (0.0, 0.0)
    }

    /// Whether the widget's `state` selects **no drawable part at all**, in which case it draws
    /// nothing however much its `.lay` block carries.
    ///
    /// A `.lay` block's `state=` is not decoration: it is the name of one of the template's
    /// `[state.<name>]` sections, and that section's `parts=` *is* the widget's draw list.
    /// The chain in the original:
    ///
    /// * the nine state names live in a 25-byte-stride table at **0x5D81FC**, ending at 0x5D82DD —
    ///   `normal`, `stringnotused`, `hover`, `selected`, `disabled`, `hidden`, `focused`,
    ///   `focusedhover`, `selectedhover`, in that order;
    /// * **0x4C778F…0x4C7818** walks that table and, for each name the template's `states=` list
    ///   carries, sets bit `1 << i` in `[widget+0x110]` — the states the class supports;
    /// * **0x4C7820…0x4C78B2** clears `[widget+0x10C]` and matches the widget's own `state`
    ///   attribute against names 2…8, setting that name's bit on a hit;
    /// * **0x4C82A0** turns those flags back into a name by priority — 0x20 → `"hidden"`
    ///   (0x4C82FE), 0x10 → `"disabled"`, 0x40+0x04 → `"focusedhover"`, 0x40 → `"focused"`,
    ///   0x08+0x04 → `"selectedhover"`, 0x08 → `"selected"`, 0x04 → `"hover"`, else `"normal"`;
    /// * **0x525285…0x5252F5** matches that name against the loaded `[state.*]` sections and hands
    ///   the index to 0x525580 → **0x5271F0**, which *copies that state's `parts` vector over the
    ///   widget's active part list* (`[[widget+0xF4]+0x10]`).
    ///
    /// So when the chosen section lists only `DATA` parts — and `DATA` draws nothing, it is the
    /// attribute block (`RE/notes/controls.md` §1) — the widget is erased. Three shipped widgets
    /// are in exactly that case, all `NFS5.PANEL` with `state=hidden` against `Panel.ctr`'s
    /// `[state.hidden] parts=data`: `carsetup.lay` §77, `carsetupdesc.lay` §11 and
    /// `usedcars.lay` §19. Each carries `color=0x80ffFF00`, so drawing them anyway paints a
    /// half-transparent yellow slab over the middle of those three screens.
    fn state_draws_nothing(&self, e: &nfs_formats::lay::Element) -> bool {
        let Some(c) = self.controls.get(&e.class) else { return false };
        let Some(state) = e.get("state") else { return false };
        // an unknown state name leaves the ctor's own list in place, which is every part
        if !c.has_state(state) {
            return false;
        }
        !c.state_parts(state).iter().filter_map(|n| c.part(n)).any(|p| p.kind() != nfs_formats::ctr::PartKind::Data)
    }

    /// An embedded layout's sprites, moved into the widget's own place. The offset is applied in
    /// the same normalised space [`place`](Self::place) maps into, so it carries the letterbox
    /// factor with it.
    #[allow(clippy::too_many_arguments)]
    fn embedded(&self, g: &LayoutScreen, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>, x: f32, y: f32) -> Vec<Sprite> {
        // an embedded layout is one of the 98 the sweep already walks on its own, so its gaps are
        // reported there and not again here — see the note on the background above
        let _ = &blank;
        let mut inner = g.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new());
        if x != 0.0 || y != 0.0 {
            let design_aspect = DESIGN.0 / DESIGN.1;
            let fw = if aspect > design_aspect { design_aspect / aspect } else { 1.0 };
            let (dx, dy) = (x / DESIGN.0 * fw, y / DESIGN.1);
            for sp in &mut inner {
                sp.pos[0] += dx;
                sp.pos[1] += dy;
            }
        }
        inner
    }

    /// The `RECT` parts of the widget's class template that `state` draws, each resolved to a
    /// rectangle and a colour (`RE/notes/controls.md` §1). This is how a class that is nothing but
    /// boxes — `NFS5.CONTROLLERSETUP` is a filled box with a one-pixel border — draws without the
    /// port knowing anything about it. The state is named rather than taken from the block, because
    /// a widget that can change state draws the parts of the state it is in now.
    fn rect_parts_in(&self, e: &nfs_formats::lay::Element, state: &str) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let Some((x, y)) = at("pos") else { continue };
            let Some((w, h)) = at("size") else { continue };
            // a colour may be a literal or a reference to one of the `data` attributes
            let colour = self
                .part_attr(e, p, "color")
                .and_then(|v| parse_color(v).or_else(|| v.split_once('.').and_then(|(_, key)| self.attr(e, key)).and_then(parse_color)));
            let Some(colour) = colour else { continue };
            out.push((x as f32, y as f32, w as f32, h as f32, colour));
        }
        out
    }

    /// A part's colour attribute: a literal `0x…`, or an **expression** over the widget's own
    /// attributes — `GoToButton.ctr` writes its caption's colour as
    /// `data.medium?0x00000000:0xffffffff`, so which of its three labels is opaque is arithmetic.
    fn part_colour(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, key: &str) -> Option<[u8; 4]> {
        let v = self.part_attr(e, p, key)?;
        if let Some(c) = parse_color(v) {
            return Some(c);
        }
        let n = nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true))?;
        if !(0.0..=4294967295.0).contains(&n) {
            return None;
        }
        let n = n as u32;
        Some([((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8, (n >> 24) as u8])
    }

    /// The `RECT` parts of one state of the widget's class, each resolved to a rectangle and a
    /// colour, with the fully transparent ones dropped. Unlike [`rect_parts_in`](Self::rect_parts_in)
    /// it evaluates the colour as an expression — a button's highlight is `[state.selected]`,
    /// which the `.lay` block never says, and its colour can be arithmetic.
    fn state_rects(&self, e: &nfs_formats::lay::Element, state: &str) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let (Some((px, py)), Some((pw, ph))) = (at("pos"), at("size")) else { continue };
            let Some(col) = self.part_colour(e, p, "color").filter(|c| c[3] != 0) else { continue };
            out.push((px as f32, py as f32, pw as f32, ph as f32, col));
        }
        out
    }

    /// One part attribute with the `%…%` indirection resolved: `%<part>.<key>%` is that part's
    /// value for that key, which is how `GoToButton.ctr`'s two spare captions say
    /// `stringID.value=%label.stringID%` — "the same string the first one shows".
    fn part_ref<'a>(&'a self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<String> {
        let v = self.part_attr(e, p, key)?;
        let Some(inner) = v.strip_prefix('%').and_then(|v| v.strip_suffix('%')) else {
            return Some(v.to_string());
        };
        let (part, k) = inner.split_once('.')?;
        if part == "data" {
            return e.get(k).map(str::to_string);
        }
        let q = self.controls.get(&e.class)?.part(part)?;
        self.part_attr(e, q, k).map(str::to_string)
    }

    /// Draw a widget entirely out of its class template: the `RECT` parts of the state, then its
    /// `SHAPE` parts, then its `TEXT` / `LOCALIZEDTEXT` parts — each where the template puts it,
    /// in the font and colour it names, and skipping any whose colour is fully transparent. That
    /// last rule is how a class carries several captions and shows one (`controls.md` §4.3).
    #[allow(clippy::too_many_arguments)]
    fn template_widget(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, state: &str, x: f32, y: f32, strings: &Strings) {
        self.template_widget_at(out, aspect, e, state, x, y, strings, None)
    }

    /// The same, with the `picture` part pinned to one entry of its pack — the animation of §4.53.
    #[allow(clippy::too_many_arguments)]
    fn template_widget_at(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, state: &str, x: f32, y: f32, strings: &Strings, frame: Option<usize>) {
        for (px, py, pw, ph, col) in self.state_rects(e, state) {
            out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(col)));
        }
        for (px, py, pw, ph, file, index, flip) in self.shape_parts_at(e, frame) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
            let (pos, size) = self.place(aspect, x + px, y + py, dw, dh);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None });
        }
        let Some(c) = self.controls.get(&e.class) else { return };
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            let text = match p.kind() {
                nfs_formats::ctr::PartKind::LocalizedText => {
                    let id = self.part_ref(e, p, "stringID").and_then(|v| v.trim().parse::<u32>().ok());
                    match id.filter(|n| *n != 0).and_then(|n| strings.get(n)) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => continue,
                    }
                }
                nfs_formats::ctr::PartKind::Text => match self.part_ref(e, p, "text").filter(|t| !t.is_empty()) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            };
            let Some(col) = self.part_colour(e, p, "foreColor").filter(|c| c[3] != 0) else { continue };
            let (px, py) = self
                .part_attr(e, p, "pos")
                .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)))
                .unwrap_or((0.0, 0.0));
            let Some(f) = self
                .part_attr(e, p, "fontPathName")
                .map(|f| f.to_lowercase())
                .and_then(|f| self.fonts.get(&f))
                .or_else(|| self.fonts.get(DEFAULT_FONT))
            else {
                continue;
            };
            self.text(out, aspect, f, &text, x + px as f32, y + py as f32, 0.0, 0, tint(col));
        }
    }

    /// One attribute of one part, resolved the way the original does: the `.lay` block's value for
    /// the attribute's **screen key** when it has one, else the template's `<key>.value`
    /// (`nfs_formats::ctr::Part::screen_key`, 0x52BC90 / 0x52BFB4 / 0x52C000).
    fn part_attr<'a>(&self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<&'a str> {
        if let Some(k) = p.screen_key(key)
            && let Some(v) = e.get(&k).filter(|v| !v.is_empty())
        {
            return Some(v);
        }
        // an **empty** `<key>.value` is not a value: `Graphic Tab Button.ctr` writes `size.value=`
        // on the parts whose size is their art's, and `shapePathName.value=` on the two whose pack
        // only the screen key names. Reading either as a value can only fail.
        p.value(key).filter(|v| !v.trim().is_empty())
    }

    /// The `SHAPE` parts of the widget's class template that its `state` draws, each resolved to a
    /// place and a `.fsh` pack entry (`RE/notes/controls.md` §1).
    ///
    /// This is the other half of [`rect_parts_in`](Self::rect_parts_in): a class whose art is entirely in
    /// its template draws through here without the port knowing anything about it.
    /// `NFS5.CARSETTINGBUTTON` is the plainest case — `CarSettingButton.ctr` gives its `picnonsel`
    /// part `shapePathName=weeicons.fsh` and `shapeIndex=data.nonselindex`, so the widget's own
    /// `nonselindex` picks the arrow. The same shape covers `NFS5.POPUPCIRCLE`,
    /// `NFS5.ExtraCircle`, the tab buttons and the rest of the art-from-template classes.
    fn shape_parts(&self, e: &nfs_formats::lay::Element) -> Vec<(f32, f32, f32, f32, String, usize, i64)> {
        self.shape_parts_at(e, None)
    }

    /// The same, with the `picture` part's entry replaced — what `NFS5.BOTTOMBARBUTTON`'s tick
    /// does every frame through `0x5271F0([this+0x17C], frame)` (§4.53).
    fn shape_parts_at(&self, e: &nfs_formats::lay::Element, frame: Option<usize>) -> Vec<(f32, f32, f32, f32, String, usize, i64)> {
        // `NFS5.POPUPCIRCLE` **is drawn**, and the reading that said otherwise rested on the
        // draw order being the wrong way round (§4.67).
        //
        // `PopUp.ctr` pins it at `pos=113,100` (`HIDDEN|READONLY`) and tiles **one** 207×175
        // quarter four ways — `p1`…`p4` at (0,0), (207,0), (0,175), (207,175) with `flip` 0…3 —
        // a 414×350 oval over the middle of a 640×480 screen. Its states are `normal` and
        // `hidden`, and `[state.hidden]` draws only `data`, which is nothing.
        //
        // **Nothing can put it in that state.** Neither the class id `NFS5.POPUPCIRCLE` nor the
        // widget name `ID_POPUPCIRCLE` appears anywhere in the executable — a byte search of the
        // whole image finds neither — so there is no class code and no lookup by name that could
        // reach `setHidden`. And all **19** shipped blocks carry `state=normal`. It is therefore
        // on, on every one of those screens, always.
        //
        // The old note argued from its being the **last** section in 18 of the 19 that it "draws
        // over everything else". That was true only while this port painted sections forwards; the
        // original walks the child vector backwards (§4.67), so the last section is painted
        // **first** — the oval goes *under* the screen, which is what a background vignette does.
        //
        // **Not** gated on `swrenderoff` either: that key reaches `[widget+0x13C]` at 0x4C7733 and
        // 0x4C7750 and a scan of the whole `.text` for the displacement finds those two writes and
        // **no read at all**, so it is dead in this build (CONFIRMED by exhaustion).
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let state = e.get("state").unwrap_or("normal");
        let mut out = Vec::new();
        for (name, p) in c.state_parts(state).iter().filter_map(|n| c.part(n).map(|p| (n.clone(), p))) {
            if p.kind() != nfs_formats::ctr::PartKind::Shape {
                continue;
            }
            // A part's `shapePathName.value` and `shapeIndex.value` are that part's **defaults**,
            // and the `.lay` block's own keys override them — the ordinary attribute rule of
            // `RE/notes/controls.md` §1, nothing special about these two. Five classes name their
            // own pack: `NFS5.PICTURE` (484 of them) also names the entry, so its single part
            // resolves to exactly the pair the widget carries; `NFS5.BOTTOMBARBUTTON`,
            // `NFS5.CheckBox`, `NFS5.AnimatingPicture` and `NFS5.CarSettingSlider` name only the
            // pack and keep the part's index — `data.startframe`, `data.onIndex`,
            // `data.start_frame` and the literal 8 of `CarSettingSlider.ctr`'s knob.
            //
            // The port used to give up on the whole part list as soon as a widget named a pack and
            // draw the widget's own pair instead, which is the same picture for a `NFS5.PICTURE`
            // and the **wrong entry** for the other four: every bottom-bar button drew `logoAn`
            // entry 0 where its `startframe` says 11, and every car-setting slider entry 0 of
            // `weeicons.fsh` where its knob is entry 8.
            // `%<part>.<key>%` is the indirection of `controls.md` §2 — that other part's value
            // for that key — and `CarSettingButton.ctr` is the one template that uses it for art:
            // its `picsel` and `picseldown` both take `%picnonsel.shapePathName%`, so the hover
            // and pressed arrows come out of the same pack as the plain one.
            let Some(file) = self.part_ref(e, p, "shapePathName").filter(|v| !v.is_empty()) else { continue };
            // `shapeIndex` is a literal or a `data.<key>` reference to the widget's own attribute
            let index = match frame.filter(|_| name == "picture") {
                Some(n) => n,
                None => match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => match nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)) {
                        Some(n) if n >= 0.0 => n as usize,
                        _ => continue,
                    },
                    None => 0,
                },
            };
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let (px, py) = at("pos").unwrap_or((0.0, 0.0));
            let (pw, ph) = at("size").unwrap_or((0.0, 0.0));
            // **`data.picoffy`** — `NFS5.BOTTOMBARBUTTON`'s own draw, 0x4C9B30, adds it to the
            // `picture` part's y and nothing else: `[this+0x1D4]` (the bind at 0x4C9639) plus the
            // part's own `pos.y` from `0x4E7060([this+0x178])` plus the widget's y, all three at
            // 0x4C9BDE…0x4C9BED, with the x left alone (0x4C9BFD). Three of `bottombar.lay`'s four
            // buttons — back, options and exit, the three with an icon — carry **6**; the other
            // nineteen shipped bar buttons carry 0. §4.80.
            let py = py
                + if name == "picture" && e.class == "NFS5.BOTTOMBARBUTTON" {
                    self.attr_num(e, "picoffy").unwrap_or(0.0)
                } else {
                    0.0
                };
            // `flip` is the part's own attribute — `Graphic Tab Button.ctr` writes
            // `flip.value=data.flipit`, so the widget's `flipit` mirrors the tab's art
            let flip = self
                .part_attr(e, p, "flip")
                .and_then(|v| nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)))
                .filter(|n| *n >= 0.0)
                .map(|n| n as i64)
                .unwrap_or_else(|| e.int("flip").unwrap_or(0).max(0));
            out.push((px as f32, py as f32, pw as f32, ph as f32, file.to_string(), index, flip));
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
            tri: None,
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
        self.clock_ms = self.clock_ms.wrapping_add(dt_ms);
        // `[widget+0x12C]` is stamped with the tick whenever the widget is *not* hovered
        // (0x4C824D), so the count restarts the moment the pointer moves on (§4.52)
        let on = self.hovered();
        if on == self.hover_of {
            self.hover_ms = self.hover_ms.saturating_add(dt_ms);
        } else {
            self.hover_of = on;
            self.hover_ms = 0;
        }
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
        // **0x4C8760's four arms**, and the multiplier they use: `[0x5DEAC8]` for x and
        // `[0x5DEACC]` for y, over `[0x5B6B2C]` = 512. Those two are the **live screen extent**,
        // written at 0x556C8A from the selected video mode (0x5DE654 / 0x5DE658) and clipped
        // against by the platform layer at 0x53550E — so the widget travels exactly one screen,
        // and multiplying by the design size in design space is that same distance. Pinned
        // 2026-09-21; the addresses were cited for `TRANS_FULL` but never for the multiplier.
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
        // **0x4C8820**: `p << 8` less one copy of `p` — the ×255 idiom — over the same 512, an
        // integer divide, and the draw takes that off the full alpha.
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
        let mut out: Vec<ListBoxWidget> = self
            .layout
            .elements
            .iter()
            .enumerate()
            // `NFS5.CHATDISPLAYBOX` takes the same binding: its draw is a row walk over its own
            // accessor too, only with a fixed spacing rather than the list box's 14 (§4.131)
            .filter(|(_, e)| e.class == "NFS5.ListBox" || e.class == "NFS5.CHATDISPLAYBOX")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect();
        // …and the ones inside the panels this screen holds, in the flattened index space, as
        // `combo_sliders` does — `Audio.lay`'s CD player lists live in its playlist panel
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for w in self.groups[&k].list_boxes() {
                out.push(ListBoxWidget { index: base + w.index, accessor: w.accessor });
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// The combo sliders and colour boxes, which take the same row binding as a list box: their
    /// drop-down is drawn by the very same code (`FeComboBox` slot 27, 0x4C5810), reached from
    /// slot **17** rather than slot 15 so that it lands over the widgets drawn after it.
    pub fn combo_sliders(&self) -> Vec<ListBoxWidget> {
        let mut out: Vec<ListBoxWidget> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.ComboBoxSlider" || e.class == "NFS5.ColorBox")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect();
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for w in self.groups[&k].combo_sliders() {
                out.push(ListBoxWidget { index: base + w.index, accessor: w.accessor });
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// Open one combo's list, or none. The index is the flattened one `combo_sliders` hands out.
    ///
    /// `FeComboBox::refreshRows` **0x4C53A0** is the rule: `[this+0x1C4]`, the height of the rows
    /// the drop-down draws, is the accessor's own total **only when the widget holds focus or is a
    /// list box** (0x4C53B1 asks 0x4C7DD0, 0x4C53BA asks the constructor's `[this+0x1DD]`), and
    /// **0** otherwise — so a combo that is not focused draws no list at all.
    pub fn set_open_combo(&mut self, index: Option<usize>) {
        if std::env::var("NFS_FE_OPEN").is_ok() {
            return;
        }
        self.open_combo = index.filter(|i| *i < self.layout.elements.len());
        let routed = index.and_then(|i| self.route(i));
        let keys: Vec<usize> = self.groups.keys().copied().collect();
        for k in keys {
            let inner = routed.filter(|(g, _)| *g == k).map(|(_, j)| j);
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_open_combo(inner);
            }
        }
    }

    /// Fill a list box with the rows its accessor would hand out, and say which row the accessor
    /// has selected (`current` — the row the original paints with `highlightcolorselected`).
    pub fn set_list_rows(&mut self, index: usize, rows: Vec<ListRow>, current: Option<usize>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_list_rows(j, rows, current);
            }
            return;
        }
        if rows.is_empty() {
            self.lists.remove(&index);
            self.scroll.remove(&index);
        } else {
            self.scroll.entry(index).and_modify(|f| *f = (*f).min(rows.len().saturating_sub(1))).or_insert(0);
            self.lists.insert(index, (rows, current));
        }
    }

    /// Move a list's first drawn row. The original keeps that position in a scrollbar object of
    /// its own — `[this+0x1BC]`, whose slot 10 the row loop asks for at 0x4C66F8 — and the
    /// scrollbar widget `[this+0x1B8]` (0x4C5175) moves it. This port keeps the position itself
    /// and moves it by the arrow the player clicks.
    pub fn scroll_list(&mut self, index: usize, delta: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.scroll_list(j, delta);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let last = self.last_first_row(index, rows);
        let at = self.scroll.entry(index).or_insert(0);
        *at = (*at as i32 + delta).clamp(0, last as i32 + 1) as usize;
    }

    /// The largest first row that still fills the box — walking up from the last row while the
    /// heights fit, which is the shape of `FeComboBox`'s own loop at 0x4C54EC…0x4C5508.
    fn last_first_row(&self, index: usize, rows: &[ListRow]) -> usize {
        let Some(e) = self.layout.elements.get(index) else { return 0 };
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let mut acc = 0.0;
        let mut n = rows.len();
        while n > 0 {
            acc += rows[n - 1].height();
            if acc > max_h {
                break;
            }
            n -= 1;
        }
        n
    }

    /// A list's scrollbar, in design coordinates, when it has one.
    ///
    /// The regions are the ones `0x4D0CC0` hit-tests: **1** the decrement button `[+0xE0]`, **2**
    /// the increment button `[+0xE4]`, **3** the knob `[+0xDC]`, and 4 / 5 the track either side
    /// of it. `VerticalSlider.ctr` places them — `decrement_button` at `0,7` with `weeicons.fsh`
    /// **13**, `increment_button` at the bottom with **14**, the knob
    /// `decrement_button.size.width × 15`.
    pub fn list_scrollbar(&self, index: usize) -> Option<ListScrollbar> {
        if let Some((k, j)) = self.route(index) {
            let g = self.groups.get(&k)?;
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            let mut b = g.list_scrollbar(j)?;
            for r in [&mut b.up, &mut b.down, &mut b.knob] {
                r.0 += gx;
                r.1 += gy;
            }
            b.track_top += gy;
            return Some(b);
        }
        let e = self.layout.elements.get(index)?;
        let (rows, _) = self.lists.get(&index)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        if full < max_h {
            return None;
        }
        let (x, y) = self.position_of(e);
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
            None => (x, y),
        };
        let sx = px + width - LIST_SCROLLBAR_W;
        let art = |n: usize| self.shapes.get(&("weeicons.fsh".to_string(), n)).copied();
        let (uw, uh) = art(13).map(|(_, w, h)| (w, h)).unwrap_or((LIST_SCROLLBAR_W, 8.0));
        let (dw, dh) = art(14).map(|(_, w, h)| (w, h)).unwrap_or((LIST_SCROLLBAR_W, 8.0));
        let base = py + uh;
        let span = (max_h - LIST_KNOB_H - uh).max(0.0);
        // `+2`, not `+1`: the list's tick hands the scrollbar `rowsWalkedUp + 2` (0x4C550A) and
        // the model keeps that as its **count** (`[model+0x14]`, slot 24 at 0x4BC310), so there is
        // one position past the last full page — the list can be scrolled one row further and show
        // a blank line. §4.59.
        let positions = self.last_first_row(index, rows) + 2;
        let at = self.scroll.get(&index).copied().unwrap_or(0);
        let dy = if positions > 1 { at as f32 * span / (positions - 1) as f32 } else { 0.0 };
        // A press on the **track** (regions 4 and 5, 0x4D0995) leaves the model alone and moves the
        // knob's own rect so its top sits at the pointer (0x4D0A0A / 0x4D0A18 → 0x5276E0), then
        // re-runs the hit test so the region becomes the knob. It lasts until the first move, which
        // puts the model somewhere and re-places the knob from it (slot 11).
        let ky = match self.knob_at {
            Some((i, y)) if i == index => y,
            _ => base + dy,
        };
        Some(ListScrollbar {
            up: (sx, py + 7.0, uw, uh),
            down: (sx, py + max_h - dh, dw, dh),
            knob: (sx, ky, uw, LIST_KNOB_H),
            track_top: base,
            span,
            positions,
            at,
        })
    }

    /// Put a list's first row at an absolute position, clamped — what the knob's drag does.
    pub fn set_list_scroll(&mut self, index: usize, at: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_list_scroll(j, at);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let last = self.last_first_row(index, rows) as i32;
        self.scroll.insert(index, at.clamp(0, last + 1) as usize);
        // the model moved, so slot 11 re-places the knob from it and the parked position is gone
        self.knob_at = None;
    }

    /// Which row of a list the pointer is on, in the same walk `FeListBox`'s slot 8 uses —
    /// 0x4C6F60 box-tests the pointer (0x4E2970), then accumulates row heights and takes the
    /// **first** row whose running bottom is past the pointer's y (0x4C7013).
    ///
    /// This is the hovered row the draw highlights, and it is what a click lands on.
    pub fn list_row_at(&self, index: usize, x: f32, y: f32) -> Option<usize> {
        if let Some((k, j)) = self.route(index) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.list_row_at(j, x - gx, y - gy);
        }
        let e = self.layout.elements.get(index)?;
        let (rows, _) = self.lists.get(&index)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let panel_w = if full >= max_h { width - LIST_SCROLLBAR_W } else { width };
        let (wx, wy) = self.position_of(e);
        let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)).map(|(a, b)| (wx + a as f32, wy + b as f32)).unwrap_or((wx, wy)),
            None => (wx, wy),
        };
        if !(x >= px && x < px + panel_w && y >= py && y < py + max_h) {
            return None;
        }
        let first = self.scroll.get(&index).copied().unwrap_or(0);
        let mut acc = py;
        rows.iter().skip(first).position(|r| {
            acc += r.height();
            y < acc
        }).map(|n| n + first)
    }

    /// Park a list's knob where a press on the **track** put it, until the model moves
    /// (`RE/notes/controls.md` §4.46 — the tail of 0x4D0995). `None` gives it back to the model.
    pub fn park_knob(&mut self, index: usize, y: Option<f32>) {
        if let Some((k, j)) = self.route(index) {
            let dy = self.layout.elements.get(k).map(|e| self.position_of(e).1).unwrap_or(0.0);
            if let Some(g) = self.groups.get_mut(&k) {
                g.park_knob(j, y.map(|v| v - dy));
            }
            return;
        }
        self.knob_at = y.map(|v| (index, v));
    }

    /// Hand an `NFS5.3DCAR` the offscreen view its car was rendered into. The view covers the whole
    /// screen — the car's place in it comes from the camera, not from the widget's rectangle — and
    /// is drawn in the widget's own place in the layout order, which is what puts it over the art
    /// behind it and under the art in front.
    pub fn set_car_view(&mut self, index: usize, view: Option<TextureId>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_car_view(j, view);
            }
            return;
        }
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
    /// How many index slots this screen occupies: its own widgets, then every layout its panels
    /// and tab strips embed, in widget order. Binding a screen's content is keyed by widget index
    /// (`set_bar`, `set_list_rows`, …), and a panel's sub-layout is as much a part of the screen as
    /// the widgets beside it — the audio sliders of `Options.lay` live in `audioreal.lay`, three
    /// files down. This flattening is what lets one index reach them.
    fn span(&self) -> usize {
        self.layout.elements.len() + self.groups.values().map(|g| g.span()).sum::<usize>()
    }

    /// Every embedded screen with the index its own slot 0 sits at, in widget order.
    fn group_spans(&self) -> Vec<(usize, &LayoutScreen)> {
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        let mut out = Vec::new();
        for k in keys {
            let g = &self.groups[&k];
            out.push((base, g.as_ref()));
            base += g.span();
        }
        out
    }

    /// Which embedded screen an index belongs to, and its index there. `None` means this screen's
    /// own — an index past the end routes nowhere and every setter drops it.
    fn route(&self, index: usize) -> Option<(usize, usize)> {
        if index < self.layout.elements.len() {
            return None;
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let span = self.groups[&k].span();
            if index < base + span {
                return Some((k, index - base));
            }
            base += span;
        }
        None
    }

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
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.indicator_bars().into_iter().map(move |(i, a, o)| (i + base, a, o))))
            .collect()
    }

    /// Every `NFS5.CarSettingSlider` and the accessor it watches, in the flattened index space.
    pub fn car_setting_sliders(&self) -> Vec<(usize, String)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CarSettingSlider")
            .filter_map(|(index, e)| e.get("accessor").filter(|a| !a.is_empty()).map(|a| (index, a.to_string())))
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.car_setting_sliders().into_iter().map(move |(i, a)| (i + base, a))))
            .collect()
    }

    /// The screen's `NFS5.CheckBox`es and the `data.boolAccessor` each one watches — the string
    /// its bind looks up at 0x4C3639 and keeps in `[this+0x144]`, whose value `FECheckBox::refresh`
    /// 0x4C3730 turns into the `onIndex` / `offIndex` entry it draws. §4.96.
    pub fn check_boxes(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CheckBox")
            .filter_map(|(i, e)| e.get("boolaccessor").filter(|a| !a.is_empty()).map(|a| (i, a.to_string())))
            .collect();
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for (i, a) in self.groups[&k].check_boxes() {
                out.push((base + i, a));
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// Tick or untick one `NFS5.CheckBox`.
    pub fn set_check(&mut self, index: usize, value: Option<bool>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_check(j, value);
            }
            return;
        }
        match value {
            Some(v) => self.checks.insert(index, v),
            None => self.checks.remove(&index),
        };
    }

    /// Put one slider's knob where its accessor is: the index it is on and how many entries its
    /// list has. Without it the knob sits at the track's left end.
    pub fn set_knob(&mut self, index: usize, value: Option<(i32, i32)>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_knob(j, value);
            }
            return;
        }
        match value {
            Some(v) => self.knobs.insert(index, v),
            None => self.knobs.remove(&index),
        };
    }

    /// Give one indicator bar its value, or `None` to leave it blank.
    pub fn set_bar(&mut self, index: usize, value: Option<i32>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_bar(j, value);
            }
            return;
        }
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
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.numbered_widgets(class).into_iter().map(move |(i, n)| (i + base, n))))
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
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_pack_picture(r, paths, dir, j, file, entry);
            }
            return;
        }
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

    /// Load one entry of a pack into the shape cache, without binding it to a widget — the
    /// timeline's icons are ten cells inside a single widget, not ten widgets (§4.123).
    pub fn load_pack_entry(&mut self, r: &mut Renderer, paths: &GamePaths, dir: &str, file: &str, entry: usize) {
        let key = (file.to_string(), entry);
        if self.shapes.contains_key(&key) {
            return;
        }
        let dir = paths.fe_data().join(dir);
        let entries = GamePaths::find_file(&dir, file)
            .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
            .and_then(|(p, d)| decode_pack(&p, &d).ok())
            .unwrap_or_default();
        let Some(img) = entries.get(entry) else { return };
        let Some(px) = img.mips.first() else { return };
        let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
        self.shapes.insert(key, (id, img.width as f32, img.height as f32));
    }

    /// Bind a `NFS5.MOVIEBOX` to a `.mad` and start it playing.
    ///
    /// The whole file is kept, its video chunks indexed, and one texture allocated to decode into.
    /// [`LayoutScreen::advance_movies`] steps it; the period is the header's own (see
    /// [`MoviePlayer`]).
    pub fn set_movie(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, dir: &str, file: &str) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_movie(r, paths, j, dir, file);
            }
            return;
        }
        if self.movies.contains_key(&index) {
            return;
        }
        if let Some(m) = MoviePlayer::open(r, paths, dir, file) {
            self.movies.insert(index, m);
        }
    }

    /// True when this screen — or the one behind it — has a movie playing.
    pub fn has_movies(&self) -> bool {
        !self.movies.is_empty() || self.background.as_ref().is_some_and(|b| b.has_movies())
    }

    /// Step every `NFS5.MOVIEBOX` on this screen by `dt_ms`, decoding and uploading the frames its
    /// own period calls for. A movie that has run out starts again, which is what a front-end loop
    /// does.
    /// Step the cursor's animation clock and take the button state 0x5299C0 reads out of
    /// `[0x69CAE8]`. The rates are the original's, off `TIMER_getTicks()` — see
    /// [`CURSOR_SLOW_MS`]; the clock used to carry a rate of this port's own.
    ///
    /// The period is a whole number of both animations' cycles so the wrap is invisible: 125 ms a
    /// frame over at most five frames, and 31.25 ms a frame over at most six, divide into
    /// 1000 × 125 ms.
    pub fn advance_cursor(&mut self, dt_ms: u32, mouse_down: bool) {
        if self.cursor_ms != u32::MAX {
            self.cursor_ms = self.cursor_ms.wrapping_add(dt_ms) % (CURSOR_SLOW_MS * 1000);
        }
        self.cursor_btn = mouse_down;
    }

    /// Which cursor shape this screen asks for, and the frame of it to draw.
    ///
    /// **A shape is requested, not set**: 0x4E1E50 is `[0x690720] = max([0x690720], n)` and
    /// 0x4D2F3D clears the slot at the top of the next frame, so the highest number anyone asked
    /// for during a frame is the one drawn (§4.152). The two requesters the notes have read are
    /// `FEControl`'s slot 24 — 0x4C7F40, `push 1`, dispatched from the per-tick slot 16 at
    /// 0x4C7FDF while a widget is hovered — and `FECar3DControl`'s slot 15, `push 7`, the
    /// magnifier over a hovered 3D car (`frontend-3d.md` §5).
    fn cursor_shape(&self, hovered: Option<usize>) -> usize {
        let mut want = 0usize;
        if let Some(h) = hovered {
            want = want.max(1);
            if let Some(e) = self.layout.elements.get(h)
                && e.class.eq_ignore_ascii_case("NFS5.3DCAR")
            {
                // A **focused** 3D car is the magnifier, shape 7 — 0x4C09F7 tests 0x4C7DD0,
                // bit 0x40 of `[widget+0x10C]` (§4.109), and only then pushes 7. The car holds
                // focus while zoomed in on an open door (slot 9, 0x4C1002). A merely *hovered*
                // one — 0x4C7C90, bit 0x04 — asks per zone.
                let parts = self.car_parts.get(&h);
                if parts.is_some_and(|p| p.zoomed) {
                    return 7;
                }
                want = want.max(match self.car_zone(h, e) {
                    Some(0) => 6,
                    // zones 1 and 2 read `byte[this+0x1AE + (zone != 1)]` at 0x4C0A41: a door
                    // clicked open is `magP`, a closed one `grab`
                    Some(z @ (1 | 2)) => {
                        if parts.is_some_and(|p| p.door_flag[usize::from(z != 1)]) { 8 } else { 5 }
                    }
                    Some(_) => 3,
                    // hovered but off the box — the plain hovered arrow slot 24 already asked for
                    None => 1,
                });
            }
        }
        want
    }

    /// Which of the five zones of a `NFS5.3DCAR` the pointer is in — `FECar3DControl`'s
    /// **0x4C0D10**, `RE/notes/controls.md` §4.153. `None` when the pointer is not on it.
    ///
    /// The zone is the turntable angle's fraction crossed with the horizontal third of the
    /// widget's box, and the answers are the ones 0x4C0DCD…0x4C0E89 write into `edi`.
    fn car_zone(&self, index: usize, e: &nfs_formats::lay::Element) -> Option<u32> {
        let (x, y) = self.cursor?;
        let (bx, by) = self.position_of(e);
        let (w, h) = self.box_of(e, 0);
        let roof = self.car_roof.contains(&index);
        car_zone_of(self.car_spin, roof, x, y, bx, by, w, h)
    }

    /// Whether the car a `NFS5.3DCAR` shows has a roof animation — `0x438190(model, 1)` non-zero,
    /// the test 0x4C0D10 makes before its roof zone.
    pub fn set_car_roof(&mut self, index: usize, roof: bool) {
        if roof {
            self.car_roof.insert(index);
        } else {
            self.car_roof.remove(&index);
        }
    }

    /// A click released on a `NFS5.3DCAR` — slot 9, 0x4C0EB0: the zone under the pointer
    /// (0x4C0D10 on the event's own x, y) and the parts state machine of `crate::carparts`.
    /// Returns what the car and the sound layer are asked to do.
    pub fn car_click(&mut self, index: usize) -> Vec<crate::carparts::PartEvent> {
        let mut out = Vec::new();
        let Some(e) = self.layout.elements.get(index) else { return out };
        if !e.class.eq_ignore_ascii_case("NFS5.3DCAR") {
            return out;
        }
        let zone = self.car_zone(index, e);
        let now = self.ticks();
        let parts = self.car_parts.entry(index).or_default();
        let focused = parts.zoomed;
        if let Some(z) = zone {
            parts.click(z, now, focused, &mut out);
        }
        out
    }

    /// A click on `zone` of 3D car `index` at tick `now`, as `car_click` makes one — for
    /// `NFS_FE_PARTS`, which opens parts for a still.
    pub fn car_click_zone(&mut self, index: usize, zone: u32, now: u32) -> Vec<crate::carparts::PartEvent> {
        let mut out = Vec::new();
        let parts = self.car_parts.entry(index).or_default();
        let focused = parts.zoomed;
        parts.click(zone, now, focused, &mut out);
        out
    }

    /// The parts' per-frame step (0x4C0696…0x4C07CF) for every 3D car on the screen.
    pub fn step_car_parts(&mut self) -> Vec<crate::carparts::PartEvent> {
        let now = self.ticks();
        let mut out = Vec::new();
        for p in self.car_parts.values_mut() {
            p.step(now, &mut out);
        }
        out
    }

    /// The widget's `Car.Roof` check each tick (0x4C05E7…0x4C063F; the bind 0x4BFE71 on the first).
    pub fn sync_car_roof(&mut self, index: usize, down: bool) -> Vec<crate::carparts::PartEvent> {
        let now = self.ticks();
        let mut out = Vec::new();
        self.car_parts.entry(index).or_default().sync_roof(down, now, &mut out);
        out
    }

    /// The room zoom a 3D car's tick hands on (`CarParts::zoom`), and whether it is zoomed in —
    /// while it is, the tick covers the screens every frame (0x4C052F → 0x4D2000).
    pub fn car_zoom(&self, index: usize) -> (f32, bool) {
        let now = self.ticks();
        self.car_parts.get(&index).map(|p| (p.zoom(now), p.zoomed)).unwrap_or((0.0, false))
    }

    /// Zone 0's state, 0…255 — what 0x4C07E8 hands 0x4D1990(1, ·) after every step.
    pub fn car_roof_state(&self, index: usize) -> i32 {
        self.car_parts.get(&index).map(|p| p.state[0]).unwrap_or(0)
    }

    /// Channels 6 and 7 of the car a `NFS5.3DCAR` widget shows — its two doors, in turns.
    pub fn car_doors(&self, index: usize) -> (f32, f32) {
        self.car_parts.get(&index).map(|p| p.doors()).unwrap_or((0.0, 0.0))
    }

    /// `TIMER_getTicks()` (0x53C2E0) — the 128 Hz count — from the screen's clock.
    pub fn ticks(&self) -> u32 {
        (self.clock_ms as u64 * 128 / 1000) as u32
    }

    /// Where the 3D room's turntable stands, in turns, for [`Self::car_zone`].
    pub fn set_car_spin(&mut self, turns: f32) {
        self.car_spin = turns;
    }
}

/// The body of [`LayoutScreen::car_zone`], free of the screen so it can be tested on its own.
pub fn car_zone_of(spin: f32, roof: bool, x: f32, y: f32, bx: f32, by: f32, w: f32, h: f32) -> Option<u32> {
    {
        if x < bx || y < by || x >= bx + w || y >= by + h {
            return None;
        }
        // 0x4C0D5D: the top **5/16** of the box is zone 0 whatever the angle — reached only when
        // the car's roof animation has a length, `0x438190(model, 1)` non-zero
        if roof && y <= by + 5.0 * h / 16.0 {
            return Some(0);
        }
        let t = spin.rem_euclid(1.0);
        let third = w / 3.0;
        let left = x < bx + third;
        let right = x > bx + w - third;
        // 0x4C0DD2…0x4C0E89, in the order the compares run
        Some(match t {
            t if t <= 0.1 || t > 0.9 => 4,
            t if t <= 0.4 =>
            // 0x4C0E57: left third 4, middle 2, right 3
            {
                if left {
                    4
                } else if right {
                    3
                } else {
                    2
                }
            }
            t if t <= 0.6 => 3,
            // 0x4C0E0F: left third 3, middle 1, right 4
            _ => {
                if left {
                    3
                } else if right {
                    4
                } else {
                    1
                }
            }
        })
    }
}

impl LayoutScreen {

    /// The cursor itself, drawn over everything. Nothing else in the screen may come after it.
    fn cursor_sprite(&self, out: &mut Vec<Sprite>, aspect: f32, hovered: Option<usize>) {
        // `u32::MAX` marks a screen that is somebody's background; only the screen in front draws
        if self.cursor_ms == u32::MAX {
            return;
        }
        let Some((cx, cy)) = self.cursor else { return };
        let shape = self.cursor_shape(hovered);
        let Some(&(first, last)) = CURSOR_SHAPES.get(shape) else { return };
        // the two stamps of 0x4E1C90, restamped here because this is the only place that knows
        // which shape won the frame's request and what the button is doing
        if self.cursor_shape_at.get().0 != shape {
            self.cursor_shape_at.set((shape, self.cursor_ms));
        }
        if self.cursor_btn_at.get().0 != self.cursor_btn {
            self.cursor_btn_at.set((self.cursor_btn, self.cursor_ms));
        }
        let frame = cursor_frame(
            shape,
            (first, last),
            self.cursor_ms.wrapping_sub(self.cursor_shape_at.get().1),
            self.cursor_ms.wrapping_sub(self.cursor_btn_at.get().1),
            self.cursor_btn,
        );
        let Some(&(tex, w, h)) = self.shapes.get(&(POINTER.to_string(), frame)) else { return };
        // **The hot spot, settled 2026-09-21 (0x4E1DF1…0x4E1E42).** The drawing function has no
        // per-shape table: it compares the requested shape against **2** and, only then, subtracts
        // `[0x5B2460]` = 6.0 from **both** coordinates before the blit — the two `fld [esp+0x54]`
        // at 0x4E1E03 and 0x4E1E11 read slots four bytes apart because a push sits between them,
        // and the `jne` arm at 0x4E1E2C passes the same two through untouched in the same order.
        // So every other shape really is drawn with its top-left corner on the pointer, and this
        // retires §4.152's last approximation. The port used to offset `x` alone.
        let o = cursor_hot_spot(shape);
        let (pos, size) = self.place(aspect, cx + o, cy + o, w, h);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None });
    }

    pub fn advance_movies(&mut self, r: &mut Renderer, dt_ms: u32) {
        for m in self.movies.values_mut() {
            m.advance(r, dt_ms);
        }
        // the end-of-era movie runs from the moment the show starts it (0x4FF76F)
        if let Some(e) = self.end_of_era.as_mut()
            && e.show.borrow().movie_started().is_some()
            && let Some(m) = e.movie.as_mut()
        {
            m.advance(r, dt_ms);
        }
        if let Some(b) = self.background.as_mut() {
            b.advance_movies(r, dt_ms);
        }
    }

    /// Bind a `NFS5.KNOCKOUTTRACKLIST`: how many rows the knockout has and which one is current.
    pub fn set_ko_strip(&mut self, index: usize, rows: usize, current: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_ko_strip(j, rows, current);
            }
            return;
        }
        self.ko_strip.insert(index, (rows, current));
    }

    /// Bind the events a `NFS5.TrackSmall2dSpline` shows. Each one's `<art>.fsh` is loaded and its
    /// entries 0 (the map) and 1/2 (the start and finish markers) are uploaded at the position the
    /// entry header itself carries — see [`nfs_assets::texture::TextureImage::place`].
    pub fn set_track_map(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, events: &[TrackMapEvent]) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_track_map(r, paths, j, events);
            }
            return;
        }
        let dir = paths.fe_data().join("TrackArt");
        let mut out = Vec::with_capacity(events.len());
        for ev in events {
            let entries = GamePaths::find_file(&dir, &format!("{}.fsh", ev.art))
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            let mut upload = |k: usize| -> Option<(TextureId, f32, f32, f32, f32)> {
                let img = entries.get(k)?;
                let px = img.mips.first()?;
                let key = (ev.art.clone(), k);
                let &mut (tex, w, h) = self.shapes.entry(key).or_insert_with(|| {
                    let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
                    (id, img.width as f32, img.height as f32)
                });
                Some((tex, w, h, img.place[0] as f32, img.place[1] as f32))
            };
            let map = upload(0);
            let mut marker = upload(if ev.backwards { 2 } else { 1 });
            if ev.mirror
                && let (Some(m), Some(k)) = (map, marker.as_mut())
            {
                k.3 = m.3 + m.1 - k.3 - k.1;
            }
            out.push(TrackMapArt { map, marker, name: ev.name.clone() });
        }
        self.track_maps.insert(index, out);
    }

    /// Bind a `NFS5.BADGE` — the Factory Driver licence card.
    ///
    /// `pack` is the card (`license`, `license2`, `license3`, `license4` by stage, 0x4CF1C8), `name`
    /// the player's name (0x501270 returns the global the front end keeps it in), `rank` the line
    /// under it and `colour` the colour the name is printed in. The drop shadow is loaded here too.
    pub fn set_badge(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, pack: &str, name: &str, rank: &str, colour: [f32; 4]) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_badge(r, paths, j, pack, name, rank, colour);
            }
            return;
        }
        self.set_pack_picture(r, paths, "Art", index, pack, 0);
        for entry in [0usize, 1] {
            if self.shapes.contains_key(&(LICENSE_SHADOW.to_string(), entry)) {
                continue;
            }
            let dir = paths.fe_data().join("Art");
            let Some(img) = GamePaths::find_file(&dir, LICENSE_SHADOW)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .and_then(|v| v.into_iter().nth(entry))
            else {
                continue;
            };
            let Some(px) = img.mips.first() else { continue };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
            self.shapes.insert((LICENSE_SHADOW.to_string(), entry), (id, img.width as f32, img.height as f32));
        }
        self.badges.insert(index, (name.to_string(), rank.to_string(), colour));
    }

    /// Point a `NFS5.CarLogo` at a **chronicle** picture instead of a car logo.
    ///
    /// The chronicle's own art is not in `FEData/Art` and is not a logo pair: `0x50757F` builds the
    /// name with `"%s%c%c%02d%d%c.fsh"` — the directory, `'M'` for a picture (`'p'` is the other
    /// kind), the model-group letter from the table at **`0x5B8B18` = `"CBADE"`** indexed by the
    /// group at `[0x5DBA44]`, the year mod 100, a 1-based index within the year, and a variant
    /// `'a' + n`. `.mad` is the same name for the movie. So one whole image, drawn on its own.
    pub fn set_chronicle_picture(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, file: &str) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_chronicle_picture(r, paths, j, file);
            }
            return;
        }
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
            // The picture is a **grid**, and `FEChroniclePicture::draw` (0x508310) measures it:
            // it scans the pack from entry **2** — 0 and 1 are the `bi` and `ci` icons — counting
            // the leading run of entries tagged `1111` as the number of columns (0x5083EF…0x508443),
            // summing those widths for the picture's width and the height of every row's first
            // entry for its height, and then blits entry `2 + row*cols + col` at the running
            // column x and row y (0x50856A…0x50861E). A pack's second row is tagged `Laye`, not
            // `1111`, which is why only the first row is matched by name. CONFIRMED by the size:
            // `MA601a.fsh` measures 4 × (128+128+128+74) by (148+138) = 458×286, and both
            // `chroniclepicture.lay` and `chroniclemedia.lay` give the widget `size=458,286`.
            let tiles: Vec<&nfs_assets::texture::TextureImage> = entries.iter().skip(2).collect();
            let cols = tiles.iter().take_while(|t| t.name.trim() == CHRONICLE_PICTURE_TAG).count();
            if cols == 0 || tiles.is_empty() {
                tracing::debug!(file, entries = entries.len(), "chronicle picture has no `1111` tiles");
                self.logos.remove(&index);
                return;
            }
            let rows = tiles.len() / cols;
            let w: u32 = tiles[..cols].iter().map(|t| t.width).sum();
            let h: u32 = (0..rows).map(|r| tiles[r * cols].height).sum();
            if w == 0 || h == 0 {
                self.logos.remove(&index);
                return;
            }
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            let mut y0 = 0u32;
            for r in 0..rows {
                let mut x0 = 0u32;
                for c in 0..cols {
                    let t = tiles[r * cols + c];
                    if let Some(px) = t.mips.first() {
                        for y in 0..t.height.min(h.saturating_sub(y0)) {
                            let src = (y * t.width * 4) as usize;
                            let dst = (((y + y0) * w + x0) * 4) as usize;
                            let n = (t.width.min(w.saturating_sub(x0)) * 4) as usize;
                            if src + n <= px.len() && dst + n <= rgba.len() {
                                rgba[dst..dst + n].copy_from_slice(&px[src..src + n]);
                            }
                        }
                    }
                    x0 += t.width;
                }
                y0 += tiles[r * cols].height;
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
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_car_logo(r, paths, j, pack, pair);
            }
            return;
        }
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
            let offset = self.attr_num(e, "offset").unwrap_or(0.0) as i32;
            let step = e.int("step").unwrap_or(0) as i32;
            out.push(Control { index: i, x, y, destination: dest, callback, accessor, string_id: e.string_id(), offset, step });
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
        Sprite { texture: self.white, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None }
    }

    /// One gouraud quad of the front end's own primitive **0x4D98D0**, whose four `(x, y, colour)`
    /// triples arrive in the order upper-left, upper-right, lower-left, lower-right (§4.7).
    ///
    /// APPROXIMATION, the one §4.7 already carries: the sprite pipeline has a single colour per
    /// quad, so the quad is stepped into a grid and each cell is filled with the colour at its
    /// middle. An axis is stepped only when something varies along it — the colour, or the two
    /// opposite edges running out of parallel — and then by a step a pixel, to at most eight.
    fn gouraud(&self, out: &mut Vec<Sprite>, aspect: f32, p: [[f32; 2]; 4], c: [[f32; 4]; 4]) {
        let differs = |a: [f32; 4], b: [f32; 4]| (0..4).any(|j| (a[j] - b[j]).abs() > 1.0 / 255.0);
        let steps = |vary: bool, extent: f32| if vary { (extent.abs().ceil() as usize).clamp(2, 8) } else { 1 };
        // u runs UL→UR and BL→BR; v runs UL→BL and UR→BR
        let slant_u = (p[0][1] - p[1][1]).abs() > 0.5 || (p[2][1] - p[3][1]).abs() > 0.5;
        let slant_v = (p[0][0] - p[2][0]).abs() > 0.5 || (p[1][0] - p[3][0]).abs() > 0.5;
        let nx = steps(differs(c[0], c[1]) || differs(c[2], c[3]) || slant_u || slant_v, p[1][0] - p[0][0]);
        let ny = steps(differs(c[0], c[2]) || differs(c[1], c[3]) || slant_u || slant_v, p[2][1] - p[0][1]);
        let at = |u: f32, v: f32, d: usize| {
            let top = p[0][d] + (p[1][d] - p[0][d]) * u;
            let bot = p[2][d] + (p[3][d] - p[2][d]) * u;
            top + (bot - top) * v
        };
        for gy in 0..ny {
            for gx in 0..nx {
                let (u0, v0) = (gx as f32 / nx as f32, gy as f32 / ny as f32);
                let (u1, v1) = ((gx + 1) as f32 / nx as f32, (gy + 1) as f32 / ny as f32);
                let (u, v) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
                let col: [f32; 4] = std::array::from_fn(|j| {
                    let top = c[0][j] + (c[1][j] - c[0][j]) * u;
                    let bot = c[2][j] + (c[3][j] - c[2][j]) * u;
                    top + (bot - top) * v
                });
                let xs = [at(u0, v0, 0), at(u0, v1, 0), at(u1, v0, 0), at(u1, v1, 0)];
                let ys = [at(u0, v0, 1), at(u0, v1, 1), at(u1, v0, 1), at(u1, v1, 1)];
                let (px, py) = (xs.iter().fold(f32::MAX, |m, v| m.min(*v)), ys.iter().fold(f32::MAX, |m, v| m.min(*v)));
                let cw = xs.iter().fold(f32::MIN, |m, v| m.max(*v)) - px;
                let ch = ys.iter().fold(f32::MIN, |m, v| m.max(*v)) - py;
                out.push(self.quad(aspect, px, py, cw.max(0.5), ch.max(0.5), col));
            }
        }
    }

    /// The state a widget draws in, which `FeWidget::updateState` **0x4C82A0** decides from its
    /// flag word `[this+0x10C]` and the mask `[this+0x110]` of states its template defines. The
    /// cascade is first-match-wins, each arm jumping to the end at 0x4C842E:
    ///
    /// | | condition | state |
    /// |---|---|---|
    /// | 0x4C82E5 | flags & 0x02 | keep whatever it has — no update |
    /// | 0x4C82FA | flags & 0x20 | `hidden` |
    /// | 0x4C831E | flags & 0x10, or `[+0x114]` set with `[+0x110] & 0x10` | `disabled` |
    /// | 0x4C835E | `[+0x110] & 0x80` and flags & 0x40 and flags & 0x04 | `focusedhover` |
    /// | 0x4C839A | flags & 0x40 | `focused` |
    /// | 0x4C83C1 | `[+0x110] & 0x100` and flags & 0x08 and flags & 0x04 | `selectedhover` |
    /// | 0x4C83EE | flags & 0x08 | `selected` |
    /// | 0x4C840F | flags & 0x04 | `hover` |
    /// | | otherwise | `normal` |
    ///
    /// So **0x04 is hover, 0x08 selected and 0x40 focused**, and hover is the weakest of the
    /// three — it only combines with the other two when the template names the combination.
    ///
    /// The glinty box a widget builds out of `data.glintybox` — `RE/notes/controls.md` §4.60.
    ///
    /// `0x51FDA0(0xFF000000, data.width, data.height, data.glintybox)` (0x4C9077 for a button,
    /// from the three the bind read at 0x4C8FCF…0x4C8FF5) and `0x51FDE0` draws it: a one-pixel
    /// outline of the box in that colour through the fade, and then §4.27's flourish with the
    /// style — 1…4 a fixed corner and **5 a fresh `rand() % 4 + 1` each run** (0x51FE86).
    ///
    /// It is the same painter, behind the same `THRASH_setstate(0x1C3, 0)` gate (§4.61), that
    /// `NFS5.GlintyRectangle` uses for its own 181 widgets — and this port draws those. Drawing
    /// one set and not the other would be the port disagreeing with itself, so it draws both; the
    /// gate is one uncertainty, not two, and it is `RE/AUDIT.md` §E's.
    fn glinty_box(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let style = match self.attr_num(e, "glintybox") {
            Some(v) if v as i32 != -1 => v as i32,
            _ => return,
        };
        // each class sizes its box its own way, and all three read the same three arguments into
        // `0x51FDA0(0xFF000000, w, h, style)`:
        //
        // * `NFS5.GOTOBUTTON` — `data.width` by `data.height` (0x4C8FEA, 0x4C8FF5);
        // * the tab buttons — `data.width` by the literal **18**, `0x12` at 0x4D0108;
        // * the combo family — **`data.width`** (`[this+0x1C8]`, 0x4C593B) by the
        //   `backrect`'s height, which is `data.height` (0x4C594B on `[this+0x17C]`), and it is
        //   built lazily in the draw, only once `[+0x20C]` is still null (0x4C5937).
        let (gw, gh) = match e.class.as_str() {
            "NFS5.TABBUTTON" | "NFS5.GRAPHICTABBUTTON" => (self.attr_num(e, "width").unwrap_or(0.0) as f32, 18.0),
            "NFS5.ComboBoxSlider" | "NFS5.ColorBox" | "NFS5.ListBox" => (
                // `[this+0x1C8]` is `data.width`, not `data.selectionboxwidth`: 0x4C4FCB…0x4C5002
                // pushes each attribute name *before* storing the previous call's answer, so the
                // whole run is one slot along from what an earlier reading of it had. With
                // `selectionboxwidth` here the box came out too narrow for 43 of the 86 combo
                // sliders and had **no width at all** for the 11 colour boxes, which all carry 0.
                self.attr_num(e, "width").unwrap_or(0.0) as f32,
                self.attr_num(e, "height").unwrap_or(0.0) as f32,
            ),
            _ => (self.attr_num(e, "width").unwrap_or(0.0) as f32, self.attr_num(e, "height").unwrap_or(0.0) as f32),
        };
        if gw < 1.0 || gh < 1.0 {
            return;
        }
        let c = tint([0, 0, 0, 0xff]);
        for (bx, by, bw, bh) in [(x, y, gw, 1.0), (x, y + gh - 1.0, gw, 1.0), (x, y, 1.0, gh), (x + gw - 1.0, y, 1.0, gh)] {
            out.push(self.quad(aspect, bx, by, bw, bh, c));
        }
        match style {
            0 => {}
            5 => self.glinty_flourish(out, aspect, x, y, gw as i32, gh as i32, (i % 4) as i32 + 1),
            v @ 1..=4 => self.glinty_flourish(out, aspect, x, y, gw as i32, gh as i32, v),
            _ => {}
        }
    }

    /// `FeWidget::hoveredWithButtonDown`, **0x4C7CB0** — the test almost every decoration in the
    /// front end hangs off, and which these notes read wrong until §4.55.
    ///
    /// It is `flags & 0x04` (hover) **and** `0x5299C0`, which is one byte, `[0x69CAE8]`, written in
    /// exactly one place — `EventPump::poll` at 0x5297E5, with whether the button that just
    /// changed is now **down** — and read in exactly one place, 0x5299C1. So the test is
    /// *hovered while a mouse button is held*, not "hovered and the screen settled", and the last
    /// three conditions are the ordinary ones: not hidden (0x20), not disabled (0x10), and not
    /// disabled by its accessor (0x4E08E0 on `[+0x114]`).
    fn pressed_on(&self, i: usize, hovered: Option<usize>) -> bool {
        (hovered == Some(i) || self.capture == Some(i)) && self.pressed && !self.state_hidden_or_disabled(i)
    }

    /// The last two arms of 0x4C7CB0 — hidden (0x20) and disabled (0x10).
    ///
    /// **Both are read out of `[this+0x10C]`, the live flag word, not out of the file**: 0x4C7CC5
    /// loads that dword and 0x4C7CCB / 0x4C7CCF test it. A `.lay` block's `state=` is only how the
    /// word *starts*; 0x50D910 sets and clears the hidden bit by name at fifty-one call sites and
    /// 0x4C7E80 / 0x4C7EB0 do it directly (§4.112), and the port keeps both as
    /// [`Self::hidden_flags`] and [`Self::disabled_flags`]. Reading the attribute alone left a
    /// widget the screen had hidden at run time still pressable. *(The comment here used to say
    /// "nothing in this port disables a widget", which `disabled_flags` and `state_for`'s own
    /// first arm had already made false. Corrected 2026-09-21.)*
    ///
    /// Not covered: 0x4C7CD3's third way out — `[this+0x114]` set, the template carrying a
    /// `disabled` state (`[this+0x110] & 0x10`) and 0x4E08E0 agreeing — the accessor-driven
    /// disable, which is the same condition §4.110 reads at 0x4C831E.
    /// The state a widget is drawn in right now, by name — what `FEControl::onMouseEnter`
    /// 0x4C7BC3…0x4C7BFC compares with `"hover"`. `None` off this screen's own widgets.
    pub fn state_name(&self, i: usize, selected: Option<usize>) -> Option<String> {
        let e = self.layout.elements.get(i)?;
        Some(self.state_for(e, i, selected, self.hovered()).to_string())
    }

    /// 0x4C7E40 — the disabled bit (0x10) of the live flag word, or a `.lay` block that starts
    /// `state=disabled`.
    pub fn widget_disabled(&self, i: usize) -> bool {
        self.disabled_flags.contains(&i)
            || self.layout.elements.get(i).and_then(|e| e.get("state")).is_some_and(|s| s == "disabled")
            || self.covered_disables(i)
    }

    /// 0x4C7E4C…0x4C7E62, `isDisabled`'s third arm: the owning screen is covered and the
    /// widget's template has a `disabled` state.
    fn covered_disables(&self, i: usize) -> bool {
        self.covered
            && self.layout.elements.get(i).is_some_and(|e| self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled")))
    }

    /// Cover or uncover this screen (`[+0x261]`, 0x4E08C0 / 0x4E08A0).
    pub fn set_covered(&mut self, on: bool) {
        self.covered = on;
    }

    fn state_hidden_or_disabled(&self, i: usize) -> bool {
        if self.is_hidden(i) || self.disabled_flags.contains(&i) {
            return true;
        }
        self.layout
            .elements
            .get(i)
            .and_then(|e| e.get("state"))
            .is_some_and(|s| s == "hidden" || s == "disabled")
    }

    /// The entry a `NFS5.BOTTOMBARBUTTON` shows — its tick, **0x4C9980** (§4.53).
    ///
    /// It keeps **three** six-field blocks, laid out `start, end, totalticks, delay, mindelay,
    /// maxdelay` and bound at 0x4C9778 onwards: the plain one at `[this+0x18C]`, the hover one at
    /// `[+0x1A4]` and the hovermousedown one at `[+0x1BC]`. The tick picks between them at
    /// 0x4C99C7: when `data.seperatehovermousedown` (`[+0x180]`) is set **and** 0x4C7CB0 says the
    /// widget is hovered and the screen settled, the **hovermousedown** block; otherwise, when the
    /// plain hover flag is up (0x4C7C90), the hover block; otherwise the plain one. All four
    /// shipped buttons carry `seperatehovermousedown=1`, so hovering shows the *hovermousedown*
    /// run and the `hover…` one is only reached while a screen is still moving.
    ///
    /// Within a block the machine is `NFS5.AnimatingPicture`'s, and a `totalticks` of 0 is forced
    /// to **1** at 0x4C97DF, which with `start == end` is simply a still frame — which is what
    /// three of the four are at rest.
    fn bar_button_frame(&self, e: &nfs_formats::lay::Element, hot: bool, down: bool) -> Option<usize> {
        let sep = self.attr_num(e, "seperatehovermousedown").unwrap_or(0.0) != 0.0;
        let p = match (hot, down && sep) {
            (_, true) => "hovermousedown",
            (true, false) => "hover",
            _ => "",
        };
        let key = |k: &str| e.int(&format!("{p}{k}")).unwrap_or(0).max(0);
        let (a, b) = (key("startframe"), key("endframe"));
        let n = e.int(&format!("{p}totalticks")).filter(|v| *v > 0).unwrap_or(1);
        let lo = key("mindelay");
        let hi = key("maxdelay").max(lo);
        let ticks = (self.clock_ms as u64 * 128 / 1000) as i64;
        let period = n + (lo + hi) / 2;
        let t = if period > 0 { ticks.rem_euclid(period) } else { 0 };
        Some((a + (t.min(n) * (b - a) / n)).clamp(a.min(b), a.max(b)) as usize)
    }

    /// The tooltip a widget with `data.helptext` puts up when the pointer rests on it —
    /// `FeWidget`'s tick, 0x4C7FE2…0x4C8226 (`RE/notes/controls.md` §4.52).
    ///
    /// ```text
    /// 0x4C7FE2  no `[this+0x128]` (the string id) -> nothing
    /// 0x4C7FEE  not hovered -> nothing, and 0x4C824D restamps `[this+0x12C]` with the tick
    /// 0x4C8000  hovered for fewer than 0x80 ticks — **one second** at 128 Hz -> nothing
    /// 0x4C8057  wrap `FEStrings[[+0x128]]` to **310.0f** in `Univers8b.ffn` (0x4C72B1)
    /// 0x4C808A  take the pointer from the mouse device (0x5372A0(6), x at +4 and y at +8)
    /// 0x4C80C7  x = pointer.x, pushed to 636 - width when x + width + 8 would pass 640
    /// 0x4C80EE  y = pointer.y - height - 4, floored at 0
    /// 0x4C8112  a = clamp((tick - [+0x12C] - 0x80) * 4, 0, 255) — half a second of fade-in
    /// 0x4C8193  a **white** panel, `width + 8` by `height + 4`, at `a · 0xC0 / 255`
    /// 0x4C81E0  the same box outlined black at `a`
    /// 0x4C8226  the wrapped text at (+4, +2), black at `a`
    /// ```
    fn tooltip(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, strings: &Strings) {
        let Some((mx, my)) = self.cursor else { return };
        let id = self.attr_num(e, "helptext").unwrap_or(0.0) as u32;
        if id == 0 {
            return;
        }
        // the port counts milliseconds where the original counts 128 Hz ticks
        let ticks = self.hover_ms() * 128 / 1000;
        if ticks < 0x80 {
            return;
        }
        let Some(text) = strings.get(id) else { return };
        let Some(f) = self.fonts.get(SMALL_FONT) else { return };
        let lines = wrap_lines(f, &text, 310.0);
        if lines.is_empty() {
            return;
        }
        let tw = lines.iter().map(|l| f.place(l).1).fold(0.0f32, f32::max).trunc();
        let th = (lines.len() as f32 * (f.line_px() + 2.0)).trunc();
        let x = if mx + tw + 8.0 > 640.0 { 636.0 - tw } else { mx };
        let y = (my - th - 4.0).max(0.0);
        let a = (((ticks - 0x80) * 4).min(255) as f32) / 255.0;
        out.push(self.quad(aspect, x, y, tw + 8.0, th + 4.0, [1.0, 1.0, 1.0, a * 192.0 / 255.0]));
        for (bx, by, bw, bh) in [(x, y, tw + 8.0, 1.0), (x, y + th + 3.0, tw + 8.0, 1.0), (x, y, 1.0, th + 4.0), (x + tw + 7.0, y, 1.0, th + 4.0)] {
            out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), [0.0, 0.0, 0.0, a]));
        }
        let mut ly = y + 2.0;
        for l in &lines {
            self.text(out, aspect, f, l, x + 4.0, ly, 0.0, 0, [0.0, 0.0, 0.0, a]);
            ly += f.line_px() + 2.0;
        }
    }

    /// How long the pointer has rested where it is. A still parks the pointer with
    /// `NFS_FE_CURSOR`, and then the screen clock is the rest — so `NFS_FE_CLOCK=1500` with a
    /// parked pointer shows the tooltip half way through its fade.
    fn hover_ms(&self) -> u32 {
        if std::env::var("NFS_FE_CURSOR").is_ok() { self.clock_ms } else { self.hover_ms }
    }

    /// An open combo's drop-down list — `FeComboBox` slot 27, 0x4C5810, the stretch a list box
    /// jumps over (0x4C58D6, 0x4C6555).
    ///
    /// The rect is the `selectionbox` part's, `[this+0x178]`: `pos=0,data.height` and
    /// `size=data.width,data.selectionboxmaxheight` from `ComboBox.ctr`, its **width** reduced by
    /// 0x4DB030's 16 while the list scrolls (0x4C542D / 0x4C544A through the setter 0x4E2BF0 —
    /// the only geometry the code touches, so the **height stays the template's**).
    ///
    /// * the panel is a gouraud rect, 0x4D96B0 at 0x4C6605, amber `0xffffbb00` across the top and
    ///   `0x4D8850(amber, 0xb282a8b0, t)` across the bottom, both through the fade (0x4D87F0),
    ///   with `t = [this+0x1C4] · 255 / [this+0x1C0]` — the rows' height over the box's, so how
    ///   full the list is (0x4C655C…0x4C658C). Its width takes the 16 back (0x4C65F1);
    /// * then the same rect outlined in black — `0x4D87E0` of the fade through the four-bar
    ///   0x4D9200 (0x4C66A6…0x4C66E0), this one at the plain width;
    /// * then the rows, from the shared loop at 0x4C66E8, the first being the scrollbar's own
    ///   position `[this+0x1BC]->slot10()`. Their colours are §4.42's three, and `ComboBox.ctr`
    ///   defines none of them — so the text is black, which is what the amber panel wants.
    /// The combo drop-down's own scrollbar — **not** the list box's `NFS5.VerticalSlider`.
    /// `FeComboBox` builds it at 0x4C516A through **0x4DADB0**, a class of its own (vtable
    /// 0x5B6A64) whose six arrows come out of `WeeIcons.fsh` by tag, and **0x4DAF10** lays it out
    /// from `(x, y, h)` in literals:
    ///
    /// ```text
    /// 0x4DAF2A  trough    [+0x10C…0x118] = (x,      y,        16, h)
    /// 0x4DAF33  up arrow  [+0x12C…0x138] = (x,      y,        16, 13)
    /// 0x4DAF91  down      [+0x11C…0x128] = (x,      y + h-13, 16, 13)
    /// 0x4DAF24  knob      [+0xFC…0x108]  = (x + 1,  y + 13,   14, 30)
    /// 0x4DAFC5  and the knob's height is redone: max(16, h - 2·13 - count)
    /// ```
    ///
    /// and **0x4DB530** paints it: the trough filled `0x80FEDD81` through the fade (0x4DB592)
    /// with §4.21's bevel at a thickness of 3 over it (0x4DB5DB), then the knob through
    /// **0x5215C0** with a thickness of 7 (0x4DB622) — whose own body, the one 0x4D96B0 in it
    /// (0x5216DA), is a vertical ramp from **0xFF9CD1FF** to **0xFFFEDD81**, pale blue over pale
    /// cream.
    ///
    /// 0x5215C0's four 0x4D98D0 bands over that body — 0x521748, 0x5217A0, 0x5217C8 and 0x5217FB —
    /// **are drawn**, by `bevel_knob` below, with the palette §4.97 and §4.98 pinned. This comment
    /// used to say they were not, and that "its palette is 0x521250's exactly (§4.77) … putting
    /// §4.77's there would be a substitution": both halves were stale, and the second was also
    /// wrong. 0x5215C0 mixes **seventeen** colours, not eight; its first eight *are* 0x521250's,
    /// but four of them are dead here (0x80000000 at 0x5215FC and 0xB2FFFFFF, 0xCCFFFFFF,
    /// 0xCC000000 at 0x521620…0x521636, each left in `eax` and clobbered by the next call), and
    /// nine more follow. Re-read from the image 2026-09-21 and it agrees with §4.98 exactly. The knob's travel is the
    /// port's own scroll position, the same reading §4.45 makes for the list box.
    fn combo_scrollbar(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, h: f32, rows: usize, first: usize, tint: &dyn Fn([u8; 4]) -> [f32; 4]) {
        const W: f32 = 16.0;
        const ARROW_H: f32 = COMBO_ARROW_H;
        out.push(self.quad(aspect, x, y, W, h, tint([0xFE, 0xDD, 0x81, 0x80])));
        self.bevel(out, aspect, x, y, W, h, 3.0);
        if let Some(a) = self.combo_arrows {
            // `[this+0xEC]` picks the frame: the resting `ARup`/`A2up`, the hovered `ARov`/`A2ov`
            // or the held `ARin`/`A2in` — 0x4DB820 and 0x4DB86B choose between them by comparing
            // it against the region the hit test 0x4DB2F0 wrote. §4.101.
            let frame = |base: usize, region: u32| {
                a[base + match self.combo_scroll {
                    Some((_, r, held)) if r == region => usize::from(held) + 1,
                    _ => 0,
                }]
            };
            for (n, ay) in [(frame(0, 1), y), (frame(3, 2), y + h - ARROW_H)] {
                let Some(&(tex, aw, ah)) = self.shapes.get(&("weeicons.fsh".to_string(), n)) else { continue };
                let (pos, size) = self.place(aspect, x, ay, aw, ah);
                out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None });
            }
        }
        // the knob: its height is 0x4DAFC5's, and its travel the port's own first row
        let knob_h = (h - 2.0 * ARROW_H - rows as f32).max(16.0);
        let travel = (h - 2.0 * ARROW_H - knob_h).max(0.0);
        let steps = rows.saturating_sub(1).max(1) as f32;
        let ky = y + ARROW_H + travel * (first as f32 / steps).min(1.0);
        let n = knob_h.round().max(1.0) as usize;
        for d in 0..n {
            let t = (d as f32 + 0.5) / n as f32;
            let c: [f32; 4] = std::array::from_fn(|j| {
                let (a, b) = (tint([0x9C, 0xD1, 0xFF, 0xFF]), tint([0xFE, 0xDD, 0x81, 0xFF]));
                a[j] + (b[j] - a[j]) * t
            });
            out.push(self.quad(aspect, x + 1.0, ky + d as f32 * knob_h / n as f32, 14.0, (knob_h / n as f32).max(0.5), c));
        }
        // and §4.98's outline **over** the body, the way 0x5215C0 lays its four quads after the
        // one 0x4D96B0, at the thickness 7 that 0x4DB622 passes
        self.bevel_knob(out, aspect, x + 1.0, ky, 14.0, knob_h, 7.0);
        // Then two three-pixel slivers, one above the knob and one below, through the **clipped**
        // gouraud rect 0x4D94A0 that §4.29's glint sweep also uses — clipped to the trough:
        //
        // ```text
        // 0x4DB6E6  (troughX + 1, knobY - 3,        14, 3)  0xFFFFFFFF -> 0x19FFFFFF
        // 0x4DB783  (troughX + 1, knobY + knobH,    14, 3)  0x19000000 -> 0xCC000000
        // ```
        //
        // — a lit edge over the knob and a shadow under it, the 14.0 and 3.0 being the literals
        // 0x41600000 and 0x40400000 the pushes carry. §4.100.
        for (sy, top, bot) in [
            (ky - 3.0, tint([0xff, 0xff, 0xff, 0xff]), tint([0xff, 0xff, 0xff, 0x19])),
            (ky + knob_h, tint([0, 0, 0, 0x19]), tint([0, 0, 0, 0xcc])),
        ] {
            for d in 0..3 {
                let sy = sy + d as f32;
                // **0x4D94A0's clip box, pinned 2026-09-21.** 0x4DB65D…0x4DB67A builds it from the
                // widget's own four-int rectangles — the ones the hit test 0x4DB2F0 walks — plus
                // the screen origin in `ebx`/`ebp` that every coordinate here carries:
                //
                // ```text
                // left   = trough.x                  ([this+0x10C] + ebp)
                // top    = upArrow.y + upArrow.h     ([this+0x130] + [this+0x138] + ebx)
                // right  = trough.x + trough.w       ([this+0x10C] + [this+0x114] + ebp)
                // bottom = downArrow.y               ([this+0x120] + ebx)
                // ```
                //
                // — the trough's **interior**, between the two arrows, not the widget's box. The
                // port used to clip to the whole box, which differs only when the knob is at an
                // extreme: there a sliver would show over an arrow. §4.100.
                if !combo_sliver_visible(sy, y, h) {
                    continue;
                }
                let t = (d as f32 + 0.5) / 3.0;
                let c: [f32; 4] = std::array::from_fn(|j| top[j] + (bot[j] - top[j]) * t);
                out.push(self.quad(aspect, x + 1.0, sy, 14.0, 1.0, c));
            }
        }
    }

    /// The scrollbar geometry of the open drop-down `i` — `(x, y, height, rows, first)`, the four
    /// numbers `combo_scrollbar` draws from. `None` when the list is short enough not to scroll.
    fn combo_scroll_geom(&self, i: usize) -> Option<(f32, f32, f32, usize, usize)> {
        let e = self.layout.elements.get(i)?;
        let (rows, _) = self.lists.get(&i)?;
        if rows.is_empty() {
            return None;
        }
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        if full < max_h {
            return None;
        }
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (x, y) = self.position_of(e);
        let px = if left { x - grow } else { x };
        Some((px + width + grow - LIST_SCROLLBAR_W, y + height, max_h, rows.len(), self.scroll.get(&i).copied().unwrap_or(0)))
    }

    /// `FeListScrollBar::hitTest` 0x4DB2F0 — which of the bar's four rectangles the pointer is in.
    /// It tries them in a fixed order, each with the same `x <= px <= x + w && y <= py <= y + h`
    /// pair of `fcompp` runs, and writes the answer to `[this+0xF0]`:
    ///
    /// ```text
    /// 0x4DB2F7  [ecx+0x12C] -> 1   ; the up arrow      (0x4DB371)
    /// 0x4DB388  [ecx+0x11C] -> 2   ; the down arrow    (0x4DB3F5)
    /// 0x4DB40C  [ecx+0x0FC] -> 3   ; the knob          (0x4DB479)
    /// 0x4DB490  [ecx+0x10C] -> 4   ; the trough        (0x4DB4FD)
    /// 0x4DB510              -> 0   ; none of them
    /// ```
    ///
    /// The knob is tested **before** the trough, so a press on the knob never counts as the track
    /// — which is what lets arm 3 (drag) and arm 4 (page) of the press handler 0x4DB040 differ.
    /// The four rectangles are the ones the constructor 0x4DAF10 laid out (§4.90).
    pub fn combo_scroll_at(&self, px: f32, py: f32) -> Option<(usize, u32)> {
        let i = self.open_combo?;
        let (x, y, h, rows, first) = self.combo_scroll_geom(i)?;
        match scrollbar_region(x, y, h, rows, first, px, py) {
            0 => None,
            n => Some((i, n)),
        }
    }

    /// Which combo has its drop-down open, if any.
    pub fn open_combo(&self) -> Option<usize> {
        self.open_combo
    }

    /// Remember where the pointer is on the open drop-down's scrollbar, and whether it is held, so
    /// the arrows can show `ARov`/`A2ov` and `ARin`/`A2in` (§4.101).
    pub fn set_combo_scroll(&mut self, at: Option<(usize, u32, bool)>) {
        self.combo_scroll = at;
    }

    /// The class of the widget at the flattened index, following a group the same way the hit
    /// tests do.
    pub fn class_of(&self, index: usize) -> Option<String> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.class_of(j);
        }
        self.layout.elements.get(index).map(|e| e.class.clone())
    }

    /// An activate on an input box takes focus, as `FeInputBox`'s slot 9 does:
    ///
    /// ```text
    /// 0x4CCB73  if (!isFocused())            ; 0x4C7DD0, [this+0x10C] & 0x40
    /// 0x4CCB7E      setFocused()             ; 0x4C7D70 — only when [this+0x110] & 0x40 allows it
    /// 0x4CCB85      [this+0x148] = 1         ; the editing latch
    /// 0x4CCB9B  else if (![this+0x148]) clearFocused()   ; 0x4C7DA0
    /// ```
    ///
    /// so the first click focuses the box and it stays focused: the latch is 1 from then on, and
    /// the `else` arm cannot fire. `NFS5.CHATINPUTBOX` shares that slot, and its own bind
    /// 0x4C32D3 starts by calling the input box's 0x4CC430, so `data.alwaysfocused` reaches it
    /// too (§4.109).
    /// Every class on this screen with the parts its template declares that **no state lists**.
    ///
    /// The part walk can never reach one, so only the class's own draw can put it down — and
    /// `NFS5.CAMERAPICTURE`'s `shp` (§4.129) is exactly that: a part the bind looks up by name.
    /// A measurement axis for the same kind of gap: a part here that the port draws nowhere is a
    /// thing the original puts on the screen and this port does not.
    pub fn unlisted_parts(&self) -> Vec<(String, Vec<String>)> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for e in &self.layout.elements {
            let Some(c) = self.controls.get(&e.class) else { continue };
            let used: std::collections::BTreeSet<&str> = c.states.values().flatten().map(String::as_str).collect();
            let dead: Vec<String> = c.parts_order.iter().filter(|p| p.as_str() != "data" && !used.contains(p.as_str())).cloned().collect();
            if !dead.is_empty() {
                out.insert(e.class.clone(), dead);
            }
        }
        for (_, g) in self.group_spans() {
            out.extend(g.unlisted_parts());
        }
        out.into_iter().collect()
    }

    /// Every class on this screen with the states its `.ctr` template declares — the very set
    /// `FeWidget`'s slot 12 (0x4C7760) turns into `[widget+0x110]`, and therefore the complete
    /// list of looks the original can ever put a widget of that class into (§4.109, §4.110).
    /// A measurement axis: a state a template declares and this port never returns is a look the
    /// original has and the port does not, and no blank count can see it.
    pub fn declared_states(&self) -> Vec<(String, Vec<String>)> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for e in &self.layout.elements {
            if let Some(c) = self.controls.get(&e.class) {
                let mut v: Vec<String> = c.states.keys().cloned().collect();
                v.sort();
                out.insert(e.class.clone(), v);
            }
        }
        for (_, g) in self.group_spans() {
            out.extend(g.declared_states());
        }
        out.into_iter().collect()
    }

    /// The flattened indices of the widgets whose `.lay` `name` is one of `names`, with the
    /// position of the name that matched — what 0x50D9A0's `FeScreen::findWidget` does.
    pub fn widgets_named(&self, names: &[&str]) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let n = e.get("name")?;
                names.iter().position(|w| w.eq_ignore_ascii_case(n)).map(|k| (i, k))
            })
            .collect();
        for (base, g) in self.group_spans() {
            out.extend(g.widgets_named(names).into_iter().map(|(i, k)| (i + base, k)));
        }
        out
    }

}

/// `FEChronicleMainBoxControl`'s grid, from its slot 8 — **0x508020**:
///
/// ```text
/// 0x50802D  p = the widget's position (0x525310)
/// 0x508042  [this+0x154] = (ev.x - p.x) / 91          ; 0xB40B40B5, add, sar 6
/// 0x508055  [this+0x150] = (ev.y - p.y - 0x26) / 61   ; 0x4325C53F, sar 4
/// 0x50807A  [this+0x14C] = 0 <= col < 5 && 0 <= row < 3
/// ```
///
/// The two magic constants are emulated rather than guessed: 91 and 61 are the only divisors that
/// reproduce them over the whole range. Slot 7 (0x508010) clears the flag when the pointer leaves,
/// and slot 9 (0x5080C0) is the click — **row 0 goes to `chroniclepicture` and row 1 to
/// `chroniclemedia`**, each through `0x5074A0(row, [this+0x160 + 4*col], [this+0x174 + 4*col])`.
/// So the row is the kind and the column the year-and-variant. `RE/notes/controls.md` §4.120.
pub fn chronicle_cell(px: f32, py: f32, x: f32, y: f32) -> Option<(usize, usize)> {
    let (dx, dy) = (px - x, py - y - 0x26 as f32);
    if dx < 0.0 || dy < 0.0 {
        return None;
    }
    let (col, row) = ((dx as i32 / 91) as usize, (dy as i32 / 61) as usize);
    (col < 5 && row < 3).then_some((col, row))
}

impl LayoutScreen {
    /// Which timeline cell the pointer is on, against this screen's own
    /// `NFS5.ChronicleMainBoxControl` (§4.120).
    pub fn chronicle_cell_at(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        let i = self.layout.elements.iter().position(|e| e.class == "NFS5.ChronicleMainBoxControl")?;
        let (x, y) = self.position_of(&self.layout.elements[i]);
        chronicle_cell(px, py, x, y)
    }
}

impl LayoutScreen {
    /// The timeline's five columns, in the order 0x507B4F walks them (§4.122).
    pub fn set_chronicle_columns(&mut self, cols: Vec<(u32, usize)>) {
        self.chronicle_columns = cols;
        for g in self.groups.values_mut() {
            g.set_chronicle_columns(self.chronicle_columns.clone());
        }
    }

    /// The pack and the per-cell entries the timeline's icons come from (§4.123).
    pub fn set_chronicle_icons(&mut self, pack: String, cells: std::collections::BTreeMap<(usize, usize), (usize, usize)>) {
        self.chronicle_icons = (pack, cells);
    }

    /// `FEChroniclePictureControl`'s three mouse slots — the class `NFS5.ChroniclePicture`
    /// instantiates, vtable **0x5B8C74**:
    ///
    /// ```text
    /// 0x508250  slot 5  [+0x154] = [+0x14C]; [+0x158] = [+0x150]     ; remember the offset
    ///                   [+0x140] = 1; [+0x144] = ev.x; [+0x148] = ev.y
    /// 0x5082A0  slot 8  while [+0x140]:
    ///                     [+0x14C] = [+0x154] - ev.x + [+0x144]
    ///                     [+0x150] = [+0x158] - ev.y + [+0x148]
    /// 0x508290  slot 9  [+0x140] = 0
    /// ```
    ///
    /// so the offset moves **against** the pointer, the way a document scrolls under a hand, and
    /// the bind 0x508220 starts all five at zero. §4.119.
    pub fn picture_press(&mut self, index: usize, x: f32, y: f32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.picture_press(j, x, y);
            }
            return;
        }
        let (ox, oy) = self.picture_pan.get(&index).copied().unwrap_or((0.0, 0.0));
        self.picture_drag = Some((index, x, y, ox, oy));
    }

    /// Slot 8, 0x5082A0 — the pan follows the pointer while the button is held.
    pub fn picture_drag_to(&mut self, x: f32, y: f32) {
        if let Some((i, gx, gy, ox, oy)) = self.picture_drag {
            self.picture_pan.insert(i, (ox - x + gx, oy - y + gy));
        }
        for g in self.groups.values_mut() {
            g.picture_drag_to(x, y);
        }
    }

    /// Slot 9, 0x508290.
    pub fn picture_release(&mut self) {
        self.picture_drag = None;
        for g in self.groups.values_mut() {
            g.picture_release();
        }
    }

    /// Enable or disable a widget, as the by-name helper **0x50D880** does: 0x50D8DB reads the
    /// bit, and its argument is **enable**, not disable — 0x50D8EA takes 0x4C7DF0, which is
    /// `and al, 0xEF`, and 0x50D8F1 takes 0x4C7E10, which is `or al, 0x10` behind the template's
    /// own `[state.disabled]`. §4.113.
    pub fn set_disabled_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_disabled_flag(j, on);
            }
            return;
        }
        if on {
            self.disabled_flags.insert(index);
        } else {
            self.disabled_flags.remove(&index);
        }
    }

    /// Hide or show a widget, as the by-name helper **0x50D910** does: it finds the widget on the
    /// screen, clears the hidden bit with 0x4C7ED0 and then sets it with 0x4C7E80 or clears it
    /// with 0x4C7EB0 (0x50D96B…0x50D981). Fifty-one call sites use it — the repair screen's ten
    /// damage bars, the parts screen's install and uninstall buttons, the used-car screen's two
    /// panels. §4.112.
    pub fn set_hidden_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_hidden_flag(j, on);
            }
            return;
        }
        if on {
            self.hidden_flags.insert(index);
        } else {
            self.hidden_flags.remove(&index);
        }
    }

    /// Whether the screen has hidden this widget.
    pub fn is_hidden(&self, index: usize) -> bool {
        match self.route(index) {
            Some((k, j)) => self.groups.get(&k).is_some_and(|g| g.is_hidden(j)),
            None => self.hidden_flags.contains(&index),
        }
    }

    /// Set or clear a widget's **`selected`** flag, as 0x50D9A0 does by name: it looks the widget
    /// up on the screen, asks 0x4C7D50 whether the flag is already what is wanted, and calls
    /// 0x4C7D00 or 0x4C7D30 only when it is not (0x50D9FB…0x50DA1B). §4.111.
    pub fn set_selected_flag(&mut self, index: usize, on: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_selected_flag(j, on);
            }
            return;
        }
        if on {
            self.selected_flags.insert(index);
        } else {
            self.selected_flags.remove(&index);
        }
    }

    pub fn focus_input(&mut self, index: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.focus_input(j);
            }
            return;
        }
        // 0x4C7D77's guard: `[this+0x110] & 0x40`. That word is the **mask of the states the
        // widget's template declares**, built by slot 12 (0x4C7760) by walking the name table at
        // 0x5D81FC in 0x19-byte steps — bit 0 `normal`, 1 `stringnotused`, 2 `hover`, 3
        // `selected`, 4 `disabled`, 5 `hidden`, 6 `focused`, 7 `focusedhover`, 8 `selectedhover`,
        // 9 `wrenderoff`. So bit 6 means the `.ctr` has a `[state.focused]`, and a template
        // without one can never take focus. §4.109.
        if self.controls.get(&self.layout.elements[index].class).is_some_and(|c| c.has_state("focused")) {
            self.focused.insert(index);
        }
    }

    /// The longest string an input box's accessor will hold, from the **third argument** of
    /// `FEStringAccessor`'s constructor 0x4BC8B0 — the ctor allocates `maxLen + 1` bytes and the
    /// insert at `vt[0x30]` refuses to pass it. Read out of the three registrations that carry a
    /// name this port can see: `UserList.NameInput` at 0x4E5D9E pushes **8**, `ANOTHER` at
    /// 0x4E6016 pushes **15**, `COMMS.PASSWORD` at 0x50B9C1 pushes **7**.
    ///
    /// Every other box's limit lives in a registration this port has not read, so it is left
    /// **unlimited** rather than guessed — a cap invented here would silently truncate a name the
    /// original accepts, which is worse than one it would have refused.
    pub fn input_max_len(accessor: &str) -> Option<usize> {
        match accessor {
            "UserList.NameInput" => Some(8),
            "ANOTHER" => Some(15),
            "COMMS.PASSWORD" => Some(7),
            _ => None,
        }
    }

    /// Which input box has focus, if any.
    pub fn focused_input(&self) -> Option<usize> {
        self.focused.iter().copied().find(|i| {
            self.layout.elements.get(*i).is_some_and(|e| {
                matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX")
            })
        })
    }

    /// One typed character, filtered exactly as the shared key handler **0x4CCC20** filters it:
    ///
    /// ```text
    /// 0x4CCD95  c >= 0xFF  -> refuse
    /// 0x4CCD9D  c <  0x20  -> refuse
    /// 0x4CCDA2  c == 0x25  -> refuse          ; '%', and nothing else is singled out
    ///           otherwise accessor->vt[0x30](c)
    /// ```
    ///
    /// `'%'` is the only printable character a box turns down, and the reason is visible in what
    /// the strings are for: every front-end string reaching 0x5A0FBF is a `printf` format, so a
    /// typed `%` would be read as a conversion. Returns whether the character was taken — the
    /// caller plays front-end sound 9 on a yes, which is what 0x4AADB0(9) does on every accepted
    /// path.
    pub fn type_char(&mut self, index: usize, c: char) -> bool {
        let code = c as u32;
        if !(0x20..0xFF).contains(&code) || code == 0x25 {
            return false;
        }
        let max = self.layout.elements.get(index)
            .and_then(|e| e.get("accessor"))
            .and_then(Self::input_max_len);
        let buf = self.edits.entry(index).or_default();
        if max.is_some_and(|m| buf.chars().count() >= m) {
            return false;
        }
        buf.push(c);
        true
    }

    /// Backspace — `accessor->vt[0x38]()`, reached from 0x4CCC20's `c == 8` arm. Returns whether
    /// anything was erased.
    pub fn backspace(&mut self, index: usize) -> bool {
        self.edits.get_mut(&index).is_some_and(|b| b.pop().is_some())
    }

    /// Return, `c == 0x0D`: 0x4CCDB5 drops the focus flag through `0x4C7DA0` and then calls
    /// `accessor->vt[0x0C]()` to commit. Hands back the accessor's name and what was typed so the
    /// caller can store it; the focus goes, the text stays — it is the accessor's own buffer,
    /// which `CREATEPLAYERDONE` (0x5107A8) reads through `vt[0x10]` after the box has let go.
    pub fn commit_input(&mut self, index: usize) -> Option<(String, String)> {
        let name = self.layout.elements.get(index)?.get("accessor")?.to_string();
        let text = self.edits.get(&index)?.clone();
        self.focused.remove(&index);
        Some((name, text))
    }

    /// Empty the input box bound to `accessor` — the string accessor's clear, its slot 0x44.
    pub fn clear_input(&mut self, accessor: &str) {
        if let Some(i) = self.layout.elements.iter().position(|e| e.get("accessor") == Some(accessor)) {
            self.edits.remove(&i);
        }
    }

    /// What the input box bound to `accessor` holds — `accessor->vt[0x10]()`, the string
    /// accessor's buffer (empty until something is typed; the buffers go with the screen).
    pub fn input_text(&self, accessor: &str) -> String {
        self.layout
            .elements
            .iter()
            .position(|e| e.get("accessor") == Some(accessor))
            .and_then(|i| self.edits.get(&i).cloned())
            .unwrap_or_default()
    }

    /// The `NFS5.CONTROLLERSETUP` row waiting for a control, if one has focus — its flattened
    /// index and its accessor (`Controller.SteerLeft` …). A row takes focus on activation
    /// (0x51C5F0) and, while it holds it, slot 4 0x51C610 takes every key message (§4.117).
    pub fn focused_binding_row(&self) -> Option<(usize, String)> {
        for &i in &self.focused {
            if let Some(e) = self.layout.elements.get(i)
                && e.class == "NFS5.CONTROLLERSETUP"
            {
                return Some((i, e.get("accessor").unwrap_or_default().to_string()));
            }
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if let Some((j, a)) = g.focused_binding_row() {
                return Some((base + j, a));
            }
            base += g.span();
        }
        None
    }

    /// Let focus go — 0x4C7DA0, which every path of a binding row's capture ends in (0x51C5DE,
    /// 0x51C65C).
    pub fn release_focus(&mut self, index: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.release_focus(j);
            }
            return;
        }
        self.focused.remove(&index);
    }

    /// What an input box should display: what has been typed into it, if anything, and otherwise
    /// whatever its accessor answers.
    fn edited(&self, index: usize) -> Option<&str> {
        self.edits.get(&index).map(String::as_str)
    }

    /// A press on the open drop-down's scrollbar, as `FeListScrollBar::onPress` 0x4DB040 handles
    /// it: arm 1 steps the list back one row (`[this+0xDC] = -1` at 0x4DB080), arm 2 forward one
    /// (`+1` at 0x4DB096), both after the click 0x4AADB0(8). Arms 3 and 4 start a drag, which this
    /// port's drop-down does through `park_knob`, so they are left alone here.
    pub fn combo_scroll_press(&mut self, region: u32) -> bool {
        let Some(i) = self.open_combo else { return false };
        let Some((_, _, _, rows, first)) = self.combo_scroll_geom(i) else { return false };
        let next = match region {
            1 => first.saturating_sub(1),
            2 => (first + 1).min(rows.saturating_sub(1)),
            _ => return false,
        };
        if next == first {
            return false;
        }
        self.scroll.insert(i, next);
        true
    }

    fn drop_down(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let Some((rows, current)) = self.lists.get(&i) else { return };
        if rows.is_empty() {
            return;
        }
        let Some(f) = self.font_for(e) else { return };
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        // `refreshRows` 0x4C53F3: the rows' height, clamped, and the scrollbar that clamping raises
        let (rows_h, scrolls) = if full >= max_h { (max_h, true) } else { (full, false) };
        // **The panel is `data.selectionboxwidth` wide when that is wider than the widget**, and
        // `data.selectionboxslidedirection` says which way it grows. `FeComboBox::refreshRows`
        // 0x4C5607…0x4C5743 animates both the height and the width off the tick clock:
        //
        // ```text
        // 0x4C5626  h    = min((now - [+0x1D4]) * [+0x1C4] / 16, [+0x1C4])   ; the rows' height
        // 0x4C5669  grow = clamp((now - [+0x1D4]) * (sbw - w) / 16, 0, sbw - w)
        // 0x4C56B9  if (sbw < w) grow = 0
        // 0x4C56C1  if (!focused) both count back down — the panel retracts the same way
        // 0x4C56F6  [+0x1D0] == -1 -> 0x4E2BB0(selectionbox, x0 - grow)      ; grows leftwards
        // 0x4C5736  0x4E2BF0(selectionbox, w - <16 while scrolling> + grow)  ; the new width
        // ```
        //
        // so it reaches `max(data.width, data.selectionboxwidth)` after **16 ticks**. This port
        // draws the settled end of that ramp, which is what a screen at rest shows.
        //
        // The three offsets this rests on are **not** what `§4.44` had: 0x4C4FCB…0x4C5002 push
        // each name *before* storing the previous call's answer, so `[+0x1C8]` is `data.width`,
        // `[+0x1CC]` is `data.selectionboxwidth` and `[+0x1D0]` is
        // `data.selectionboxslidedirection`, one slot along from the old table.
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (px, py) = (if left { x - grow } else { x }, y + height);
        let width = width + grow;
        let rect_w = if scrolls { width - LIST_SCROLLBAR_W } else { width };
        let t = if max_h > 0.0 { (rows_h * 255.0 / max_h).round().clamp(0.0, 255.0) as u8 } else { 0 };
        let amber = [0xff, 0xbb, 0x00, 0xff];
        let low = [0x82, 0xa8, 0xb0, 0xb2];
        // 0x4D8850, the per-channel lerp the front end uses everywhere: `a + (b - a) * t / 255`
        let mix = |a: [u8; 4], b: [u8; 4]| -> [u8; 4] {
            std::array::from_fn(|j| {
                let (a, b) = (a[j] as i32, b[j] as i32);
                (a + (b - a) * t as i32 / 255) as u8
            })
        };
        let bottom = mix(amber, low);
        let (ca, cb) = (tint(amber), tint(bottom));
        self.gouraud(
            out,
            aspect,
            [[px, py], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py], [px, py + max_h], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py + max_h]],
            [ca, ca, cb, cb],
        );
        let black = tint([0, 0, 0, 255]);
        // the sixteen pixels the panel gave up carry the drop-down's own scrollbar (§4.90)
        if scrolls {
            self.combo_scrollbar(out, aspect, px + rect_w, py, max_h, rows.len(), self.scroll.get(&i).copied().unwrap_or(0), &tint);
        }
        for (bx, by, bw, bh) in [(px, py, rect_w, 1.0), (px, py + max_h - 1.0, rect_w, 1.0), (px, py, 1.0, max_h), (px + rect_w - 1.0, py, 1.0, max_h)] {
            out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), black));
        }
        let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
        let sel_c = self.attr(e, "highlightcolorselected").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
        // the same foot-of-loop test as the list box's (0x4C6915): the last row overhangs
        let mut acc = 0.0f32;
        let mut ry = py;
        for (n, row) in rows.iter().enumerate() {
            if acc > max_h {
                break;
            }
            acc += row.height();
            if *current == Some(n) {
                out.push(self.quad(aspect, px, ry, rect_w, row.height(), tint(sel_c)));
            }
            let fg = if *current == Some(n) { [1.0, 1.0, 1.0, 1.0] } else { tint(text_c) };
            if let ListRow::Text(t) = row {
                self.text(out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg);
            }
            ry += row.height();
        }
    }

    /// `NFS5.ColorBox`'s custom colour editor — its own **slot 17**, 0x4C3C30, which draws when
    /// the widget holds focus and the class's global latch [0x65B710] is set.
    ///
    /// Everything hangs off the widget's own box `(l, t, r, b)` and the six rects the bind builds
    /// from it (§4.47, [`picker_rects`]). In order, and each one traced:
    ///
    /// 1. the box itself becomes the **title bar** — filled `0x4D87C0(fade)`, which is
    ///    `0xFFBB00` amber (0x4C3D79), outlined black (0x4C3D91), with `FEStrings` **610**,
    ///    "CUSTOM COLOR EDITOR", centred at `y + 2` in black and the `[+0x298]` font,
    ///    `Univers7b.ffn` (0x4C3DF2);
    /// 2. a **160-pixel** panel below it, the widget's width, gouraud amber `0xffffbb00` across
    ///    the top and `0xb282a8b0` across the bottom (0x4C3E33), outlined black (0x4C3E4E);
    /// 3. rect **A** is the **luminance ramp**: one 1-pixel column per x with the hue and the
    ///    saturation held and the third float sweeping 0…1 through 0x4D8900
    ///    (0x4C3E64…0x4C3F06), a black frame one pixel around it (0x4C3F73) and a **3-pixel**
    ///    black marker at `(A.x1 - A.x0 - 2)·l + A.x0 + 1` (0x4C3FEC);
    /// 4. rect **B** is the current colour, filled (0x4C4062) and outlined (0x4C40BD);
    /// 5. rect **C** is the sprite the factory looked up by the tag `COLB` (0x4C40EE), drawn white
    ///    at its top-left, with a black frame one pixel around it (0x4C4151) — the hue across it
    ///    and the saturation up it, which is what its mouse slots read back out of it (§4.66);
    /// 6. rect **D** is the **eight wells**, `[0x691D98 + n·4]` through 0x5019B0 — all
    ///    **zero in the image**, so they start empty — each filled and outlined black, and the
    ///    selected slot `[+0x284]` (**0** out of the factory, 0x4C38B3) outlined white (0x4C42A1);
    /// 7. rect **E** is two buttons, `FEStrings` **702** "ok" and **703** "cancel": dark red
    ///    `0x4D87B0(fade)` — amber `0x4D87C0` while `[this+0x218]`, the region the hit test left,
    ///    is **3** for the left one or **4** for the right (0x4C42D0, 0x4C42D7) — the label
    ///    centred at `y + 2` in white and the `[+0x294]` font, the §4.21 bevel at a thickness of
    ///    **3** over the hovered one (0x4C4414) and a black outline (0x4C44C9).
    ///
    /// The three colour floats are `[+0x288]`, `[+0x28C]` and `[+0x290]` — **hue, saturation and
    /// luminance**, because 0x4D8900 is an HLS→RGB (§4.66) — 0.5 each out of the factory
    /// (0x4C38B9). The port moves them from the pointer exactly as the class's three mouse slots
    /// do, so the editor is live.
    fn color_picker(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let (l, t, r, b) = (x, y, x + w, y + h);
        let rects = picker_rects(l, t, r, b);
        let region = self.cursor.map(|(cx, cy)| picker_region(&rects, cx, cy)).unwrap_or(5);
        let black = tint([0, 0, 0, 255]);
        let white = tint([255, 255, 255, 255]);
        let outline = |out: &mut Vec<Sprite>, x: f32, y: f32, w: f32, h: f32, c: [f32; 4]| {
            for (bx, by, bw, bh) in [(x, y, w, 1.0), (x, y + h - 1.0, w, 1.0), (x, y, 1.0, h), (x + w - 1.0, y, 1.0, h)] {
                out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), c));
            }
        };
        let word = |c: u32| tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]);
        // 1 — the title bar
        out.push(self.quad(aspect, l, t, w, h, tint([0xff, 0xbb, 0x00, 0xff])));
        outline(out, l, t, w, h, black);
        if let Some(f) = self.fonts.get(PICKER_TITLE_FONT) {
            let s = self.fe_strings.get(610).unwrap_or_default();
            self.text(out, aspect, f, &s, l, t + 2.0, w, 1, black);
        }
        // 2 — the panel
        const PANEL_H: f32 = 160.0;
        let amber = tint([0xff, 0xbb, 0x00, 0xff]);
        let muted = tint([0x82, 0xa8, 0xb0, 0xb2]);
        self.gouraud(out, aspect, [[l, b], [l + w, b], [l, b + PANEL_H], [l + w, b + PANEL_H]], [amber, amber, muted, muted]);
        outline(out, l, b, w, PANEL_H, black);
        let (ph, ps, pl) = self.picker.hsl;
        // 3 — the luminance ramp
        let (ax0, ay0, ax1, ay1) = rects[rect::A];
        let mut px = ax0;
        while px <= ax1 {
            let tt = (px - ax0) / (ax1 - ax0);
            out.push(self.quad(aspect, px, ay0, 1.0, ay1 - ay0, word(hls_to_rgb(ph, ps, tt))));
            px += 1.0;
        }
        outline(out, ax0 - 1.0, ay0 - 1.0, (ax1 - ax0) + 2.0, (ay1 - ay0) + 2.0, black);
        outline(out, ((ax1 - ax0 - 2.0) * pl).round() + ax0 + 1.0, ay0 - 2.0, 3.0, (ay1 - ay0) + 4.0, black);
        // 4 — the swatch
        let (bx0, by0, bx1, by1) = rects[rect::B];
        out.push(self.quad(aspect, bx0, by0, bx1 - bx0, by1 - by0, word(hls_to_rgb(ph, ps, pl))));
        outline(out, bx0, by0, bx1 - bx0, by1 - by0, black);
        // 5 — the `COLB` sprite
        let (cx0, cy0, cx1, cy1) = rects[rect::C];
        if let Some(n) = self.colb
            && let Some(&(tex, sw, sh)) = self.shapes.get(&("weeicons.fsh".to_string(), n))
        {
            let (pos, size) = self.place(aspect, cx0, cy0, sw, sh);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: white, rotation: 0.0, pivot: [0.5, 0.5], tri: None });
        }
        outline(out, cx0 - 1.0, cy0 - 1.0, (cx1 - cx0) + 2.0, (cy1 - cy0) + 2.0, black);
        // 6 — the eight wells, and the selected one in white
        let (dx0, dy0, dx1, dy1) = rects[rect::D];
        let w8 = ((dx1 - dx0) / 8.0).trunc();
        for n in 0..8 {
            let sx = dx0 + n as f32 * w8;
            out.push(self.quad(aspect, sx, dy0, w8.max(1.0), dy1 - dy0, word(self.picker.wells[n])));
            outline(out, sx, dy0, w8.max(1.0), dy1 - dy0, black);
        }
        let sel = dx0 + self.picker.well.min(7) as f32 * w8;
        outline(out, sel, dy0, w8.max(1.0), dy1 - dy0, white);
        // 7 — the two buttons
        for (i, id, hot) in [(rect::E_LEFT, 702, 3), (rect::E_RIGHT, 703, 4)] {
            let (ex0, ey0, ex1, ey1) = rects[i];
            let on = region == hot;
            let fill = if on { [0xff, 0xbb, 0x00, 0xff] } else { [0xbc, 0x00, 0x00, 0xff] };
            out.push(self.quad(aspect, ex0, ey0, ex1 - ex0, ey1 - ey0, tint(fill)));
            if let Some(f) = self.fonts.get(PICKER_BUTTON_FONT) {
                let s = self.fe_strings.get(id).unwrap_or_default();
                self.text(out, aspect, f, &s, ex0, ey0 + 2.0, ex1 - ex0, 1, white);
            }
            // pressed takes the bevel (0x4C4414, 0x4C4603) and anything else the inset
            // highlight (0x4C446B, 0x4C465A) — the same two-arm tail every button here has
            if on {
                self.bevel(out, aspect, ex0, ey0, ex1 - ex0, ey1 - ey0, 3.0);
            } else {
                self.glass(out, aspect, ex0, ey0, ex1 - ex0, ey1 - ey0, &tint);
            }
            outline(out, ex0, ey0, ex1 - ex0, ey1 - ey0, black);
        }
    }

    /// The colour editor, as the port's own state (§4.66) — the three floats, the selected well
    /// and the eight wells.
    pub fn picker(&self) -> Picker {
        self.picker
    }

    /// The `NFS5.ColorBox` whose picker is up, if one is.
    ///
    /// The original's gate is two things at 0x4C3D0C: the widget holds **focus** and the class's
    /// global latch `[0x65B710]` is set. The latch is raised by `0x4C39F0`, called from one place
    /// — 0x4C6D6C, in the shared combo commit, and only for a widget whose constructor byte
    /// `[this+0x1DE]` is set, which is `NFS5.ColorBox` alone (§4.44): the widget asks its accessor
    /// for the index it calls custom (`0x4B94B0`, `accessor->slot 0x24`) and opens the editor when
    /// the row just chosen **is** that one. The port has no colour accessor for `OwnedCar.*Colour`
    /// to ask, so it opens the editor on the click that would open a combo's list.
    /// APPROXIMATION, and the only one here.
    /// **`data.allowcustom` decides whether there is a custom colour to open at all.** The bind
    /// 0x4C3BF0 hands it to the colour accessor at `[this+0x220]` through 0x4B9590, which is one
    /// instruction — `[accessor+0x30] = flag` — and the accessor's own count reads that byte
    /// twice, at 0x4B9232 and 0x4B9353, adding `0x5019D0() + 1` entries to the list when it is set
    /// and none when it is not. So a box with `allowcustom=0` offers the car's paint sets and
    /// nothing else. Two of the eleven shipped boxes carry 0 and they are `dealership.lay`'s
    /// `DealerCar.Colour` and `DealerCar.IntColour` — you cannot repaint a car you have not bought
    /// — while the paint shop's four and the five race screens' all carry 1. §4.86.
    pub fn open_picker(&self) -> Option<usize> {
        self.open_combo.filter(|i| {
            self.layout
                .elements
                .get(*i)
                .is_some_and(|e| e.class == "NFS5.ColorBox" && self.attr_num(e, "allowcustom").unwrap_or(0.0) != 0.0)
        })
    }

    /// The picker's box and its six rects, for the widget that has it up.
    fn picker_geometry(&self) -> Option<[(f32, f32, f32, f32); 6]> {
        let i = self.open_picker()?;
        let e = self.layout.elements.get(i)?;
        let (x, y) = self.position_of(e);
        // the same box the draw hangs the picker off — `NFS5.ColorBox` is one of the eight
        // templates with `autosize=0` (§4.62), so this is its `size` attribute
        let (w, h) = self.box_of(e, 0);
        Some(picker_rects(x, y, x + w, y + h))
    }

    /// Where the pointer is in the open picker, `[this+0x218]` — 5 when it is nowhere in it.
    pub fn picker_region(&self) -> u32 {
        match (self.picker_geometry(), self.cursor) {
            (Some(rects), Some((x, y))) => picker_region(&rects, x, y),
            _ => 5,
        }
    }

    /// The picker's **slot 5**, the button going down — 0x4C4710.
    ///
    /// It hit-tests, then, only while the widget is focused and the latch is up, moves whichever
    /// of the two float pairs the pointer is on: region **0** takes the hue from x across rect C
    /// and the saturation from `(C.y1 - y)` **up** it (0x4C474D…0x4C479D — the y term is inverted,
    /// the x term is not), region **1** takes the luminance from x across rect A
    /// (0x4C47A8…0x4C47CB). Whatever the region, it then packs the three with 0x4D8900 and writes
    /// the result into the selected well (0x4C47E6, 0x4C47F3) and tells the accessor
    /// (`slot 0x6C`).
    pub fn picker_press(&mut self, x: f32, y: f32) {
        let Some(rects) = self.picker_geometry() else { return };
        match picker_region(&rects, x, y) {
            0 => {
                let (x0, y0, x1, y1) = rects[rect::C];
                self.picker.hsl.0 = (x - x0) / (x1 - x0);
                self.picker.hsl.1 = (y1 - y) / (y1 - y0);
            }
            1 => {
                let (x0, _, x1, _) = rects[rect::A];
                self.picker.hsl.2 = (x - x0) / (x1 - x0);
            }
            _ => {}
        }
        self.commit_well();
    }

    /// The picker's **slot 8**, the pointer moving — 0x4C4830.
    ///
    /// The same two arms as the press, behind one more gate: **0x5299C0**, a mouse button is held
    /// (0x4C4863). The widget holds the capture while it is focused (§4.57), so the move reaches
    /// it wherever the pointer goes — but each arm still needs its own rect, so a drag that leaves
    /// rect C stops moving the hue and the saturation instead of running away with them.
    pub fn picker_move(&mut self, x: f32, y: f32) {
        if !self.pressed {
            return;
        }
        self.picker_press(x, y);
    }

    /// The picker's **slot 9**, the button coming up — 0x4C4960. Answers whether it closed.
    ///
    /// Region **3** is "ok": drop the latch, drop focus (0x4C7DA0) and hand the accessor back what
    /// it already has, `slot 0x28` into `slot 0x2C` (0x4C49AD…0x4C49BF). Region **4** is "cancel":
    /// drop both, then put all **eight** wells back from the copy the first drawn frame took
    /// (0x4C49E6…0x4C4A00) and hand the accessor the value it had then, `[this+0x29C]`
    /// (0x4C4A08). Both play sound **5** (0x4AADB0). Region **2** picks a well —
    /// `(x - D.x0)·8 / (D.x1 - D.x0)`, an integer divide clamped to `0…7` (0x4C4A2C…0x4C4A6E) —
    /// and, **only when that well is not zero** (0x4C4A7D), reads its colour back into the three
    /// floats with 0x4D8BD0.
    pub fn picker_release(&mut self, x: f32, y: f32) -> bool {
        let Some(rects) = self.picker_geometry() else { return false };
        match picker_region(&rects, x, y) {
            3 => return true,
            4 => {
                self.picker.wells = self.picker.saved;
                return true;
            }
            2 => {
                let (x0, _, x1, _) = rects[rect::D];
                let n = ((x - x0) as i32 * 8) / (x1 - x0) as i32;
                self.picker.well = n.clamp(0, 7) as usize;
                let c = self.picker.wells[self.picker.well];
                if c != 0 {
                    self.picker.hsl = rgb_to_hls(c);
                }
            }
            _ => {}
        }
        false
    }

    /// The tail every press and move runs: pack the three floats and write the selected well
    /// (0x4C47E3…0x4C47F8).
    fn commit_well(&mut self) {
        let (h, s, l) = self.picker.hsl;
        let w = self.picker.well.min(7);
        self.picker.wells[w] = hls_to_rgb(h, s, l);
    }

    /// Take the copy of the eight wells that **cancel** puts back. The original takes it in the
    /// first frame the picker draws and clears the guard whenever it is down (0x4C3CB3, 0x4C3CFA,
    /// 0x4C3D03), so it is one copy per opening — which is what this is.
    pub fn picker_opened(&mut self) {
        self.picker.saved = self.picker.wells;
    }

    /// `focused` is reached here by holding the button down over a widget whose class takes focus
    /// on the press (§4.40, `can_press`) — for `NFS5.CARSETTINGBUTTON` that is `picseldown` at
    /// `data.seldownindex`, the pressed art. `focusedhover` needs `[+0x110] & 0x80`, which no
    /// shipped template of those three sets, so the cascade falls to `focused`.
    fn state_for<'a>(&self, e: &'a nfs_formats::lay::Element, i: usize, selected: Option<usize>, hovered: Option<usize>) -> &'a str {
        // a widget that has the mouse keeps its pressed look wherever the pointer goes (§4.57)
        let hot = hovered == Some(i) || self.capture == Some(i);
        // 0x4C831E comes second, after `hidden` and before everything else (§4.110), and a
        // disabled widget shows `[state.disabled]` whatever the pointer is doing
        // a covered screen (§4.147) disables it the same way — but `hidden` is tested first, so a
        // widget the screen has hidden stays hidden under a dialog
        if (self.disabled_flags.contains(&i) || (self.covered && !self.is_hidden(i)))
            && self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled"))
        {
            return "disabled";
        }
        // `data.alwaysfocused` (read at 0x4CC537 into `[this+0x156]`): the input box's bind calls
        // `setFocused` outright when it is set (0x4CC56E…0x4CC570), so the box is focused with no
        // pointer involved at all. Two shipped screens use it, `accountlogin` and `createplayer`.
        if self.attr_num(e, "alwaysfocused").unwrap_or(0.0) != 0.0 || self.open_combo == Some(i) || self.focused.contains(&i) {
            // 0x4C835E comes before 0x4C839A: a focused widget that is also hovered takes
            // `focusedhover` when its template has one (§4.110)
            if hot && self.controls.get(&e.class).is_some_and(|c| c.has_state("focusedhover")) {
                return "focusedhover";
            }
            return "focused";
        }
        // **`NFS5.CAMERASELECT` highlights the camera that is on.** Its bind takes three
        // accessors, not one: `data.accessornum` into `[this+0x14C]` (0x4BF2C7), `Camera.Current`
        // into `[+0x140]` (0x4BF2DB) and `Camera.<accessornum + 1>` into `[+0x144]` — the four
        // arms at 0x4BF2F8, 0x4BF32C, 0x4BF35F and 0x4BF392. Its draw 0x4BF410 then asks the
        // first of those for its value and compares:
        //
        // ```text
        // 0x4BF428  v = [this+0x140]->slot10()
        // 0x4BF42B  v == [this+0x14C] -> setState("highlighty")   (0x4BF433, 0x4C7970)
        // 0x4BF441  else the ordinary state
        // ```
        //
        // and `CameraSelect.ctr`'s `[state.highlighty]` swaps `bg` for `hbg`. The four shipped
        // widgets are `cameras.lay`'s, carrying `accessornum` 0…3 and `text` 1…4. §4.89.
        if e.class == "NFS5.CAMERASELECT"
            && self.attr_num(e, "accessornum").unwrap_or(-1.0) == self.camera_current as f64
            && self.controls.get(&e.class).is_some_and(|c| c.has_state("highlighty"))
        {
            return "highlighty";
        }
        if hot && self.pressed && self.can_press(&e.class) {
            if self.controls.get(&e.class).is_some_and(|c| c.has_state("focusedhover")) {
                return "focusedhover";
            }
            return "focused";
        }
        // 0x4C83C1 and 0x4C83EE: the `selected` flag, `[this+0x10C] & 8`. The port's own
        // keyboard cursor stands in for it where nothing else sets it (§4.110).
        if selected == Some(i) || self.selected_flags.contains(&i) {
            if hot && self.controls.get(&e.class).is_some_and(|c| c.has_state("selectedhover")) {
                return "selectedhover";
            }
            return "selected";
        }
        if hot && self.can_hover(&e.class) {
            return "hover";
        }
        e.get("state").unwrap_or("normal")
    }

    /// Put the pointer somewhere, in design coordinates, or take it away.
    ///
    /// `RE/notes/controls.md` §4.39: the front end is mouse-driven, and hover is a **flag** —
    /// bit **0x04** of `[widget+0x10C]`, set by the base vtable's slot 6 (0x4C7B90) and cleared by
    /// slot 7 (0x4C7C50), each of which then re-runs `updateState` (0x4C82A0) to turn the flags
    /// into the state name the widget draws.
    pub fn set_cursor(&mut self, at: Option<(f32, f32)>) {
        // a still parked its own pointer with `NFS_FE_CURSOR`; the live one must not clobber it
        if std::env::var("NFS_FE_CURSOR").is_ok() {
            return;
        }
        self.place_cursor(at);
    }

    /// Hand every group the pointer in **its own** coordinates. `embedded` draws a panel's
    /// sub-layout offset by the panel's position, because 0x4CEBC0 hands the loaded screen the
    /// panel's place (§4.12) — so the hit test inside it has to move the same way.
    fn place_cursor(&mut self, at: Option<(f32, f32)>) {
        self.cursor = at;
        let offsets: Vec<(usize, (f32, f32))> = self
            .groups
            .keys()
            .copied()
            .filter_map(|k| self.layout.elements.get(k).map(|e| (k, self.position_of(e))))
            .collect();
        for (k, (gx, gy)) in offsets {
            let inner = at.map(|(x, y)| (x - gx, y - gy));
            if let Some(g) = self.groups.get_mut(&k) {
                g.place_cursor(inner);
            }
        }
    }

    /// Whether this class can hover at all: 0x4C7B90 sets the bit only when `[this+0x110] & 4`,
    /// and that mask is built from the template's own state list (0x4C7784, 0x4C7800) — so it is
    /// exactly "the `.ctr` has a `[state.hover]`". Checked against the classes whose draws gate
    /// something on hover: all seven have one, and `BevelOptionBox.ctr`, whose bevel is *not*
    /// gated, has none.
    fn can_hover(&self, class: &str) -> bool {
        self.controls.get(class).is_some_and(|c| c.has_state("hover"))
    }

    /// Hold the left button down, or let it up.
    ///
    /// `RE/notes/controls.md` §4.40: `EventPump::poll` 0x529700 stamps a `MouseButtonEvent` with
    /// type **4** when the button goes down and **8** when it comes up (0x529806), and
    /// `Control::handleEvent` 0x525F80 sends them to **slot 5** and **slot 9**.
    pub fn set_pressed(&mut self, down: bool) {
        if std::env::var("NFS_FE_PRESS").is_ok() {
            return;
        }
        self.pressed = down;
        // §4.57: a widget that takes focus takes the mouse with it (0x4C8486 → 0x528CB0), and
        // `Window::handleEvent`'s button arm then hands it every button event wherever the pointer
        // is (0x528AD5). The classes that take focus on the press are §4.40's three.
        self.capture = if down {
            self.hovered().filter(|i| self.layout.elements.get(*i).is_some_and(|e| self.can_press(&e.class)))
        } else {
            None
        };
        for g in self.groups.values_mut() {
            g.set_pressed(down);
        }
    }

    /// The classes that take **focus on the press** — the ones whose slot 5 calls
    /// `FeWidget::setFocused` 0x4C7D70, which is what makes the held-down art appear:
    /// `NFS5.CARSETTINGBUTTON` at 0x4C1F6C, `NFS5.DEADZONE` at 0x51BB83 and
    /// `NFS5.GouraudIndicatorBar` at 0x4CA556. The other three callers
    /// (`NFS5.CAMERAPICTURE` 0x4BF9BF, `NFS5.3DCAR` 0x4C1002, `NFS5.CONTROLLERSETUP` 0x51C5F0)
    /// take it in **slot 9**, on the release, and keep it — that is a selection, not a press, and
    /// the port does not raise it.
    const PRESS_FOCUS: [&'static str; 3] = ["NFS5.CARSETTINGBUTTON", "NFS5.DEADZONE", "NFS5.GouraudIndicatorBar"];

    /// Whether this widget shows a held-down state: its class must set focus on the press, and
    /// `setFocused` only sets the bit when `[this+0x110] & 0x40`, the mask bit the template's own
    /// `[state.focused]` puts there.
    fn can_press(&self, class: &str) -> bool {
        Self::PRESS_FOCUS.contains(&class) && self.controls.get(class).is_some_and(|c| c.has_state("focused"))
    }

    /// The widget the pointer is over — the **first** section whose box contains it and whose
    /// class can hover.
    ///
    /// `Window::hitTest` **0x528C00** walks the child vector forwards and returns the **first**
    /// child that contains the point, and that vector is in section order (§4.67) with the draw
    /// walking it backwards — so the first section is both the topmost and the first tested.
    ///
    /// The box is the widget's own, the same `0x4C8670` gives the original. A widget with no size
    /// of its own has a box of (0, 0) here (§4.37) and so can never be hit; those are the 27
    /// bottom-bar buttons and graphic tabs, whose classes carry no hover state anyway.
    pub fn hovered(&self) -> Option<usize> {
        let (cx, cy) = self.cursor?;
        // the bases the flattened index space gives each embedded screen, by the element the
        // panel sits at, so a sub-layout is tested exactly where the draw reaches it
        let bases: std::collections::HashMap<usize, usize> = {
            let mut keys: Vec<usize> = self.groups.keys().copied().collect();
            keys.sort_unstable();
            let mut base = self.layout.elements.len();
            let mut m = std::collections::HashMap::new();
            for k in keys {
                m.insert(k, base);
                base += self.groups[&k].span();
            }
            m
        };
        for (i, e) in self.layout.elements.iter().enumerate().skip(1) {
            // a panel draws a whole screen of its own here, at the panel's own depth
            if let Some(base) = bases.get(&i) {
                // a hidden panel takes no pointer either
                if !self.closed.contains(&i)
                    && let Some(j) = self.groups[&i].hovered()
                {
                    return Some(base + j);
                }
                continue;
            }
            if !self.can_hover(&e.class) || self.state_draws_nothing(e) {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_hit(e);
            // `Control::containsPoint` 0x525A90 takes the widget's box (slot 13 → 0x4C8670) and
            // fails only on `x < left`, `x > right`, `y < top` or `y > bottom` — so both edges are
            // **inclusive** (§4.56), where this port used to be half-open.
            if w >= 1.0 && h >= 1.0 && cx >= x && cx <= x + w && cy >= y && cy <= y + h && self.tab_slant(e, cx, cy) {
                return Some(i);
            }
        }
        None
    }

    /// `NFS5.GRAPHICTABBUTTON`'s own hit test, **0x4CABA0** — the one class whose clickable area is
    /// not its box. Its bind takes four ints (0x4CAB40): `data.TLoffsetx` into `[this+0x15C]`,
    /// `TRoffsetx` into `[+0x160]`, `BLoffsetx` into `[+0x164]` and `BRoffsetx` into `[+0x168]`,
    /// and the test slants each vertical edge between the top value and the bottom one:
    ///
    /// ```text
    /// 0x4CABCD  y outside the box -> false
    /// 0x4CABDE  dx = TL ? (h - dy)·TL / h : BL·dy / h        ; the left edge
    /// 0x4CAC09  x <  box.l + dx -> false
    /// 0x4CAC19  dx = TR ? (h - dy)·TR / h : BR·dy / h        ; the right edge
    /// 0x4CAC3C  x >  box.r + dx -> false
    /// ```
    ///
    /// so each side is a straight line from `top ± T` to `bottom ± B`, and a zero at the top means
    /// "take the bottom one" rather than "no slant". All five shipped buttons are on
    /// `optiontabs.lay` and they carry the pairs (0, −60, 94, 0), (0, −20, 60, 0), (0, 0, 20, −20),
    /// (20, 0, 0, −60) and (60, 0, 0, −94) — a strip of parallelograms whose rectangles overlap,
    /// which is why the box alone picks the wrong tab. §4.82.
    fn tab_slant(&self, e: &nfs_formats::lay::Element, cx: f32, cy: f32) -> bool {
        if e.class != "NFS5.GRAPHICTABBUTTON" {
            return true;
        }
        let (x, y) = self.position_of(e);
        let (w, h) = self.box_hit(e);
        let (h, dy) = (h as i64, (cy - y as f32) as i64);
        if h <= 0 {
            return true;
        }
        let at = |top: &str, bottom: &str| {
            let t = self.attr_num(e, top).unwrap_or(0.0) as i64;
            if t != 0 { (h - dy) * t / h } else { self.attr_num(e, bottom).unwrap_or(0.0) as i64 * dy / h }
        };
        let cx = cx as i64;
        cx >= x as i64 + at("TLoffsetx", "BLoffsetx") && cx <= (x + w) as i64 + at("TRoffsetx", "BRoffsetx")
    }

    /// The glint that sweeps the screen, shared by `NFS5.HEADER` (0x4CB024…0x4CB0F6) and
    /// `NFS5.CarLogo` (0x4C1CE0…0x4C1DAF) — `RE/notes/controls.md` §4.25. Both keep the same three
    /// fields (running, started, wait), both take the travel from 0x4E3550 — which is `[0x6907E0]`,
    /// set to **640.0f** by 0x4D35B4 — and both wait `rand() % 128 + 128` ticks between sweeps.
    /// The sprite's **x is the elapsed tick count itself**, at 128 Hz, so it crosses the whole
    /// screen at a pixel a tick, and its y is the widget's own.
    ///
    /// APPROXIMATION: the port takes the middle of the random wait, and every widget on a screen
    /// sweeps together — which is what the original does on its first cycle anyway, since they all
    /// start their timers at the same activate.
    fn sweep_glint(&self, out: &mut Vec<Sprite>, aspect: f32, y: f32, alpha: f32) {
        const TRAVEL: u32 = 640;
        const WAIT: u32 = 128 + 64;
        let Some(n) = self.glint else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(&("weeicons.fsh".to_string(), n)) else { return };
        let phase = (self.clock_ms * 128 / 1000) % (TRAVEL + WAIT);
        if phase >= TRAVEL {
            return;
        }
        let (pos, size) = self.place(aspect, phase as f32, y, aw, ah);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None });
    }

}

/// Break `s` into lines that fit `w`, the way `FeWordWrap` does it — **0x523600**.
///
/// The original does not fit words forward; it **guesses** a break and walks back to a space:
///
/// ```text
/// 0x523679  strip the trailing spaces
/// 0x5236C1  first line:  guess = start + trunc(width * 0.125) + 10     ; [0x5BAF60] is -0.125
/// 0x5236B7  after that:  guess = start + (the previous line's length) + 10
/// 0x5236E7  if the guess is past the end, the line is the whole remainder
/// 0x5236F5  else walk back to the last space, then back over the run of spaces
/// 0x52374F  measure the line, and while its width is **not strictly less** than the box
/// 0x523765  walk back another word and measure again
/// 0x52371D  a line with no space left in it is taken as it stands
/// ```
///
/// Two of those matter and the port used neither: a line can never reach **past** the guess, so a
/// narrow font breaks earlier than a greedy fit would; and the test is `<`, not `<=`, so a line
/// that comes out exactly the width of the box is one word too long.
pub fn wrap_lines(f: &BitmapFont, s: &str, w: f32) -> Vec<String> {
    wrap_lines_by(&|t: &str| f.place(t).1, s, w)
}

/// The same, against any metric — which is how it is tested without a renderer to upload a font.
pub fn wrap_lines_by(measure: &dyn Fn(&str) -> f32, s: &str, w: f32) -> Vec<String> {
    let text: Vec<char> = s.trim_end_matches(' ').chars().collect();
    let width = |c: &[char]| measure(&c.iter().collect::<String>());
    let mut out = Vec::new();
    let mut start = 0usize;
    // the first guess is the box's width over 8 — one character per 8 pixels — and 10 more
    let mut reach = (w * 0.125).trunc().max(0.0) as usize + 10;
    while start < text.len() {
        let guess = start + reach;
        let mut cut = if guess >= text.len() {
            text.len()
        } else {
            // back to the last space, then over the run of spaces before it
            let mut i = guess;
            while i > start && text[i] != ' ' {
                i -= 1;
            }
            while i > start && text[i] == ' ' {
                i -= 1;
            }
            if i == start { text.len() } else { i + 1 }
        };
        // and back another word while the line is not strictly narrower than the box
        while cut > start && width(&text[start..cut]) >= w {
            let mut i = cut - 1;
            while i > start && text[i] != ' ' {
                i -= 1;
            }
            while i > start && text[i] == ' ' {
                i -= 1;
            }
            if i == start {
                break;
            }
            cut = i + 1;
        }
        let line: String = text[start..cut].iter().collect();
        let line = line.trim_end_matches(' ').to_string();
        reach = cut - start + 10;
        out.push(line);
        start = cut;
        while start < text.len() && text[start] == ' ' {
            start += 1;
        }
    }
    out
}

/// One piece of `NFS5.GlintyRectangle`'s flourish: a rectangle relative to the widget's origin
/// and the two colours it ramps between, the first at its left or top edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlintPiece {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub c0: [u8; 4],
    pub c1: [u8; 4],
    /// the ramp runs along x; otherwise along y
    pub along_x: bool,
}

/// The flourish of **0x51FEE8…0x52061C** — the bright segment that runs round a glinty
/// rectangle's outline, as at most four pieces. `RE/notes/controls.md` §4.27.
///
/// Two segments travel the border in mirror image, each drawn as one piece on a vertical edge and
/// one on a horizontal edge as it turns the corner, all of them **one pixel** thick, through the
/// gouraud rect 0x4D96B0. `p` is the leading edge and `band` the segment's length; `variant`,
/// 1…4, is `animate` — which corner the pair starts from.
///
/// Every rule here is one of the sixteen arms behind the four jump tables at 0x520628, 0x520638,
/// 0x520648 and 0x520658, with the three-way split before each table at 0x51FF5E / 0x520036 /
/// 0x520063 and 0x5202AC / 0x520399 / 0x5203D4, and the gates at 0x51FFF2, 0x5201AB, 0x520338 and
/// 0x520525.
pub fn glint_pieces(w: i32, h: i32, p: i32, band: i32, variant: i32) -> Vec<GlintPiece> {
    let mut out = Vec::new();
    if band <= 0 || w < 2 || h < 2 {
        return out;
    }
    // 0x4D8850: `(c0·(255−t) + c1·t) / 255`, channel by channel
    let lerp = |c0: [u8; 4], c1: [u8; 4], t: i32| -> [u8; 4] {
        let t = t.clamp(0, 255) as u32;
        std::array::from_fn(|k| ((c0[k] as u32 * (255 - t) + c1[k] as u32 * t) / 255) as u8)
    };
    // 0xDDFFFFFF and 0x00FFFFFF as this port spells colours. `count` is the loop index handed to
    // 0x4D8850 as a colour at 0x51FFA9 — a near-transparent black, which is why a segment's far
    // end disappears; it is kept rather than tidied because it is what the original mixes.
    let glint: [u8; 4] = [0xff, 0xff, 0xff, 0xdd];
    let clear: [u8; 4] = [0xff, 0xff, 0xff, 0x00];
    let count = |i: i32| -> [u8; 4] { [0, 0, i.clamp(0, 255) as u8, 0] };
    let (wf, hf) = (w as f32, h as f32);
    let mut push = |x: f32, y: f32, pw: f32, ph: f32, c0, c1, along_x| {
        // the original passes a signed extent, so a negative one runs back from `x`/`y`
        let (x, pw) = if pw < 0.0 { (x + pw, -pw) } else { (x, pw) };
        let (y, ph) = if ph < 0.0 { (y + ph, -ph) } else { (y, ph) };
        if pw >= 0.5 && ph >= 0.5 {
            out.push(GlintPiece { x, y, w: pw, h: ph, c0, c1, along_x });
        }
    };
    // ---- the segment that rides a vertical edge first, then a horizontal one (0x51FF5E…)
    let (i0, i1, a, ca0, ca1, cb0, cb1);
    if p > h || band + p < h {
        if p > h {
            i0 = 0;
            i1 = (w + h - p).min(band);
            a = p;
        } else if p < 0 {
            i0 = band + p;
            i1 = 0;
            a = 0;
        } else {
            i0 = band;
            i1 = 0;
            a = p;
        }
        ca0 = lerp(count(i0), glint, 255 - i0 * 255 / band);
        ca1 = glint;
        cb0 = clear;
        cb1 = lerp(clear, glint, i1 * 255 / band);
    } else {
        // 0x520063: the segment straddles the corner, and both pieces take the one blend
        i0 = h - p;
        i1 = band - i0;
        a = p;
        let t = lerp(clear, glint, i0 * 255 / band);
        ca0 = clear;
        ca1 = t;
        cb0 = t;
        cb1 = glint;
    }
    if a < h {
        // table 0x520628: left edge for 1 and 3, right for 2 and 4; down for 1 and 2, up for 3, 4
        let x = if variant == 1 || variant == 3 { 0.0 } else { wf - 1.0 };
        let (y, hh) = if variant <= 2 { (a as f32, i0 as f32) } else { (hf - a as f32, -i0 as f32) };
        push(x, y, 1.0, hh, ca0, ca1, false);
    }
    if band + a >= h {
        // table 0x520638: the bottom edge for 1 and 2, the top for 3 and 4
        let b = (a - h).max(0) as f32;
        let (x, ww) = if variant == 1 || variant == 3 { (b, i1 as f32) } else { (wf - b, -(i1 as f32)) };
        let y = if variant <= 2 { hf - 1.0 } else { 0.0 };
        push(x, y, ww, 1.0, cb0, cb1, true);
    }
    // ---- and its mirror, riding a horizontal edge first (0x5202AC…)
    let (j0, j1, c, cc0, cc1, cd0, cd1);
    if p > w || band + p < w {
        if p > w {
            j0 = 0;
            j1 = (w + h - p).min(band);
            c = p;
        } else if p < 0 {
            j0 = band + p;
            j1 = 0;
            c = 0;
        } else {
            j0 = band;
            j1 = 0;
            c = p;
        }
        cc0 = lerp(count(j0), glint, 255 - j0 * 255 / band);
        cc1 = glint;
        cd0 = clear;
        cd1 = lerp(clear, glint, j1 * 255 / band);
    } else {
        // 0x5203D4
        j0 = w - p;
        j1 = band - j0;
        c = p;
        let t = lerp(clear, glint, j0 * 255 / band);
        cc0 = clear;
        cc1 = t;
        cd0 = t;
        cd1 = glint;
    }
    if c < w {
        // table 0x520648: the top edge for 1 and 2, the bottom for 3 and 4
        let (x, ww) = if variant == 1 || variant == 3 { (c as f32, j0 as f32) } else { (wf - c as f32, -(j0 as f32)) };
        let y = if variant <= 2 { 0.0 } else { hf - 1.0 };
        push(x, y, ww, 1.0, cc0, cc1, true);
    }
    if band + c >= w {
        // table 0x520658: the right edge for 1 and 3, the left for 2 and 4
        let d = (c - w).max(0) as f32;
        let x = if variant == 1 || variant == 3 { wf - 1.0 } else { 0.0 };
        let (y, hh) = if variant <= 2 { (d, j1 as f32) } else { (hf - d, -(j1 as f32)) };
        push(x, y, 1.0, hh, cd0, cd1, false);
    }
    out
}

/// `NFS5.GLINTYLINE`'s sweep — the bright segment that slides along the line, on top of the bar
/// the class draws for its body (§4.28). `RE/notes/controls.md` §4.29.
///
/// The object's setup (0x51F9A0) puts the sweep's range in `[+0x40]`/`[+0x44]`: **the long axis**,
/// left…right when `data.horizontal` is set and top…bottom when it is not. Its width is a fifth of
/// that range (0x51FB38, the same 0x66666667 as the rectangle's band), it starts one width outside
/// one end and runs to one width past the other at `rand() % 3 + 3` pixels a tick (0x51FB4D,
/// 0x51FBC5), and between runs it waits `rand() % 128` (0x51FC10).
///
/// The rectangle it draws is one pixel thick across the line and `width` long along it (0x51FCCA
/// for the horizontal case, 0x51FD71 for the vertical), in a ramp between **0xDDFFFFFF** through
/// the fade and **0x00FFFFFF** (0x51FBA1), and it goes through **0x4D94A0** rather than 0x4D96B0 —
/// the same gouraud rect but **clipped to a box**, the widget's own, with the colours interpolated
/// at the cut (0x4D94C8…0x4D9577). Returns the clipped rectangle and the two colours, or `None`
/// when it is entirely outside.
///
/// `along` is the position of the leading edge on the long axis, relative to the widget's origin.
pub fn glint_sweep(len: f32, along: f32, width: f32, forward: bool) -> Option<(f32, f32, [u8; 4], [u8; 4])> {
    let glint: [u8; 4] = [0xff, 0xff, 0xff, 0xdd];
    let clear: [u8; 4] = [0xff, 0xff, 0xff, 0x00];
    let (c0, c1) = if forward { (clear, glint) } else { (glint, clear) };
    let lerp = |a: [u8; 4], b: [u8; 4], t: f32| -> [u8; 4] {
        let t = t.clamp(0.0, 1.0);
        std::array::from_fn(|k| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t).round() as u8)
    };
    let (mut s, mut e) = (along, along + width);
    if e <= 0.0 || s >= len || width <= 0.0 {
        return None;
    }
    let (mut ca, mut cb) = (c0, c1);
    if s < 0.0 {
        ca = lerp(c0, c1, -s / width);
        s = 0.0;
    }
    if e > len {
        cb = lerp(c0, c1, (len - along) / width);
        e = len;
    }
    (e - s >= 0.5).then_some((s, e - s, ca, cb))
}

/// `band = min((w+h)/5, w−1, h−1)` (0x51FEE8…0x51FF2E) and the leading edge
/// `p = (band + w + h)·elapsed/duration − band` (0x51FF32…0x51FF4D).
pub fn glint_band(w: i32, h: i32) -> i32 {
    ((w + h) / 5).min(w - 1).min(h - 1)
}

impl LayoutScreen {
    /// Draw the flourish of `glint_pieces` at the widget's origin.
    ///
    /// APPROXIMATION, the same one the glints of §4.25 carry: the original draws
    /// `rand() % 32 + 64` ticks for each run and `rand() % 256 + 256` for each wait (0x51FE6C,
    /// 0x51FEC2), so the port takes the middle of both and every rectangle on a screen runs
    /// together.
    fn glinty_flourish(&self, out: &mut Vec<Sprite>, aspect: f32, px: f32, py: f32, w: i32, h: i32, variant: i32) {
        const RUN: u32 = 64 + 16;
        const WAIT: u32 = 256 + 128;
        if w < 2 || h < 2 {
            return;
        }
        let band = glint_band(w, h);
        if band <= 0 {
            return;
        }
        let phase = (self.clock_ms * 128 / 1000) % (RUN + WAIT);
        if phase >= RUN {
            return;
        }
        let p = (band + w + h) * phase as i32 / RUN as i32 - band;
        for g in glint_pieces(w, h, p, band, variant) {
            let (a, b) = (tint(g.c0), tint(g.c1));
            let (x, y) = (px + g.x, py + g.y);
            let pts = [[x, y], [x + g.w, y], [x, y + g.h], [x + g.w, y + g.h]];
            let cols = if g.along_x { [a, b, a, b] } else { [a, a, b, b] };
            self.gouraud(out, aspect, pts, cols);
        }
    }
}

impl LayoutScreen {
    /// The front end's drop shadow, **0x520F00** — the widget's box offset by `(a, b)` and painted
    /// black, as the L of what the offset uncovers. `NFS5.GOTOBUTTON`'s draw calls it with
    /// **(4, 4)** (0x4C91FE, 0x4C9202) for every button whose `dropshadow` is **1**, which is the
    /// template's default and what 132 of the 153 shipped ones carry.
    ///
    /// Three colours, each through the fade (0x4D87F0): **0xCC000000** at the box's own edge,
    /// **0x19000000** at the far edge of the offset, and **0x0C000000** in the two corner wedges
    /// (0x520F19…0x520F40; the fourth mix, a fully transparent 0, is dropped unused). The four
    /// quads, in the order the function draws them:
    ///
    /// | | upper-left | upper-right | lower-left | lower-right |
    /// |---|---|---|---|---|
    /// | right strip (0x520FAF) | `(x+w, y+b)` CC | `(x+w+a, y+b)` 19 | `(x+w, y+h)` CC | `(x+w+a, y+h+b)` 19 |
    /// | bottom strip (0x520FEB) | `(x+2a, y+h)` CC | `(x+w, y+h)` CC | `(x+a, y+h+b)` 19 | `(x+w+a, y+h+b)` 19 |
    /// | lower-left wedge (0x521008) | `(x+a, y+h)` 0C | `(x+2a, y+h)` CC | `(x+a, y+h+b)` 0C | `(x+a, y+h+b)` 0C |
    /// | upper-right wedge (0x521036) | `(x+w, y+b)` 0C | `(x+w+a, y+b)` 0C | `(x+w, y+2b)` CC | `(x+w, y+2b)` CC |
    ///
    /// Behind the same global gate 0x4E35E0 as the bevel (0x520F03).
    fn drop_shadow(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, a: f32, b: f32) {
        let (near, far, edge) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x19]), tint([0, 0, 0, 0x0c]));
        let (r, bot) = (x + w, y + h);
        self.gouraud(out, aspect, [[r, y + b], [r + a, y + b], [r, bot], [r + a, bot + b]], [near, far, near, far]);
        self.gouraud(out, aspect, [[x + 2.0 * a, bot], [r, bot], [x + a, bot + b], [r + a, bot + b]], [near, near, far, far]);
        self.gouraud(out, aspect, [[x + a, bot], [x + 2.0 * a, bot], [x + a, bot + b], [x + a, bot + b]], [edge, near, edge, edge]);
        self.gouraud(out, aspect, [[r, y + b], [r + a, y + b], [r, y + 2.0 * b], [r, y + 2.0 * b]], [edge, edge, near, near]);
    }

    /// The front end's beveled frame, **0x521050** — a shared painter with twenty call sites, of
    /// which `NFS5.BEVELEDOPTIONBOX`'s (0x520EEC, thickness **3**) is the one this port draws so
    /// far. It takes `(x, y, w, h, fade, thickness)` as four floats and two ints, and paints four
    /// trapezoidal bands with the gouraud quad primitive 0x4D98D0 (§4.7's four `(x, y, colour)`
    /// triples, in the order upper-left, upper-right, lower-left, lower-right):
    ///
    /// | band | outer edge, opaque | inner edge, nearly clear |
    /// |---|---|---|
    /// | top | `(x+1, y+1)`…`(x+w−1, y+1)` **0xCC000000** | `(x+t+1, y+t+1)`…`(x+w−t−1, y+t+1)` **0x19000000** |
    /// | left | `(x+1, y+1)`…`(x+1, y+h−1)` 0xCC000000 | `(x+t, y+t+1)`…`(x+t, y+h−t−1)` 0x19000000 |
    /// | right | `(x+w−1, y+1)`…`(x+w−1, y+h−1)` **0xCCFFFFFF** | `(x+w−t−1, y+t+1)`…`(x+w−t−1, y+h−t−1)` **0x33FFFFFF** |
    /// | bottom | `(x−2, y+h−1)`…`(x+w−2, y+h−1)` 0xCCFFFFFF | `(x+t+1, y+h−t−1)`…`(x+w−t+1, y+h−t−1)` 0x33FFFFFF |
    ///
    /// Dark above and to the left, light below and to the right: a **sunken** bevel. The two the
    /// bottom band is shifted by — the −2 on its outer edge (the 2.0f at 0x5B2580, 0x521206 and
    /// 0x52121D) and the +1 where the right band has −1 — are the original's own asymmetry, kept.
    ///
    /// Three of the seven colours 0x521050 mixes are computed and dropped without being used
    /// (0xCC808080, 0x19FFFFFF and 0xB2FFFFFF — the results of the second, third and fifth
    /// 0x4D87F0 calls are overwritten before anything reads them), so only these four reach the
    /// screen. The whole thing is behind the global gate 0x4E35E0 that §4.9 records, the one the
    /// glinty classes share; this port draws those, so it draws this.
    ///
    /// APPROXIMATION in one respect only, the same one §4.7 carries: each band is a gouraud
    /// trapezoid and the sprite pipeline has one colour per quad, so a band is stepped across its
    /// depth — `t` strips, each between two interpolated edges, in the colour at its middle.
    /// **0x521810** — the inset highlight the text boxes and the dead-zone marker carry. Two
    /// gouraud halves through `0x4D96B0(x, y, w, h, c00, c10, c01, c11)`, inset one pixel and two
    /// smaller than the box:
    ///
    /// ```text
    /// 0x5218CE  top    (x+1, y+1,             w-2, (h-2)/2)  0x80FFFFFF over 0x00FFFFFF
    /// 0x5218F3  bottom (x+1, y+1 + (h-2)/2,   w-2, (h-2)/2)  0x00000000 over 0x91000000
    /// ```
    ///
    /// with 2.0 at [0x5B2580], 0.5 at [0x5B23F0] and 1.0 at [0x5B24A8], so the top half fades a
    /// half-white down to clear and the bottom half clear down to a near-opaque black — a lit
    /// upper edge and a shaded lower one.
    ///
    /// The gate 0x4E35E0 (`[0x6197AA] == 0`) has a second arm: a plain one-pixel black outline
    /// through 0x4D9200 at the widget's own fade (0x4D87E0, which is just `fade << 24`). This
    /// port takes the other arm, the same choice §4.21 makes for the bevel.
    ///
    /// APPROXIMATION, the one every gradient here carries: the sprite pipeline has one colour per
    /// quad, so each half is stepped in one-pixel strips.
    fn glass(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, tint: &dyn Fn([u8; 4]) -> [f32; 4]) {
        let half = (h - 2.0) * 0.5;
        if half < 1.0 || w < 3.0 {
            return;
        }
        for (top, c0, c1) in [
            (y + 1.0, tint([0xFF, 0xFF, 0xFF, 0x80]), tint([0xFF, 0xFF, 0xFF, 0])),
            (y + 1.0 + half, tint([0, 0, 0, 0]), tint([0, 0, 0, 0x91])),
        ]
        {
            let n = half.round().max(1.0) as usize;
            for d in 0..n {
                let u = (d as f32 + 0.5) / n as f32;
                let col = std::array::from_fn(|j| c0[j] + (c1[j] - c0[j]) * u);
                out.push(self.quad(aspect, x + 1.0, top + d as f32 * half / n as f32, w - 2.0, (half / n as f32).max(0.5), col));
            }
        }
    }

    /// **0x521970**, the *raised* bevel — §4.21's sunken one turned over. Six colours go through
    /// the fade at 0x521989…0x5219CB and the first two, 0xFFE00C0C and 0xFF820000 (which are
    /// 0x521900's own pair), are **dead**: each is left in `eax` and clobbered by the next call
    /// before anything reads it, exactly as three of 0x521050's are. The four that live are
    /// 0x521050's palette with **0x33000000** where it has 0x19000000, and they go into the same
    /// four twelve-argument 0x4D98D0 quads (§4.74):
    ///
    /// ```text
    /// band            outer edge                              inner edge
    /// left  0x521A6C  (x+1, y+1)…(x+1, b)        0xCCFFFFFF   (x+t+1, y+t+1)…(x+t+1, b-t)     0x33FFFFFF
    /// top   0x521AC2  (x+1, y+1)…(x+w, y+1)      0xCCFFFFFF   (x+t+1, y+t+1)…(x+w-t, y+t+1)   0xCCFFFFFF
    ///                            ramping to      0x33FFFFFF               ramping to          0x33FFFFFF
    /// right 0x521B00  (x+w-1, y+1)…(x+w-1, b)    0xCC000000   (x+w-1-t, y+t+1)…(x+w-1-t, b-t) 0x33000000
    /// bottom 0x521B4B (x+1, b)…(x+w, b)          0xCC000000   (x+t+1, b-t)…(x+w-t, b-t)       0x33000000
    /// ```
    ///
    /// with `b = y + h - 1`. Light on the **left and top**, dark on the right and bottom — the
    /// mirror of 0x521050 — and the top band alone also fades **along its length**, bright at the
    /// left corner and dim at the right, which is the one thing neither bevel's other bands do.
    ///
    /// APPROXIMATION, the same one `bevel` carries: one colour per quad, so each band is stepped
    /// across its depth, and the top band along its length as well.
    fn bevel_up(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let (lit_out, lit_in) = (tint([0xff, 0xff, 0xff, 0xcc]), tint([0xff, 0xff, 0xff, 0x33]));
        let (dark_out, dark_in) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x33]));
        let (r, b) = (x + w, y + h - 1.0);
        // each band: the outer segment and its two end colours, then the inner segment and its two
        let bands = [
            ([x + 1.0, y + 1.0], [x + 1.0, b], [lit_out; 2], [x + t + 1.0, y + t + 1.0], [x + t + 1.0, b - t], [lit_in; 2]),
            ([x + 1.0, y + 1.0], [r, y + 1.0], [lit_out, lit_in], [x + t + 1.0, y + t + 1.0], [r - t, y + t + 1.0], [lit_out, lit_in]),
            ([r - 1.0, y + 1.0], [r - 1.0, b], [dark_out; 2], [r - 1.0 - t, y + t + 1.0], [r - 1.0 - t, b - t], [dark_in; 2]),
            ([x + 1.0, b], [r, b], [dark_out; 2], [x + t + 1.0, b - t], [r - t, b - t], [dark_in; 2]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    /// The stepping the four-band bevels share: each band is an **outer** segment with a colour at
    /// either end and an **inner** one the same, and the quad ramps between them across its depth
    /// as well as along its length. APPROXIMATION — the sprite pipeline has one colour per quad,
    /// so a band is cut into `t` strips across and, where its two ends differ, twelve along.
    #[allow(clippy::type_complexity)]
    fn bevel_bands(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        bands: &[([f32; 2], [f32; 2], [[f32; 4]; 2], [f32; 2], [f32; 2], [[f32; 4]; 2])],
        t: f32,
    ) {
        let n = (t.round() as usize).max(1);
        for &(a0, a1, ca, c0, c1, cb) in bands {
            let m = if ca[0] == ca[1] && cb[0] == cb[1] { 1 } else { 12 };
            for d in 0..n {
                let (s0, s1) = (d as f32 / n as f32, (d + 1) as f32 / n as f32);
                let at = |s: f32, p: [f32; 2], q: [f32; 2]| [p[0] + (q[0] - p[0]) * s, p[1] + (q[1] - p[1]) * s];
                for k in 0..m {
                    let (u0, u1) = (k as f32 / m as f32, (k + 1) as f32 / m as f32);
                    let e0 = (at(u0, a0, a1), at(u0, c0, c1));
                    let e1 = (at(u1, a0, a1), at(u1, c0, c1));
                    let corners =
                        [at(s0, e0.0, e0.1), at(s0, e1.0, e1.1), at(s1, e0.0, e0.1), at(s1, e1.0, e1.1)];
                    let (px, py) = (corners.iter().fold(f32::MAX, |m, c| m.min(c[0])), corners.iter().fold(f32::MAX, |m, c| m.min(c[1])));
                    let cw = corners.iter().fold(f32::MIN, |m, c| m.max(c[0])) - px;
                    let ch = corners.iter().fold(f32::MIN, |m, c| m.max(c[1])) - py;
                    let (s, u) = ((s0 + s1) * 0.5, (u0 + u1) * 0.5);
                    let outer: [f32; 4] = std::array::from_fn(|j| ca[0][j] + (ca[1][j] - ca[0][j]) * u);
                    let inner: [f32; 4] = std::array::from_fn(|j| cb[0][j] + (cb[1][j] - cb[0][j]) * u);
                    let col = std::array::from_fn(|j| outer[j] + (inner[j] - outer[j]) * s);
                    out.push(self.quad(aspect, px, py, cw.max(0.5), ch.max(0.5), col));
                }
            }
        }
    }

    /// **0x5215C0**, the outline over the combo scrollbar's knob (0x4DB622, thickness **7**) —
    /// the last painter in the front end, and 0x521250's shape with a richer palette. Seventeen
    /// colours go through the fade and **seven die**: the 0x80000000 at 0x5215FC, the 0xB2FFFFFF,
    /// 0xCCFFFFFF and 0xCC000000 at 0x521620…0x521636, each left in `eax` and clobbered by the
    /// next call, and the two the body uses belong to §4.90's 0x4D96B0. The four that make the
    /// quads are:
    ///
    /// ```text
    /// band             outer edge                          inner edge
    /// top    0x521748  (x, y)…(x+w, y)                     (x+t, y+t)…(x+w-t, y+t)
    ///                  0xE5FFFFFF -> 0x80808080            0x00FFFFFF
    /// left   0x5217C8  (x, y)…(x, y+h)                     (x+t, y+t)…(x+t, y+h-t)
    ///                  0xE5FFFFFF -> 0x80FFFFFF            0x00FFFFFF
    /// right  0x5217FB  (x+w, y)…(x+w, y+h)                 (x+w-t, y+t)…(x+w-t, y+h-t)
    ///                  0x66000000 -> 0x50000000            0x00000000
    /// bottom 0x5217A0  (x, y+h)…(x+w, y+h)                 (x+t, y+h-t)…(x+w-t, y+h-t)
    ///                  0x800C0C0C -> 0xE5000000            0x00808080 -> 0x0C000000
    /// ```
    ///
    /// Light on the top and left, dark on the right and bottom — raised, like §4.77's two — on
    /// the box's own edge with no `-1` anywhere, and **every** band fading along its length.
    /// §4.98.
    fn bevel_knob(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let c = |v: [u8; 4]| tint(v);
        let (r, b) = (x + w, y + h);
        let clear_w = c([0xff, 0xff, 0xff, 0]);
        let bands = [
            ([x, y], [r, y], [c([0xff, 0xff, 0xff, 0xe5]), c([0x80, 0x80, 0x80, 0x80])], [x + t, y + t], [r - t, y + t], [clear_w; 2]),
            ([x, y], [x, b], [c([0xff, 0xff, 0xff, 0xe5]), c([0xff, 0xff, 0xff, 0x80])], [x + t, y + t], [x + t, b - t], [clear_w; 2]),
            ([r, y], [r, b], [c([0, 0, 0, 0x66]), c([0, 0, 0, 0x50])], [r - t, y + t], [r - t, b - t], [c([0, 0, 0, 0]); 2]),
            ([x, b], [r, b], [c([0x0c, 0x0c, 0x0c, 0x80]), c([0, 0, 0, 0xe5])], [x + t, b - t], [r - t, b - t], [c([0x80, 0x80, 0x80, 0]), c([0, 0, 0, 0x0c])]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    /// **0x521250**, the outline every `NFS5.DEADZONE` handle that is *not* being dragged carries
    /// (0x51B916, 0x51B9B5, 0x51BA4E, and the fourth's `jne` arm) — §4.17's last gap.
    ///
    /// All **eight** colours its 0x4D87F0 calls mix reach the screen here, unlike 0x521050's and
    /// 0x521970's, and each of the four twelve-argument 0x4D98D0 quads (§4.74) fades along its
    /// own length:
    ///
    /// ```text
    /// band            outer edge                          inner edge
    /// top    0x52133B  (x, y)…(x+w, y)                     (x+t, y+t)…(x+w-t, y+t)
    ///                  0xE5FFFFFF -> 0x80FFFFFF            0x00FFFFFF -> 0x00000000
    /// left   0x521382  (x, y)…(x, y+h)                     (x+t, y+t)…(x+t, y+h-t)
    ///                  0xCCFFFFFF -> 0xB2FFFFFF            0x00FFFFFF
    /// right  0x5213B4  (x+w, y)…(x+w, y+h)                 (x+w-t, y+t)…(x+w-t, y+h-t)
    ///                  0x80000000 -> 0xCC000000            0x00000000
    /// bottom 0x5213E4  (x, y+h)…(x+w, y+h)                 (x+t, y+h-t)…(x+w-t, y+h-t)
    ///                  0x80000000 -> 0xCC000000            0x00FFFFFF -> 0x00000000
    /// ```
    ///
    /// White on the top and left, black on the right and bottom — raised, like 0x521970 — but on
    /// the widget's own edge rather than a pixel inside it, and with no `-1` anywhere.
    fn bevel_glow(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let c = |v: [u8; 4]| tint(v);
        let (r, b) = (x + w, y + h);
        let bands = [
            ([x, y], [r, y], [c([0xff, 0xff, 0xff, 0xe5]), c([0xff, 0xff, 0xff, 0x80])], [x + t, y + t], [r - t, y + t], [c([0xff, 0xff, 0xff, 0]), c([0, 0, 0, 0])]),
            ([x, y], [x, b], [c([0xff, 0xff, 0xff, 0xcc]), c([0xff, 0xff, 0xff, 0xb2])], [x + t, y + t], [x + t, b - t], [c([0xff, 0xff, 0xff, 0]); 2]),
            ([r, y], [r, b], [c([0, 0, 0, 0x80]), c([0, 0, 0, 0xcc])], [r - t, y + t], [r - t, b - t], [c([0, 0, 0, 0]); 2]),
            ([x, b], [r, b], [c([0, 0, 0, 0x80]), c([0, 0, 0, 0xcc])], [x + t, b - t], [r - t, b - t], [c([0xff, 0xff, 0xff, 0]), c([0, 0, 0, 0])]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    fn bevel(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let (dark_out, dark_in) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x19]));
        let (lit_out, lit_in) = (tint([0xff, 0xff, 0xff, 0xcc]), tint([0xff, 0xff, 0xff, 0x33]));
        let (r, b) = (x + w, y + h);
        let bands = [
            ([x + 1.0, y + 1.0], [r - 1.0, y + 1.0], dark_out, [x + t + 1.0, y + t + 1.0], [r - t - 1.0, y + t + 1.0], dark_in),
            ([x + 1.0, y + 1.0], [x + 1.0, b - 1.0], dark_out, [x + t, y + t + 1.0], [x + t, b - t - 1.0], dark_in),
            ([r - 1.0, y + 1.0], [r - 1.0, b - 1.0], lit_out, [r - t - 1.0, y + t + 1.0], [r - t - 1.0, b - t - 1.0], lit_in),
            ([x - 2.0, b - 1.0], [r - 2.0, b - 1.0], lit_out, [x + t + 1.0, b - t - 1.0], [r - t + 1.0, b - t - 1.0], lit_in),
        ];
        let n = (t.round() as usize).max(1);
        for (a0, a1, ca, b0, b1, cb) in bands {
            for d in 0..n {
                let (s0, s1) = (d as f32 / n as f32, (d + 1) as f32 / n as f32);
                let at = |s: f32, p: [f32; 2], q: [f32; 2]| [p[0] + (q[0] - p[0]) * s, p[1] + (q[1] - p[1]) * s];
                let corners = [at(s0, a0, b0), at(s0, a1, b1), at(s1, a0, b0), at(s1, a1, b1)];
                let (px, py) = (corners.iter().fold(f32::MAX, |m, c| m.min(c[0])), corners.iter().fold(f32::MAX, |m, c| m.min(c[1])));
                let cw = corners.iter().fold(f32::MIN, |m, c| m.max(c[0])) - px;
                let ch = corners.iter().fold(f32::MIN, |m, c| m.max(c[1])) - py;
                let s = (s0 + s1) * 0.5;
                let col = std::array::from_fn(|j| ca[j] + (cb[j] - ca[j]) * s);
                out.push(self.quad(aspect, px, py, cw.max(0.5), ch.max(0.5), col));
            }
        }
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
    /// `data.animating`'s typewriter — how many characters of a wordwrap widget's text are on
    /// screen yet, or `None` when the widget does not animate.
    ///
    /// The flag is bound to a byte — `[this+0x144]` on `NFS5.WORDWRAPCONTROL` (0x5228AC) and
    /// `[this+0x180]` on `NFS5.WORDWRAPACCESSORWATCH` (0x522B90); the two names were swapped here
    /// until 2026-09-21, and the sizes settle it — the control is 0x160 bytes and cannot hold a
    /// field at +0x180, while the watch is 0x1A8. Nothing here keys on the offsets, so only the
    /// comment was wrong. It drives three things:
    ///
    /// ```text
    /// 0x5229A0  activate:  [+0x14C] = [+0x150] = now;  [+0x158] = animating ? 0x97 : 0
    /// 0x522A5B  each draw: not transitioning (0x4C8820 == 0) -> [+0x150] = now
    /// 0x522A6E  [+0x158] == 0 -> the ordinary draw
    /// 0x522A83  else now - [+0x14C] <= [+0x158] -> **draw nothing this frame**
    /// 0x522A87  else [+0x14C] = [+0x150] = now, [+0x158] = 0, and still nothing this frame
    /// 0x522AE2  the draw hands ([+0x144], [+0x14C], [+0x150]) to the text object 0x523290
    /// 0x5232E5  which, when the flag is set, takes a budget of ([+0x150] - [+0x14C]) / 3
    ///           (0x5232FB's 0x55555556) and reveals that many characters
    /// ```
    ///
    /// So a widget with `animating=1` is **blank for 0x97 = 151 ticks** — 1.18 s at 128 Hz — and
    /// then types itself out at **one character every three ticks**, about 43 a second. Five of
    /// the eleven shipped wordwrap widgets carry it, and they are the narrative screens:
    /// `missionbrief`, `levelvictory`, `factdone`, `tournament` and `VictoryMessage`.
    ///
    /// APPROXIMATION, one frame wide: the original resets its stamps on whichever tick first sees
    /// the wait exceeded, and this port counts from tick 152 exactly.
    fn typewriter(&self, e: &nfs_formats::lay::Element) -> Option<usize> {
        if self.attr_num(e, "animating").unwrap_or(0.0) == 0.0 {
            return None;
        }
        let ticks = (self.clock_ms as u64 * 128 / 1000) as i64;
        Some((((ticks - 152).max(0)) / 3) as usize)
    }

    /// `NFS5.WORDWRAPACCESSORWATCH` — the class the front end uses where a value is a sentence
    /// rather than a figure: a tournament's description, a part's, a mission briefing.
    ///
    /// The break is the original's, **0x523600** (`RE/notes/controls.md` §4.51), and the lines sit
    /// `lineHeight + 2` apart (0x523916 adds the 2.0f at [0x5B2580]).
    #[allow(clippy::too_many_arguments)]
    fn wrapped_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4], budget: Option<usize>) {
        if w <= 0.0 {
            self.text(out, aspect, f, s, x, y, w, align, color);
            return;
        }
        let mut ly = y;
        // `budget` is `data.animating`'s character allowance — 0x523327's loop walks the line
        // vector and, for each line, draws it whole while `strlen(line) <= ebx` and takes `ebx`
        // off the budget (0x523340…0x52336E), or stops after truncating one line at `ebx`
        // characters (0x52337D writes the NUL and 0x523392 puts it back). §4.87.
        let mut left = budget.unwrap_or(usize::MAX);
        for line in wrap_lines(f, s, w) {
            if left == 0 {
                break;
            }
            let n = line.chars().count();
            let shown: String = if n <= left {
                left -= n;
                line
            } else {
                let t = line.chars().take(left).collect();
                left = 0;
                t
            };
            self.text(out, aspect, f, &shown, x, ly, w, align, color);
            ly += f.line_px() + 2.0;
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
            out.push(Sprite { texture: tex, pos, size, uv_min: uv0, uv_max: uv1, color, rotation: 0.0, pivot: [0.0, 0.0], tri: None });
            // `NFS_TEXT_AB=1` repeats each glyph one line lower **in red**, same texture, same UV
            // window, same size — so one image carries the widget's text twice and the only
            // difference between the two is `color.rgb`. The red copy draws and the black original
            // does not (`AUDIT.md` F2).
            if std::env::var("NFS_TEXT_AB").is_ok() {
                out.push(Sprite { texture: f.texture(), pos: [pos[0], pos[1] + size[1] * 1.2], size, uv_min: uv0, uv_max: uv1, color: [1.0, 0.0, 0.0, 1.0], rotation: 0.0, pivot: [0.0, 0.0], tri: None });
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
            // the background's own gaps are not this screen's: every background layout is one of
            // the 98 in `FEData/Layouts` and the sweep measures it in its own right, so reporting
            // it here would count it once per screen that sits on it
            out.extend(bg.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new()));
        }
        let bg_end = out.len();
        // `FEEndOfEraCallBackList`'s slot 15 (0x4FF1F0) is the screen's whole draw: it does not
        // call `Window::draw` (0x528760), so neither the fill of section 0 nor any child is painted
        if let Some(e) = &self.end_of_era {
            self.end_of_era_sprites(e, aspect, &mut out);
            // the pointer is the front end's own, drawn over every screen (§4.152)
            self.cursor_sprite(&mut out, aspect, None);
            return out;
        }
        // which widget the pointer is over, once for the whole screen (§4.39)
        let hovered = self.hovered();
        // `FeComboBox` paints an open drop-down from **slot 17** (0x4C6A88), the pass after the
        // ordinary draw, and steps the sprite depth for it (0x4DAA10 / 0x4DAAC0) — so it lands over
        // every widget on the screen. The port collects it here and appends it at the end.
        let mut overlay: Vec<Sprite> = Vec::new();
        // **The draw order is the section order, backwards** — `RE/notes/controls.md` §4.67.
        // `Window::load` (slot 20, 0x5282E0) walks the layout's sections **1 … count-1 forwards**
        // and `Window::addChild` (slot 22, 0x528120) is a `push_back`, so the child vector is in
        // section order; `Window::draw` (slot 15, 0x528760) then walks it from `_Last` down to
        // `_First` (0x528785…0x528799). So the **last** section is painted first and **section 1
        // is on top**. Section **0** is not a child at all: 0x5285D3 reads it once for the screen
        // descriptor, and it is the screen's own fill, painted before everything.
        let order: Vec<usize> = std::iter::once(0).chain((1..self.layout.elements.len()).rev()).collect();
        for i in order {
            let e = &self.layout.elements[i];
            let ((tdx, tdy), talpha) = self.transition_of(e);
            let (x, y) = self.position_of(e);
            let (x, y) = (x + tdx, y + tdy);
            let (w, h) = self.box_of(e, 0);
            let first = out.len();
            // the widget's `state` selects a part list that draws nothing at all — it is erased,
            // not undrawn, so it is not counted as a gap either
            if self.state_draws_nothing(e) || self.hidden_flags.contains(&i) {
                continue;
            }
            let mut closed = false;
            // what every per-class draw in `layout/widgets.rs` reads
            let d = WidgetDraw { i, e, x, y, w, h, talpha, aspect, strings, selected, hovered, accessor };
            match e.class.as_str() {
                c if c.starts_with("REAL.BLACK") => self.draw_screen_fill(d, &mut out),
                // `NFS_NO_GOURAUD=1` skips them — the probe for which sprites bury the text
                "NFS5.GOURAUDBOX" if std::env::var("NFS_NO_GOURAUD").is_ok() => {}
                "NFS5.GOURAUDBOX" => self.draw_gouraud_box(d, &mut out),
                "NFS5.GouraudIndicatorBar" => self.draw_gouraud_indicator_bar(d, &mut out),
                "NFS5.GlintyRectangle" => self.draw_glinty_rectangle(d, &mut out),
                "NFS5.PANEL" => {
                    closed = self.draw_panel(d, &mut out, blank);
                }
                "NFS5.3DCAR" => self.draw_3d_car(d, &mut out),
                "NFS5.DEADZONE" => {
                    closed = self.draw_deadzone(d, &mut out);
                }
                "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHATDISPLAYBOX"
                | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP" => self.draw_text_boxes(d, &mut out),
                "NFS5.ListBox" => self.draw_list_box(d, &mut out),
                "NFS5.RADIOGROUP" => self.draw_radio_group(d, &mut out, blank),
                "NFS5.TrackSmall2dSpline" => self.draw_track_small_spline(d, &mut out),
                "NFS5.PRIZERACEMAP" | "NFS5.FACTORYMAP" => self.draw_race_or_factory_map(d, &mut out),
                "NFS5.PLAYERPICTURE" | "NFS5.MINIPLAYERPICTURE" | "NFS5.BADGE" | "NFS5.MINIBADGE" | "NFS5.FACTORYPIC" | "NFS5.HUD" => self.draw_pack_pictures(d, &mut out),
                "NFS5.MOVIEBOX" | "NFS5.MOVIECIRCLE" => self.draw_movie(d, &mut out),
                "NFS5.ACCOUNTTHING" => self.draw_account_thing(d, &mut out),
                "NFS5.ANIMSHOWCASEPICTURE" => self.draw_anim_showcase_picture(d, &mut out),
                "NFS5.ChroniclePictureIcon" | "NFS5.ShowcasePicture" | "NFS5.ChroniclePicture" => self.draw_chronicle_or_showcase_picture(d, &mut out),
                "NFS5.CarLogo" => self.draw_car_logo(d, &mut out),
                "NFS5.KNOCKOUTTRACKLIST" => self.draw_knockout_track_list(d, &mut out),
                "NFS5.ChronicleMainBoxControl" => self.draw_chronicle_main_box(d, &mut out),
                "NFS5.AnimatingPicture" => self.draw_animating_picture(d, &mut out),
                "NFS5.HEADER" | "NFS5.SUBHEADER"
                | "REAL.LABEL" | "NFS5.PICTURE" | "NFS5.NonLocalizedLabel" | "NFS5.ExtraCircle"
                | "NFS5.OutlineRectangle" | "NFS5.Rectangle" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.GLINTYLINE" => {
                    closed = self.draw_template_classes(d, &mut out);
                }
                "NFS5.CheckBox" => self.draw_check_box(d, &mut out),
                "NFS5.CarSettingSlider" => self.draw_car_setting_slider(d, &mut out),
                "NFS5.ComboBoxSlider" | "NFS5.ColorBox" => self.draw_combo(d, &mut out, &mut overlay),
                "NFS5.GOTOBUTTON" => self.draw_goto_button(d, &mut out),
                "NFS5.BOTTOMBARBUTTON" | "NFS5.GRAPHICTABBUTTON" => self.draw_bar_or_graphic_tab(d, &mut out),
                "NFS5.TABBUTTON" | "NFS5.CAMERASELECT" | "NFS5.CAMERAPICTURE" => self.draw_tab_or_camera(d, &mut out),
                _ => {
                    closed = self.draw_default(d, &mut out);
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
                let f0: Vec<(String, [f32; 4])> = { let mut v: Vec<(String, [f32; 4])> = out[first..].iter().map(|s| (format!("{:?}", s.texture), s.color)).collect(); v.dedup(); v.truncate(3); v };
                let hot = match self.state_for(e, i, selected, hovered) {
                    "normal" => String::new(),
                    st => format!(" [{st}]"),
                };
                println!("  #{i:3} {:<26} pos=({x:.0},{y:.0}) size=({w:.0}x{h:.0}) sprites={n}{hot} box=({:.3},{:.3})-({:.3},{:.3}) alpha={talpha:.2} first={f0:?}", e.class, b.0, b.1, b.2, b.3);
            }
            // **`NFS5.3DCAR` is never a sprite, in either build.** Its draw, `FECar3DControl`'s
            // slot 15 = **0x4C0990**, calls none of the painter family: 0x4C09A3 places the car
            // through 0x4C0D10 and the only other thing it can do is 0x4E1E50(7), a sound, when
            // the widget is hovered (0x4C7DD0) and its accessor answers (0x4F09C0). The car
            // itself belongs to the 3D pass — `positiontype` 0 hands it to the screen's room
            // (0x4BFEEC) and 1…3 place one of the widget's own at a fixed camera-relative spot
            // (`RE/notes/frontend-3d.md` §4). This port puts that car into an offscreen view so
            // it lands in the right place in the 2D layering, which is a porting device and not
            // something the original does; a sweep that counts 2D sprites therefore has nothing
            // to see here, the same way `deadzone.lay`'s axis bars have nothing to show without
            // a pad. Reported on its own line rather than as a blank.
            if out.len() == first && i > 0 && !closed && e.class != "NFS5.3DCAR" {
                blank.push((e.class.clone(), source_of(e)));
            }
        }
        // the tooltip last of all: the original builds it in the widget's own tick, and it has to
        // be over the screen it describes
        if let Some(h) = hovered
            && let Some(e) = self.layout.elements.get(h)
        {
            self.tooltip(&mut overlay, aspect, e, strings);
        }
        out.append(&mut overlay);
        // last of all, over the tooltip and the drop-down alike
        self.cursor_sprite(&mut out, aspect, hovered);
        if std::env::var("NFS_LAYER_DUMP").is_ok() {
            println!("layer: background {} sprites, own {} sprites", bg_end, out.len() - bg_end);
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

    /// What the progress is *worth* — the two formulas `FeWidget` applies to it, read 2026-09-21.
    ///
    /// The slide is **0x4C8760**, four arms off the bit word:
    ///
    /// ```text
    /// 0x4C87BF  bl & 2  -> [ecx]   -= [0x5DEAC8] * p / [0x5B6B2C]     ; left
    /// 0x4C87D5  bl & 4  -> [ecx]   += [0x5DEAC8] * p / [0x5B6B2C]     ; right
    /// 0x4C87EB  bl & 8  -> [ecx+4] -= [0x5DEACC] * p / [0x5B6B2C]     ; top
    /// 0x4C8802  bl & 16 -> [ecx+4] += [0x5DEACC] * p / [0x5B6B2C]     ; bottom
    /// ```
    ///
    /// `[0x5B6B2C]` is **512** in the image, which is where `TRANS_FULL` comes from, and
    /// `[0x5DEAC8]` / `[0x5DEACC]` are the **live screen width and height** — zero in the image
    /// and written at **0x556C8A** / 0x556C90 from the selected video mode's record at 0x5DE654 /
    /// 0x5DE658, the same pair the platform layer clips against at 0x53550E and 0x536F8A. So a
    /// sliding widget travels exactly **one screen**, and this port multiplying by its design size
    /// in design space is that same distance. (The port cited 0x4C87CD for `TRANS_FULL` but had
    /// never pinned the multiplier.)
    ///
    /// The fade is **0x4C8820**: `p << 8` minus one copy of `p` — the ×255 idiom — over the same
    /// 512, an integer divide.
    #[test]
    fn a_sliding_widget_travels_exactly_one_screen_and_fades_by_p_255_over_512() {
        // the four arms, at half of the way
        let half = TRANS_FULL / 2;
        let f = half as f32 / TRANS_FULL as f32;
        assert_eq!(f, 0.5);
        assert_eq!(DESIGN.0 * f, DESIGN.0 / 2.0, "left/right move by half the screen's width");
        assert_eq!(DESIGN.1 * f, DESIGN.1 / 2.0, "top/bottom by half its height");
        // and the whole way is the whole screen, which is what 512 means
        assert_eq!(DESIGN.0 * (TRANS_FULL as f32 / TRANS_FULL as f32), DESIGN.0);
        // the fade, exactly as 0x4C8834…0x4C883C computes it
        let fade = |p: i32| ((p << 8) - p) / TRANS_FULL;
        assert_eq!(fade(0), 0, "in place, nothing is taken off");
        assert_eq!(fade(TRANS_FULL), 255, "a whole screen away, all of it");
        assert_eq!(fade(half), 127, "and the integer divide truncates at the halfway point");
        // which is the alpha the draw uses
        for p in [0, 1, half, TRANS_FULL] {
            assert_eq!(1.0 - fade(p) as f32 / 255.0, 1.0 - (p * 255 / TRANS_FULL) as f32 / 255.0);
        }
    }
}
