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

mod bar_oval;
mod career_tabs;
mod dialog_draw;
mod end_of_era_draw;
pub mod fe_depth;
pub mod glints;
pub mod loading;
pub mod nav;
mod widgets;
use widgets::WidgetDraw;

/// The front end's and the HUD's string tables, as the original loads them (`RE/notes/hud.md`
/// "FEStrings.loc"):
///
/// ```text
/// 0x4E04F0 (FE start-up, 0x4D366E)
///   sprintf(buf, "%sFEStrings.loc", [0x65B340] = install.txt line 40, FEData\locale\)  0x4E0532
///   file = 0x59D8E0(buf, 0x4E07B0())       ; 0x4E07B0 is the FEMemory pool [0x5D9B48], not a language
///   push file onto [0x690714]             ; the only table 0x4E06D0 walks
///   lang = 0x4B73F0([0x65B39C])           ; the registry string "Language"
///   lang >= 7 -> 0, lang < 0 -> 6; [0x690710] = lang   (0x4E0586…0x4E0598; read back by 0x4E0750)
///   0x536D80(file, 0, lang) for every table
/// 0x41E170 (HUD start-up): sprintf("%shud.loc", [0x65B310] = line 28, gamedata\dashhud\),
///   0x59D8E0, [0x60C974] = file, 0x536D80(file, 0, 0x4B73F0())       (0x41E1BF…0x41E1FE)
/// ```
///
/// 0x4B73F0 maps the registry value `Language` (HKLM, REG_SZ; written as `"English"` when it is
/// missing, 0x4B7434…0x4B7474) by exact, case-sensitive compare: `English` 0, `German` 1,
/// `French` 2, `Spanish` 3, `Italian` 4, `Swedish` 5, `PortBrzl` 6, anything else 0
/// (0x4B74DD…0x4B76E6). The port has no Windows registry; it reads the value from the install's
/// registry exports (`GamePaths::language`, `game_info/regs/*.reg`), with the original's default.
/// The same index is what the original copies into the `LANGUAGE` option (id 0x32, `[0x657C84]`,
/// 0x4E0750 → 0x4D53A9 / 0x4ED6CB) that the loading screen's `loadtext.fsh` reads.
/// *(Until 2026-09-27 the port took `fe.txt`'s `language` instead — an APPROXIMATION.)*
///
/// **Fallback (the port's, not the original's):** without `FEStrings.loc` the port reads that
/// language's column of `FEStrings.csv` (English first; the original has no CSV reader and would
/// dereference the null table). Without `hud.loc` the HUD reads the CSV's `hud` rows, else the
/// front end's table.
///
/// The front end also needs the table for one geometry rule: a `TEXT` or `LOCALIZEDTEXT` part's
/// `size` is the **extent of the string it draws**, which several templates then position other
/// parts from (`BottomBarButton.ctr`'s own `pos.value` is
/// `(data.align&1)?data.posx-(label.size.width+picture.size.width+data.space):data.posx`).
pub fn load_strings(paths: &GamePaths) -> Strings {
    let raw = paths.language();
    // 0x4E0586: 7 and above fold to 0 (0x4B73F0 answers 0…6, so the fold never acts)
    let lang = if raw >= 7 { 0 } else { raw };
    let table = |dir: std::path::PathBuf, name: &str| {
        let mut t = nfs_formats::loc::LocTable::parse(std::fs::read(GamePaths::find_file(&dir, name)?).ok()?).ok()?;
        t.set_language(lang);
        Some(t)
    };
    let hud = table(paths.game_data().join("DashHud"), "hud.loc");
    if let Some(fe) = table(paths.fe_data().join("Locale"), "FEStrings.loc") {
        return Strings::from_loc(&fe, hud.as_ref());
    }
    tracing::warn!("no FEStrings.loc: falling back to FEStrings.csv");
    let csv = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").and_then(|p| std::fs::read(p).ok());
    let mut s = csv.map(|b| Strings::parse(&b, lang as usize)).unwrap_or_default();
    if let Some(h) = &hud {
        s.hud_by_id = Strings::from_loc(h, None).fe_by_id;
        s.hud_table = true;
    }
    s
}

/// Read `FEData/Controls/*.ctr` — the widget class templates that hold every attribute's default
/// and the pieces each class is made of (`RE/notes/controls.md`).

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

/// `NFS5.WORDWRAPACCESSORWATCH`'s own state beyond the accessor watch's (vtable 0x5BAEF4): the
/// text its word-wrap object was built from and the change it is fading out of.
///
/// ```text
/// 0x522EC0  tick: the watch's own tick 0x4BD230 (the label's text from the accessor), then
///           not changing and the label's text != the copy [+0x198]  -> changing [+0x1A0] = 1,
///                                                                        [+0x1A4] = now
///           changing and (now − [+0x1A4] > 0x40, or not animating)  -> rebuild the wrap from the
///           new text (0x523130 / 0x523080), copy it, [+0x184] = [+0x188] = now, changing = 0
/// 0x522DC0  draw: changing -> the colour's alpha times clamp(([+0x1A4] − now)·4 + 0x100, 0, 0xFF)
///           (0x4D87F0) — the old text fades out over 64 ticks
/// ```
///
/// So an animating watch (`data.animating`, `[+0x180]`) whose text changes fades the old text out
/// in half a second and types the new one from its first letter (0x5232E5's budget
/// `([+0x188] − [+0x184]) / 3` starts again from 0); one that does not animate takes the new text on
/// the same tick. The shipped animating ones are `tournament`'s description and the four narrative
/// screens'.
#[derive(Debug, Clone, PartialEq)]
pub struct WrapWatch {
    /// `[+0x198]` — the text the wrap was built from
    pub text: String,
    /// `[+0x1A4]` while `[+0x1A0]` is set — the tick the change was seen
    pub changing: Option<i64>,
    /// `[+0x184]` after a rebuild — the tick the typing starts again from; `None` until the first
    /// change, while the activation's wait (`typewriter`) still rules
    pub restart: Option<i64>,
}

impl WrapWatch {
    /// One tick of 0x522EC0 against the label's current `text` at tick `now`.
    pub fn tick(&mut self, text: &str, now: i64, animating: bool) {
        if self.changing.is_none() && self.text != text {
            self.changing = Some(now);
        }
        if let Some(at) = self.changing
            && (now - at > 0x40 || !animating)
        {
            self.text = text.to_string();
            self.restart = Some(now);
            self.changing = None;
        }
    }

    /// The draw's fade while a change runs (0x522DC4…0x522E10), as a factor on the alpha.
    pub fn fade(&self, now: i64) -> f32 {
        match self.changing {
            Some(at) => ((at - now) * 4 + 0x100).clamp(0, 0xFF) as f32 / 255.0,
            None => 1.0,
        }
    }
}

/// The typing of an animating word-wrap widget — `NFS5.WORDWRAPCONTROL`'s `[+0x14C]`, `[+0x150]`
/// and `[+0x158]`, `NFS5.WORDWRAPACCESSORWATCH`'s `[+0x184]`, `[+0x188]` and `[+0x18C]`; the two
/// classes run the same code on their own offsets:
///
/// ```text
/// 0x5229A0 / 0x522CB4  activate: start = last = now; wait = animating ? 0x97 : 0
/// 0x522A52 / 0x522E14  each draw: 0x4C8820 (the transition's fade) == 0 -> last = now
/// 0x522A6E / 0x522E30  wait != 0: now − start <= wait -> draw nothing
///                                  else start = last = now, wait = 0, and draw nothing
/// 0x522AE2 / 0x522EA4  else the text object 0x523290 gets (start, last): with the flag set it
///                      reveals (last − start) / 3 characters (0x5232FB, a signed divide)
/// 0x52301A             the watch's rebuild on a new text: start = last = now (wait untouched)
/// ```
///
/// So the typing starts from **the tick of the first draw that finds the wait over**, not from
/// tick 152 — a frame that spans several ticks moves it — and it **stops growing while the screen
/// is in a transition**, since `last` is only restamped when the fade is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typing {
    pub start: i64,
    pub last: i64,
    pub wait: i64,
}

impl Typing {
    /// The activation (0x5229A0 / 0x522CB4) at tick `now`.
    pub fn new(now: i64, animating: bool) -> Self {
        Typing { start: now, last: now, wait: if animating { 0x97 } else { 0 } }
    }

    /// One draw at tick `now`: `None` draws nothing, `Some(n)` reveals `n` characters (the caller
    /// shows the whole text when the widget does not animate).
    pub fn draw(&mut self, now: i64, transitioning: bool) -> Option<usize> {
        if !transitioning {
            self.last = now;
        }
        if self.wait != 0 {
            if now - self.start > self.wait {
                self.start = now;
                self.last = now;
                self.wait = 0;
            }
            return None;
        }
        Some(((self.last - self.start) / 3).max(0) as usize)
    }

    /// The watch's rebuild (0x52301A).
    pub fn restart(&mut self, now: i64) {
        self.start = now;
        self.last = now;
    }
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
    /// The clip has played to its last frame at least once — what `NFS5.MOVIECIRCLE`'s tick asks
    /// the player (0x4E2960) before `careerfirst` moves on (0x4CD608).
    finished: bool,
    /// `FEMovie`'s own `[+0x75]`, `data.audio` (`MovieBox.ctr` / `MovieCircle.ctr` default 1):
    /// with it clear the tick (0x4E25A0, shared by every `FEMovie`, `frontend-3d.md` §11) may
    /// advance **at most one frame a call**, the clock pulled back to it (0x4E261D…0x4E2631) —
    /// the same rule `room::VictoryMovie::tick` already carries for the victory room's own
    /// silent `MOVIE`. Without it a movie that misses ticks (a stall) does not skip frames to
    /// catch up; the skipped time is dropped, not queued. **Also** (`mad.md` §14): whether the
    /// movie has an audio track to play at all — the same flag, in the same field, `0x4E1F07`.
    catchup: bool,
    /// The whole `SC**` track, decoded once (`mad.md` §14, 0x4E1F9A…0x4E1FB4's registration):
    /// `None` when `catchup` is clear, the `.mad` carries no audio chunks, or the file could not
    /// be decoded. Kept so a loop restart ([`Self::restart_voice`]) does not re-decode.
    clip: Option<Arc<nfs_audio::mixer::Clip>>,
    /// The playing track and a handle that can stop it without the whole `AudioOutput`
    /// (`mad.md` §14: `FEMovie`'s destructor, 0x4E2250, does the same thing at the same point —
    /// `[+0x60]` (the player) stopped via 0x567690 first, before any other field is released —
    /// which [`Drop`] mirrors with `stop()`; the destructor's own further frees (the decode
    /// buffer, the stream at `[+0x2c]` via 0x56B3B0, `[+0x1c]`/`[+0x24]`/`[+0x40]`, the shared
    /// movie-subsystem refcount at `[0x6907A0]`) have no port-side counterpart — Rust's own
    /// `Vec`/`Arc` drops cover them).
    voice: Option<(nfs_audio::VoiceId, nfs_audio::StopHandle)>,
    /// The `MUSICLEVEL` (`[0x657CA0]`) byte last applied to [`Self::voice`] (`FEMovie+0x6C`,
    /// 0x4E27BF's cache) — set again only when the setting changes.
    last_level: Option<u8>,
}

/// `FEMovie`'s destructor, 0x4E2250 (`mad.md` §14): stops `[+0x60]` via 0x567690 first, before
/// anything else is freed. [`Drop::drop`] does the equivalent — `stop()` the voice — and leaves
/// the rest of the struct to Rust's own drop glue.
impl Drop for MoviePlayer {
    fn drop(&mut self) {
        if let Some((id, stop)) = self.voice.take() {
            stop.stop(id);
        }
    }
}

/// `0x566340` is `Voice_setVolume(handle, level: 0..127)` (`engine-audio.md`'s own CONFIRMED
/// reading), and the tick's own call site (0x4E27BA…0x4E27CE) passes it the raw `MUSICLEVEL` byte
/// straight through — unlike `MusicSetVolume` (0x4AE8C0), which first rescales through
/// `MusicLevelToVolume` (0x4A82F0, `(0x50·v) >> 7`), `FEMovie`'s own path never calls that
/// function. Read to the instruction (0x5662B0/0x589DF0, `mad.md` §14): `Voice_setVolume`
/// multiplies the level in as one more independent linear gain factor alongside the channel's
/// own cached volume and two per-channel/global bytes, normalized by `/127³` (the `0x4186143D`
/// magic, `sar 0x13`, round-toward-zero — the same divide `sfx.md` §"Voice_play" already
/// documents); no logarithmic curve or lookup table applies to this argument itself (a
/// per-channel table at `+0x54`/`+0x5C` exists in the mixer but is unset for a plain `SC**`
/// stream — wind loops and patch-defined instruments are the only users, `sfx.md`). So `level/127`
/// is the exact mapping, not an approximation: it was mislabelled before this byte's path was
/// traced past 0x567810.
fn movie_gain(music_level: u8) -> f32 {
    music_level as f32 / 127.0
}

#[cfg(test)]
mod movie_gain_tests {
    use super::movie_gain;

    /// 0x566340 (`Voice_setVolume`) takes the raw `MUSICLEVEL` byte as one linear 0..127 gain
    /// factor (0x4E27CE's own call, no `MusicLevelToVolume` rescale) — `level/127` end to end.
    #[test]
    fn linear_0_64_127() {
        assert_eq!(movie_gain(0), 0.0);
        assert!((movie_gain(64) - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(movie_gain(127), 1.0);
    }
}

impl MoviePlayer {
    /// The whole file kept, its video chunks indexed, the first frame decoded into a texture, and
    /// (`mad.md` §14) its own `SC**` audio track decoded (not yet started — see
    /// [`Self::advance`]). `FEMovie`'s constructor 0x4E1EB0 decodes frame 0 (0x4E2127) and
    /// registers the track, but the sound library's player only starts on the first ready tick
    /// (0x56CA20, then 0x56CA00) — the port's own `open` is never mid-tick, so the two collapse
    /// into `advance`'s own first call rather than here, which also fixes the case this port adds
    /// that the original never has: the sound device can finish initialising strictly later than
    /// a screen's widgets are bound (`self.snd.audio`'s own lazy open in `porsche`). `audio_out`
    /// is `None` exactly when `[0x657C94]` (no sound device) forces the original's own `[+0x75]`
    /// off (`self.snd.audio.is_none()` in the port) — checked again every `advance`.
    fn open(r: &mut Renderer, paths: &GamePaths, dir: &str, file: &str, audio: bool) -> Option<Self> {
        let path = GamePaths::find_file(&paths.fe_data().join(dir), file)?;
        let Ok(data) = std::fs::read(&path) else { return None };
        let tables = nfs_formats::mad::MadTables::load(&paths.root).ok()?;
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
        let clip = audio.then(|| nfs_formats::schl::decode_all(&nfs_formats::mad::audio_track(&data)).ok()).flatten().map(|pcm| {
            Arc::new(nfs_audio::mixer::Clip {
                rate: pcm.rate,
                channels: pcm.channels,
                samples: pcm.samples,
                loop_range: None,
                gain: 1.0,
                bend_cents: 0,
                fx_send: pcm.fx_send,
                priority: nfs_audio::mixer::STREAM_PRIORITY,
                pan: pcm.pan,
                pan_scale: pcm.pan_scale,
                records: Vec::new(),
                layers: Vec::new(),
                tables: Vec::new(),
            })
        });
        Some(MoviePlayer {
            data,
            chunks,
            decoder,
            next: 1,
            period_q16: header.period.max(1) as u64,
            clock_q16: 0,
            tex,
            size: (f.width as f32, f.height as f32),
            finished: false,
            catchup: audio,
            clip,
            voice: None,
            last_level: None,
        })
    }

    /// (Re)start the audio track from its own beginning — `FEMovie`'s constructor's own start
    /// (0x4E212C…0x4E2178) and, at a loop restart, 0x4E2380's `0x567690` (stop the old player)
    /// followed by a fresh 0x56CA20 (`mad.md` §14). No `looping` flag on the voice itself: the
    /// original never lets the sound library loop the stream on its own — it hard-restarts a
    /// fresh player at the exact tick the video also wraps, which this mirrors, so drift between
    /// the two cannot accumulate across a long-running loop (0x4E25A0 carries no drift-sync — the
    /// video's own wall-clock pacing is the whole story, `mad.md` §14).
    fn restart_voice(&mut self, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        if let Some((id, stop)) = self.voice.take() {
            tracing::info!(frame = self.next.saturating_sub(1), "FEMovie audio: stop (0x567690)");
            stop.stop(id);
        }
        if let (Some(clip), Some(a)) = (&self.clip, audio_out) {
            let gain = movie_gain(music_level);
            let id = a.play_word(clip.clone(), gain, nfs_audio::resample::PITCH_UNIT, 0x40, 0, false);
            tracing::info!(frame = self.next.saturating_sub(1), rate = clip.rate, channels = clip.channels, music_level, gain, "FEMovie audio: start (0x56CA20/0x56CA00)");
            self.voice = Some((id, a.stop_handle()));
            self.last_level = Some(music_level);
        }
    }

    /// Step by `dt_ms`, decoding and uploading the frames the period calls for; a movie that has
    /// run out starts again (and, with an audio track, restarts it — [`Self::restart_voice`]).
    /// `music_level` is `MUSICLEVEL` (`[0x657CA0]`), applied to a playing track only when it
    /// changed since the last call (0x4E27BA…0x4E27CE's own cache). A track that has not started
    /// yet — every widget movie's own first call (`mad.md` §14: the original starts it on the
    /// first *ready* tick, which the port's always-ready file read collapses into this one), the
    /// end-of-era movie's own deferred start once its show reaches `movie_started()`, and any
    /// movie whose `audio_out` was `None` at `open` because the port's own sound device had not
    /// finished initialising yet — starts here too, the same call as a loop restart.
    fn advance(&mut self, r: &mut Renderer, dt_ms: u32, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        self.clock_q16 += (dt_ms as u64) << 16;
        let (clock_q16, frames) = movie_advance_frames(self.clock_q16, self.period_q16, self.catchup);
        self.clock_q16 = clock_q16;
        let mut drawn = false;
        let mut looped = false;
        for _ in 0..frames {
            if self.next >= self.chunks.len() {
                self.next = 0;
                self.finished = true;
                looped = true;
            }
            let (off, len) = self.chunks[self.next];
            self.next += 1;
            let _ = self.decoder.decode_chunk(&self.data[off..off + len]);
            drawn = true;
        }
        if self.catchup && (looped || self.voice.is_none()) {
            self.restart_voice(audio_out, music_level);
        } else if let (Some((id, _)), Some(a)) = (&self.voice, audio_out)
            && self.last_level != Some(music_level)
        {
            let gain = movie_gain(music_level);
            tracing::debug!(music_level, gain, "FEMovie audio: MUSICLEVEL changed (0x567810)");
            a.set_word(*id, gain, nfs_audio::resample::PITCH_UNIT, 0x40);
            self.last_level = Some(music_level);
        }
        if drawn {
            let (w, h) = (self.size.0 as u32, self.size.1 as u32);
            r.update_texture(self.tex, &TextureData::single(w, h, self.decoder.rgba()));
        }
    }
}

/// `FEStringAccessor::insert` **0x4BCA90**'s own forbidden set, active only on the one shipped
/// instance whose ctor argument enables it (`UserList.NameInput`, `LayoutScreen::type_char`'s own
/// doc) — nine bytes byte-for-byte the set Windows refuses in a file name: `\ / : * ? " < > |`.
/// The shared key handler's own `%` refusal (0x4CCC20, `type_char`'s doc) is unconditional and
/// already checked separately; this is the *second*, per-instance layer 0x4BCA90 adds on top.
fn filename_illegal_char(code: u32) -> bool {
    matches!(code, 0x5c | 0x2f | 0x3a | 0x2a | 0x3f | 0x22 | 0x3c | 0x3e | 0x7c)
}

/// The tick's own frame count, 0x4E25A0: with `catchup` (`data.audio` set) as many periods as
/// `clock_q16` covers. Without it, **only a genuine backlog — more than one frame due — is
/// clamped** to one, dropping the rest (0x4E261D…0x4E2631 resets the accumulator to exactly the
/// one frame taken); the ordinary case of one frame due (`ebp == edi + 1`, `jle 0x4e2634` skips
/// the clamp) carries its own true remainder just as the audio path does. Returns the clock left
/// over and how many chunks to decode.
fn movie_advance_frames(clock_q16: u64, period_q16: u64, catchup: bool) -> (u64, u32) {
    let frames = clock_q16 / period_q16;
    if frames == 0 {
        return (clock_q16, 0);
    }
    if catchup || frames <= 1 {
        (clock_q16 - frames * period_q16, frames as u32)
    } else {
        (0, 1)
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
    /// the open drop-down's depth mask in the frame's `overlay` list — the stencil
    /// `[.0 .. .1)`, the masked draws `[.1 .. .2)` — given its depth when the list is appended,
    /// which is where the original's later pass draws it (`drop_down`)
    overlay_mask: std::cell::Cell<Option<(usize, usize, usize)>>,
    /// widget classes the file does not place, with how many of each were skipped
    unrendered: Vec<(String, usize)>,
    /// element index of an `NFS5.CarLogo` → the `.fsh` pack and the first of its two entries
    logos: HashMap<usize, (String, usize)>,
    /// `NFS5.PANEL` element indices the screen deliberately leaves **closed** — the tabs that are
    /// not the selected one, and the CD player. A closed panel draws nothing (0x4CEBC0 returns at
    /// once when `[panel+0x14C]` is null), so it is not a gap; a panel whose file simply would not
    /// load is not in here and is still counted.
    closed: std::collections::HashSet<usize>,
    /// A tab strip's pending button, `FERadioGroup`'s `[+0x158]`: `(strip, button)` — the strip's
    /// index here and the button's in the strip. Its tick (`tick_panels`) makes it current once
    /// the current button is no longer selected.
    pending_tab: Option<(usize, usize)>,
    /// The transition a panel's owner hands its sub-screen for the draw — `(going out, progress)`
    /// of the screen above, `(false, 0)` on a top-level one (see `widget_state`).
    inherited: std::cell::Cell<(bool, i32)>,
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
    /// element index of an `NFS5.ColorBox` → its accessor's current colour, `[+0x2C]` — what the
    /// closed box shows as a swatch (0x4C60FA…0x4C618B, §4.171.4)
    swatches: HashMap<usize, [u8; 4]>,
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
    /// each `NFS5.WORDWRAPACCESSORWATCH`'s text as it last laid it out, and its change fade
    /// (`WrapWatch`, 0x522EC0 / 0x522D60), made on its first draw
    wrap_watch: std::cell::RefCell<HashMap<usize, WrapWatch>>,
    /// each animating word-wrap widget's typing stamps ([`Typing`]), made on its first draw
    typing: std::cell::RefCell<HashMap<usize, Typing>>,
    /// each glinting widget's own timer (`glints`), and whether a draw has stepped it yet
    glints: std::cell::RefCell<HashMap<usize, (glints::Glint, bool)>>,
    /// the ticks `NFS_FE_CLOCK` preloaded the clock with — 0 in a game
    preload_ticks: i64,
    /// each `NFS5.AccessorWatch`'s `label.text` as its tick last wrote it (0x4BD32C, 0x526980) —
    /// what the part measures its `size` over (0x4E7160); before the first draw, the template's
    watch_text: std::cell::RefCell<HashMap<usize, String>>,
    /// each widget's last state name the template actually had an entry for ([`drawn_state`](Self::drawn_state)) —
    /// 0x5271F0 (§"drawn_state") copies nothing when the live state has no entry, so the part
    /// list it draws is whatever the last successful match left it as, not the current state
    last_drawn_state: std::cell::RefCell<HashMap<usize, String>>,
    /// `FEEndOfEraCallBackList`'s show, when this is the end-of-era screen (`set_end_of_era`)
    end_of_era: Option<end_of_era_draw::EndOfEraScreen>,
    /// the bottom bar's career era tabs, loaded by its slot 21 0x4D7FD0 (`load_career_tabs`)
    career_tabs: Option<career_tabs::CareerTabs>,
    /// the bottom bar's oval flash `[bar+0x278]` (0x520AD0 / 0x520B80, `load_bar_oval`)
    bar_oval: Option<bar_oval::BarOval>,
    /// `[0x65B940]`, the name-box codes the player has typed (`nfs_game::cheats`) — bit 6 is read
    /// by the credits roll
    pub cheat_bits: u32,
    /// `[0x5D8F2C]`, the driver picture the front end has chosen: 8 in the image, written by
    /// `CREATEPLAYERENTER`'s 0x4CF790(8) and by `NFS5.PLAYERDRIVERPICTURE`'s release 0x4CF940.
    /// The app owns the global and hands it to each screen it opens; the pictures' draw 0x4CF7C0
    /// compares it against their own `data.ournumber` (0x4CF83B, 0x4CF8E1).
    pub driver_picture: i32,
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
    /// The pointer the **cursor sprite** is drawn at, when it is not `cursor`. The cursor draw
    /// 0x4E1C90 takes the device's position straight from 0x5372A0(6) (0x4D32D9…0x4D32EF), not
    /// from an event, so the pointer moves while the input dispatch 0x4D2850 is dropping the
    /// move events the widgets' hover comes from (`set_pointer`). `RE/notes/controls.md` §4.171.
    pointer: Option<Option<(f32, f32)>>,
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
    /// `NFS5.ColorBox`'s latch `[0x65B710]` — raised by 0x4C39F0 when the row chosen in a colour
    /// box's list is the custom row (0x4C6D63…0x4C6D6C), read by 0x4C3A00; the editor draws while
    /// the box holds focus and this is up (0x4C3D0C). §4.171.
    picker_latch: bool,
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
    /// `Camera.1 … Camera.4` — the view each of the four camera slots holds (0x4E4D40 registers
    /// them as 0, 1, 2, 3), which `NFS5.CAMERAPICTURE`'s draw 0x4BF870 compares with its own
    /// `data.picturenum`
    camera_list: [i32; 4],
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
    /// The widgets whose `selected` flag has been **cleared** since the screen was built — a tab
    /// the block starts as `state=selected` (the bind's own flag) that another tab's click has
    /// since deselected (0x4CFF30 → the old tab's slot 28, 0x4D0310)
    unselected: std::collections::BTreeSet<usize>,
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
    /// One caption per timeline column — the `фото`-row (kind 0) entry's own `0x6F` text
    /// attachment (`nfs_formats::fsh::Fsh::entry_caption`), always the **resting** frame's
    /// (0x507340's fourth argument, `hot`, is a hardcoded `0` at the caption's own call site,
    /// 0x507C3B — §4.173), `None` when there is no picture-kind entry for the column or its name
    /// is empty (0x507CB9's `je`).
    chronicle_captions: Vec<Option<String>>,
    /// the six `WeeIcons.fsh` entries the combo scrollbar's two arrows cycle (§4.90, §4.101)
    combo_arrows: Option<[usize; 6]>,
    /// `Parts.AllParts`'s own collapsible-header icon — the ctor **0x515160** builds
    /// `"%sWeeIcons.fsh"` (0x5151C0: string 0x5D976C) and caches four entries by index —
    /// `pls1`(3)→`[this+0xA4]`, `min4`(0xB)→`[this+0xA8]`, `pls2`(4)→`[this+0xAC]`,
    /// `pls3`(5)→`[this+0xB0]` — plus a fifth, `BarD`(0xE)→`[this+0xB4]`, for the list's own
    /// scrollbar (already the generic `NFS5.ListBox` art above). The row draw **0x515620**
    /// picks one of the first four by `(al = [this+0x2C]`/`[this+0x2D]`, this section's own
    /// **open** flag) and a second flag at `[esp+0xcc]`, read to the instruction 2026-09-29: the
    /// row loop, `0x4C64FE`, calls the accessor's own vtable slot 15 (`call [eax+0x3C]` at
    /// `0x4C68F5`) with exactly 8 stack args (`0x515620` itself ends `ret 0x20`); the first-
    /// pushed (`[esp+0xcc]`, the 8th/last arg) is `ebx`, **0** by default (`0x4C672E`) and
    /// overwritten only when this row is the list's **selected-and-focused** row (`0x4C674C`'s
    /// `cmp edi,[this+0x1A8]` gated by the byte at `[this+0x1A4]`) or the row the accessor's own
    /// hover query (its vtable slot 10, called at `0x4C678E`) reports — the same two conditions
    /// this port already computes as `hover_row`/`current` for the row's highlight bar. The flag
    /// is that bar's own presence: `pls1` closed/`min4` open when neither holds, `pls2`
    /// closed/`pls3` open when either does. `[pls1, min4, pls2, pls3]`.
    parts_glyphs: Option<[usize; 4]>,
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
    let (ky, knob_h, _) = combo_knob(y, h, rows, first);
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

/// `FeListScrollBar`'s knob — `(top, height, travel)` for a bar at `y`, `h` tall, whose model
/// answers `count` positions (slot 9) and sits on `first` (slot 10). The constructor 0x4DAF10 and
/// the re-place slot 11, **0x4DB220**, compute it in integers:
///
/// ```text
/// 0x4DB26A  knobH  = max(16, [+0x118] − 2·[+0x128] − count)   ; h − 2·13 − count
/// 0x4DB2B0  travel = [+0x118] − 2·[+0x128] − knobH
/// 0x4DB2CE  top    = [+0xF4] + (count ? first·travel / count : 0)   ; idiv, truncating
/// ```
///
/// with `[+0xF4]` the knob's resting top, `y + 13` (0x4DAF30 / 0x4DB016). The divisor is the
/// **count**, not `count − 1` — this is not `NFS5.VerticalSlider`'s placement (0x4D0BBE, §4.46),
/// so the knob never quite reaches the down arrow. The drag, 0x4DB9B0, is its exact inverse
/// (`App::fe_apply_combo_drag`). *(Until 2026-09-30 the port placed this knob with the list box's
/// `count − 1` in floats.)*
pub fn combo_knob(y: f32, h: f32, count: usize, first: usize) -> (f32, f32, i32) {
    const ARROW_H: i32 = COMBO_ARROW_H as i32;
    let (h, count) = (h as i32, count as i32);
    let knob_h = (h - 2 * ARROW_H - count).max(16);
    let travel = h - 2 * ARROW_H - knob_h;
    let dy = if count == 0 { 0 } else { first as i32 * travel / count };
    (y + ARROW_H as f32 + dy as f32, knob_h as f32, travel)
}

/// The drag step of `FeListScrollBar`'s mouse-move slot 8, **0x4DB9B0** — the position the model
/// is set to (before its own clamp, slot 11 = 0x4BC240) when the pointer is at `py` and the grab
/// was taken at `grab_y` on position `grab_at`:
///
/// ```text
/// 0x4DBA03  travel = [+0x118] − 2·[+0x128] − [+0x108]          ; h − 2·13 − knobH
/// 0x4DBA18  dy     = ev.y − [+0xE4]                            ; the grab's y
/// 0x4DBA24  pos    = [+0xE8] + dy · model->slot9() / travel    ; imul, idiv — truncating
/// 0x4DBA29  model->slot11(pos)
/// ```
///
/// The event's y is an integer, so both ends are taken to the design pixel first — the port's
/// cursor is sub-pixel (APPROXIMATION of the original's integer mouse position). `None` when the
/// travel is zero (the original would divide by zero there; it cannot arise, since the knob is at
/// most `h − 26` tall only when the bar has no travel and then nothing scrolls).
pub fn combo_drag_pos(py: f32, grab_y: f32, grab_at: usize, count: usize, travel: i32) -> Option<i32> {
    if travel == 0 {
        return None;
    }
    let dy = py.floor() as i32 - grab_y.floor() as i32;
    Some(grab_at as i32 + dy * count as i32 / travel)
}

/// The 150 ms auto-repeat's own arithmetic — `0x529220(this, 0x96, 1)`'s tick. Every held
/// scrollbar arrow shares one shape: `NFS5.VerticalSlider`'s own bar (0x4D0A97/0x4D0B40),
/// `NFS5.CARSETTINGBUTTON` (0x4C1F40/0x4C1F80) and the open combo's own `FeListScrollBar`
/// (`0x4DB040`/`0x4DB1D0`, §4.101) all arm the identical repeating timer on press and step once
/// per firing — `porsche::App`'s `list_repeat` / `setting_repeat` / `combo_arrow_repeat` bank the
/// frame's `dt_ms` against the time already carried and take out whole 150 ms periods, exactly
/// the way the timer's own callback would fire once per elapsed period rather than at most once
/// per game tick. Returns `(steps, new accumulator)`.
pub fn repeat_steps(acc_ms: u32, dt_ms: u32, period_ms: u32) -> (u32, u32) {
    let acc = acc_ms + dt_ms;
    (acc / period_ms, acc % period_ms)
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
    /// the offset from the anchor. Its **sign** is the alignment: a negative offset — `-0.0`
    /// included — right-aligns the text there (align 2), any other left-aligns it, so `-0.0` is
    /// "right-aligned at the anchor itself" (the parts basket's `ID_PRICEPOS`, 0x516051)
    pub dx: f32,
    pub text: String,
    /// the level 0x4D87F0 scales the row colour's alpha by — `a · level / 255`, floored (the
    /// `0x80808081` divide by 255 at 0x4D8812…) — 0xFF for the colour as it is (0x4D87F4 returns
    /// it untouched)
    pub fade: u8,
    /// An explicit alignment (0 left, 1 centre, 2 right) where the offset's sign is not what the
    /// row draw passes — `Comms.PlayerList`'s row number is drawn with align 1 at `x − 4`
    /// (0x4BBF7B…0x4BBF91).
    pub align: Option<u8>,
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
    /// them (`drawPartsHeading` 0x518DE0), 13 px tall. The `bool` is whether the section this
    /// heading opens is expanded — meaningless here (category headings are not collapsible), but
    /// `Parts.AllParts`'s **own** two section headers reuse this variant and do read it, to pick
    /// the `+`/`−` icon (`LayoutScreen::parts_glyphs`'s own doc, ctor 0x515160 / row draw
    /// 0x515620).
    Header(String, bool),
    /// The garage list's own grouping heading — two rules **13 px** apart with the name between
    /// them, drawn by `FEOwnedCarAccessor`'s row painter 0x4F5D54…0x4F5DB3 and costing **14** px
    /// of row height. Its height comes out of slot 26 (0x4F60F0): a row whose era differs from
    /// the one above it is **42** — two headings and the row — one whose class differs is **28**,
    /// and any other row is **14**. `RE/notes/controls.md` §4.144.
    GroupHeading(String),
    /// A colour list's colour row — `FEColourAccessor`'s row draw **0x4B9390** (slot 15 of vtable
    /// 0x5B4E60): over the row's bar, a **20-wide swatch** at `(x + 3, y + 2)`, `h − 4` tall
    /// (`0x4D9280(x + inset, y + 2, 20.0, h − 4, 0x4D87F0(c, alpha))`, the inset the combo's
    /// 3.0 at 0x4C688F), outlined in black at the same alpha (0x4D9200 of 0x4D87E0). No text.
    /// The colour is the accessor's `[this+0x28](row)` — the byte-swapped dword of the car
    /// record's entry, `b0` as alpha — or, past the custom row, a saved custom colour.
    Swatch([u8; 4]),
    /// The row **at** the colour count — `FEStrings` **0x263** (`custom...`) at `(x + 3, y + 2)`,
    /// align 0, in **black** whatever the row's own text colour (0x4B945C…0x4B949F).
    CustomColour(String),
    /// `Comms.PlayerList`'s row (`FEPlayerListAccessor` 0x4BBEB0): columns like [`ListRow::Columns`]
    /// plus the player's car colour as a 15 × 10 swatch with a black outline at `(anchor x + dx,
    /// row y)` — `0x4D9280(x + 4, y, 15, 10, 0x4D87F0(colour, alpha))` then the outline
    /// `0x4D9200` of `0x4D87E0(alpha)` (0x4BC0C8…0x4BC116). The swatch is `(anchor, dx, rgba)`.
    Racer(Vec<ListCell>, Option<(String, f32, [u8; 4])>),
}

impl ListRow {
    /// How tall this row is. The base accessor answers 14 for every row (0x4BC700); a parts list's
    /// rows are 13 and its headings another 13 (0x516080).
    pub fn height(&self) -> f32 {
        match self {
            ListRow::Text(_) => LIST_ROW_H,
            ListRow::Columns(_) | ListRow::Racer(..) => LIST_ROW_H,
            ListRow::Header(..) => PART_HEADER_H,
            ListRow::GroupHeading(_) => LIST_ROW_H,
            // `FEColourAccessor` keeps the base slot 26, 0x4BC700
            ListRow::Swatch(_) | ListRow::CustomColour(_) => LIST_ROW_H,
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
    /// the widget's transition alpha, 0…255 — `FeWidget::transitionAlpha` 0x4C8840, which the
    /// widget's slot 25 0x4C0A90 lights its own car by
    pub alpha: i32,
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
/// only when the device's texture memory `[0x619790]` is above 4 000 000 **and** the `FELOWENDCARD`
/// option `[0x657E5C]` is **clear**), and swaps `screenoptionoval` for `screenoptionovallite` on a
/// screen with `canplaygame=1` when `FENOUSELESSART` `[0x657E60]` is set or the machine has under
/// 0x2100000 bytes (33 MiB) of **total physical** memory (0x4E35D0 → 0x53C3D0, `GlobalMemoryStatus`'s
/// `dwTotalPhys`). A port with memory to spare always takes the full table below.
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

/// A front-end colour, as the shader wants it: the byte over 255.
///
/// The original writes these bytes into an 8-bit frame buffer with no colour management at all:
/// a `NFS5.GOTOBUTTON`'s `0xFFBC0000` is red **188** on the player's screen. The renderer is a
/// byte pipeline now (no sRGB surface or textures, `RE/RENDERING.md` "Colour space"), so the byte
/// goes through as it is. (Until 2026-09-24 this undid the sRGB encoding of the surface, which
/// fixed flat colours but left blends and gradients in linear light.)
fn tint(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// `NFS5.MOVIECIRCLE`'s flash frames and its two run fonts (0x4CDE99, 0x4CDED3).
const MOVIE_FLASHES: &str = "circles.fsh";
const RUN_FONTS: [&str; 2] = ["univers7b.ffn", "univers11b.ffn"];
/// The credits roll's body `[+0x21C]` and heading `[+0x220]` fonts.
const ROLL_FONTS: [&str; 2] = ["univers11b.ffn", "univers8b.ffn"];

/// The tag the chronicle's picture tiles carry, which the control's own name repeats at 0x5DBBFE.
const CHRONICLE_PICTURE_TAG: &str = "1111";
/// The shape-cache entry a chronicle picture's composed mosaic lives under. Entry 0 of the same
/// pack is the thumbnail's `bi` icon (`set_pack_picture` keeps it under `(file, 0)`): sharing the
/// key made a picture opened after its own thumbnail draw the 64×36 icon instead of the mosaic
/// (found when the thumbnails could first select a file, 0x5089C0).
const CHRONICLE_MOSAIC_ENTRY: usize = usize::MAX;

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
        // The bottom bar's buttons (0x4C95E4…0x4C95FE) and the word-wrap text object (0x5230E1…
        // 0x523105, the typewriter's leading letter) look up the same entry.
        let glint = layout.elements.iter().any(|e| matches!(e.class.as_str(), "NFS5.HEADER" | "NFS5.CarLogo" | "NFS5.BOTTOMBARBUTTON") || e.class.to_ascii_lowercase().contains("wordwrap")).then(|| {
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
        // `Parts.AllParts`'s two section headers (`КУПИТЬ ЗАПЧАСТИ ДЛЯ …`/`ПРОДАТЬ ЗАПЧАСТИ`) draw
        // a `+`/`−` collapse icon from the same `weeicons.fsh` the ctor **0x515160** caches by
        // index (`pls1`=3, `min4`=0xB, `pls2`=4, `pls3`=5 — see the field doc above)
        let parts_glyphs = layout.elements.iter().any(|e| e.get("accessor").is_some_and(|a| a.starts_with("Parts.AllParts"))).then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let at = |tag: &str| pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(tag));
            let four = [at("pls1")?, at("min4")?, at("pls2")?, at("pls3")?];
            for n in four {
                wanted.push(("weeicons.fsh".to_string(), n));
            }
            Some(four)
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
        // (`NFS5.ANIMSHOWCASEPICTURE`'s runs load the same pair, 0x4BDD48 / 0x4BDD5E)
        if layout.elements.iter().any(|e| e.class == "NFS5.ANIMSHOWCASEPICTURE") {
            want.push(RUN_FONTS[0].to_string());
            want.push(RUN_FONTS[1].to_string());
        }
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
        // The timeline's caption is smaller than its year header — `Univers7b.ffn` against the
        // year's own `Univers8b.ffn` — read off `FEChronicleMainBoxControl`'s bind, 0x507F10:
        // `[this+0x158]` ("Univers8b") and `[this+0x15c]` ("Univers7b") are two distinct fonts,
        // not one reused (§4.173). Nothing in the `.lay`/`.ctr` files names the caption's font, so
        // without this rule it is never loaded and the caption never draws.
        if layout.elements.iter().any(|e| e.class == "NFS5.ChronicleMainBoxControl") {
            want.push(PICKER_TITLE_FONT.to_string());
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
        let screen = Self { layout, background: None, shapes, fonts, white, cursor_ms: 0, cursor_shape_at: std::cell::Cell::new((usize::MAX, 0)), cursor_btn_at: std::cell::Cell::new((false, 0)), cursor_btn: false, car_spin: 0.0, car_parts: HashMap::new(), car_roof: Default::default(), covered: false, combo_hover: std::cell::Cell::new(None), overlay_mask: std::cell::Cell::new(None), unrendered, logos: HashMap::new(), checks: HashMap::new(), knobs: HashMap::new(), closed: std::collections::HashSet::new(), groups: HashMap::new(), bars: HashMap::new(), lists: HashMap::new(), swatches: HashMap::new(), car_views: HashMap::new(), track_maps: HashMap::new(), movies: HashMap::new(), ko_strip: HashMap::new(), badges: HashMap::new(),
            // `NFS_FE_CLOCK` preloads the screen clock in milliseconds, so a still can be taken
            // part way through an animation the front end would otherwise only show in motion
            clock_ms: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(0), glint, colb, arcs, parts_glyphs,
            movie_overlays: Default::default(),
            credits_roll: Default::default(),
            wrap_watch: Default::default(),
            typing: Default::default(),
            pending_tab: None,
            inherited: std::cell::Cell::new((false, 0)),
            glints: Default::default(),
            preload_ticks: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse::<u64>().ok()).map_or(0, |ms| (ms * 128 / 1000) as i64),
            watch_text: Default::default(),
            last_drawn_state: Default::default(),
            end_of_era: None,
            career_tabs: None,
            bar_oval: None,
            cheat_bits: 0,
            driver_picture: crate::save::DEFAULT_PICTURE as i32,
            // `NFS_FE_CURSOR=x,y` parks the pointer for a still, the way `NFS_FE_CLOCK` parks the
            // clock — a screenshot has no pointer of its own
            cursor: std::env::var("NFS_FE_CURSOR").ok().and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            }),
            pointer: None,
            // `NFS_FE_PRESS=1` holds the button down for a still, the pair of `NFS_FE_CURSOR`
            pressed: std::env::var("NFS_FE_PRESS").ok().is_some_and(|v| v != "0"),
            capture: None,
            // `NFS_FE_OPEN=<index>` drops a combo's list open for a still
            open_combo: std::env::var("NFS_FE_OPEN").ok().and_then(|v| v.parse().ok()),
            // `NFS_FE_PICKER=1` with `NFS_FE_OPEN` raises the latch for a still of the editor
            picker_latch: std::env::var("NFS_FE_PICKER").is_ok_and(|v| v != "0"),
            picker: Picker::default(),
            pads: Vec::new(),
            pad_sel: 0,
            deadzone: Vec::new(),
            dz_grab: None,
            dz_hover: None,
            camera_current: 0,
            camera_list: [0, 1, 2, 3],
            combo_scroll: None,
            focused: Default::default(),
            edits: HashMap::new(),
            selected_flags: Default::default(),
            unselected: Default::default(),
            hidden_flags: Default::default(),
            disabled_flags: Default::default(),
            picture_pan: Default::default(),
            picture_drag: None,
            chronicle_columns: Vec::new(),
            chronicle_icons: (String::new(), Default::default()),
            chronicle_captions: Vec::new(),
            combo_arrows,
            knob_at: None,
            scroll: HashMap::new(),
            hover_ms: 0,
            hover_of: None,
            controls, fe_strings, transition: Transition::Idle, overlay: false };
        screen.activate_glints();
        Ok(screen)
    }

    /// The activates that draw from `rand` — `NFS5.HEADER` (0x4CAE0D), `NFS5.CarLogo`
    /// (0x4C1409), `NFS5.GLINTYLINE` (0x5223F0 → 0x51F9A0), `NFS5.AnimatingPicture`'s bind
    /// (0x4BD8C7) and `NFS5.BOTTOMBARBUTTON`'s own sweep bind (0x4C9616, `widgets.rs`'s
    /// `draw_bar_or_graphic_tab`) — each stamping the screen's open, tick 0 of its clock. They are
    /// made in the widgets' own order: slot 21 is the **`INUSE`** handler — `FEControl`'s slot 12
    /// 0x4C7760 registers 0x4C7B50 (`call [vtbl+0x54]`) under `"INUSE"` through the message-map
    /// insert 0x526000 (0x4C7916…0x4C792A), and the screen's slot 38 0x4E0C60 broadcasts `INUSE`
    /// to its children in the child vector's order (read 2026-09-27) — which for the bottom bar's
    /// own four buttons is *this* screen (the bar loads and binds them, `bar_oval.rs`'s marker,
    /// before the front end's first layout). The flourish objects draw nothing when they are built
    /// and are made on their first draw (`glinty_flourish`).
    fn activate_glints(&self) {
        let mut g = self.glints.borrow_mut();
        for (i, e) in self.layout.elements.iter().enumerate() {
            let made = match e.class.as_str() {
                "NFS5.HEADER" => glints::Glint::Sweep(glints::Sweep::new(0, false)),
                "NFS5.CarLogo" | "NFS5.BOTTOMBARBUTTON" => glints::Glint::Sweep(glints::Sweep::new(0, true)),
                "NFS5.GLINTYLINE" => {
                    // 0x51F960 parks the widget's box: with `horizontal` set the bar runs top…bottom
                    // and the sweep left…right, and the other way round without it
                    let horizontal = self.attr_num(e, "horizontal").unwrap_or(1.0) != 0.0;
                    let (w, h) = self.box_of(e, 0);
                    let (short, long) = if horizontal { (h, w) } else { (w, h) };
                    glints::Glint::Line(glints::Line::new(0, short as i32, long as i64))
                }
                "NFS5.AnimatingPicture" => {
                    let lo = e.int("mindelay").unwrap_or(0) as i64;
                    let hi = e.int("maxdelay").unwrap_or(0) as i64;
                    glints::Glint::Anim(glints::Anim::new(0, lo, hi))
                }
                _ => continue,
            };
            g.insert(i, (made, false));
        }
    }

    /// Step widget `i`'s glint to tick `now`. A preloaded clock (`NFS_FE_CLOCK`) has no frames
    /// behind it, so the first step first runs the ticks it skipped, one a tick, the screen at
    /// rest — the diagnostic's stand-in; in a game the first draw is at tick 0…2.
    fn step_glint<T>(&self, i: usize, now: i64, make: impl FnOnce() -> glints::Glint, mut step: impl FnMut(&mut glints::Glint, i64, bool) -> T) -> T {
        let mut all = self.glints.borrow_mut();
        let (g, seen) = all.entry(i).or_insert_with(|| (make(), false));
        if !*seen {
            *seen = true;
            for t in 1..self.preload_ticks.min(now) {
                step(g, t, true);
            }
        }
        step(g, now, false)
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

    /// One name of a `.ctr` expression, answered the way the macro dictionary `[0x69CAF8]`
    /// answers it: with the **text** a part published, which the evaluator splices in and expands
    /// again (`nfs_formats::expr`). What is in the dictionary:
    ///
    /// * **`<part>.<attr>`** for every attribute of every part — `Attribute::slot3` 0x52C140
    ///   defines it as the attribute's string `[attr+0x10]` (0x52C1E3), which is the `.lay`
    ///   block's value for the attribute's screen key when it may be overridden (0x52C000) and
    ///   the template's `<attr>.value` otherwise. The `DATA` part is called `data`, so this is
    ///   `data.width` too — and only for an attribute its `attributes.value` **declares**;
    /// * a **point** attribute (`pos`, `size`) instead defines `<part>.<attr>.x` and `.width` as
    ///   the text before its first comma and `.y` and `.height` as all of the text after it, and
    ///   **nothing** — not even the bare name — when the text has no comma (`PointAttribute::slot3`
    ///   0x5277F0: `strchr` at 0x527814, `je 0x527B68`; the four `0x52DC20` at 0x52794F,
    ///   0x527A81, 0x527AE4, 0x527B43).
    ///
    /// A name that is not there is [`Macro::Undefined`](nfs_formats::ctr::Macro::Undefined) and
    /// reads 0: `BottomBarButton.ctr`'s `data.area`, `ChatDisplayBox.ctr`'s `backrect.*`,
    /// `Tab Button.ctr`'s `data.height`.
    ///
    /// Three answers are the port's own, as before: `control.size.*` is the widget's box as this
    /// port computes it (which prefix the original gives the control's own attributes was not
    /// traced); a `SHAPE` part's `size` the template leaves empty is its art's (the pack loader's
    /// 0x5276E0 at 0x4E6EE2) — and a **text** part's `size` is always its text's truncated measure,
    /// which its load 0x4E7160 writes over the attribute and the next update publishes
    /// (`part_extent`, `part_text`; CONFIRMED 2026-09-26, correcting "no code publishes a
    /// measured size"); and a name the port
    /// cannot answer — that extent when it cannot measure, a part attribute the template leaves
    /// empty whose constructor default the port does not keep, a chain deeper than five — is
    /// [`Macro::Unanswerable`](nfs_formats::ctr::Macro::Unanswerable), so the caller keeps its
    /// fallback. Names are matched without regard to case; the original's dictionary is
    /// case-sensitive (`std::map` over `memcmp`, 0x4242C0), and every reference in the 72
    /// shipped templates matches its definition exactly, so the two agree on the data.
    fn reference(&self, e: &nfs_formats::lay::Element, name: &str, depth: u32) -> nfs_formats::ctr::Macro {
        use nfs_formats::ctr::Macro;
        let name = name.to_lowercase();
        // every identifier of the text comes here, `x1f` of a `0x1f` included
        let Some((head, rest)) = name.split_once('.') else { return Macro::Undefined };
        if head == "control" {
            if depth > 4 {
                return Macro::Unanswerable;
            }
            let (bw, bh) = self.box_of(e, depth + 1);
            return match rest {
                "size.width" => Macro::Text((bw as i32).to_string()),
                "size.height" => Macro::Text((bh as i32).to_string()),
                _ => Macro::Unanswerable,
            };
        }
        let Some(c) = self.controls.get(&e.class) else { return Macro::Unanswerable };
        let Some(part) = c.part(head) else { return Macro::Undefined };
        let point = |key: &str, field: &str, text: &str| -> Macro {
            let Some((x, y)) = text.split_once(',') else { return Macro::Undefined };
            match field {
                "x" | "width" => Macro::Text(x.to_string()),
                "y" | "height" => Macro::Text(y.to_string()),
                _ => {
                    let _ = key;
                    Macro::Undefined
                }
            }
        };
        if part.kind() == nfs_formats::ctr::PartKind::Data {
            // `data.<attr>`: declared attributes only — every shipped one is an `INT`, a `COLOR`
            // or a `STRING`, none a point, so a dotted field after it names nothing
            if rest.contains('.') || !part.attributes.iter().any(|a| a == rest) {
                return Macro::Undefined;
            }
            return match self.part_attr(e, part, rest) {
                Some(t) => Macro::Text(t.to_string()),
                // declared with no value anywhere: the attribute's constructor string — `"%d"` of
                // 0 for an INT (0x52B744 → 0x527140 → 0x5242E0), taken as empty for the others
                None if part.ty(rest).is_some_and(|t| t.trim().eq_ignore_ascii_case("INT")) => Macro::Text("0".into()),
                None => Macro::Text(String::new()),
            };
        }
        let (key, field) = match rest.split_once('.') {
            Some((k, f)) => (k, Some(f)),
            None => (rest, None),
        };
        let is_point = key == "pos" || key == "size";
        let text_part = matches!(part.kind(), nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText);
        match (field, is_point) {
            // a text part's `text` is what it holds — for `LOCALIZEDTEXT` the string its load
            // wrote from `stringID` (0x4E66E0 → `[+0x170]`, the string `Attribute::slot3`
            // publishes)
            (None, false) if text_part && key == "text" => self.part_text(e, part, depth + 1).map_or(Macro::Unanswerable, Macro::Text),
            // …and its `size` is always that text's truncated measure: 0x4E7160 writes it over
            // whatever the template or the block gave (0x4E72A0), and the update 0x5256F0
            // publishes it (`Part::slot3`, 0x5257CD) before the parts are evaluated again — which
            // is why every tick that changes a watch's text runs the update **twice** (0x4BD349,
            // 0x4BD350), and the input box's bind too (0x4CC577 / 0x4CC57E)
            (Some(f), true) if text_part && key == "size" => match (depth <= 4).then(|| self.part_extent(e, part, depth)).flatten() {
                Some((w, h)) => point(key, f, &format!("{},{}", w as i32, h as i32)),
                None => Macro::Unanswerable,
            },
            (None, false) => self.part_attr(e, part, key).map_or(Macro::Unanswerable, |t| Macro::Text(t.to_string())),
            (Some(f), true) => match self.part_attr(e, part, key) {
                Some(t) => point(key, f, t),
                // A drawable part's `size` is the extent of **what it draws** when the template
                // does not give it one: `Picture.ctr`'s picture part and `BottomBarButton.ctr`'s
                // label both carry `size.flags=HIDDEN|READONLY` and no `size.value` at all, and
                // the bar button's own position is written over `label.size.width +
                // picture.size.width`. A measured extent is only for an explicit
                // `<part>.size.*` reference, never for the widget's own box: `Label.ctr`'s
                // `size.value` is `label.size.width*2,…`, and letting that resolve would give
                // every label a box twice its text and move it, because this port implements the
                // alignment rule as an anchor in the box (`layouts.md` §2.1).
                None if key == "size" && depth <= 4 => match self.part_extent(e, part, depth) {
                    Some((w, h)) => point(key, f, &format!("{},{}", w as i32, h as i32)),
                    None => Macro::Unanswerable,
                },
                None => Macro::Unanswerable,
            },
            _ => Macro::Undefined,
        }
    }

    /// Every part expression on this screen that does **not** resolve, as
    /// `(class, part, key, expression)`.
    ///
    /// `reference` answers `Unanswerable` for a name the port cannot resolve and every caller
    /// falls back to a zero or to the widget's box, so a template the port cannot evaluate is
    /// silent. (A name the original does not define is not one of these: it reads 0.) This is the
    /// measurement of that: `--layout-sweep` with `NFS_EXPR_SWEEP=1` runs it over all 98 layouts.
    pub fn expr_failures(&self) -> Vec<(String, String, String, String)> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter() {
            let Some(c) = self.controls.get(&e.class) else { continue };
            let state = e.get("state").unwrap_or("normal");
            for name in c.state_parts(state) {
                let Some(p) = c.part(&name) else { continue };
                for key in ["pos", "size"] {
                    // a value with no comma is never evaluated (0x527B80), so it cannot fail
                    let Some(v) = self.part_attr(e, p, key).filter(|v| v.contains(',')) else { continue };
                    if nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)).is_none() {
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

    /// Tell the screen the four `Camera.1 … Camera.4` values — what `NFS5.CAMERAPICTURE`'s draw
    /// 0x4BF870 tests each picture against (§4.89).
    pub fn set_camera_list(&mut self, list: [i32; 4]) {
        self.camera_list = list;
        for g in self.groups.values_mut() {
            g.set_camera_list(list);
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
    fn part_extent(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, depth: u32) -> Option<(f64, f64)> {
        match p.kind() {
            nfs_formats::ctr::PartKind::Shape => {
                // the same resolution `shape_parts` uses — the part attribute's screen key against
                // the block, else the template's own value. `Graphic Tab Button.ctr`'s
                // `selectedbg` has an **empty** `shapePathName.value` and only the screen key
                // `selectedbgSmallPathName` reaches its pack, so without this the four unopened
                // tabs could not measure the art they centre their captions over.
                let file = self.part_attr(e, p, "shapePathName").filter(|v| !v.is_empty())?;
                let index = match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => nfs_formats::ctr::eval(v, &|n| self.reference(e, n, depth + 1)).filter(|n| *n >= 0)? as usize,
                    None => 0,
                };
                let &(_, w, h) = self.shapes.get(&(file.to_string(), index))?;
                Some((w as f64, h as f64))
            }
            nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText => {
                // `FETextPart`'s load **0x4E7160**, after the base update 0x52B250 → 0x52A4F0 has
                // evaluated every attribute: the font `fontPathName` (`[+0xDC]`, rebuilt by
                // 0x4E37D0 only when the name changed), the part's text `[+0x19C]` measured by
                // 0x536970 (`BitmapFont::measure`: its third and fourth outputs, 0x4E727C /
                // 0x4E7285), both **truncated** (`_ftol` 0x5A0F98) and written over the part's
                // own `size` with the `"%d,%d"` setter 0x5276E0 (0x4E72A0). Whatever the template
                // says, a text part's `size` is the measure of the string it holds.
                let text = self.part_text(e, p, depth)?;
                let named = e
                    .get("fontPathName")
                    .filter(|f| !f.is_empty())
                    .or_else(|| p.value("fontPathName").filter(|f| !f.is_empty()))
                    .map(|f| f.to_lowercase());
                let f = named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))?;
                let (w, h) = f.measure(&text);
                Some((w.trunc() as f64, h.trunc() as f64))
            }
            _ => None,
        }
    }

    /// The string a `TEXT` / `LOCALIZEDTEXT` part holds, `[part+0x19C]`: the `text` attribute's
    /// value after `StringAttribute::slot4` **0x526B60** has spliced each `%name%` with that
    /// macro's text (0x52DCE0; a name the dictionary does not hold stays as written). `None` when
    /// a name in it is one the port cannot answer.
    ///
    /// * `LOCALIZEDTEXT`'s load **0x4E66E0** first writes the text from its `stringID`: `atoi`
    ///   (0x5A1F4A) of the attribute's value, then **0x4E06D0** — id 0 is the empty string
    ///   0x5E8E50, an id no table holds is `"Not Found"` (0x5D9B38). 0x4E06D0 never answers null,
    ///   so 0x4E66E0's `"stringID = "` arm (0x4E6772) is dead code.
    /// * `NFS5.AccessorWatch`'s tick writes the accessor's answer into `label.text` (0x4BD32C);
    ///   the port keeps what its draw last showed (`watch_text`), and before that the part holds
    ///   the template's `WATCH`.
    /// * a `TEXT` part whose template gives no text keeps its constructor's `"Hello, World!"`
    ///   (0x5DDB84, pushed at 0x52B036).
    fn part_text(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, depth: u32) -> Option<String> {
        use nfs_formats::ctr::PartKind;
        if depth > 4 {
            return None;
        }
        // 0x526B60's loop, read to the instruction 2026-09-26: the pair finder 0x526DF0 takes the
        // **first** `%` of the whole string at `a` and gives up — ending the loop — when `a + 2`
        // is not inside the string or `s[a+1]` is `%` (0x526E2D…0x526E3A); the closing `%` is the
        // first from `a + 2` (0x526E3C…). The name between is expanded (0x52DCE0) and **every**
        // `%name%` replaced with it by 0x526E80, which searches from the string's start after each
        // replacement (0x526F85…0x526FC4, 0x4E1920 from `[esi]`); then the search starts over
        // (0x526D4D). After the loop 0x526E80 turns every `%%` (0x5DD9E0) into `%` (0x5CCC90), the
        // same way (0x526D62…0x526DB6).
        let replace_all = |s: &mut String, pat: &str, with: &str| {
            // a replacement that brings its own pattern back would spin the original forever;
            // the port stops (guard of its own)
            for _ in 0..256 {
                let Some(k) = s.find(pat) else { break };
                s.replace_range(k..k + pat.len(), with);
            }
        };
        let splice = |v: &str| -> Option<String> {
            let mut s = v.to_string();
            for _ in 0..256 {
                let Some(a) = s.find('%') else { break };
                if a + 2 >= s.len() || s.as_bytes()[a + 1] == b'%' {
                    break;
                }
                let Some(b) = s[a + 2..].find('%').map(|k| k + a + 2) else { break };
                let name = s[a + 1..b].to_string();
                let x = nfs_formats::expr::expand(&name, &|n| self.reference(e, n, depth + 1));
                if x.contains(nfs_formats::expr::UNKNOWN) {
                    return None;
                }
                replace_all(&mut s, &format!("%{name}%"), &x);
            }
            replace_all(&mut s, "%%", "%");
            Some(s)
        };
        if p.kind() == PartKind::LocalizedText {
            let raw = splice(self.part_attr(e, p, "stringID").unwrap_or(""))?;
            // `atoi`: leading white space, a sign, then the digits
            let t = raw.trim_start();
            let (neg, t) = match t.as_bytes().first() {
                Some(b'-') => (true, &t[1..]),
                Some(b'+') => (false, &t[1..]),
                _ => (false, t),
            };
            let n = t.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, c| a.wrapping_mul(10).wrapping_add((c - b'0') as i32));
            let id = if neg { n.wrapping_neg() } else { n };
            if id == 0 {
                return Some(String::new());
            }
            return Some(self.fe_strings.get(id as u32).unwrap_or("Not Found").to_string());
        }
        if e.class.eq_ignore_ascii_case("NFS5.AccessorWatch")
            && let Some(i) = self.layout.elements.iter().position(|x| std::ptr::eq(x, e))
            && let Some(t) = self.watch_text.borrow().get(&i)
        {
            return Some(t.clone());
        }
        match self.part_attr(e, p, "text") {
            Some(v) => splice(v),
            None => Some("Hello, World!".to_string()),
        }
    }

    /// The widget's box. `Control::computeBox` 0x5253C0, re-read to the instruction 2026-09-30:
    /// `eax = [this+0xF0]` (the class template's bound `autosize` attribute object) then
    /// `ecx = [eax+0x38]` (its `value`, `[[this+0xF0]+0x38]`) — when that is nonzero
    /// (0x52540a…0x525516) the box is **unconditionally** the union of the parts the current
    /// state draws (0x5253E9…0x5254FF, zeroed then grown from the origin, exactly
    /// [`parts_union`](Self::parts_union)); the `.lay` block's own `size=`, if it carries one,
    /// is never read on that branch — not `[this+0xE4]` (the size attribute) nor anything
    /// derived from it. Only when autosize is **0** (0x525501: eight of the seventy-two
    /// templates that declare the key, §4.62) does the box come from `[this+0xE4]`'s pair —
    /// the `.lay`'s `size=` if it gives one, else the template's own `size.value` expression
    /// (`data.width,data.height` for a button, `data.width,data.selectionboxmaxheight` for a
    /// list, …).
    ///
    /// **Until 2026-09-30 this only took the union when the `.lay` block carried no `size=` of
    /// its own** — an extra guard the disassembly does not have, added when `parts_union` was
    /// first ported and never audited against every autosizing class. `NFS_BOX_SWEEP=1`
    /// measured the guard's effect over all 98 screens once it was lifted (`box_disagreements`,
    /// below, now audits every autosizing widget rather than only the no-`size=` subset): 875
    /// widget-instances across 16 classes differ from the guarded answer, `NFS5.PICTURE` (512)
    /// and `NFS5.AccessorWatch` (277) by far the largest groups. Two shapes turned up:
    ///
    /// * **The box changes and nothing reads it.** `NFS5.PICTURE`/`NFS5.NonLocalizedLabel`/
    ///   `NFS5.ExtraCircle` (`draw_template_classes`) and a matched `NFS5.AccessorWatch`
    ///   (`draw_default`'s `watch_pos`, `NFS5.POPUPCIRCLE`'s and `NFS5.PLAYERDRIVERPICTURE`'s
    ///   picture-only arms) draw every part at that part's **own** `pos`/`size`, never the
    ///   widget's outer box — `box_of` only matters to these for hit-testing, which
    ///   [`box_hit`](Self::box_hit) already implemented the *un*guarded rule for independently
    ///   (§4.62), so lifting the guard here changes nothing they draw.
    /// * **The box changes and something did read it, wrongly** — `NFS5.CarLogo`
    ///   (`draw_car_logo`) and `NFS5.ACCOUNTTHING` (`draw_account_thing`) both centred/sized
    ///   against the guarded box and were patched locally (33d21f13, 2c864da4) rather than here,
    ///   for exactly the reason this comment used to give: touching `box_of` looked like it would
    ///   touch every other autosizing class too. Both local patches are redundant now and are
    ///   removed — they read `d.w`/`d.h`, which is this function's own answer.
    ///
    /// **One more shape, not in the 875**: `NFS5.WORDWRAPCONTROL`/`NFS5.WORDWRAPACCESSORWATCH`
    /// (wrap width, `draw_default`) and `NFS5.MOVIEBOX`/`NFS5.MOVIECIRCLE` (movie blit size,
    /// `draw_movie`) also autosize, but their own draws were **already** reading something other
    /// than `box_of`'s depth-0 answer for their size — the wrap engine measures the `area` part
    /// (`size.value=control.size.width,control.size.height`, the attribute, §4.64) and the movie
    /// player's box is read straight off the widget's own `size` attribute at bind time
    /// (0x4CD034). Both keep reading the attribute (`box_of(e, 1)`, depth > 0) rather than this
    /// function's union, which is right for them but would have been very wrong for a long
    /// paragraph's `label` part (measured unwrapped, hundreds of pixels wide) had they read `w`.
    ///
    /// **Only at depth 0**, which is to say only for the widget's own box and never for a
    /// `control.size.*` term inside a part's expression. §4.64: those two are not the same
    /// thing in the original either — 31 of the templates that reference `control.size.width`
    /// autosize, and if that term went through the box the union would re-enter itself before
    /// `computeBox` sets its valid flag (0x525516) and never come back. The term is the `size`
    /// attribute; the box is the union.
    fn box_of(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        if depth == 0
            && self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false)
            && let Some(u) = self.parts_union(e)
        {
            return u;
        }
        self.box_sized(e, depth)
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
        self.box_sized(e, 0)
    }

    /// Every **autosizing** widget whose box disagrees with the union of its own parts —
    /// `box_of` now returns that union unconditionally at depth 0 (§4.62's rule, re-read
    /// 2026-09-30), so since this walks every autosizing widget rather than only the ones a
    /// `.lay` block leaves `size=`-less, it should always come back empty; a non-empty result
    /// means `box_of` and `parts_union` have drifted apart again. `NFS_BOX_SWEEP=1` prints it.
    pub fn box_disagreements(&self) -> Vec<(String, (f32, f32), (f32, f32))> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter().skip(1) {
            let autosizes = self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false);
            if !autosizes {
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
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (px, py) = at("pos").unwrap_or((0, 0));
            // a text part's size is its text's measure whatever the template says (0x4E72A0)
            let text_part = matches!(p.kind(), nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText);
            let (pw, ph) = match at("size").filter(|_| !text_part) {
                Some((w, h)) => (w as f64, h as f64),
                None => self.part_extent(e, p, 0).unwrap_or((0.0, 0.0)),
            };
            l = l.min(px as f32);
            t = t.min(py as f32);
            r = r.max(px as f32 + pw as f32);
            b = b.max(py as f32 + ph as f32);
            any = true;
        }
        (any && r - l >= 1.0 && b - t >= 1.0).then_some((r - l, b - t))
    }

    fn box_sized(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
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
            let h = self.box_template(e, depth).map(|(_, h)| h).unwrap_or(0.0);
            return (given[0], h);
        }
        // **A `size=` key present but carrying no token at all is (0, 0), not the template's
        // default.** The one shipped instance is `carcompare.lay` §14, a stray duplicate of
        // §11/12's hairline divider (same `pos=322,~370`, same `color=0x80184258`) whose `size=`
        // line was left blank — the only blank `size=` in any of the 98 shipped `.lay` files
        // (checked by grep over all of them). **Why (0, 0), read 2026-09-30 (the note of
        // 2026-09-29 that put this on an "unconditional" pair parser 0x527BC0 was wrong):** the
        // pair parser is `PointAttribute::slot4` **0x527B80** (vtable 0x5BB0AC + 0x10) and it
        // cuts the value at its first comma (`strchr` 0x527BA9) — **no comma, nothing is
        // evaluated** (`je 0x527CAD` at 0x527BB7), so a blank `size=` leaves the attribute what
        // its constructor **0x527590** made it: the two dwords `[+0x38]`, `[+0x3C]` copied from the
        // default the control's constructor 0x5246E0 passes for `size` (0x5247FB), a point whose
        // every stack cell that constructor zeroes (0x5247AF…0x5247E1) — (0, 0). The template's
        // own `size.value` is never evaluated in its place, because the attribute's string is the
        // `.lay` value (0x52C000) and that is the empty one. This agrees with the reference
        // capture (`RE/reference/2026-09-28/pass6_carcompare.jpg`: no rectangle behind the
        // "PORSCHE" name graphic, where this port drew a 50×50 teal-grey box before this fix,
        // `crop_carcompare_badge_port.png`). The *evaluator's* answer for an empty half of a
        // comma pair (`size=150,`) is a different path and also 0: 0x52F84A…0x52F880 → the operand
        // reader 0x5309E0 → 0x530F0A → 0x530F2F, which only reports error 5 (0x531C30) and returns
        // `xor eax, eax` (0x530F41). CONFIRMED. `layouts.md` "carcompare, read 2026-09-29".
        if given.is_empty() && e.get("size").is_some_and(|v| v.trim().is_empty()) {
            return (0.0, 0.0);
        }
        if let Some(p) = self.box_template(e, depth) {
            return p;
        }
        (e.float("width").unwrap_or(0.0), e.float("height").unwrap_or(0.0))
    }

    /// The class template's own `size.value`, evaluated.
    fn box_template(&self, e: &nfs_formats::lay::Element, depth: u32) -> Option<(f32, f32)> {
        if depth > 4 {
            return None;
        }
        let c = self.controls.get(&e.class)?;
        let v = c.value("size")?;
        nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1)).map(|(w, h)| (w as f32, h as f32))
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
        g.inherited.set(self.widget_state());
        let mut inner = g.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new());
        if x != 0.0 || y != 0.0 {
            let design_aspect = DESIGN.0 / DESIGN.1;
            let fw = if aspect > design_aspect { design_aspect / aspect } else { 1.0 };
            let (dx, dy) = (x / DESIGN.0 * fw, y / DESIGN.1);
            for sp in &mut inner {
                sp.pos[0] += dx;
                sp.pos[1] += dy;
                // a per-corner triangle carries its own points
                if let Some((t, _)) = sp.tri.as_mut() {
                    for v in t {
                        *v = [v[0] + dx, v[1] + dy];
                    }
                }
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
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
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
        // a 32-bit pattern: `0xffffffff` is −1 to the evaluator and every bit of it the colour
        let n = nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0))? as u32;
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
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
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
        // the art of the state the widget is in **now**, like its boxes and its text: the draw
        // 0x4C8260 ends in 0x525830, which walks the part list of the current state
        // (`[[this+0xD4]] + 0x525570()·4`, 0x525876…0x525886) — the index `updateState` 0x4C82A0
        // sets through 0x525580 — not the `.lay` block's `state=`
        for (px, py, file, index, flip) in self.shape_parts_at(e, state, frame) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (pos, size) = self.place(aspect, x + px, y + py, aw, ah);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
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
                .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)))
                .unwrap_or((0, 0));
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

    /// The `SHAPE` parts of the widget's class template that `state` draws, each resolved to a
    /// place and a `.fsh` pack entry (`RE/notes/controls.md` §1), with the `picture` part's entry
    /// replaced by `frame` — what `NFS5.BOTTOMBARBUTTON`'s tick does every frame through
    /// `0x5271F0([this+0x17C], frame)` (§4.53).
    ///
    /// This is the other half of [`rect_parts_in`](Self::rect_parts_in): a class whose art is entirely in
    /// its template draws through here without the port knowing anything about it.
    /// `NFS5.CARSETTINGBUTTON` is the plainest case — `CarSettingButton.ctr` gives its `picnonsel`
    /// part `shapePathName=weeicons.fsh` and `shapeIndex=data.nonselindex`, so the widget's own
    /// `nonselindex` picks the arrow. The same shape covers `NFS5.POPUPCIRCLE`,
    /// `NFS5.ExtraCircle`, the tab buttons and the rest of the art-from-template classes.
    ///
    /// `state` is the **current** one for every draw that ends in 0x525830 (§2026-09-27 in
    /// `layouts.md`); there is no caller left that takes the `.lay` block's `state=` (the old
    /// `shape_parts`, retired 2026-09-27).
    fn shape_parts_at(&self, e: &nfs_formats::lay::Element, state: &str, frame: Option<usize>) -> Vec<(f32, f32, String, usize, i64)> {
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
        self.shape_parts_in(e, c.state_parts(state), frame)
    }

    /// The `SHAPE` parts among `names`, in that order, whatever state the widget is in — for a
    /// class whose draw holds its parts by pointer instead of walking a state's list (the movie
    /// circle's nine stencil parts, 0x4CDB2C…0x4CDB57).
    fn shape_parts_in(&self, e: &nfs_formats::lay::Element, names: &[String], frame: Option<usize>) -> Vec<(f32, f32, String, usize, i64)> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for (name, p) in names.iter().filter_map(|n| c.part(n).map(|p| (n.clone(), p))) {
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
                    Some(v) => match nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0)) {
                        Some(n) if n >= 0 => n as usize,
                        _ => continue,
                    },
                    None => 0,
                },
            };
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (px, py) = at("pos").unwrap_or((0, 0));
            let (px, py) = (px as f64, py as f64);
            // No size: a `SHAPE` part is always drawn at its **art's** size. `FEShapePart`'s load
            // (vtable 0x5B6E64 slot 8, **0x4E6AA0**) fetches the entry (0x4D7A10 at 0x4E6EBE) and
            // writes its width and height (`+0x14`, `+0x18`) over the part's `size` attribute
            // (0x5276E0 on `[part+0x60]` at 0x4E6EE2), and the draw 0x4E6F60 hands the entry to
            // 0x4D9AF0, which reads the same two fields for the quad's extent (0x4D9B04, 0x4D9B0E).
            // The only `SHAPE` parts that write a `size.value` are `CarSettingSlider.ctr`'s three
            // knobs, `6,11` — which is the art (`weeicons.fsh` 8, 34, 35 are 6×11).
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
                .and_then(|v| nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0)))
                .filter(|n| *n >= 0)
                .map(|n| n as i64)
                .unwrap_or_else(|| e.int("flip").unwrap_or(0).max(0));
            out.push((px as f32, py as f32, file.to_string(), index, flip));
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
            tri: None, corner_colors: None,
            depth: None,
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
        // and every panel open on it: the strip's selected button runs its slot 27, 0x4D02E0,
        // whose show 0x4CEC60 starts the sub-screen in (its slot 0x78, 0x4E09A0) as the screen
        // opens (MEDIUM on the frame: the call that runs it at the open was not traced)
        let panels: Vec<usize> = self.panel_groups().filter(|i| !self.closed.contains(i)).collect();
        for i in panels {
            if let Some(g) = self.groups.get_mut(&i) {
                g.transition_in();
            }
        }
    }

    /// The groups that are `NFS5.PANEL`s (the rest are tab strips).
    fn panel_groups(&self) -> impl Iterator<Item = usize> + '_ {
        self.groups.keys().copied().filter(|i| self.layout.elements.get(*i).is_some_and(|e| e.class == "NFS5.PANEL"))
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

    /// Hand a top-level screen the **first layer's** screen transition, which its widgets fall
    /// back on while their own screen is up: 0x4C8760 / 0x4C8840 read the widget's screen state
    /// (0x4E0E00 on `[this+0x114]`) and, when it is 2, **0x4D63C0** — 0x4E0E00 of `[[[fe+0xD8]]]`,
    /// the screen on show of layer 0 (2 when there is none); the progress is 0x4E0D90(1), the
    /// larger (`jg`, signed) of the own `+0x130` and **0x4D6300** — `0x4E0D90(0)`, the bare
    /// `+0x130`, of that same screen (0 when there is none). So the bottom bar (whose own
    /// transition is up almost always) and the screens of the layers above the first slide and
    /// fade with the first layer's screen. `front` is that screen's own transition, not what it
    /// inherits in turn (0x4D6312 pushes 0).
    pub fn take_front_transition(&self, front: Transition) {
        self.inherited.set((matches!(front, Transition::Out { .. }), front.progress()));
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
        let own = self.transition;
        self.tick_panels(dt_ms, matches!(own, Transition::Out { .. }));
        self.transition.done()
    }

    /// The screen's children that are sub-screens, ticked in the widgets' order (0x4E0950 walks
    /// the child vector from 0):
    ///
    /// * a panel, **0x4CEBC0**: when the screen is going out and the sub-screen is in or up
    ///   (0x4E0DE0, state 1 or 2) it starts it out (+0x7C); then the sub-screen's advance (+0x80)
    ///   and draw; and once the sub-screen is down (0x4E0E10, state 0 — an `Out` run to its end)
    ///   the panel closes (+0x70, 0x4CEB00);
    /// * a tab strip, **0x4CFEB0**: its buttons' ticks first — **0x4D0320** clears the selected
    ///   flag of a button whose panel is hidden (0x4CEC70: none, or its sub-screen down) — and
    ///   then, with a pending button, the current one no longer selected and the screen neither
    ///   going out (0x4E0E00 ≠ 3) nor down, the pending one becomes current: slot 27 0x4D02E0,
    ///   the selected flag (0x4C7D00), the panel's load (0x4CEA90) and show (0x4CEC60).
    ///
    /// So a tab switch is **the old panel out, then the new one in**, each three frames and 64
    /// ticks, and whether the new one starts on the frame the old one closes or the next depends
    /// on whether the panel comes before the strip in the file.
    fn tick_panels(&mut self, dt_ms: u32, screen_out: bool) {
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        for i in keys {
            let is_panel = self.layout.elements.get(i).is_some_and(|e| e.class == "NFS5.PANEL");
            if is_panel {
                if self.closed.contains(&i) {
                    continue;
                }
                let Some(g) = self.groups.get_mut(&i) else { continue };
                if screen_out && !matches!(g.transition, Transition::Out { .. }) {
                    g.transition_out();
                }
                g.advance_transition(dt_ms);
                if matches!(g.transition, Transition::Out { .. }) && g.transition.done() {
                    self.closed.insert(i);
                }
                continue;
            }
            // a strip: its clock and its own children first
            if let Some(g) = self.groups.get_mut(&i) {
                g.advance_transition(dt_ms);
            }
            self.tick_strip(i, screen_out);
        }
    }

    /// The panel of this screen a tab button names (0x4D01F0 resolves `data.panel` on the screen
    /// that holds the strip).
    fn panel_index(&self, name: &str) -> Option<usize> {
        self.layout.elements.iter().position(|e| e.class == "NFS5.PANEL" && e.get("name").is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }

    /// 0x4CEC70: the panel shows nothing — none there, never loaded, or its sub-screen down.
    fn panel_hidden(&self, name: Option<&str>) -> bool {
        match name.and_then(|n| self.panel_index(n)) {
            Some(i) => self.closed.contains(&i) || !self.groups.contains_key(&i),
            None => true,
        }
    }

    /// The strip at `k`'s tick, 0x4CFEB0 (see `tick_panels`).
    fn tick_strip(&mut self, k: usize, screen_out: bool) {
        let is_tab = |c: &str| c == "NFS5.TABBUTTON" || c == "NFS5.GRAPHICTABBUTTON";
        let Some(g) = self.groups.get(&k) else { return };
        // the buttons' tick 0x4D0320
        let mut unselect = Vec::new();
        let mut current = None;
        for (n, e) in g.layout.elements.iter().enumerate() {
            if !is_tab(&e.class) {
                continue;
            }
            let selected = g.selected_flags.contains(&n) || (e.get("state") == Some("selected") && !g.unselected.contains(&n));
            if selected {
                if self.panel_hidden(e.get("panel").filter(|p| !p.is_empty())) {
                    unselect.push(n);
                } else {
                    current = Some(n);
                }
            }
        }
        let g = self.groups.get_mut(&k).expect("strip");
        for n in unselect {
            g.selected_flags.remove(&n);
            g.unselected.insert(n);
        }
        let Some((s, j)) = self.pending_tab.filter(|(s, _)| *s == k) else { return };
        let _ = s;
        if current.is_some() || screen_out {
            return;
        }
        // slot 27, 0x4D02E0: selected, the panel's load and show
        g.unselected.remove(&j);
        g.selected_flags.insert(j);
        let panel = g.layout.elements.get(j).and_then(|e| e.get("panel")).filter(|p| !p.is_empty()).map(str::to_string);
        self.pending_tab = None;
        if let Some(i) = panel.and_then(|p| self.panel_index(&p))
            && let Some(p) = self.groups.get_mut(&i)
        {
            self.closed.remove(&i);
            p.reopen();
            p.transition_in();
        }
    }

    /// The panel's load, 0x4CEA90: `0x4E1200` builds the sub-screen afresh from its file every
    /// time, so its widgets are new — their timers, typing and glints start from the open. The
    /// port keeps the loaded layout and restarts those.
    fn reopen(&mut self) {
        self.clock_ms = 0;
        self.preload_ticks = 0;
        self.typing.borrow_mut().clear();
        self.wrap_watch.borrow_mut().clear();
        self.glints.borrow_mut().clear();
        self.activate_glints();
    }

    /// The offset and the alpha a widget draws with at this point in the transition
    /// (`FeWidget::transitionOffset` 0x4C8760 and `::transitionAlpha` 0x4C8840): the slide is the
    /// screen size scaled by the progress, the fade is the progress itself, and the bits come from
    /// `transon` while the screen comes in and from `transoff` while it goes out.
    fn transition_of(&self, e: &nfs_formats::lay::Element) -> ((f32, f32), f32) {
        let (out, p) = self.widget_state();
        if p == 0 {
            return ((0.0, 0.0), 1.0);
        }
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
            // `NFS5.CHATDISPLAYBOX` is not one: its rows are the chat's, and its accessor only
            // names the players (see `chat_boxes`). *(It was bound here until 2026-09-25.)*
            .filter(|(_, e)| e.class == "NFS5.ListBox")
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

    /// The `NFS5.CHATDISPLAYBOX` widgets on this screen (top level; the three shipped ones are).
    ///
    /// Their rows are **not** their accessor's. The bind 0x4C2B60 keeps two accessors: `[+0x154]`
    /// is `data.accessor` — `Comms.PlayerList` on all three shipped boxes — and is only asked for
    /// **names** (0x4C2530, 0x4C259A: 0x4BBB60 finds a player's row, slot 12 its text); `[+0x150]`
    /// is a range accessor of the box's own, `"%s.SliderBar"` (0x4C2BEF, 0x4BA310), which the
    /// scrollbar moves. The rows the draw 0x4C2F50 walks are the vector `[0x65B6A8]` of chat
    /// lines (0x4C2FD1), filled by 0x4C24F0 as lines arrive. *(Corrected 2026-09-25: §4.131 read
    /// the rows as the accessor's, and the box showed the player list.)*
    pub fn chat_boxes(&self) -> Vec<usize> {
        self.layout.elements.iter().enumerate().filter(|(_, e)| e.class == "NFS5.CHATDISPLAYBOX").map(|(i, _)| i).collect()
    }

    /// Fill a chat box with its lines — what **0x4C24F0** does with each one after formatting it:
    ///
    /// ```text
    /// 0x4C2600  at_end = slider.index == slider.count - 1
    /// 0x4C2625  wrap the line in "Univers8b" to [0x5D7DE0] - 8   ; 0x523080, the box's width
    ///           (0x4C29F7) less eight
    /// 0x4C267D  push every wrapped row onto [0x65B6A8]
    /// 0x4C2711  while there are more than 100 rows, drop the first
    /// 0x4C2764  slider range = 0 .. max(0, rows - fit)          ; fit = [+0x148], §4.131
    /// 0x4C27A2  at_end -> slider.index = the new last
    /// ```
    ///
    /// This port has no scrolling of its own for the box, so the slider is always at its end and
    /// the first row drawn is `max(0, rows − fit)`; its count, which the scrollbar's knob is laid
    /// out by, is that plus one.
    pub fn set_chat_lines(&mut self, index: usize, lines: &[String]) {
        let Some(e) = self.layout.elements.get(index) else { return };
        let (w, _) = self.box_of(e, 0);
        let spacing = self.attr_num(e, "spacing").unwrap_or(14.0) as f32;
        let fit = if spacing > 0.0 { (self.attr_num(e, "height").unwrap_or(0.0) as f32 / spacing) as usize } else { 0 };
        let Some(f) = self.fonts.get("univers8b.ffn") else { return };
        let mut rows: Vec<ListRow> = lines.iter().flat_map(|l| wrap_lines(f, l, w - 8.0)).map(ListRow::Text).collect();
        let over = rows.len().saturating_sub(100);
        rows.drain(..over);
        let first = rows.len().saturating_sub(fit);
        if rows.is_empty() {
            self.lists.remove(&index);
        } else {
            self.lists.insert(index, (rows, Some(first)));
        }
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
        let open = index.filter(|i| *i < self.layout.elements.len());
        if open != self.open_combo {
            self.picker_latch = false;
        }
        self.open_combo = open;
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
        // `FEColourAccessor`'s `[+0x30]`, which the colour box's bind 0x4C3BF0 sets from
        // `data.allowcustom` through 0x4B9590: clear, the list is the car's colours alone —
        // no custom row and no saved colours (0x4B9262, 0x4B9374). §4.171.
        let mut rows = rows;
        if self.layout.elements.get(index).is_some_and(|e| e.class == "NFS5.ColorBox" && self.attr_num(e, "allowcustom").unwrap_or(0.0) == 0.0)
            && let Some(n) = rows.iter().position(|r| matches!(r, ListRow::CustomColour(_)))
        {
            rows.truncate(n);
        }
        if rows.is_empty() {
            self.lists.remove(&index);
            self.scroll.remove(&index);
        } else {
            self.scroll.entry(index).and_modify(|f| *f = (*f).min(rows.len().saturating_sub(1))).or_insert(0);
            self.lists.insert(index, (rows, current));
        }
    }

    /// A colour box's current colour — `FEColourAccessor`'s `[+0x2C]`, which the closed box reads
    /// through its slot 29 0x4C4BF0 → 0x4B94C0 and paints as a swatch (§4.171.4). `None` clears it.
    pub fn set_swatch(&mut self, index: usize, colour: Option<[u8; 4]>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_swatch(j, colour);
            }
            return;
        }
        match colour {
            Some(c) => self.swatches.insert(index, c),
            None => self.swatches.remove(&index),
        };
    }

    /// Whether the colour box bound to `accessor` offers the custom row and the saved colours —
    /// `FEColourAccessor`'s `[+0x30]`, which the box's bind 0x4C3BF0 sets from `data.allowcustom`
    /// through 0x4B9590. With no box on the screen the accessor keeps the constructor's **1**
    /// (0x4B91F0).
    pub fn colour_allows_custom(&self, accessor: &str) -> bool {
        self.layout
            .elements
            .iter()
            .find(|e| e.class == "NFS5.ColorBox" && self.attr(e, "accessor").is_some_and(|a| a.eq_ignore_ascii_case(accessor)))
            .is_none_or(|e| self.attr_num(e, "allowcustom").unwrap_or(0.0) != 0.0)
    }

    /// Whether the widget has an **integer** attribute `data.<key>` — what 0x5258C0 (the
    /// attribute lookup by `group.name`: 0x525620 the group, 0x52A370 / 0x52A360 the attribute,
    /// null when either is missing) followed by the cast 0x5A134B to `FERT::IntAttribute`
    /// (0x5D7868) answers. The attributes a widget carries are its template's `DATA` part, so
    /// the test is the `.ctr`'s `<key>.type=INT`, whatever value it or the `.lay` holds.
    /// A key the `.lay` sets that the template does not declare is taken as absent — UNREACHABLE
    /// for the one key ever asked (2026-09-30): the only caller asks `disablesound`, and
    /// all 18 `disablesound=` lines of the 98 shipped `.lay` files (15 layouts; 3 378 elements
    /// scanned) sit on `NFS5.ListBox` elements, whose `ListBox.ctr` declares `disablesound.type=INT`
    /// (the only `.ctr` of 75 that names it). No `.lay` sets it on a class that does not declare it.
    pub fn has_int_attr(&self, index: usize, key: &str) -> bool {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k).is_some_and(|g| g.has_int_attr(j, key));
        }
        self.layout
            .elements
            .get(index)
            .and_then(|e| self.controls.get(&e.class))
            .and_then(|c| c.data())
            .and_then(|d| d.ty(key))
            .is_some_and(|t| t.eq_ignore_ascii_case("INT"))
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
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
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
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)).map(|(a, b)| (wx + a as f32, wy + b as f32)).unwrap_or((wx, wy)),
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

    /// `NFS5.KNOCKOUTTRACKLIST`'s slot 9, **0x4FA470**: the row under a release —
    /// `(ev.y − box.top) · 7 / (box.bottom − box.top)` (`lea eax,[ecx*8]; sub eax,ecx; cdq; idiv`,
    /// 0x4FA4AF…0x4FA4B9, truncating) over the widget's box (0x4C8670) — handed to the
    /// `[this+0x144]` accessor's `SetSelection` (slot 0x2C) when it is **below** the count
    /// `[this+0x140]` answers (slot 0x6C, `jge` at 0x4FA4BD); otherwise nothing, and the slot
    /// answers 0. There is no lower bound and no x test. `None` for any other widget, or a row
    /// at or past the count. The pointer is in design pixels, taken whole (APPROXIMATION: the
    /// original's are the event's integer pixels).
    pub fn ko_row_at(&self, index: usize, x: f32, y: f32) -> Option<usize> {
        if let Some((k, j)) = self.route(index) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.ko_row_at(j, x - gx, y - gy);
        }
        let e = self.layout.elements.get(index)?;
        if e.class != "NFS5.KNOCKOUTTRACKLIST" {
            return None;
        }
        let &(rows, _) = self.ko_strip.get(&index)?;
        let (_, wy) = self.position_of(e);
        let (_, h) = self.box_of(e, 0);
        let (top, span) = (wy as i32, h as i32);
        if span == 0 {
            return None;
        }
        let row = (y.floor() as i32 - top).wrapping_mul(7) / span;
        usize::try_from(row).ok().filter(|r| *r < rows)
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
                // `transition_of`'s alpha is `1 − k / 255` with 0x4C8840's integer `k`
                alpha: (self.transition_of(e).1 * 255.0).round() as i32,
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

    /// The flat index (the one [`Self::tab_click`] takes) of the `nth` (0-based, in file order,
    /// depth-first) widget named `name`, here or in an embedded layout — for the `NFS_FE_TAB`
    /// diagnostic. `nth` exists because several shipped tab strips give every button in the
    /// strip the same `name` — `controllertabs.lay`'s five buttons are all `ID_TABBUTTON`,
    /// `graphicstabs.lay`'s two are too — where only `showcasetabs.lay` and `optiontabs.lay`
    /// name each of their buttons uniquely. The original never needs to pick one by name; its
    /// own click is a screen coordinate, so a name collision is invisible to it.
    pub fn flat_index_of(&self, name: &str, nth: usize) -> Option<usize> {
        self.flat_index_of_skip(name, nth).1
    }

    /// `flat_index_of`'s recursion: the index found, or how many more matches were skipped over
    /// without yet reaching `nth`, so the caller above can keep counting into the next group.
    fn flat_index_of_skip(&self, name: &str, mut skip: usize) -> (usize, Option<usize>) {
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.name.eq_ignore_ascii_case(name) {
                if skip == 0 {
                    return (0, Some(i));
                }
                skip -= 1;
            }
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            let (rem, found) = g.flat_index_of_skip(name, skip);
            if let Some(j) = found {
                return (0, Some(base + j));
            }
            skip = rem;
            base += g.span();
        }
        (skip, None)
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
    /// [`MoviePlayer`]). `audio` is the widget's own `data.audio` (default 1, `MovieBox.ctr` /
    /// `MovieCircle.ctr`) — whether the tick may skip frames to catch up after a stall, and
    /// (`mad.md` §14) whether it has its own `SC**` track at all; that track is decoded here but
    /// only started by the first [`LayoutScreen::advance_movies`] call that has a sound device.
    pub fn set_movie(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, dir: &str, file: &str, audio: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_movie(r, paths, j, dir, file, audio);
            }
            return;
        }
        if self.movies.contains_key(&index) {
            return;
        }
        if let Some(m) = MoviePlayer::open(r, paths, dir, file, audio) {
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
            // `FEChroniclePicture::draw` asks for shape **4** while it is hovered (0x4C7C90) and
            // its picture is bigger than its box on either axis (0x50851C…0x508531)
            if self.layout.elements.get(h).is_some_and(|e| e.class == "NFS5.ChroniclePicture")
                && let Some((_, _, pw, ph)) = self.chronicle_pan(h)
                && let Some(e) = self.layout.elements.get(h)
                && {
                    let (bw, bh) = self.box_of(e, 0);
                    pw - bw > 0.0 || ph - bh > 0.0
                }
            {
                want = want.max(4);
            }
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
        let Some((cx, cy)) = self.pointer.unwrap_or(self.cursor) else { return };
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
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
    }

    /// True once any movie on this screen has played through (0x4E2960).
    pub fn movie_finished(&self) -> bool {
        self.movies.values().any(|m| m.finished)
    }

    pub fn advance_movies(&mut self, r: &mut Renderer, dt_ms: u32, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        for m in self.movies.values_mut() {
            m.advance(r, dt_ms, audio_out, music_level);
        }
        // the end-of-era movie runs from the moment the show starts it (0x4FF76F) — its audio
        // track (`mad.md` §14) was decoded at `set_end_of_era` time but only starts here, on the
        // first tick this is true, the same as any movie's own not-yet-started case
        // (`MoviePlayer::advance`) — the video and the audio start together, matching the video's
        // own gate rather than the widget movies' (ticked from the same frame as `open`)
        if let Some(e) = self.end_of_era.as_mut()
            && e.show.borrow().movie_started().is_some()
            && let Some(m) = e.movie.as_mut()
        {
            m.advance(r, dt_ms, audio_out, music_level);
        }
        if let Some(b) = self.background.as_mut() {
            b.advance_movies(r, dt_ms, audio_out, music_level);
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
        if self.logos.get(&index).is_some_and(|(f, b)| f == file && *b == CHRONICLE_MOSAIC_ENTRY) {
            return;
        }
        if !self.shapes.contains_key(&(file.to_string(), CHRONICLE_MOSAIC_ENTRY)) {
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
            // `1111`, which is why only the first row is matched by name. The widget on both
            // `chroniclepicture.lay` and `chroniclemedia.lay` is `size=458,286`, which is the
            // picture in 140 of the 183 packs; the other 43 are bigger — `MA601a.fsh` is **five**
            // rows, 4 × (128+128+128+74) by (148+138+148+138+78) = 458×650 (this said 458×286
            // until 2026-09-26, counting only its first two rows) — and the draw shows a
            // box-sized window of them that the player drags (`chronicle_pan`).
            //
            // The measure's own row height is the height of entry `2 + r·cols + 1` for r ≥ 1 (the
            // first `Laye` entry resets the column without adding, 0x50844D…0x50847F) and the
            // draw advances by the row's **last** tile (0x5085FE); every shipped row is one
            // height, so all three agree.
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
            self.shapes.insert((file.to_string(), CHRONICLE_MOSAIC_ENTRY), (id, w as f32, h as f32));
        }
        self.logos.insert(index, (file.to_string(), CHRONICLE_MOSAIC_ENTRY));
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
        Sprite { texture: self.white, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
    }

    /// One gouraud quad of the front end's own primitive **0x4D98D0**, whose four `(x, y, colour)`
    /// triples arrive in the order upper-left, upper-right, lower-left, lower-right (§4.7).
    ///
    /// **The node's own stored order, read to the instruction (2026-09-27):** 0x4D98D0 does not
    /// draw — it copies its twelve floats into a node and inserts it into a sorted `_Rb_tree`
    /// rooted at `[0x65B958]` (compare 0x4D9450, node alloc 0x4DAD30, insert 0x4314C0). Its own
    /// fill (0x4D993F…0x4D99DA) does **not** keep the caller's four corners — bottom-right,
    /// bottom-left, top-right, top-left (controls.md §4.7) — in that order: it swaps the last two,
    /// storing pos/colour slot 0 = the caller's corner 0 (BR), slot 1 = corner 1 (BL), slot 2 =
    /// corner **3** (TL), slot 3 = corner **2** (TR) — traced by which argument offset (`[esp+…]`)
    /// feeds which output slot. The flush **0x4DA5F0** copies a node's 64 B pos / 16 B colour / 32 B
    /// uv blocks verbatim (0x4DA79E/0x4DA7BD/0x4DA7D1) into the batch arrays, so this is also the
    /// order the device sees: **BR, BL, TL, TR**.
    ///
    /// **The diagonal.** Which two triangles the device call actually cuts from those four
    /// vertices was still not read this pass — past the batch-array copy the call goes through an
    /// `EAGL::Material` (`[0x65B978]`, vtable 0x5B3C94) whose own "apply" (0x53F1B0) only stages a
    /// render-context and walks a chain of state-layer objects (`LayerColours` 0x5B3BF0 among
    /// them), and the actual `DrawPrimitive` is somewhere past that chain or inside `dx7z.dll`
    /// itself — unread. But the stored order settles it anyway: a **strip** split (`(v0,v1,v2)` +
    /// `(v1,v2,v3)`) of BR, BL, TL, TR shares the edge BL–TL, which is a side of the quad, not a
    /// diagonal — the two triangles would overlap and not tile the quad at all (checked against
    /// coordinates). Only a split that shares **v0–v2** (a fan, or any index pattern built on that
    /// diagonal) tiles the quad correctly for this vertex order — the **BR–TL** diagonal, not the
    /// BL–TR one this port drew before. *(Until this pass the port kept the caller's own BR, BL,
    /// TR, TL order and read it as a strip, splitting BL–TR — wrong on two counts now traced: the
    /// node does not keep that order, and that order read as a strip is not even a valid tiling.)*
    /// This port used to step the quad into a grid of flat cells (up to 8 × 8), since its sprites had
    /// one colour a quad; the renderer's per-corner triangles make that unnecessary.
    fn gouraud(&self, out: &mut Vec<Sprite>, aspect: f32, p: [[f32; 2]; 4], c: [[f32; 4]; 4]) {
        let design_aspect = DESIGN.0 / DESIGN.1;
        let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
        let at = |k: usize| [x0 + p[k][0] / DESIGN.0 * fw, p[k][1] / DESIGN.1];
        let tri = |a: usize, b: usize, d: usize| Sprite {
            texture: self.white,
            pos: [0.0; 2],
            size: [0.0; 2],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: Some(([at(a), at(b), at(d)], [c[a], c[b], c[d]])), corner_colors: None,
            depth: None,
        };
        // UL 0, UR 1, BL 2, BR 3 — 0x4D98D0's node stores them BR, BL, TL, UR (swapping the
        // caller's TR/TL), which tiles the quad only along the BR–UL diagonal: (BR, BL, UL) and
        // (BR, UL, UR).
        out.push(tri(3, 2, 0));
        out.push(tri(3, 0, 1));
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
    #[allow(clippy::too_many_arguments)]
    fn glinty_box(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, selected: Option<usize>, hovered: Option<usize>, tint: impl Fn([u8; 4]) -> [f32; 4]) {
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
        // the buttons' *may start* — the latch of 0x4C93DC / 0x4D04C0 / 0x4C6415 over the
        // widget's flags: disabled (0x4C7E40), hovered (0x4C7C90), selected (0x4C7D50), focused
        // (0x4C7DD0), and the fade (0x4C8820)
        let disabled = self.disabled_flags.contains(&i)
            || e.get("state") == Some("disabled")
            || (self.covered && self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled")));
        let hot = hovered == Some(i);
        let sel = selected == Some(i) || self.selected_flags.contains(&i) || (e.get("state") == Some("selected") && !self.unselected.contains(&i));
        let focus = self.attr_num(e, "alwaysfocused").unwrap_or(0.0) != 0.0 || self.open_combo == Some(i) || self.focused.contains(&i);
        let fade_zero = self.widget_progress() * 255 / TRANS_FULL == 0;
        // the combo family builds its object in its draw (0x4C5937), the others in their bind
        let combo = !matches!(e.class.as_str(), "NFS5.GOTOBUTTON" | "NFS5.TABBUTTON" | "NFS5.GRAPHICTABBUTTON");
        let may = |f: &mut glints::Flourish| f.button_may_start(disabled, hot, sel, focus, fade_zero);
        if (1..=5).contains(&style) {
            self.glinty_flourish(i, if combo { None } else { Some(0) }, &may, style, out, aspect, x, y, gw as i32, gh as i32);
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
        // 0x4C809A: 0x523260's `[+8]`, `_ftol` — the widest **measured** line, and only of the
        // lines that fit: 0x5237A3 raises it on the `<` branch alone, so a word too long for the
        // box (taken whole at 0x52379F) does not count
        let tw = lines.iter().map(|l| f.measure(l).0).filter(|w| *w < 310.0).fold(0.0f32, f32::max).trunc();
        // 0x4C80AF: 0x523250's total, `_ftol`; the lines step by the last one's pitch (0x523290)
        let (pitch, total) = wrap_metrics(f, &lines);
        let th = total.trunc();
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
            ly += pitch;
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
                out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
            }
        }
        // the knob: its height is 0x4DAFC5's, and its top 0x4DB220's from the first row
        let (ky, knob_h, _) = combo_knob(y, h, rows, first);
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

    /// The scrollbar geometry of the open drop-down `i` — `(x, y, height, positions, first)`, the
    /// four numbers `combo_scrollbar` draws from. `None` when the list is short enough not to
    /// scroll.
    ///
    /// `positions` is **not** the row count. `FeComboBox`'s slot 1 (0x4C5460) is the same shared
    /// tick §4.45/§4.59 read for the list box: it walks up from the last row while the heights
    /// fit and hands the scrollbar model `rowsWalkedUp + 2` (0x4C550A) — one first-row position
    /// past the last full page — and that `+2` count is what `slot9`/`slot24` (§4.59) answer back
    /// as the knob's travel divisor, for a combo exactly as for a list box. Until this fix the
    /// combo's own geometry passed the plain row count instead, which is why the combo scrollbar's
    /// knob height, travel and hit-test disagreed with the list box's for the same shape of list.
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
        let positions = self.last_first_row(i, rows) + 2;
        Some((px + width + grow - LIST_SCROLLBAR_W, y + height, max_h, positions, self.scroll.get(&i).copied().unwrap_or(0)))
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
    ///
    /// A drop-down open inside an embedded screen (a tab panel) is held by that screen alone —
    /// `set_open_combo` keeps only indices of this screen's own elements in `open_combo` — so with
    /// none open here, ask each embedded screen at the panel's own offset, the same routing
    /// `combo_row_at` uses (2026-09-29's nested-panel fix). *(Until this fix a nested panel's
    /// combo's scrollbar never hit-tested at all — its arrows and knob were unreachable, on top of
    /// this file's own knob-drag gap.)*
    pub fn combo_scroll_at(&self, px: f32, py: f32) -> Option<(usize, u32)> {
        if let Some(i) = self.open_combo {
            let (x, y, h, rows, first) = self.combo_scroll_geom(i)?;
            return match scrollbar_region(x, y, h, rows, first, px, py) {
                0 => None,
                n => Some((i, n)),
            };
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if !self.closed.contains(&k)
                && let Some(e) = self.layout.elements.get(k)
            {
                let (gx, gy) = self.position_of(e);
                if let Some((j, r)) = g.combo_scroll_at(px - gx, py - gy) {
                    return Some((base + j, r));
                }
            }
            base += g.span();
        }
        None
    }

    /// A press on the open combo's scrollbar **knob** (region 3) or **track** (region 4) —
    /// `FeListScrollBar::onPress` 0x4DB040's other two arms (§4.46, §4.101). Returns
    /// `(index, grabY, grabPosition)`, the drag anchor 0x4DB9B0 steps from on every move —
    /// `[this+0xE4]` and `[this+0xE8]`:
    ///
    /// ```text
    /// arm 3  0x4DB0B0  [+0xE0], [+0xE4] = ev.x, ev.y          ; the pointer
    ///        0x4DB0C1  [+0xE8] = model->slot10()              ; the position it is on
    /// arm 4  0x4DB0D5  [+0xE0], [+0xE4] = [+0xFC], [+0x100]   ; the knob's own top-left
    ///        0x4DB0F0  [+0xE8] = model->slot10()
    ///        0x4DB0F6  [+0x100] = ev.y, then the hit test again -> region 3
    /// ```
    ///
    /// Region 3 anchors to the **pointer's own** y; region 4 to the **knob's current top**. Arm 4
    /// then asks for a synthetic move (`bl = 1` at 0x4DB101 → 0x4D6140 at 0x4DB119, a type-7
    /// `MouseMoveEvent` at the current position, controls.md §4.40), which reaches the drag slot
    /// 0x4DB9B0 at once — so a press on the trough moves the list by `(pointer − knobTop)`
    /// straight away; `App::fe_left_button` runs that first step right after the grab.
    pub fn combo_scroll_grab(&self, index: usize, region: u32, py: f32) -> Option<(usize, f32, usize)> {
        if let Some((k, j)) = self.route(index) {
            let (_, gy) = self.position_of(self.layout.elements.get(k)?);
            let (_, y, at) = self.groups.get(&k)?.combo_scroll_grab(j, region, py - gy)?;
            return Some((index, y + gy, at));
        }
        let (_, y, h, positions, first) = self.combo_scroll_geom(index)?;
        let grab_y = if region == 4 { combo_knob(y, h, positions, first).0 } else { py };
        Some((index, grab_y, first))
    }

    /// The open combo scrollbar's knob travel (in whole design pixels) and position count — the
    /// numbers `combo_knob` (0x4DB220) places the knob with, which the drag 0x4DB9B0 divides by:
    /// `model->slot11([+0xE8] + (y − [+0xE4]) · count / travel)` (`combo_drag_pos`).
    pub fn combo_scroll_track(&self, index: usize) -> Option<(i32, usize)> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.combo_scroll_track(j);
        }
        let (_, y, h, positions, first) = self.combo_scroll_geom(index)?;
        Some((combo_knob(y, h, positions, first).2, positions))
    }

    /// Put the open combo's first-drawn row at an absolute position, clamped — what the
    /// scrollbar's knob drag does (0x4D0F20's inverse of the placement, §4.46), mirroring
    /// `set_list_scroll`.
    pub fn set_combo_scroll_pos(&mut self, index: usize, at: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_combo_scroll_pos(j, at);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let positions = self.last_first_row(index, rows) as i32 + 2;
        self.scroll.insert(index, at.clamp(0, positions - 1) as usize);
    }

    /// Which combo has its drop-down open, if any.
    pub fn open_combo(&self) -> Option<usize> {
        self.open_combo
    }

    /// 0x4C39F0, `[0x65B710] = 1`: the colour editor's latch, raised by the combo commit when the
    /// chosen row is the custom one (0x4C6D6C). The box keeps focus — 0x4C6DD9 asks the class's
    /// slot 28 (0x4C46F0, "not the custom row") and skips the unfocus 0x4C7DA0 when it answers 0.
    pub fn raise_picker_latch(&mut self) {
        self.picker_latch = true;
    }

    /// The open drop-down's row under the pointer — `[this+0x1A8]`, which the commit at 0x4C6CF7
    /// reads once the release lands inside the selection box (0x4E2970 on `[this+0x178]`). The
    /// walk is the move handler's, 0x4C6F60: from the scrollbar's position `[+0x1BC]->slot10()`
    /// (0x4C6FC7, the drop-down's first drawn row), each row its accessor height (slot 26,
    /// 0x4C6FF6), the first whose running bottom is past the pointer (0x4C7013, signed `jge`) —
    /// an **absolute** row, which the commit hands to the accessor's slot 11 as it is (0x4C6D82).
    /// `(combo, row)`.
    pub fn combo_row_at(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        // A drop-down open inside an embedded screen (a tab panel — `Options` → `graphics.lay` →
        // `graphicsettings.lay`'s six combos) is held by that screen alone: `set_open_combo`
        // keeps only indices of this screen's own elements in `open_combo`. So with none open
        // here, ask each embedded screen at the panel's own offset and hand its answer back in
        // the flattened index space. *(Until 2026-09-29 this returned at once, so a row of a
        // nested drop-down never committed — the release fell through to whatever widget lay
        // under the list, typically the next combo, which it opened instead. User report.)*
        let Some(i) = self.open_combo else {
            let mut keys: Vec<usize> = self.groups.keys().copied().collect();
            keys.sort_unstable();
            let mut base = self.layout.elements.len();
            for k in keys {
                let g = &self.groups[&k];
                if !self.closed.contains(&k)
                    && let Some(e) = self.layout.elements.get(k)
                {
                    let (gx, gy) = self.position_of(e);
                    if let Some((j, r)) = g.combo_row_at(px - gx, py - gy) {
                        return Some((base + j, r));
                    }
                }
                base += g.span();
            }
            return None;
        };
        if let Some((k, j)) = self.route(i) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.combo_row_at(px - gx, py - gy).filter(|(g, _)| *g == j).map(|(_, r)| (i, r));
        }
        let e = self.layout.elements.get(i)?;
        let (x, y) = self.position_of(e);
        let (rx, ry, rw, rh) = self.drop_down_rect(e, i, x, y)?;
        if !(px >= rx && px < rx + rw && py >= ry && py < ry + rh) {
            return None;
        }
        let (rows, _) = self.lists.get(&i)?;
        let first = self.scroll.get(&i).copied().unwrap_or(0);
        let mut acc = ry;
        rows.iter().enumerate().skip(first).find(|(_, r)| {
            acc += r.height();
            py < acc
        }).map(|(n, _)| (i, n))
    }

    /// Whether a release at `(px, py)` on the **closed** combo `index` lands in its caption
    /// strip — `FeComboBox` slot 9, 0x4C6B7E…0x4C6BB8: inside `backrect` (0x4E2970 on
    /// `[+0x17C]`, `left ≤ x < right`, `top ≤ y < bottom`) and `x < backrect.left + [+0x1F0]`
    /// (0x4E2B20, signed `jge`), [`Self::combo_strip`]. Such a release does not open the list.
    pub fn in_combo_strip(&self, index: usize, px: f32, py: f32) -> bool {
        if let Some((k, j)) = self.route(index) {
            let (Some(g), Some(ek)) = (self.groups.get(&k), self.layout.elements.get(k)) else { return false };
            let (gx, gy) = self.position_of(ek);
            return g.in_combo_strip(j, px - gx, py - gy);
        }
        let Some(e) = self.layout.elements.get(index) else { return false };
        let (x, y) = self.position_of(e);
        let w = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let h = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        px >= x && px < x + w && py >= y && py < y + h && px < x + self.combo_strip(e)
    }

    /// The drop-down's panel, the `selectionbox` rect `[+0x178]`: `(x, y, rows' width, h)` — the
    /// same arithmetic `drop_down` draws with. `h` is **the rows' height, clamped to
    /// `data.selectionboxmaxheight`**: `FeComboBox`'s slot 1 0x4C5460 ends each tick with
    /// 0x4E2C20(selectionbox, h) at 0x4C57D5 — 0x4E2C20 replaces the size pair's **height** — `h`
    /// the 16-tick ramp `min((now − [+0x1D4])·[+0x1C4]/16, [+0x1C4])` (0x4C5626…0x4C564D) of the
    /// clamped rows' height `[+0x1C4]` (0x4C53F3), and 0 while the combo is not focused
    /// (0x4C53C4 zeroes `[+0x1C4]`). The port takes the settled end.
    fn drop_down_rect(&self, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32) -> Option<(f32, f32, f32, f32)> {
        let (rows, _) = self.lists.get(&i)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        let scrolls = full >= max_h;
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (px, py) = (if left { x - grow } else { x }, y + height);
        let width = width + grow;
        let rect_w = if scrolls { width - LIST_SCROLLBAR_W } else { width };
        Some((px, py, rect_w, if scrolls { max_h } else { full }))
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

    /// `NFS5.PLAYERDRIVERPICTURE`'s slot 9, **0x4CF940** — the release on one of
    /// `createplayer.lay`'s eight pictures answers its `data.ournumber` (`[this+0x140]`, bound by
    /// slot 20 0x4CF7A0 through 0x4C8B00), which the caller stores in `[0x5D8F2C]`. No sound, no
    /// other state; the slot returns 1. `None` for any other widget.
    pub fn driver_picture_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.driver_picture_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.PLAYERDRIVERPICTURE").then(|| self.attr_num(e, "ournumber").unwrap_or(0.0) as i32)
    }

    /// `NFS5.CAMERASELECT`'s slot 9, **0x4BF5F0** — the release hands `Camera.Current` (`[this+
    /// 0x140]`, bound at 0x4BF2DB) its `SetSelection`, slot 0x2C, with the widget's own
    /// `data.accessornum` (`[this+0x14C]`, 0x4BF2C7), and answers 1. No hit test, no sound. The
    /// number, for any other widget `None`.
    pub fn camera_select_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.camera_select_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.CAMERASELECT").then(|| self.attr_num(e, "accessornum").unwrap_or(0.0) as i32)
    }

    /// `NFS5.CAMERAPICTURE`'s `data.picturenum` (`[this+0x144]`, bound at 0x4BF730) — what its slot 9
    /// **0x4BF920** hands `Camera.<Camera.Current + 1>`'s `SetSelection` when the release is the
    /// first of its widget for half a second. The number, for any other widget `None`.
    pub fn camera_picture_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.camera_picture_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.CAMERAPICTURE").then(|| self.attr_num(e, "picturenum").unwrap_or(0.0) as i32)
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

/// The chronicle slider's geometry, the numbers `FEHorizontalSlider`'s helper works with
/// (0x4D0B90, 0x4D0CC0, 0x4D0F20): the track (`size=342,21`, the `track` RECT part) and the knob
/// (`weeicons.fsh` 38, its own size), both at the widget's position — the knob's base offset is the
/// template's `knob.pos.value` 0,0 (`[helper+0x114]`, 0x4D08BA).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChronicleSliderGeom {
    /// the widget's index in the layout
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub knob_w: f32,
    pub knob_h: f32,
}

impl ChronicleSliderGeom {
    /// The knob's left edge for accessor index `idx` of `count` — 0x4D0C0C…0x4D0C68:
    /// `idx * (track - knob) / (count - 1)`, integer, `0` for a single entry.
    pub fn knob_left(&self, idx: i32, count: i32) -> f32 {
        let dx = if count > 1 { idx * (self.w - self.knob_w).max(0.0) as i32 / (count - 1) } else { 0 };
        self.x + dx as f32
    }

    /// Whether `(px, py)` is on the knob part's rectangle (0x4E2970: left/top inclusive,
    /// right/bottom exclusive).
    pub fn on_knob(&self, px: f32, py: f32, idx: i32, count: i32) -> bool {
        let kx = self.knob_left(idx, count);
        px >= kx && px < kx + self.knob_w && py >= self.y && py < self.y + self.knob_h
    }

    /// Whether `(px, py)` is on the `track` part's rectangle.
    pub fn on_track(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The accessor index a drag has reached — 0x4D0F20: `(mx - anchor) * (count - 1) / (track -
    /// knob) + start`, `idiv` truncating, before the accessor's setter (0x4BC240) clamps it.
    pub fn dragged_index(&self, mx: f32, anchor: i32, start: i32, count: i32) -> i32 {
        let span = (self.w - self.knob_w) as i32;
        if span == 0 {
            return start;
        }
        (mx as i32 - anchor) * (count - 1) / span + start
    }
}

#[cfg(test)]
mod chronicle_slider_tests {
    use super::ChronicleSliderGeom;

    fn geom() -> ChronicleSliderGeom {
        // chronicle.lay: pos 150,350 size 342,21; the knob is `weeicons.fsh` 38, 45 wide, 11 high
        ChronicleSliderGeom { index: 0, x: 150.0, y: 350.0, w: 342.0, h: 21.0, knob_w: 45.0, knob_h: 11.0 }
    }

    #[test]
    fn the_knob_sits_at_index_times_travel_over_count() {
        let g = geom();
        // 0x4D0C0C…0x4D0C68: idx * (342 - 45) / (count - 1), integer
        assert_eq!(g.knob_left(0, 49), 150.0);
        assert_eq!(g.knob_left(48, 49), 150.0 + 297.0);
        assert_eq!(g.knob_left(12, 49), 150.0 + (12 * 297 / 48) as f32);
        assert_eq!(g.knob_left(5, 1), 150.0);
    }

    #[test]
    fn a_drag_is_the_inverse_and_truncates_toward_zero() {
        let g = geom();
        // 0x4D0F20: (x - anchor) * (count - 1) / (track - knob) + start
        assert_eq!(g.dragged_index(232.0, 172, 0, 49), 60 * 48 / 297);
        assert_eq!(g.dragged_index(172.0, 232, 9, 49), (-60 * 48 / 297) + 9);
        assert_eq!(g.dragged_index(200.0, 200, 7, 49), 7);
    }

    #[test]
    fn regions_are_the_knob_and_the_track() {
        let g = geom();
        assert!(g.on_knob(150.0, 350.0, 0, 49));
        assert!(!g.on_knob(195.0, 350.0, 0, 49), "right edge exclusive");
        assert!(!g.on_knob(160.0, 361.0, 0, 49), "below the knob's 11 rows");
        assert!(g.on_track(160.0, 361.0));
        assert!(!g.on_track(160.0, 371.0));
    }
}

impl LayoutScreen {
    /// This screen's `NFS5.CHRONICLESLIDER`, if it has one.
    pub fn chronicle_slider(&self) -> Option<ChronicleSliderGeom> {
        let index = self.layout.elements.iter().position(|e| e.class == "NFS5.CHRONICLESLIDER")?;
        let e = &self.layout.elements[index];
        let (x, y) = self.position_of(e);
        let (w, h) = e.pair("size")?;
        let (knob_w, knob_h) = self
            .shape_parts_at(e, "normal", None)
            .into_iter()
            .find_map(|(_, _, file, i, _)| self.shapes.get(&(file, i)).map(|&(_, dw, dh)| (dw, dh)))?;
        Some(ChronicleSliderGeom { index, x, y, w, h, knob_w, knob_h })
    }

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

    /// One caption per timeline column, the `фото`-row entry's own `0x6F` text (§4.173).
    pub fn set_chronicle_captions(&mut self, captions: Vec<Option<String>>) {
        self.chronicle_captions = captions;
    }

    /// `FEChroniclePictureControl`'s four mouse slots — the class `NFS5.ChroniclePicture`
    /// instantiates, vtable **0x5B8C74**:
    ///
    /// ```text
    /// 0x508250  slot 5  [+0x154] = [+0x14C]; [+0x158] = [+0x150]     ; remember the offset
    ///                   [+0x140] = 1; [+0x144] = ev.x; [+0x148] = ev.y
    /// 0x5082A0  slot 8  while [+0x140]:
    ///                     [+0x14C] = [+0x154] - ev.x + [+0x144]
    ///                     [+0x150] = [+0x158] - ev.y + [+0x148]
    /// 0x5082F0  slot 7  the base leave 0x4C7C50 (clears the hover flag), then [+0x140] = 0
    /// 0x508290  slot 9  [+0x140] = 0
    /// ```
    ///
    /// so the offset moves **against** the pointer, the way a document scrolls under a hand, and
    /// the bind 0x508220 starts all five at zero. §4.119.
    ///
    /// **Slot 7 (0x5082F0), COVERAGE §14.15's one honest gap in this class, now ported.** The
    /// window's own move handler (`Window::handleEvent`'s move arm, 0x5289D9, §4.56) recomputes
    /// the hit test on every device move and, whenever the hit widget changes away from the one
    /// that had been hovered, sends it a type-6 **leave** event before the move event itself is
    /// dispatched — regardless of capture, which this class's slot 5 never takes (unlike
    /// `NFS5.DEADZONE`'s press, §4.40, it does not call `setFocused`, so §4.57's capture-on-focus
    /// never applies to a chronicle-picture drag). So as soon as the pointer's box test
    /// (`Control::containsPoint` 0x525A90, both edges inclusive, §4.56) fails for this widget, the
    /// leave event's slot 7 clears `[+0x140]` — and slot 6 ("enter", 0x4C7B90, not overridden by
    /// this class) never sets it back, so the drag stays stopped for the rest of the press even if
    /// the pointer wanders back over the picture while the button is still held. `picture_drag_to`
    /// below checks the same box before applying the pan, for the identical outcome.
    pub fn picture_press(&mut self, index: usize, x: f32, y: f32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.picture_press(j, x, y);
            }
            return;
        }
        // slot 5 copies the pan as the **draw** last left it, and the draw clamps it in place
        // (0x5084E8…0x508516), so a drag starts from the clamped value, never from a raw one
        let (ox, oy) = self.chronicle_pan(index).map_or((0.0, 0.0), |p| (p.0, p.1));
        self.picture_drag = Some((index, x, y, ox, oy));
    }

    /// `FEChroniclePicture::draw`'s pan, clamped the way the draw clamps it, with the size of the
    /// whole picture: `(pan_x, pan_y, picture_w, picture_h)`.
    ///
    /// 0x5084AC takes the widget's box (0x4C8670: the local rect of vtable slot `0x34` plus the
    /// parent's origin — **not** moved by the transition), and 0x5084C7…0x5084E2 take
    /// `picture − box` on each axis as the furthest the pan can go. The stored pan is then
    /// clamped to that (0x5084E4, 0x5084EE) and **afterwards** to 0 (0x508504, 0x50850E), so a
    /// picture no bigger than its box never moves. `None` when no picture is bound.
    pub(crate) fn chronicle_pan(&self, index: usize) -> Option<(f32, f32, f32, f32)> {
        let e = self.layout.elements.get(index)?;
        let &(_, pw, ph) = self.shapes.get(self.logos.get(&index)?)?;
        let (bw, bh) = self.box_of(e, 0);
        let (rx, ry) = self.picture_pan.get(&index).copied().unwrap_or((0.0, 0.0));
        Some((rx.min(pw - bw).max(0.0), ry.min(ph - bh).max(0.0), pw, ph))
    }

    /// Slot 8, 0x5082A0 — the pan follows the pointer while the button is held, unless slot 7,
    /// 0x5082F0, has just ended it: the pointer leaving this widget's own hit box (the box
    /// `hovered_scan` tests it against, both edges inclusive — `Control::containsPoint` 0x525A90,
    /// §4.56) stands in for the window's leave event, and once it fires the drag does not resume
    /// even if the pointer comes back while the button is still held (slot 6, "enter", is not
    /// overridden by this class, so nothing sets `[+0x140]` back to 1 — see `picture_press`'s doc).
    pub fn picture_drag_to(&mut self, x: f32, y: f32) {
        if let Some((i, gx, gy, ox, oy)) = self.picture_drag {
            let inside = self.layout.elements.get(i).is_some_and(|e| {
                let (bx, by) = self.position_of(e);
                let (bw, bh) = self.box_hit(e);
                bw >= 1.0 && bh >= 1.0 && x >= bx && x <= bx + bw && y >= by && y <= by + bh
            });
            if inside {
                self.picture_pan.insert(i, (ox - x + gx, oy - y + gy));
            } else {
                self.picture_drag = None;
            }
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

    /// Tooling only (`NFS_FE_CLICKS=drag:…`): the flattened index and current pan of whichever
    /// `NFS5.ChroniclePicture` a drag is still active for — `None` once slot 7 (0x5082F0) has
    /// cleared it, which is exactly the observation the diagnostic checks for a drag that left
    /// the widget's box.
    pub fn dragging_picture(&self) -> Option<(usize, f32, f32)> {
        if let Some((i, ..)) = self.picture_drag {
            return self.picture_pan.get(&i).map(|&(px, py)| (i, px, py));
        }
        self.groups.values().find_map(|g| g.dragging_picture())
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
            self.unselected.remove(&index);
        } else {
            self.selected_flags.remove(&index);
            self.unselected.insert(index);
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
    /// 0x4E6016 pushes **15**, `COMMS.PASSWORD` at 0x50B9C1 pushes **7**, `Comms.ChatInput` at
    /// 0x50BA95 pushes **45** (0x2D).
    ///
    /// Every other box's limit lives in a registration this port has not read, so it is left
    /// **unlimited** rather than guessed — a cap invented here would silently truncate a name the
    /// original accepts, which is worse than one it would have refused.
    pub fn input_max_len(accessor: &str) -> Option<usize> {
        match accessor {
            "UserList.NameInput" => Some(8),
            "ANOTHER" => Some(15),
            "COMMS.PASSWORD" => Some(7),
            "Comms.ChatInput" => Some(45),
            _ => None,
        }
    }

    /// Which input box has focus, if any. A box with `data.alwaysfocused` counts from the moment
    /// it is built: its bind **0x4CC430** calls `setFocused` outright (0x4CC56E…0x4CC570, §4.41),
    /// so the keyboard reaches it with no click. Until 2026-10-01 the port only *drew* such a box
    /// focused, and `createplayer`'s name field took no keys at all.
    pub fn focused_input(&self) -> Option<usize> {
        let is_input = |i: usize| {
            self.layout.elements.get(i).is_some_and(|e| {
                matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX")
            })
        };
        self.focused.iter().copied().find(|&i| is_input(i)).or_else(|| {
            (0..self.layout.elements.len()).find(|&i| {
                is_input(i) && !self.is_hidden(i) && self.attr_num(&self.layout.elements[i], "alwaysfocused").unwrap_or(0.0) != 0.0
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
    /// `'%'` is the only printable character a box turns down **at this shared layer**, and the
    /// reason is visible in what the strings are for: every front-end string reaching 0x5A0FBF is
    /// a `printf` format, so a typed `%` would be read as a conversion.
    ///
    /// **A second, per-instance filter, found this pass and not previously ported.** `vt[0x30]`
    /// (slot 12, `FEStringAccessor::append` **0x4BC950**, §4.139) does not insert the character
    /// itself — it calls slot 11, **0x4BCA90**, which insists on `[this+0x1C]` before it filters
    /// at all: a byte the ctor **0x4BC8B0** takes as its 5th argument, `false` for most of the
    /// class's registrations (e.g. `"ANOTHER"`, 0x4E6016) but **`true` for `UserList.NameInput`**
    /// (0x4E5D9E, confirmed by disassembling that one call site's own pushed args — the same site
    /// §4.139's `maxLen` table already cites for its `8`). When it is set, 0x4BCA90 rejects nine
    /// more characters no other box turns down — `\ / : * ? " < > |`, byte for byte the set
    /// Windows refuses in a file name — while still consuming the keystroke (a rejected character
    /// plays no different sound; it is simply never inserted). The profile name typed here is
    /// exactly what a save file is later named (`career_ui.rs`, `TEMPLATE_STEM`/`create_profile`),
    /// so the filter is not cosmetic: a name that reached this port's own save path unfiltered
    /// (`8Kate/Kate.sav`, say) would build an invalid or wrong path the original never lets a
    /// player type. Not read for `COMMS.PASSWORD` (0x50B9C1, unconfirmed — out of scope, network
    /// play carries no filesystem write this port makes).
    pub fn type_char(&mut self, index: usize, c: char) -> bool {
        let code = c as u32;
        if !(0x20..0xFF).contains(&code) || code == 0x25 {
            return false;
        }
        let accessor = self.layout.elements.get(index).and_then(|e| e.get("accessor"));
        // 0x4BCA90's own filename-illegal set, active only where the ctor's 5th argument was
        // `true` — checked directly against every one of `input_max_len`'s own four named
        // registrations by disassembling each one's own pushed args this pass: `true` only for
        // `UserList.NameInput` (0x4E5D9E); `"ANOTHER"` (0x4E6016), `COMMS.PASSWORD` (0x50B9C1)
        // and `Comms.ChatInput` (0x50BA95) all push `false`.
        if accessor == Some("UserList.NameInput") && filename_illegal_char(code) {
            return false;
        }
        let max = accessor.and_then(Self::input_max_len);
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

    /// `NFS5.CHATINPUTBOX`'s Return — its slot 4, **0x4C3310**, which the chat box has in place of
    /// the input boxes' 0x4CCC20:
    ///
    /// ```text
    /// 0x4C3325  focused? and 0x4C3332 the key is 0x0D?   otherwise 0x4CCC20, the shared handler
    /// 0x4C3364  text = accessor->vt[0x10]()              ; sent by 0x48D9B0, the caller's part
    /// 0x4C3376  accessor->vt[0x44]()                     ; clear (0x4BC9C0)
    /// 0x4C3388  this->vt[0x38]()                         ; slot 14, 0x4CC7B0, the box's re-measure
    /// ```
    ///
    /// so, unlike 0x4CCDB5's commit, the box **keeps its focus** and the line is taken whatever it
    /// holds, an empty one included. Answers the text when `index` is a chat input box.
    pub fn take_chat(&mut self, index: usize) -> Option<String> {
        (self.layout.elements.get(index)?.class == "NFS5.CHATINPUTBOX").then_some(())?;
        Some(self.edits.remove(&index).unwrap_or_default())
    }

    /// Type `text` into the input box bound to `accessor`, a character at a time through
    /// [`Self::type_char`] (diagnostic: `NFS_FE_KEYS`'s `type:`).
    pub fn type_into(&mut self, accessor: &str, text: &str) {
        if let Some(i) = self.layout.elements.iter().position(|e| e.get("accessor") == Some(accessor)) {
            for c in text.chars() {
                self.type_char(i, c);
            }
        }
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
    /// (`+1` at 0x4DB096), both after the click 0x4AADB0(8). Arms 3 and 4 (the knob and the track)
    /// start a drag instead — `combo_scroll_grab` / `combo_scroll_track` / `set_combo_scroll_pos`,
    /// called from `App::fe_left_button` the way `list_drag` already does for the list box's own
    /// scrollbar. `index` is the combo's flattened index (`combo_scroll_at`'s own), routed into an
    /// embedded screen the same way every other combo method is — *(until this fix, a plain
    /// `self.open_combo` here meant a nested panel's combo scrollbar never stepped at all; user
    /// report's knob-drag gap was one symptom of the same missing routing)*.
    pub fn combo_scroll_press(&mut self, index: usize, region: u32) -> bool {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get_mut(&k).is_some_and(|g| g.combo_scroll_press(j, region));
        }
        let Some((_, _, _, positions, first)) = self.combo_scroll_geom(index) else { return false };
        let next = match region {
            1 => first.saturating_sub(1),
            2 => (first + 1).min(positions.saturating_sub(1)),
            _ => return false,
        };
        if next == first {
            return false;
        }
        self.scroll.insert(index, next);
        true
    }

    fn drop_down(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let Some((rows, _)) = self.lists.get(&i) else { return };
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
        // **The panel is a depth mask's stencil** (read 2026-09-27): 0x4DAA10 (0x4C6516), the
        // gouraud (0x4C6605), 0x4DAAC0 (0x4C660D — the combo's arm jumps past the list box's
        // alpha-test pair, 0x4C6612 → 0x4C669C), then the outline, the rows and the scrollbar
        // (0x4C66E0, 0x4C670B…0x4C691B, 0x4C693D) and 0x4DAB00 (0x4C6942): everything after
        // the panel lands on the panel alone, and the row that overhangs its foot is cut there.
        // The depth is given when the overlay is appended (`overlay_mask`).
        let stencil = out.len();
        self.gouraud(
            out,
            aspect,
            // the rect's height is the rows' `h` 0x4C57D5 set (`drop_down_rect`) — *corrected
            // 2026-09-27: this drew `selectionboxmaxheight` whatever the list held (§4.44)*
            [[px, py], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py], [px, py + rows_h], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py + rows_h]],
            [ca, ca, cb, cb],
        );
        let inside = out.len();
        let black = tint([0, 0, 0, 255]);
        // the sixteen pixels the panel gave up carry the drop-down's own scrollbar (§4.90). Its
        // knob travel is the scrollbar model's own **count** (§4.59's `+2`), not the row total —
        // `combo_scroll_geom`'s doc comment has the read.
        if scrolls {
            self.combo_scrollbar(out, aspect, px + rect_w, py, max_h, self.last_first_row(i, rows) + 2, self.scroll.get(&i).copied().unwrap_or(0), &tint);
        }
        for (bx, by, bw, bh) in [(px, py, rect_w, 1.0), (px, py + rows_h - 1.0, rect_w, 1.0), (px, py, 1.0, rows_h), (px + rect_w - 1.0, py, 1.0, rows_h)] {
            out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), black));
        }
        let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
        // the same foot-of-loop test as the list box's (0x4C6915): the last row is drawn and the
        // mask cuts it at the panel's foot (it "overhangs" until 2026-09-27); the
        // first row drawn is the scrollbar's position `[+0x1BC]->slot10()` (0x4C66F8, §4.44) —
        // *until 2026-09-27 the port drew from row 0 whatever the bar said*
        // `FEOpponentCarAccessor`'s own row draw, `0x4F58A0` (vtable `0x5B77D8` slot 15 —
        // `controls.md` §4.140; it shares the row-draw shape and call site (`0x4C68F5`)
        // `FEColourAccessor`'s `0x4B9390` does, confirmed by both taking the exact same
        // `(row, font, x, y, w, inset=3.0, text, bar)` args off `0x4C687D`'s branch). Read to
        // the instruction: the shared bar/text colour block just above that branch
        // (`0x4C675E`…`0x4C67AE`) leaves `bar` (`ebx`) at **0** — transparent — for every row
        // except the one currently under the pointer, which takes `data.highlightcolorhover`
        // (`[+0x1E8]`) through `0x4D8830`; `ID_OPPONENTCARBOX`'s own `ComboBoxSlider.ctr` sets
        // no such key (`QuickRace.lay`), so it keeps the code default `0x4D87B0(0xFF)` —
        // opaque **`0xBC0000`**, dark red. `0x4F58A0` always draws that bar (`0x4D9280`, its
        // own literal `14.0`, matching `LIST_ROW_H`) before its row text, unconditionally for
        // its five rows — the generic loop's own `sel_c` bar just below, gated on `current`
        // rather than the pointer, is `FeListBox`'s "selected row" arm (`[+0x1DD]`, §4.42),
        // which never fires for a `ComboBoxSlider` (`[+0x1DD]==0`) and so never reaches these
        // rows in the original either way. COVERAGE §14.31.
        let opponent_car_hover = self
            .attr(e, "accessor")
            .is_some_and(|a| a.starts_with("OpponentCar.Car"))
            .then(|| self.pointer.unwrap_or(self.cursor))
            .flatten();
        let mut acc = 0.0f32;
        let mut ry = py;
        let first = self.scroll.get(&i).copied().unwrap_or(0);
        for (n, row) in rows.iter().enumerate().skip(first) {
            if acc > max_h {
                break;
            }
            acc += row.height();
            if opponent_car_hover.is_some_and(|(cx, cy)| self.combo_row_at(cx, cy) == Some((i, n))) {
                out.push(self.quad(aspect, px, ry, rect_w, row.height(), tint([0xBC, 0x00, 0x00, 0xFF])));
            }
            // No "selected row" bar and no white text here: that arm of the shared row draw
            // (0x4C677C, §4.42) is gated on `[this+0x1DD]`, which `FeComboBox::FeComboBox`
            // (0x4C4CF0) sets to 1 only for `NFS5.ListBox` (0x4CCE5A) and to 0 for
            // `NFS5.ComboBoxSlider` (0x4C4CA7) and `NFS5.ColorBox` (0x4C38A3) — the only widgets
            // whose open list comes through here (`draw_combo`). A list box's own selected row
            // is `draw_list_box`'s. COVERAGE §14.34 (until 2026-09-30 the port drew the combo's
            // current row black with white text).
            let fg = tint(text_c);
            match row {
                ListRow::Text(t) => self.text(out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg),
                // 0x4C688F hands the row draw an inset of 3.0 (§4.171)
                ListRow::Swatch(_) | ListRow::CustomColour(_) => self.colour_row(out, aspect, f, row, px, ry, 3.0, &tint),
                // A grouped car list's heading (`garage_rows`/`catalog_rows`, §4.144): the same
                // two-black-rule-plus-bevel shape `draw_list_box`'s own `Header`/`GroupHeading`
                // arms already draw for `Parts.AllParts`/the garage list — this match had no arm
                // for either variant at all, so a combo's own drop-down (`FeComboBox`/
                // `FeListBox` share one row loop, §4.42/§4.43, but not this one — `draw_combo`
                // reaches `drop_down`, not `draw_list_box`) drew the heading's reserved height as
                // **blank space**: the row total came out right (`OwnedCars.Car`'s 42/28/14
                // already matched) but the era/class text and its two rules never appeared. Found
                // porting `GenericCar.Car`/`DealerCar.Car`/`GenericCar2.Car`'s own grouping
                // (`catalog_rows`) and fixed for both — the garage combo drew this same blank gap
                // before now, uncaught because no screenshot check had looked inside an *open*
                // grouped combo specifically.
                ListRow::Header(t, _) | ListRow::GroupHeading(t) => {
                    let black = tint([0, 0, 0, 0xFF]);
                    out.push(self.quad(aspect, px, ry, rect_w, 1.0, black));
                    out.push(self.quad(aspect, px, ry + PART_HEADER_H, rect_w, 1.0, black));
                    self.text(out, aspect, f, t, px, ry + 2.0, rect_w, 1, tint(text_c));
                    let bevel_y = if matches!(row, ListRow::GroupHeading(_)) { ry + 1.0 } else { ry };
                    let bevel_h = if matches!(row, ListRow::GroupHeading(_)) { 12.0 } else { PART_HEADER_H };
                    self.bevel_heading(out, aspect, px, bevel_y, rect_w, bevel_h, 2.0);
                }
                ListRow::Columns(_) | ListRow::Racer(..) => {}
            }
            ry += row.height();
        }
        self.overlay_mask.set(Some((stencil, inside, out.len())));
    }

    /// `FEColourAccessor`'s row draw **0x4B9390**, the part after the row's bar: a colour row is a
    /// 20-wide swatch at `(x + inset, y + 2)`, `h − 4` tall, in the colour at the row's text alpha
    /// (`0x4D87F0(c, text >> 24)`, which leaves a colour alone at 0xFF), with a black outline
    /// (0x4D9200 of `0x4D87E0(alpha)`: four one-pixel bars); the custom row is `FEStrings` 0x263
    /// at `(x + inset, y + 2)`, align 0, in that black. Both take the row height from slot 26,
    /// the base 14 (0x4BC700). `RE/notes/controls.md` §4.171.
    pub(super) fn colour_row(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, row: &ListRow, x: f32, y: f32, inset: f32, tint: &impl Fn([u8; 4]) -> [f32; 4]) {
        let black = tint([0, 0, 0, 0xFF]);
        match row {
            ListRow::Swatch(c) => {
                let (sx, sy, sw, sh) = (x + inset, y + 2.0, 20.0, row.height() - 4.0);
                out.push(self.quad(aspect, sx, sy, sw, sh, tint(*c)));
                for (bx, by, bw, bh) in [(sx, sy, sw, 1.0), (sx, sy + sh - 1.0, sw, 1.0), (sx, sy, 1.0, sh), (sx + sw - 1.0, sy, 1.0, sh)] {
                    out.push(self.quad(aspect, bx, by, bw, bh, black));
                }
            }
            ListRow::CustomColour(t) => self.text(out, aspect, f, t, x + inset, y + 2.0, 0.0, 0, black),
            _ => {}
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
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: white, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
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

    /// The eight wells are the global `[0x691D98]` (getter 0x5019B0, setter 0x5019C0), not the
    /// widget's: the profile's `PlayerInfo` loads them (0x501888) and every screen's editor shows
    /// and edits the same eight. The caller hands them in when a screen is bound.
    pub fn set_picker_wells(&mut self, wells: [u32; 8]) {
        self.picker.wells = wells;
    }

    /// The `NFS5.ColorBox` whose picker is up, if one is.
    ///
    /// The original's gate is two things at 0x4C3D0C: the widget holds **focus** and the class's
    /// global latch `[0x65B710]` is set. The latch is raised by `0x4C39F0`, called from one place
    /// — 0x4C6D6C, in the shared combo commit, and only for a widget whose constructor byte
    /// `[this+0x1DE]` is set, which is `NFS5.ColorBox` alone (§4.44): the widget asks its accessor
    /// for the index it calls custom (`0x4B94B0`: `row == accessor->[+0x24]()`, the colour count —
    /// a function-pointer member, not a vtable slot) and opens the editor when the row just chosen
    /// **is** that one, the `custom...` row of the list (§4.171). *(Until 2026-09-27 the port
    /// opened the editor on the click that opens the list — an APPROXIMATION retired here.)*
    /// **`data.allowcustom` decides whether there is a custom colour to open at all.** The bind
    /// 0x4C3BF0 hands it to the colour accessor at `[this+0x220]` through 0x4B9590, which is one
    /// instruction — `[accessor+0x30] = flag` — and the accessor's own count reads that byte
    /// twice, at 0x4B9232 and 0x4B9353, adding `0x5019D0() + 1` entries to the list when it is set
    /// and none when it is not. So a box with `allowcustom=0` offers the car's paint sets and
    /// nothing else. Two of the eleven shipped boxes carry 0 and they are `dealership.lay`'s
    /// `DealerCar.Colour` and `DealerCar.IntColour` — you cannot repaint a car you have not bought
    /// — while the paint shop's four and the five race screens' all carry 1. §4.86.
    pub fn open_picker(&self) -> Option<usize> {
        self.open_combo.filter(|_| self.picker_latch).filter(|i| {
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

    /// The state whose part list a draw ending in **0x525830** walks (`[[this+0xD4]] +
    /// 0x525570()·4`, 0x525876…0x525886): [`state_for`](Self::state_for) when the template
    /// carries that state. Settled 2026-09-28: a name the template lacks (the port's keyboard
    /// `selected` on a class with no `[state.selected]`) leaves the list as it was — 0x5271F0
    /// finds no entry and copies nothing (0x527214, confirmed by reading it: the "not found" arm
    /// jumps straight past the `memmove` at 0x527216…0x527263) — so the object goes on drawing
    /// whichever state it last *did* find an entry for, however many ticks ago that was, not the
    /// `.lay` block's static `state=`. `last_drawn_state` is that per-widget memory; before the
    /// first successful match it falls back to the `.lay` block's `state=`, since that is what a
    /// freshly-bound widget's part list holds before any draw has run.
    fn drawn_state(&self, e: &nfs_formats::lay::Element, i: usize, selected: Option<usize>, hovered: Option<usize>) -> String {
        let live = self.state_for(e, i, selected, hovered);
        match self.controls.get(&e.class) {
            Some(c) if c.has_state(live) => {
                self.last_drawn_state.borrow_mut().insert(i, live.to_string());
                live.to_string()
            }
            _ => self
                .last_drawn_state
                .borrow()
                .get(&i)
                .cloned()
                .unwrap_or_else(|| e.get("state").unwrap_or("normal").to_string()),
        }
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
        // **`NFS5.CAMERAPICTURE`'s draw 0x4BF870**: `[this+0x144]` (`data.picturenum`) is compared
        // with each of `Camera.1 … Camera.4` (the loop 0x4BF883…0x4BF89D over `[this+0x154 … 0x160]`,
        // slot 10) and, when any one holds it and the widget is not focused (0x4BF8A5 `0x4C7DD0`,
        // `[+0x10C] & 0x40` — set only by 0x4C7D70 for a widget whose `[+0x110]` has 0x40, which
        // the base ctor 0x4C7480 clears and the base bind 0x4C7530 never sets), the state is
        // `highlighty` (0x4BF8AE, `0x4C7970` at 0x4BF8B5); otherwise the ordinary one (0x4BF8BE
        // `0x4C7B70`, 0x4BF8C9 `0x4C7A00`)
        if e.class == "NFS5.CAMERAPICTURE"
            && self.camera_list.iter().any(|&v| f64::from(v) == self.attr_num(e, "picturenum").unwrap_or(-1.0))
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
        match e.get("state").unwrap_or("normal") {
            "selected" if self.unselected.contains(&i) => "normal",
            s => s,
        }
    }

    /// A release on a tab button — `FERadioItem`'s slot 9, **0x4D0290**: the widget's window's
    /// own parent (`[[this+0xD0]+0xD0]`) cast to `FERadioGroup` (0x5A134B with 0x5D9030) and, when
    /// it is one, its slot 29 **0x4CFF30** with this button; then sound 5 (0x4AADB0) whatever the
    /// cast gave — the caller plays it. Answers whether `index` is a tab button.
    ///
    /// 0x4CFF30: the button already current (`[group+0x154]`) does nothing; otherwise the current
    /// one's slot 28 **0x4D0310** hides its panel (0x4CEC50) and the button becomes the pending one
    /// `[group+0x158]`, which the group's tick **0x4CFEB0** makes current — its slot 27 **0x4D02E0**:
    /// the `selected` flag (0x4C7D00), the panel's load (+0x6C, 0x4CEA90) and show (0x4CEC60) —
    /// once the old button is no longer selected and the panel's screen is neither going out
    /// (0x4E0E00 ≠ 3) nor closed (0x4E0E10) — `tick_panels`. So the old panel goes out and only
    /// then does the new one come in. *(Until 2026-09-26 the switch was made at once, an
    /// APPROXIMATION.)* A button's `panel` names a `NFS5.PANEL` of the screen that holds the strip
    /// (0x4D01F0 looks it up three windows up).
    pub fn tab_click(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize) -> bool {
        let Some((k, j)) = self.route(index) else { return false };
        let is_tab = |c: &str| c == "NFS5.TABBUTTON" || c == "NFS5.GRAPHICTABBUTTON";
        let strip_here = self.layout.elements.get(k).is_some_and(|e| e.class == "NFS5.RADIOGROUP")
            && self.groups.get(&k).is_some_and(|g| g.layout.elements.get(j).is_some_and(|e| is_tab(&e.class)));
        if !strip_here {
            // deeper: a strip inside a panel this screen holds
            return match self.groups.get_mut(&k) {
                Some(g) => g.tab_click(r, paths, j),
                None => false,
            };
        }
        let (old, new_panel, old_panel) = {
            let g = &self.groups[&k];
            let current = g.layout.elements.iter().enumerate().find(|(n, e)| {
                is_tab(&e.class) && (g.selected_flags.contains(n) || (e.get("state") == Some("selected") && !g.unselected.contains(n)))
            });
            let old = current.map(|(n, _)| n);
            if old == Some(j) {
                return true;
            }
            let panel_of = |n: usize| g.layout.elements.get(n).and_then(|e| e.get("panel")).filter(|p| !p.is_empty()).map(str::to_string);
            (old, panel_of(j), old.and_then(panel_of))
        };
        // 0x4CFF30: the current button's slot 28, 0x4D0310, hides its panel (0x4CEC50, the
        // sub-screen's +0x7C: out), and the button becomes the pending one
        if let Some(i) = old_panel.and_then(|p| self.panel_index(&p))
            && !self.closed.contains(&i)
            && let Some(p) = self.groups.get_mut(&i)
        {
            p.transition_out();
        }
        self.pending_tab = Some((k, j));
        // the new panel's layout, loaded now so the strip's tick can open it (0x4CEA90)
        if let Some(p) = new_panel
            && let Some(i) = self.panel_index(&p)
            && !self.groups.contains_key(&i)
        {
            self.set_panel(r, paths, &p, true);
            self.closed.insert(i);
        }
        let _ = old;
        true
    }

    /// Put the pointer somewhere, in design coordinates, or take it away.
    ///
    /// `RE/notes/controls.md` §4.39: the front end is mouse-driven, and hover is a **flag** —
    /// bit **0x04** of `[widget+0x10C]`, set by the base vtable's slot 6 (0x4C7B90) and cleared by
    /// slot 7 (0x4C7C50), each of which then re-runs `updateState` (0x4C82A0) to turn the flags
    /// into the state name the widget draws.
    /// Tooling: move the pointer for a scripted click (`NFS_FE_CLICKS`), even while
    /// `NFS_FE_CURSOR` parks the live one.
    pub fn script_cursor(&mut self, at: Option<(f32, f32)>) {
        self.pointer = None;
        self.place_cursor(at);
    }

    pub fn set_cursor(&mut self, at: Option<(f32, f32)>) {
        // a still parked its own pointer with `NFS_FE_CURSOR`; the live one must not clobber it
        if std::env::var("NFS_FE_CURSOR").is_ok() {
            return;
        }
        self.pointer = None;
        self.place_cursor(at);
    }

    /// Move only the cursor sprite: the widgets keep the pointer they last saw. What the front end
    /// does while 0x4D2850 swallows its input (a screen in transition, the room busy): the move
    /// events never reach the windows, whose hover `[+0x110]` / `[+0x114]` stay put (0x5289D9),
    /// while the cursor draw reads the device (0x4D32DB).
    pub fn set_pointer(&mut self, at: Option<(f32, f32)>) {
        if std::env::var("NFS_FE_CURSOR").is_ok() {
            return;
        }
        self.pointer = Some(at);
    }

    /// 0x4E0DE0 — `[+0x12C]` is 1 or 3: this screen, or the background it carries (`[fe+0xE4]`
    /// in the original, which the port keeps per screen), is sliding in or out.
    pub fn in_transition(&self) -> bool {
        let moving = |t: Transition| matches!(t, Transition::In { .. } | Transition::Out { .. });
        moving(self.transition) || self.background.as_ref().is_some_and(|b| moving(b.transition))
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
    /// class can hover, **unless** an open combo has the window's capture, in which case only the
    /// combo itself can ever hover.
    ///
    /// `Window::handleEvent`'s move arm **0x5289D9** (§4.56) is the enter/leave machine, and it is
    /// capture-aware in a way this file had not read until it was reported: after the raw hit
    /// test (`0x528C00`) it does not always send the **enter** (type 5) to what it found —
    ///
    /// ```text
    /// 0x528a3a  [this+0x110] = hit                       ; tracked regardless, for the next leave
    /// 0x528a38  if (captured == 0)                 -> send enter to hit
    /// 0x528a46      al = captured->slot18()             ; "accepts mouse input" (§4.38)
    /// 0x528a49      if (!al)                       -> send enter to hit
    /// 0x528a4d      if (captured == hit)           -> send enter to hit (to itself)
    /// 0x528a4f      else                           -> **no enter sent at all**
    /// ```
    ///
    /// (read directly from the image, `fn.py --asm 0x5289d9`, since the prose notes stopped short
    /// of the exact branch). `[window+0x120]` is the **captured** widget — the one holding focus,
    /// which a widget takes and keeps for as long as it is focused (§4.57's `setCapture` /
    /// `releaseCapture`) — and **slot 18** is *accepts mouse input*: the base answers false
    /// (0x50DD60), `NFS5.DEADZONE` and `NFS5.ComboBoxSlider`/`NFS5.ColorBox` answer true (0x510E90,
    /// §4.38). A combo's list is open exactly while it holds focus (§4.43), so **while any combo
    /// is open, it holds the window's capture and its slot 18 is true — and no other widget's
    /// hover flag is ever set**, anywhere on the screen, until it closes. The combo's own internal
    /// row highlight (§4.42, `combo_row_at`) is unrelated machinery and keeps working underneath
    /// this — only the *window-level* `[state.hover]` this function drives is gated.
    ///
    /// *(User report, 2026-09-30 (c): hovering a row of an open drop-down also hovered whatever was
    /// underneath it, including through a tab panel's own drop-down — the raw scan below is
    /// unchanged and still finds that widget, so the fix is the capture filter `hovered` applies
    /// to its answer.)*
    pub fn hovered(&self) -> Option<usize> {
        let hit = self.hovered_scan();
        match self.any_open_combo() {
            // the combo itself is exempt (0x528a4d: captured == hit still sends the enter) —
            // harmless either way, since ComboBox.ctr's `[state.hover]` draws exactly what
            // `[state.normal]` does (§4.4, item (d) below)
            Some(oc) => hit.filter(|h| *h == oc),
            None => hit,
        }
    }

    /// The combo whose drop-down is open, here or in a visible embedded screen, by its flattened
    /// index — `None` while nothing is open. The same routing `combo_row_at` and
    /// `combo_scroll_at` use for a nested panel's own combo.
    fn any_open_combo(&self) -> Option<usize> {
        if let Some(i) = self.open_combo {
            return Some(i);
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            if !self.closed.contains(&k)
                && let Some(j) = self.groups[&k].any_open_combo()
            {
                return Some(base + j);
            }
            base += self.groups[&k].span();
        }
        None
    }

    /// `hovered`'s own raw scan, before the capture filter above — the **first** section whose box
    /// contains it and whose class can hover.
    ///
    /// `Window::hitTest` **0x528C00** walks the child vector forwards and returns the **first**
    /// child that contains the point, and that vector is in section order (§4.67) with the draw
    /// walking it backwards — so the first section is both the topmost and the first tested.
    ///
    /// The box is the widget's own, the same `0x4C8670` gives the original. A widget with no size
    /// of its own has a box of (0, 0) here (§4.37) and so can never be hit; those are the 27
    /// bottom-bar buttons and graphic tabs, whose classes carry no hover state anyway.
    fn hovered_scan(&self) -> Option<usize> {
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
            // The window's hit test (0x528C00) asks every child's containsPoint (0x525A90) and
            // knows nothing of a hover state; `can_hover` stands in for "a widget that reacts to
            // the pointer" and is right for the buttons. The three text-entry classes react to a
            // click without ever having a `[state.hover]` (their `.ctr` states are `normal,
            // hidden, focused[, disabled]`): the release goes to their slot 9 (0x4CCB70,
            // 0x4C2220) which takes the focus. Without them here a click on `Comms.IP` (or the
            // chat line) never reached the box and nothing could be typed into it.
            let takes_click = matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX");
            if !(self.can_hover(&e.class) || takes_click) || self.state_draws_nothing(e) {
                continue;
            }
            // **A hidden widget contributes no box** (`[this+0x10C] & 0x20`, the bit 0x50D910's helper
            // sets through 0x4C7E80): slot 13 (0x4C86F0) writes four zeros for it, and 0x525A90 fails on
            // an empty box (`left == right`, 0x525AAF). So the garage button `ID_MERGEDLISTCARSETUP`
            // hidden under the paint box `ID_MERGEDLISTCOLOR` (same `pos`, and the earlier section, so
            // the first the hit test tries) takes no pointer — until 2026-09-30 a click on the colour box
            // fell through to it and opened `garage`.
            if self.hidden_flags.contains(&i) {
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
    /// `NFS5.CarLogo` (0x4C1CE0…0x4C1DAF) — `RE/notes/controls.md` §4.25. Both take the travel
    /// from 0x4E3550 — `[0x6907E0]`, set to **640.0f** by 0x4D35B4 — and keep their own timer and
    /// random wait ([`glints::Sweep`]): the header's `rand() % 128 + 128`, the logo's
    /// `rand() % 128`. The sprite's **x is the elapsed tick count itself**, at 128 Hz, so it
    /// crosses the whole screen at a pixel a tick, and its y is the widget's own. The header's
    /// timer only runs with the fade at 0 (0x4CB01C); the logo's always does.
    fn sweep_glint(&self, i: usize, logo: bool, out: &mut Vec<Sprite>, aspect: f32, y: f32, alpha: f32) {
        if !logo && self.widget_progress() * 255 / TRANS_FULL != 0 {
            return;
        }
        let now = self.ticks() as i64;
        let elapsed = self.step_glint(
            i,
            now,
            || glints::Glint::Sweep(glints::Sweep::new(0, logo)),
            |g, t, _| match g {
                glints::Glint::Sweep(s) => s.step(t),
                _ => None,
            },
        );
        let Some(elapsed) = elapsed else { return };
        let Some(n) = self.glint else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(&("weeicons.fsh".to_string(), n)) else { return };
        let (pos, size) = self.place(aspect, elapsed as f32, y, aw, ah);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
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
///
/// The width tested is 0x536970's **third** output (`[esp+0x18]` against the box at 0x523754):
/// the box of the glyph quads widened to the pen's end and the low pen mark
/// (`BitmapFont::measure().0`), not the pen's advance the port used before.
pub fn wrap_lines(f: &BitmapFont, s: &str, w: f32) -> Vec<String> {
    wrap_lines_by(&|t: &str| f.measure(t).0, s, w)
}

/// The wrap object's line pitch and total height, as 0x523600 keeps them. Each committed line
/// was last measured by 0x536970 at 0x52374F, and its **height** — the fourth output, the span of
/// its glyph quads, not the font's line step — plus 2.0 (`[0x5B2580]`) is added to the total
/// `[+4]` and written to `[+0x10]` (0x523912…0x52393F). The total is what 0x523250 answers (the
/// tooltip's and the dialog's height); the draw 0x523290 steps every line down by `[+0x10]`
/// (0x523576, 0x5235DA), which is therefore the **last** line's height + 2.
pub fn wrap_metrics(f: &BitmapFont, lines: &[String]) -> (f32, f32) {
    let step = |l: &String| f.measure(l).1 + 2.0;
    let total = lines.iter().map(step).sum();
    (lines.last().map_or(0.0, step), total)
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
    /// Draw the flourish of `glint_pieces` at the widget's origin, on the widget's own timer
    /// ([`glints::Flourish`]): a run of `rand() % 32 + 64` ticks, then `rand() % 256 + 256` of
    /// rest (0x51FE6C, 0x51FEC2), the variant redrawn each run for `animate` 5 (0x51FE8E), and a
    /// run starts only when the caller's `may_start` says so. `built` is the tick the object was
    /// made at — the screen's open for the ones a bind builds, the first draw for the combo
    /// family's, which its draw builds (0x4C5937).
    #[allow(clippy::too_many_arguments)]
    fn glinty_flourish(&self, i: usize, built: Option<i64>, may_start: &dyn Fn(&mut glints::Flourish) -> bool, animate: i32, out: &mut Vec<Sprite>, aspect: f32, px: f32, py: f32, w: i32, h: i32) {
        let now = self.ticks() as i64;
        let run = self.step_glint(
            i,
            now,
            || glints::Glint::Flourish(glints::Flourish::new(built.unwrap_or(now))),
            |g, t, _| match g {
                glints::Glint::Flourish(f) => {
                    let ok = may_start(f);
                    f.step(t, ok, animate)
                }
                _ => None,
            },
        );
        let Some((elapsed, duration, variant)) = run else { return };
        if w < 2 || h < 2 {
            return;
        }
        let band = glint_band(w, h);
        if band <= 0 {
            return;
        }
        // 0x51FF32…0x51FF4D: `(band + w + h)·elapsed / duration − band`, a signed `idiv`
        let p = ((band + w + h) as i64 * elapsed / duration) as i32 - band;
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

    /// The chosen driver picture's frame, **0x521B60** — one caller, `NFS5.PLAYERDRIVERPICTURE`'s
    /// draw at 0x4CF87C, with `(x, y, w, h, fade, 5)`. Two colours go through the fade (0x4D87F0):
    /// **0xFFFFFFFF** (0x521B6E) and **0x80FFFFFF** (0x521B7B). Four gouraud quads 0x4D98D0 ring
    /// the box `t` pixels wide, opaque white where they touch it and half white at the outside:
    ///
    /// | band | upper-left | upper-right | lower-left | lower-right |
    /// |---|---|---|---|---|
    /// | top (0x521BE2) | `(x−t, y−t)` 80 | `(x+w+t, y−t)` 80 | `(x, y)` FF | `(x+w, y)` FF |
    /// | bottom (0x521C26) | `(x, y+h)` FF | `(x+w, y+h)` FF | `(x−t, y+h+t)` 80 | `(x+w+t, y+h+t)` 80 |
    /// | left (0x521C63) | `(x−t, y−t)` 80 | `(x, y)` FF | `(x−t, y+h+t)` 80 | `(x, y+h)` FF |
    /// | right (0x521C9C) | `(x+w, y)` FF | `(x+w+t, y−t)` 80 | `(x+w, y+h)` FF | `(x+w+t, y+h+t)` 80 |
    ///
    /// Unlike 0x521050 and 0x520F00 it asks no global gate. CONFIRMED (symbolic walk of the stack).
    fn picture_frame(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let (inner, outer) = (tint([0xff, 0xff, 0xff, 0xff]), tint([0xff, 0xff, 0xff, 0x80]));
        let (r, b) = (x + w, y + h);
        self.gouraud(out, aspect, [[x - t, y - t], [r + t, y - t], [x, y], [r, y]], [outer, outer, inner, inner]);
        self.gouraud(out, aspect, [[x, b], [r, b], [x - t, b + t], [r + t, b + t]], [inner, inner, outer, outer]);
        self.gouraud(out, aspect, [[x - t, y - t], [x, y], [x - t, b + t], [x, b]], [outer, inner, outer, inner]);
        self.gouraud(out, aspect, [[r, y], [r + t, y - t], [r, b], [r + t, b + t]], [inner, outer, inner, outer]);
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
    /// Each band is one gouraud trapezoid, drawn as two per-corner triangles ([`Self::gouraud`]).
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
    /// Each half is one gouraud rectangle ([`Self::gouraud`]); the port stepped them in one-pixel
    /// strips until 2026-09-24.
    fn glass(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, tint: &dyn Fn([u8; 4]) -> [f32; 4]) {
        let half = (h - 2.0) * 0.5;
        if half < 1.0 || w < 3.0 {
            return;
        }
        // each half one 0x4D96B0 rectangle, its top pair one colour and its bottom pair the other
        for (top, c0, c1) in [
            (y + 1.0, tint([0xFF, 0xFF, 0xFF, 0x80]), tint([0xFF, 0xFF, 0xFF, 0])),
            (y + 1.0 + half, tint([0, 0, 0, 0]), tint([0, 0, 0, 0x91])),
        ]
        {
            let (l, r, b) = (x + 1.0, x + w - 1.0, top + half);
            self.gouraud(out, aspect, [[l, top], [r, top], [l, b], [r, b]], [c0, c0, c1, c1]);
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
    /// Each band is one gouraud quad ([`Self::bevel_bands`]).
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

    /// **0x521400**, the list headings' bevel — `drawPartsHeading` 0x518DE0 hands it
    /// `(x, y, w, 13.0, fade, 2)` after the name (0x518E8A), and the car lists' row painters
    /// 0x4F3510 / 0x4F4AD0 / 0x4F5D10 `(x, y + 1, w, 12.0, fade, 2)` after each era or class
    /// heading (0x4F360C, 0x4F5E0C, …). Behind the glint gate 0x4E35E0 like §4.21's. Eleven
    /// colours go through the fade 0x4D87F0 (0x521418…0x5214A0) and five die in `eax`
    /// (0x80FFFFFF, 0x80000000, 0x00FFFFFF, 0, 0xB2FFFFFF); the six that live make four 0x4D98D0
    /// quads, `R = x + w`, `B = y + h`:
    ///
    /// ```text
    /// band            upper-left            upper-right          lower-left           lower-right
    /// top    0x5214FF (x, y)      19FFFFFF  (R, y)     19FFFFFF  (x+t, y+t) E5FFFFFF  (R-t, y+t) CCFFFFFF
    /// left   0x521546 (x, y)      B2000000  (x+t, y+t) 19000000  (x, B)     CC000000  (x+t, B-t) 19000000
    /// right  0x52156F (R-t, y+t)  19FFFFFF  (R, y)     E5FFFFFF  (R-t, B-t) 19FFFFFF  (R, B)     CCFFFFFF
    /// bottom 0x5215A5 (x+t, B-t)  B2000000  (R-t, B-t) CC000000  (x, B)     19000000  (R, B)     19000000
    /// ```
    ///
    /// — a white glow under the top edge and inside the right one, a dark one inside the left edge
    /// and above the bottom. CONFIRMED (symbolic walk of the stack, 2026-09-27).
    pub(super) fn bevel_heading(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32, t: f32) {
        let c = |v: u32| tint([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
        let (r, b) = (x + w, y + h);
        let (w19, we5, wcc) = (c(0x19FF_FFFF), c(0xE5FF_FFFF), c(0xCCFF_FFFF));
        let (b19, bb2, bcc) = (c(0x1900_0000), c(0xB200_0000), c(0xCC00_0000));
        self.gouraud(out, aspect, [[x, y], [r, y], [x + t, y + t], [r - t, y + t]], [w19, w19, we5, wcc]);
        self.gouraud(out, aspect, [[x, y], [x + t, y + t], [x, b], [x + t, b - t]], [bb2, b19, bcc, b19]);
        self.gouraud(out, aspect, [[r - t, y + t], [r, y], [r - t, b - t], [r, b]], [w19, we5, w19, wcc]);
        self.gouraud(out, aspect, [[x + t, b - t], [r - t, b - t], [x, b], [r, b]], [bb2, bcc, b19, b19]);
    }

    /// What the four-band bevels share: each band is an **outer** segment with a colour at either
    /// end and an **inner** one the same — one call of the gouraud quad primitive 0x4D98D0 with the
    /// outer pair as its upper corners and the inner pair as its lower ones (§4.74), drawn through
    /// [`Self::gouraud`]. This port stepped each band into `t` strips across and twelve along until
    /// 2026-09-24; `t` is kept in the signature for the callers.
    #[allow(clippy::type_complexity)]
    fn bevel_bands(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        bands: &[([f32; 2], [f32; 2], [[f32; 4]; 2], [f32; 2], [f32; 2], [[f32; 4]; 2])],
        _t: f32,
    ) {
        for &(a0, a1, ca, c0, c1, cb) in bands {
            self.gouraud(out, aspect, [a0, a1, c0, c1], [ca[0], ca[1], cb[0], cb[1]]);
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
    /// The stamps are the widget's own ([`Typing`]): the typing counts from the tick of the first
    /// draw that finds the wait over, and holds while the screen is in a transition (0x4C8820).
    /// *(Until 2026-09-26 this counted from tick 152 exactly and ignored the transition, an
    /// APPROXIMATION "one frame wide".)* The activation is the screen's open, tick 0 of its clock.
    /// `NFS_FE_CLOCK` preloads that clock with no frames behind it, so a widget first drawn past
    /// the wait takes the wait as having ended on its last tick (152) — the diagnostic's stand-in
    /// for the frames it skipped, which the game never meets (its first draw is at tick 0…2).
    ///
    /// The answer is the character budget and the ticks into the current character,
    /// `([+0x150] − [+0x14C]) % 3` — the remainder the same signed divide leaves in `edx`
    /// (0x52331D), which fades the leading letter in (`wrapped_text`).
    fn typewriter(&self, i: usize, e: &nfs_formats::lay::Element, restart: Option<i64>) -> Option<(usize, i64)> {
        let animating = self.attr_num(e, "animating").unwrap_or(0.0) != 0.0;
        let now = self.ticks() as i64;
        let mut all = self.typing.borrow_mut();
        let t = all.entry(i).or_insert_with(|| {
            let t = Typing::new(0, animating);
            if t.wait != 0 && now > t.wait {
                return Typing { start: t.wait + 1, last: t.wait + 1, wait: 0 };
            }
            t
        });
        if let Some(at) = restart {
            t.restart(at);
        }
        // 0x4C8820: the fade's `p·255 / 512`, zero only at the very ends of a transition
        let transitioning = self.widget_progress() * 255 / TRANS_FULL != 0;
        let shown = t.draw(now, transitioning);
        if !animating {
            return None;
        }
        Some((shown.unwrap_or(0), ((t.last - t.start) % 3).max(0)))
    }

    /// The progress a widget of this screen transitions with — 0x4E0D90(1) as 0x4C8760 and
    /// 0x4C8820 read it.
    fn widget_progress(&self) -> i32 {
        self.widget_state().1
    }

    /// Which way a widget of this screen transitions and how far — 0x4C8760 / 0x4C8840: the
    /// state is the widget's screen's `[+0x12C]`, and when that screen is up (2) the one on show
    /// (0x4D63C0, the front screen of the stack); the progress is 0x4E0D90(1), the larger of the
    /// screen's own and the front screen's (0x4D6300). So a panel's sub-screen at rest moves with
    /// the screen it is on, and one of its own transitions runs on top of the screen's.
    fn widget_state(&self) -> (bool, i32) {
        let (parent_out, parent_p) = self.inherited.get();
        match self.transition {
            Transition::Idle => (parent_out, parent_p),
            t => (matches!(t, Transition::Out { .. }), t.progress().max(parent_p)),
        }
    }

    /// `NFS5.WORDWRAPACCESSORWATCH` — the class the front end uses where a value is a sentence
    /// rather than a figure: a tournament's description, a part's, a mission briefing.
    ///
    /// The break is the original's, **0x523600** (`RE/notes/controls.md` §4.51), and the lines sit
    /// the last line's measured height + 2 apart ([`wrap_metrics`], 0x523916 / 0x523576) — not the
    /// font's line height, as this said before.
    #[allow(clippy::too_many_arguments)]
    fn wrapped_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4], budget: Option<(usize, i64)>) {
        if w <= 0.0 {
            self.text(out, aspect, f, s, x, y, w, align, color);
            return;
        }
        let mut ly = y;
        // `budget` is `data.animating`'s character allowance — 0x523327's loop walks the line
        // vector and, for each line, draws it whole while `strlen(line) <= ebx` and takes `ebx`
        // off the budget (0x523340…0x52336E), or cuts one line at `ebx` characters and stops
        // (0x523572 zeroes the budget). §4.87.
        let (mut left, rem) = budget.unwrap_or((usize::MAX, 0));
        let lines = wrap_lines(f, s, w);
        let (pitch, _) = wrap_metrics(f, &lines);
        for line in lines {
            if left == 0 {
                break;
            }
            let chars: Vec<char> = line.chars().collect();
            if chars.len() <= left {
                left -= chars.len();
                self.text(out, aspect, f, &line, x, ly, w, align, color);
            } else {
                self.typed_line(out, aspect, f, &chars, left, rem, x, ly, w, align, color);
                left = 0;
            }
            ly += pitch;
        }
    }

    /// The line the typewriter is in, 0x52337D…0x523569 (read 2026-09-27). With `n` the
    /// characters of it shown, the NUL goes at `n − 1` (0x523392): the first `n − 1` are drawn
    /// with 0x4DA530, and the **`n`th alone**, `ch`, the leading letter. The anchor `x0` is
    /// `_ftol` of the aligned x (`x`, `x + w/2`, `x + w`: 0x5232A9…0x5232CD, 0x523395), and per
    /// alignment (0x5233A2's switch; `p1`/`p3` are 0x536970's minX / width):
    ///
    /// ```text
    /// 0  the prefix at x0, the letter at _ftol(x0 + p3(prefix) − 1)            0x523478…0x5234AA
    /// 1  the letter at _ftol(p3(prefix)/2 + x0), the prefix at
    ///    _ftol(x0 − p1(line from n + 1)/2)                                      0x5233FB…0x52346F
    /// 2  the letter at x0, the prefix at _ftol(x0 − p1(line from n + 1))        0x5233B9…0x5233EF
    /// ```
    ///
    /// — the second measure starts one character past the letter (`[ebx+ebp+1]`) and reads the
    /// box's **minX**, not its width (`[esp+0x1C]` / `[esp+0x14]` are the first output), as
    /// written. Then, unless 0x4E35C0 answers true or the object has no `GLNT` (`[+0xDC]`, 0x523105),
    /// **a depth mask** (0x5234EC 0x4DAA10 … 0x523530 0x4DAAC0 … 0x523569 0x4DAB00): the letter is
    /// the stencil, in the text's colour at alpha × `rem·255/3` / 255 (0x4D87F0 of the ticks into the
    /// letter, 0x5234F1…0x523511 — so it fades in over its three ticks, and at 0 it is not drawn
    /// and writes no depth), and inside it the `weeicons.fsh` `GLNT` at `(letter x − 18, y)` in
    /// `0x4D87D0(alpha)` (0x523535…0x523561): a glint on the letter being typed. Without the
    /// mask's conditions the letter is not drawn at all (0x52356E restores it).
    #[allow(clippy::too_many_arguments)]
    fn typed_line(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, chars: &[char], n: usize, rem: i64, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        let prefix: String = chars[..n - 1].iter().collect();
        let ch = chars[n - 1].to_string();
        let rest: String = chars.get(n + 1..).map(|r| r.iter().collect()).unwrap_or_default();
        let x0 = match align {
            1 => x + w * 0.5,
            2 => x + w,
            _ => x,
        }
        .trunc();
        let (px, cx) = match align {
            1 => ((x0 - f.measure_box(&rest).0 * 0.5).trunc(), (f.measure_box(&prefix).2 * 0.5 + x0).trunc()),
            2 => ((x0 - f.measure_box(&rest).0).trunc(), x0),
            _ => (x0, (x0 + f.measure_box(&prefix).2 - 1.0).trunc()),
        };
        let align = align.clamp(0, 2);
        self.text(out, aspect, f, &prefix, px, y, 0.0, align, color);
        let Some(g) = self.glint.and_then(|n| self.shapes.get(&("weeicons.fsh".to_string(), n))).copied() else { return };
        let mask = fe_depth::mask_begin();
        // 0x4D87F0: alpha × k / 255 with `mul 0x80808081` (unsigned, truncating), k = rem·255/3
        let a8 = (color[3].clamp(0.0, 1.0) * 255.0).round() as u32;
        let k = (rem.clamp(0, 2) * 255 / 3) as u32;
        let letter = [color[0], color[1], color[2], (a8 * k / 255) as f32 / 255.0];
        let stencil = out.len();
        self.text(out, aspect, f, &ch, cx, y, 0.0, align, letter);
        fe_depth::apply(&mut out[stencil..], mask.stencil);
        let (tex, gw, gh) = g;
        let (pos, size) = self.place(aspect, cx - 18.0, y, gw, gh);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, a8 as f32 / 255.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: Some(mask.inside) });
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
            out.push(Sprite { texture: tex, pos, size, uv_min: uv0, uv_max: uv1, color, rotation: 0.0, pivot: [0.0, 0.0], tri: None, corner_colors: None, depth: None });
            // `NFS_TEXT_AB=1` repeats each glyph one line lower **in red**, same texture, same UV
            // window, same size — so one image carries the widget's text twice and the only
            // difference between the two is `color.rgb`. The red copy draws and the black original
            // does not (`AUDIT.md` F2).
            if std::env::var("NFS_TEXT_AB").is_ok() {
                out.push(Sprite { texture: f.texture(), pos: [pos[0], pos[1] + size[1] * 1.2], size, uv_min: uv0, uv_max: uv1, color: [1.0, 0.0, 0.0, 1.0], rotation: 0.0, pivot: [0.0, 0.0], tri: None, corner_colors: None, depth: None });
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

    /// The screen **without** the background screen it names — how a screen draws when it is
    /// one layer of several: the front end's draw 0x4D2F30 paints the one background object
    /// `[fe+0xE4]` (the top screen's `data.screen`, swapped by 0x4D1BD0) and then the layers'
    /// screens, bottom first (0x4D30F8, 0x4D323E), so a covered screen shows between the two.
    pub fn sprites_own(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.sprites_reporting_bg(aspect, strings, selected, accessor, &mut Vec::new(), false)
    }

    /// The background screen alone (`[fe+0xE4]`), empty when the screen names none.
    pub fn background_sprites(&self, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.background.as_ref().map(|bg| bg.sprites(aspect, strings, None, accessor)).unwrap_or_default()
    }

    fn sprites_reporting(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>) -> Vec<Sprite> {
        self.sprites_reporting_bg(aspect, strings, selected, accessor, blank, true)
    }

    fn sprites_reporting_bg(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>, with_bg: bool) -> Vec<Sprite> {
        let mut out = Vec::new();
        if let Some(bg) = self.background.as_ref().filter(|_| with_bg) {
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
        self.overlay_mask.set(None);
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
                // `NFS5.CHRONICLESLIDER` is the same helper (0x4D0B90, `[helper+0x130] == 0`)
                "NFS5.CarSettingSlider" | "NFS5.CHRONICLESLIDER" => self.draw_car_setting_slider(d, &mut out),
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
                    // a per-corner triangle carries its own colours
                    if let Some((_, cols)) = s.tri.as_mut() {
                        for c in cols {
                            c[3] *= talpha;
                        }
                    }
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
        // the drop-down's depth mask is stepped here, after every widget's own draw, where its
        // sprites go (0x4DAA10 at 0x4C6516 in the later pass)
        if let Some((a, b, c)) = self.overlay_mask.take() {
            let m = fe_depth::mask_begin();
            fe_depth::apply(&mut overlay[a..b], m.stencil);
            fe_depth::apply(&mut overlay[b..c], m.inside);
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

    /// `FeListScrollBar`'s knob placement (0x4DB220) and its drag (0x4DB9B0) are exact inverses,
    /// both dividing by the position **count**, not `count − 1` — verified 2026-09-30 against the
    /// live game's own `paintshop` exterior-colour list (`count` 4, `travel` 4 design px there,
    /// too small a range for a screenshot to show the fix — `combo_drag_layout.diff`'s own
    /// before/after screenshots of that list are visually identical for exactly this reason). A
    /// wider list makes the bug concrete: with `count = 4` (as in that live list) the old,
    /// wrong `count − 1 = 3` divisor gives a knob position one whole design pixel further down
    /// than the original at `first = 3` — small, but the same off-by-a-row error compounds on a
    /// list with dozens of rows (a settings/profile list), where `count − 1` drifts the knob and
    /// the drag noticeably short of the original's own placement.
    #[test]
    fn the_combo_knob_and_its_drag_divide_by_count_not_count_minus_one() {
        // 0x4DB220 on a wide list: h=200, count=50, first=25 — knobH=max(16,200-26-50)=124,
        // travel=200-26-124=50, top=y+13+25*50/50=y+13+25
        let (top, knob_h, travel) = combo_knob(100.0, 200.0, 50, 25);
        assert_eq!(knob_h, 124.0);
        assert_eq!(travel, 50);
        assert_eq!(top, 100.0 + 13.0 + 25.0);
        // 0x4DB9B0, its exact inverse: pos = grab_at + (py-grab_y)*count/travel
        assert_eq!(combo_drag_pos(150.0, 100.0, 25, 50, travel), Some(25 + 50 * 50 / 50));

        // the live `paintshop` ExtColour list's own numbers: h=160, count=4 — knobH=max(16,160-
        // 26-4)=130, travel=160-26-130=4, so a whole drop-down's worth of scroll is 4 design px
        let (_, knob_h, travel) = combo_knob(0.0, 160.0, 4, 0);
        assert_eq!((knob_h, travel), (130.0, 4));
        // at first=3 (the last position) the correct divisor (4) gives 3*4/4=3; the bug this
        // fix replaced (dividing by count-1=3) gave 3*4/3=4 — one design px further down than
        // the original, and forced full-value clamps to look identical between the two, exactly
        // what the screenshots on this same list show
        assert_eq!(3 * travel / 4, 3);
        assert_eq!(3 * travel / 3, 4);
    }

    /// `0x529220(this, 0x96, 1)`'s own rule — a 150 ms period, one step per period elapsed, not
    /// per game tick — read at the combo drop-down's own scrollbar arrow (`FeListScrollBar`
    /// `0x4DB040`/`0x4DB1D0`, §4.101), the same shape `NFS5.VerticalSlider`'s bar and
    /// `NFS5.CARSETTINGBUTTON` already use for their own held arrows.
    #[test]
    fn the_scrollbar_arrow_repeats_every_150_ms_not_once_per_tick() {
        // under one period: no step yet, and the whole elapsed time is banked
        assert_eq!(repeat_steps(0, 149, 150), (0, 149));
        // exactly one period: one step, nothing left over
        assert_eq!(repeat_steps(0, 150, 150), (1, 0));
        // several periods in one long frame (a stall, or a coarse tick): one step per period
        // completed, not one step per call — the timer would have fired three times
        assert_eq!(repeat_steps(0, 449, 150), (2, 149));
        // the accumulator carries a partial period across frames, exactly like the already-
        // ported `list_repeat`/`setting_repeat` — two 100 ms frames (200 ms banked) step once
        // and carry 50 ms, not zero
        let (steps, acc) = repeat_steps(0, 100, 150);
        assert_eq!((steps, acc), (0, 100));
        assert_eq!(repeat_steps(acc, 100, 150), (1, 50));
    }

    /// `FEStringAccessor::insert` 0x4BCA90's own forbidden set — the nine bytes Windows refuses
    /// in a file name, `\ / : * ? " < > |` — found on `UserList.NameInput`'s profile-name box,
    /// the port's own save filename source (`layout.rs::type_char`'s doc).
    #[test]
    fn the_nine_filename_illegal_bytes_and_nothing_else() {
        for c in ['\\', '/', ':', '*', '?', '"', '<', '>', '|'] {
            assert!(filename_illegal_char(c as u32), "{c:?} is Windows-illegal in a file name");
        }
        // every ordinary letter/digit, the space, and '%' (already refused one layer up by the
        // shared key handler, so this layer does not need to name it too) all pass this filter
        for c in ['a', 'Z', '5', ' ', '%', '\'', '-', '_', '.', ',', '!'] {
            assert!(!filename_illegal_char(c as u32), "{c:?} is not one of the nine");
        }
    }

    /// 0x4E25A0: a silent movie (no `data.audio`) only clamps a genuine backlog — more than one
    /// frame due — to one and drops the rest; the ordinary one-frame case carries its true
    /// remainder exactly as the audio path does (`ebp == edi + 1` skips the clamp, 0x4e2634); one
    /// with audio always catches up in full.
    #[test]
    fn a_silent_movie_only_clamps_a_genuine_backlog() {
        let period = 1u64 << 16; // 1 ms, for round numbers
        // under a period: no frame either way
        assert_eq!(movie_advance_frames(0, period, false), (0, 0));
        assert_eq!(movie_advance_frames(0, period, true), (0, 0));
        // one frame due, with a remainder: carried both ways — the common case under a fixed
        // small step, where the clamp must not fire on every tick
        assert_eq!(movie_advance_frames(period + 5000, period, false), (5000, 1));
        assert_eq!(movie_advance_frames(period + 5000, period, true), (5000, 1));
        // a stall worth 5 periods: with audio all five decode and the remainder carries; without
        // it, one frame only, and the other four (and their remainder) are dropped, not queued
        // for the next call
        assert_eq!(movie_advance_frames(period * 5 + 3, period, true), (3, 5));
        assert_eq!(movie_advance_frames(period * 5 + 3, period, false), (0, 1));
    }

    #[test]
    fn the_typing_starts_on_the_first_draw_past_the_wait_and_holds_in_a_transition() {
        let mut t = Typing::new(0, true);
        assert_eq!(t.draw(2, true), None);
        assert_eq!(t.draw(151, false), None, "151 is not more than 0x97");
        // a frame that spans ticks 152…155 lands on 155: the typing counts from there
        assert_eq!(t.draw(155, false), None, "the reset draws nothing either");
        assert_eq!(t, Typing { start: 155, last: 155, wait: 0 });
        assert_eq!(t.draw(164, false), Some(3));
        // a transition stops `last`, so the budget holds
        assert_eq!(t.draw(200, true), Some(3));
        assert_eq!(t.draw(200, false), Some(15));
        t.restart(210);
        assert_eq!(t.draw(212, false), Some(0));
        // one that does not animate has no wait
        assert_eq!(Typing::new(0, false).draw(1, false), Some(0));
    }

    #[test]
    fn a_wordwrap_watch_fades_its_old_text_out_before_typing_the_new_one() {
        let mut w = WrapWatch { text: "old".into(), changing: None, restart: None };
        w.tick("old", 10, true);
        assert_eq!(w.fade(10), 1.0);
        w.tick("new", 20, true);
        assert_eq!((w.text.as_str(), w.changing), ("old", Some(20)));
        // (20 − 52)·4 + 0x100 = 128
        assert!((w.fade(52) - 128.0 / 255.0).abs() < 1e-6);
        w.tick("new", 84, true);
        assert_eq!(w.text, "old", "64 ticks is not yet more than 0x40");
        w.tick("new", 85, true);
        assert_eq!((w.text.as_str(), w.restart, w.changing), ("new", Some(85), None));
        // one that does not animate takes the new text on the same tick
        let mut q = WrapWatch { text: "a".into(), changing: None, restart: None };
        q.tick("b", 5, false);
        assert_eq!((q.text.as_str(), q.changing), ("b", None));
    }

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
