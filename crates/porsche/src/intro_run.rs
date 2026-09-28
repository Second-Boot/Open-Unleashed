//! The start-up movies on screen: 0x4DD600's `earts-av.mad` and 0x4DD4A0's `inPC-av.mad`, each
//! through the player 0x4DC850 (`nfs_game::intro`, `RE/notes/mad.md` §11–§12), before the loading
//! mosaic. The window and renderer are the application's; they are handed back when both movies
//! are over.

use nfs_assets::GamePaths;
use nfs_formats::mad::{MadDecoder, MadHeader, MadTables};
use nfs_game::intro::{IntroPlayer, Kind, Tick, placement};
use nfs_renderer::{Camera, Renderer, Scene, Sprite, TextureData, TextureId};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use winit::window::Window;

/// The two movies in the order 0x4DD600 plays them: `"%searts-av.mad"` (0x5D99BC) and, in
/// 0x4DD4A0, `"%sinPC-av.mad"` (0x5D998C), over the movie directory `[0x65B32C]`. A missing one is
/// passed over (0x59DC30 at 0x4DD65F / 0x4DD507).
const MOVIES: [&str; 2] = ["earts-av.mad", "inPC-av.mad"];

/// Before each movie, three blank flips (0x4DD6A2…0x4DD6C4 / 0x4DD54B…0x4DD56D).
const BLANK_FLIPS: u32 = 3;

struct Movie {
    data: Vec<u8>,
    video: Vec<std::ops::Range<usize>>,
    dec: MadDecoder,
    player: IntroPlayer<Vec<u8>>,
    voice: Option<nfs_audio::VoiceId>,
    size: (i32, i32),
    started: bool,
}

pub struct IntroRun {
    pub window: Arc<Window>,
    pub renderer: Renderer,
    files: VecDeque<PathBuf>,
    tables: Option<MadTables>,
    movie: Option<Movie>,
    blank: u32,
    tex: Option<(TextureId, (i32, i32))>,
    /// the keyboard ring 0x55FF60 reads, one entry a key: `true` for Esc (0x1B)
    keys: VecDeque<bool>,
    /// the mouse device's four buttons, 0x5372A0(6) `+0x10…+0x13`
    buttons: [bool; 4],
    t0: Instant,
}

impl IntroRun {
    pub fn new(window: Arc<Window>, renderer: Renderer, paths: &GamePaths) -> Self {
        let dir = paths.fe_data().join("Movies");
        let files = MOVIES.iter().filter_map(|n| GamePaths::find_file(&dir, n)).collect();
        let tables = MadTables::load(&paths.root).ok();
        if tables.is_none() {
            tracing::warn!("start-up movies: the decoder's tables could not be read (madtables.bin / nfs5.exe)");
        }
        window.request_redraw();
        Self { window, renderer, files, tables, movie: None, blank: 0, tex: None, keys: VecDeque::new(), buttons: [false; 4], t0: Instant::now() }
    }

    pub fn key(&mut self, esc: bool) {
        self.keys.push_back(esc);
    }

    pub fn button(&mut self, i: usize, down: bool) {
        if let Some(b) = self.buttons.get_mut(i) {
            *b = down;
        }
    }

    fn open(&mut self, path: &PathBuf) -> Option<Movie> {
        let tables = self.tables.clone()?;
        let data = std::fs::read(path).ok()?;
        let mut video = Vec::new();
        let mut kinds = Vec::new();
        let mut at = 0usize;
        for c in nfs_formats::mad::chunks(&data) {
            if let Some(k) = Kind::of(&c.tag) {
                kinds.push(k);
                video.push(at..at + c.data.len());
            }
            at += c.data.len();
        }
        let first = video.first()?.clone();
        let h = MadHeader::parse(&data[first]).ok()?;
        let dec = MadDecoder::new(tables, &h);
        let player = IntroPlayer::new(kinds, h.period);
        Some(Movie { data, video, dec, player, voice: None, size: (h.width as i32, h.height as i32), started: false })
    }

    /// One redraw. `true` once both movies are over.
    pub fn frame(&mut self, audio: Option<&nfs_audio::AudioOutput>, camera: &Camera) -> bool {
        if self.movie.is_none() && self.blank == 0 {
            let Some(path) = self.files.pop_front() else {
                return true;
            };
            self.movie = self.open(&path);
            if self.movie.is_none() {
                // "a missing one is passed over" (0x59DC30 at 0x4DD65F / 0x4DD507, the doc comment
                // above `MOVIES`) — an unreadable file drops out of the queue and the next redraw
                // tries the next one. Returning early here without the `request_redraw` below used
                // to leave the window on a blank frame forever the moment either movie could not be
                // decoded (a missing `nfs5.exe` table set, or a corrupt/short movie file) — the same
                // stalled-loading-still class 548aa02b fixed for the front end past the intro.
                tracing::warn!(?path, "start-up movie unreadable, skipped");
            } else {
                self.blank = BLANK_FLIPS;
                self.tex = None;
            }
        }
        if self.blank > 0 {
            self.blank -= 1;
        } else if let Some(m) = self.movie.as_mut() {
            let Movie { data, video, dec, player, voice, started, .. } = m;
            let mut decode = |c: usize| dec.decode_frame(&data[video[c].clone()]).rgba;
            if !*started {
                *started = true;
                player.preload(&mut decode);
                // the sound starts once the ring is full, then the clock (0x4DCB8F…0x4DCC19); the
                // library plays it at its own level — 0x567550 takes none; a stream voice, started
                // at priority 0x65 (0x5612A0) and so never stolen
                *voice = audio.and_then(|a| {
                    let pcm = nfs_formats::schl::decode_all(&nfs_formats::mad::audio_track(data)).ok()?;
                    let clip = nfs_audio::mixer::Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: None, gain: 1.0, bend_cents: 0, fx_send: pcm.fx_send, priority: nfs_audio::mixer::STREAM_PRIORITY, pan: pcm.pan, pan_scale: pcm.pan_scale, records: Vec::new(), layers: Vec::new(), tables: Vec::new() };
                    Some(a.play_word(Arc::new(clip), 1.0, nfs_audio::resample::PITCH_UNIT, 0x40, 0, false))
                });
                player.start(self.t0.elapsed().as_millis() as i32, voice.is_some());
            }
            // 0x566910's position (the frames mixed) less 0x56AFA0's latency (mixed and not yet
            // heard, `AudioOutput::latency_ms`) — 0x4DCC9E and the four other sync sites.
            // *(Until 2026-09-26 the port took the latency as 0.)*
            let pos = voice.and_then(|v| audio.and_then(|a| Some(a.position_ms(v)? - a.latency_ms())));
            let tick = Tick { now: self.t0.elapsed().as_millis() as i32, audio_pos: pos.unwrap_or(0), audio_done: pos.is_none() };
            let (keys, buttons) = (&mut self.keys, self.buttons);
            let mut skip = || keys.pop_front().unwrap_or(false) | buttons.iter().any(|b| *b);
            let shown = player.step(tick, &mut decode, &mut skip);
            let size = m.size;
            if let Some(rgba) = shown {
                let td = TextureData::single(size.0 as u32, size.1 as u32, rgba);
                match self.tex {
                    Some((id, _)) => self.renderer.update_texture(id, &td),
                    None => self.tex = Some((self.renderer.upload_texture(&td), size)),
                }
            }
            if m.player.done() {
                // 0x4DD40E: the sound stopped, the stream closed
                if let (Some(a), Some(v)) = (audio, m.voice) {
                    a.stop(v);
                }
                tracing::info!(dropped = m.player.dropped, "start-up movie over");
                self.movie = None;
            }
        }
        let mut sprites = Vec::new();
        if let Some((tex, (w, h))) = self.tex.filter(|_| self.movie.is_some()) {
            let (x, y, w, h) = placement(w, h);
            sprites.push(design_sprite(self.renderer.aspect(), tex, x as f32, y as f32, w as f32, h as f32));
        }
        let scene = Scene { sprites, clear_color: [0.0, 0.0, 0.0, 1.0], ..Scene::default() };
        if let Err(e) = self.renderer.render(&scene, camera) {
            tracing::error!(error = %e, "start-up movie render failed");
        }
        self.window.request_redraw();
        false
    }
}

/// The 640×480 mode letterboxed into the window, as the loading screen places its mosaic.
fn design_sprite(aspect: f32, texture: TextureId, x: f32, y: f32, w: f32, h: f32) -> Sprite {
    let design = 640.0 / 480.0;
    let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
    Sprite {
        texture,
        pos: [x0 + x / 640.0 * fw, y / 480.0],
        size: [w / 480.0, h / 480.0],
        uv_min: [0.0, 0.0],
        uv_max: [1.0, 1.0],
        color: [1.0; 4],
        rotation: 0.0,
        pivot: [0.5, 0.5],
        tri: None,
        depth: None,
    }
}
