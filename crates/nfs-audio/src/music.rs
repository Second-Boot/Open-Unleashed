//! Music streaming from `Music/zzzymus.viv` (`SCHl` streams) and the original's playlist rules
//! (`RE/notes/engine-audio.md` §8, CONFIRMED).
//!
//! There is **no per-track, per-era or per-race-type music**. The game keeps a playlist of raw stream
//! indices (saved in `fe.txt` as `playlist =`), a position in it, and a shuffle flag; `MusicPlay` either
//! walks that list or rebuilds it from a name pattern (`win*`, `lose*`, `vic*`, `menu*`). A stream whose
//! name starts with `win`, `lose` or `vic` plays at the raw music level rather than the 0..80 scale,
//! which makes the jingles noticeably louder (0x4ADBF9).

use crate::mixer::{AudioOutput, StreamSource, VoiceId};
use nfs_formats::{bigf::Bigf, schl::SchlStream};
use std::path::{Path, PathBuf};

pub struct Music {
    pub voice: VoiceId,
    pub title: String,
    src: std::sync::Arc<StreamSource>,
    _thread: std::thread::JoinHandle<()>,
}

pub fn song_titles(music_dir: &Path) -> anyhow::Result<Vec<String>> {
    let data = std::fs::read(music_dir.join("zzzymus.viv"))?;
    Ok(Bigf::parse(&data)?.entries.iter().map(|e| e.name.clone()).collect())
}

/// Each song's length in milliseconds, which is what the original keeps at `track + 0x20` and what
/// `CDPlayer.SelectedList(TOTALPLAY)` sums (0x4BED16), `Title(TRACKPLAY)`/`Title(SONGTIMEREMAIN)`
/// read singly (0x4BED1F, 0x4BEF6E). *Corrected 2026-09-29 (35:52 vs the original's 35:46):* this is
/// **not** the stream's decoded duration (samples over rate) — the record's own builder,
/// `0x4ADE20` (§4.159's rescan), sets it from the resolved file's raw **byte size**, not from the
/// `SCHl` header, via the classic signed-division-by-constant idiom at `0x4adee6..0x4adefc`
/// (`lea ecx,[eax+eax*4]; shl ecx,1` = size×10, `imul` by the magic constant `0x2f149903`,
/// `sar edx,6` + sign fix-up = ⌊size×10 / 348⌋ for a non-negative size — i.e. a fixed assumed byte
/// rate of 34800 B/s applied to the `.viv` entry's compressed size, whatever the actual codec
/// bitrate). Confirmed by rebuilding all 14 `game*` entries' real `zzzymus.viv` sizes through this
/// exact integer formula: total 2 146 556 ms = **35:46**, matching `pass6_options_audio.jpg`'s
/// genuine capture exactly (the ms→`mm:ss` split itself, `0x4bed1f`'s magic `/1000` then a plain
/// `idiv 60`, was already right — only the per-track ms was wrong). The port had summed the
/// decode-accurate `sample_count/sample_rate` value instead (total 2 152 000 ms = 35:52, off by
/// ~0.4 s/track — real, but not what the original's own list shows, since it never decodes a
/// track just to list it). `0x5666f0`'s own out-parameter (the value this divides) was not traced
/// past this point — its identity as "the resolved entry's byte size" rests on the exact numeric
/// match above, not a read to the instruction of where that size is fetched.
pub fn song_lengths_ms(music_dir: &Path) -> anyhow::Result<Vec<u32>> {
    let data = std::fs::read(music_dir.join("zzzymus.viv"))?;
    let viv = Bigf::parse(&data)?;
    Ok(viv.entries.iter().map(|e| ((e.size as u64 * 10) / 348) as u32).collect())
}

/// `game13-Activator-Morphadron` → (`Activator`, `Morphadron`) — `0x4AD950` splits on `-`.
pub fn split_title(name: &str) -> (String, String) {
    let mut parts = name.splitn(3, '-');
    let _group = parts.next().unwrap_or("");
    let title = parts.next().unwrap_or(name).trim().to_string();
    let artist = parts.next().unwrap_or("").trim().to_string();
    (title, artist)
}

/// `*` / `?` glob, case-insensitive — the original's `PatternMatch`.
pub fn pattern_match(name: &str, pattern: &str) -> bool {
    let (n, p): (Vec<char>, Vec<char>) = (name.to_lowercase().chars().collect(), pattern.to_lowercase().chars().collect());
    fn go(n: &[char], p: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => go(n, &p[1..]) || (!n.is_empty() && go(&n[1..], p)),
            Some('?') => !n.is_empty() && go(&n[1..], &p[1..]),
            Some(c) => n.first() == Some(c) && go(&n[1..], &p[1..]),
        }
    }
    go(&n, &p)
}

/// The audio part of the `fe.txt` registry (§8.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `PLAYLIST` — raw stream indices into `zzzymus.viv`
    pub playlist: Vec<usize>,
    /// `RANDOMPLAYLIST` — off in the shipped file, so a race plays the list in order
    pub shuffle: bool,
    /// `MUSICLEVEL` 0..127
    pub music_level: u8,
    /// `SFXLEVEL` / `ENGINELEVEL` / `SPEECHLEVEL` / `AMBIENTLEVEL`, all 0..127
    pub sfx_level: u8,
    pub engine_level: u8,
    pub speech_level: u8,
    pub ambient_level: u8,
    /// `AUDIOQUALITY` — the sound-detail setting
    pub audio_quality: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            playlist: (0..11).collect(),
            shuffle: false,
            music_level: 127,
            sfx_level: 127,
            engine_level: 127,
            speech_level: 127,
            ambient_level: 127,
            audio_quality: 1,
        }
    }
}

impl Settings {
    /// **`I_SFX` (F10), 0x4AB4E0** — the four effect levels, off and back. On (`[0x656840]` set,
    /// as every race start leaves it, 0x4A8841): the live SFX / engine / speech / ambient levels
    /// `[0x5D1744…0x5D1750]` are kept in their registry variables `[0x657CA4…0x657CB0]` and set to
    /// 0 (0x4AB529…0x4AB56F). Off: the registry values come back (0x4AB576…0x4AB5A4). Music is not
    /// touched. `saved` is the port's `[0x656840]` clear, holding what the registry would.
    pub fn toggle_effects(&mut self, saved: &mut Option<[u8; 4]>) {
        match saved.take() {
            None => {
                *saved = Some([self.sfx_level, self.engine_level, self.speech_level, self.ambient_level]);
                self.sfx_level = 0;
                self.engine_level = 0;
                self.speech_level = 0;
                self.ambient_level = 0;
            }
            Some([sfx, engine, speech, ambient]) => {
                self.sfx_level = sfx;
                self.engine_level = engine;
                self.speech_level = speech;
                self.ambient_level = ambient;
            }
        }
    }

    /// The race pause 0x4AB150 ends by copying the registry variables `[0x657CA0…0x657CB0]` into
    /// the live levels `[0x5D1740…0x5D1750]` (0x4AB1BF…0x4AB1F1), the resume 0x4AB200 the live
    /// ones back (0x4AB22F…0x4AB263). They differ only while `I_SFX` has the effects off — the
    /// registry then keeps the four levels and the live ones are 0 — so **a pause brings the
    /// effects back** and the resume leaves them on, `[0x656840]` still clear: the next F10 press
    /// copies the registry back (no change) and sets it. The music level is its own registry value.
    pub fn pause_levels(&mut self, saved: &Option<[u8; 4]>) {
        if let Some([sfx, engine, speech, ambient]) = *saved {
            self.sfx_level = sfx;
            self.engine_level = engine;
            self.speech_level = speech;
            self.ambient_level = ambient;
        }
    }

    /// The resume's half of [`Self::pause_levels`]: the registry takes the live levels.
    pub fn resume_levels(&self, saved: &mut Option<[u8; 4]>) {
        if saved.is_some() {
            *saved = Some([self.sfx_level, self.engine_level, self.speech_level, self.ambient_level]);
        }
    }

    /// Read `fe.txt` from the game root through the shared registry reader.
    pub fn load(game_root: &Path) -> Self {
        let r = nfs_formats::fetxt::Registry::load(game_root);
        let mut s = Self::default();
        let list = r.list("playlist");
        if !list.is_empty() {
            s.playlist = list;
        }
        // `numplaylistsongs` caps the list; the playlist itself is capped at 128 entries
        let n = r.u32("numplaylistsongs", s.playlist.len() as u32) as usize;
        s.playlist.truncate(n.min(128));
        s.shuffle = r.bool("randomplaylist", false);
        s.music_level = r.u8("musiclevel", 127);
        s.sfx_level = r.u8("sfxlevel", 127);
        s.engine_level = r.u8("enginelevel", 127);
        s.speech_level = r.u8("speechlevel", 127);
        s.ambient_level = r.u8("ambientlevel", 127);
        s.audio_quality = r.u8("audioquality", 1);
        s
    }
}

pub struct MusicPlayer {
    dir: PathBuf,
    /// stream names in `zzzymus.viv` directory order
    pub names: Vec<String>,
    playlist: Vec<usize>,
    /// the list the pattern modes restore (`MusicGlobals+0xE8` before the rebuild)
    saved: Vec<usize>,
    pos: usize,
    shuffle: bool,
    /// `[0x65B928] != 0` — the front end is running. `MusicPlay` and the end-of-stream advance
    /// take the shuffle flag as `[0x65B928] ? 1 : RANDOMPLAYLIST` (§8.3), so in the front end the
    /// list always shuffles. 0x4D3420 sets the pointer at 0x4D3564 and clears it at 0x4D559A.
    front_end: bool,
    level: u8,
    /// `game.spl[0]` — the music trim (52 shipped, 80 compiled in).
    trim: u32,
    current: Option<Music>,
    rng: u32,
    /// `MusicGlobals+0x10`: 0 at `MusicInit` (0x4ADFAE), 1 after the end-of-stream advance opens
    /// the next song (0x4ADE0A), 0 again whenever a swap starts one (state 1, 0x4ADB6E) — what
    /// `I_MUSIC` 0x4AD790 tests to decide between off and a new song
    advanced: bool,
    /// `MusicGlobals+0x14` after an `I_MUSIC` press: state −1 (off, `None`) or 1 (a swap to
    /// that position) — the update waits for the stream's fade to reach 0 (0x4ADC88…0x4ADC9E,
    /// 0x567850) before it stops it (0x567740) and, for a swap, opens the next song (0x4ADCCD…)
    pending: Option<Option<usize>>,
}

impl MusicPlayer {
    /// `sounds_dir` is `GameData/Sounds`, for `game.spl`; a missing file leaves the compiled-in trim.
    pub fn new(music_dir: &Path, sounds_dir: &Path, settings: &Settings) -> anyhow::Result<Self> {
        let names = song_titles(music_dir)?;
        let playlist: Vec<usize> = settings.playlist.iter().copied().filter(|i| *i < names.len()).collect();
        Ok(Self {
            dir: music_dir.to_path_buf(),
            names,
            saved: playlist,
            // `MusicInit` 0x4AE100 (once, from main's 0x4A6840 at 0x4B6B09) zeroes the list's
            // length `+8` (0x4ADFA5): nothing is listed until the race start's `MusicSetPlaylist`
            // or a front-end pattern fills it
            playlist: Vec::new(),
            pos: 0,
            shuffle: settings.shuffle,
            front_end: false,
            level: settings.music_level,
            trim: nfs_formats::spl::GameSpl::load(sounds_dir).music(),
            current: None,
            rng: 0x2545_F491,
            advanced: false,
            pending: None,
        })
    }

    fn rand(&mut self) -> usize {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.rng >> 8) as usize
    }

    /// The stream indices in the playlist, in order (`PLAYLIST` in `fe.txt`).
    pub fn playlist(&self) -> &[usize] {
        &self.playlist
    }

    /// The saved playlist the race starts from, as the CD player edited it.
    pub fn set_saved(&mut self, list: Vec<usize>) {
        self.saved = list;
    }

    /// Whether the front end is running (`[0x65B928]`), which forces the shuffle on.
    pub fn set_front_end(&mut self, on: bool) {
        self.front_end = on;
    }

    pub fn now_playing(&self) -> Option<&str> {
        self.current.as_ref().map(|m| m.title.as_str())
    }

    /// `game.spl[0] · MusicLevel >> 7` (0x4A82F0), or the raw level for a `win`/`lose`/`vic` jingle
    /// (§8.3). The trim is 52 in the shipped `game.spl`, so music sits well below the other categories.
    fn volume_for(&self, name: &str) -> f32 {
        let n = name.to_lowercase();
        let v = if n.starts_with("win") || n.starts_with("lose") || n.starts_with("vic") { self.level as u32 } else { (self.trim * self.level as u32) >> 7 };
        v as f32 / 127.0
    }

    /// `MusicPlay(NULL, 0)` — the race-start pick: any entry of the saved playlist other than the
    /// current one.
    pub fn start_race(&mut self, out: &AudioOutput) {
        self.playlist = self.saved.clone();
        let n = self.playlist.len();
        if n == 0 {
            return;
        }
        self.pos = if n == 1 { 0 } else { (self.rand() % (n - 1) + self.pos + 1) % n };
        self.play_current(out);
    }

    /// **`MusicPlay(NULL, 0)`** 0x4AE4D0 alone, over the list as it stands — what the race pause's
    /// resume 0x4AB200 calls (0x4AB222), with no `MusicSetPlaylist` before it and no replay test:
    /// nothing with the volume `+0` at 0 or an empty list, else position 0 of a one-song list or
    /// any entry but the current one. The list is the race's (with a jingle `MusicPlay(…, 1)`
    /// appended), a `STARTREPLAY` playback's the front end's last pattern, and a command-line
    /// playback's none (`MusicInit`'s, a playback skipping `MusicSetPlaylist`, 0x4A8A4A).
    /// Returns whether a song was started.
    pub fn play_next(&mut self, out: &AudioOutput) -> bool {
        let n = self.playlist.len();
        if n == 0 || (self.trim * self.level as u32) >> 7 == 0 {
            tracing::info!(listed = n, "MusicPlay(NULL, 0): no playlist or no volume, nothing plays");
            return false;
        }
        self.pos = Self::next_pos(n, self.pos, self.rand());
        self.play_current(out);
        true
    }

    /// `MusicPlay`'s mode 0 with no pattern (§8.3): 0 for one song, else `(r % (n − 1) + cur + 1) % n`.
    fn next_pos(n: usize, cur: usize, r: usize) -> usize {
        if n == 1 { 0 } else { (r % (n - 1) + cur + 1) % n }
    }

    /// `MusicPlay(pattern, mode)`: rebuild the playlist from a name pattern. Mode 1 appends the pick to
    /// the saved list (the race-end jingles), mode 2 replaces the list with it (the congratulations
    /// screen), mode 0 plays a random match (the front end).
    pub fn play_pattern(&mut self, out: &AudioOutput, pattern: &str, mode: u8) {
        let mut list: Vec<usize> = (0..self.names.len()).filter(|i| pattern_match(&self.names[*i], pattern)).collect();
        if list.is_empty() && pattern.len() < 0x3D {
            let alt = format!("*-{pattern}");
            list = (0..self.names.len()).filter(|i| pattern_match(&self.names[*i], &alt)).collect();
        }
        list.truncate(128);
        if self.level == 0 || list.is_empty() {
            return;
        }
        let n = list.len();
        match mode {
            1 => {
                let pick = list[self.rand() % n];
                self.playlist = self.saved.clone();
                self.playlist.push(pick);
                self.pos = self.playlist.len() - 1;
            }
            2 => {
                let pick = list[self.rand() % n];
                self.playlist = vec![pick];
                self.pos = 0;
            }
            _ => {
                self.playlist = list;
                self.pos = if n == 1 { 0 } else { self.rand() % n };
            }
        }
        self.play_current(out);
    }

    /// `game.spl[0]` again, after the race audio init swapped the block (`nfs_formats::spl::select_mix`).
    pub fn set_trim(&mut self, trim: u32) {
        self.trim = trim;
    }

    /// End-of-stream advance (0x4ADD6D): sequential, or "any other entry" when shuffling. After
    /// an `I_MUSIC` press, the off or the swap it asked for once the fade has run out.
    pub fn update(&mut self, out: &AudioOutput) {
        if let Some(next) = self.pending {
            // 0x4ADC88: with the stream playing (`+0x8C`) wait while 0x567850 answers a level
            if let Some(m) = &self.current
                && out.stream_volume(m.voice).is_some_and(|v| v > 0.0)
            {
                return;
            }
            self.pending = None;
            self.stop(out);
            if let Some(pos) = next {
                self.pos = pos;
                self.play_current(out);
            }
            return;
        }
        let done = match &self.current {
            Some(m) => m.src.finished.load(std::sync::atomic::Ordering::Relaxed) && !out.is_playing(m.voice),
            None => false,
        };
        if !done {
            return;
        }
        let n = self.playlist.len();
        if n == 0 {
            self.current = None;
            return;
        }
        self.pos = if n == 1 {
            0
        } else if self.shuffle || self.front_end {
            (self.rand() % (n - 1) + self.pos + 1) % n
        } else {
            (self.pos + 1) % n
        };
        self.play_current(out);
        // the advance opens the next song itself (0x4AD8B0, state 2) and sets `+0x10` (0x4ADE0A)
        self.advanced = true;
    }

    /// **`I_MUSIC` (F9), 0x4AD790** — nothing without a playlist (`+8`) or with the music volume
    /// `+0` at 0. After a song that began by the end-of-stream advance (`+0x10` set) it turns the
    /// music **off**: state −1, position −1. Otherwise it asks for **another song**: `+0x10` set,
    /// state 1 (a swap), the position 0 for a one-song list or `|0x538040() + timeGetTime()| % n`
    /// (0x4AD87E…0x4AD89C) — and the swap clears `+0x10` again when it starts the song
    /// (0x4ADB6E), so the next press picks again. Either way a song that is playing (state 0,
    /// position > −1) is first faded out over 500 ms (`0x566AE0(+0x70, 0x1F4, 0)`, 0x4AD800…
    /// 0x4AD80A, `+0x20 = 1000`) and the update carries the change out once it is silent.
    /// *(Until 2026-09-26 the song stopped at once.)*
    ///
    /// `draw` is the pick's `0x538040() + timeGetTime()`, called only on the way to a new song
    /// of a longer list.
    pub fn toggle(&mut self, out: &AudioOutput, draw: &mut dyn FnMut() -> u32) {
        if self.playlist.is_empty() || (self.trim * self.level as u32) >> 7 == 0 {
            return;
        }
        if self.pending.is_none()
            && let Some(m) = &self.current
        {
            out.fade(m.voice, 500, 0);
        }
        // the draw is taken only on the way to a new song of a longer list (0x4AD87E)
        let draw = if !self.advanced && self.playlist.len() > 1 { draw() } else { 0 };
        let next = Self::toggle_step(self.advanced, self.playlist.len(), draw);
        // 0x4AD835 / 0x4AD853: `+0x10` cleared for the off, set for the swap
        self.advanced = next.is_some();
        self.pending = Some(next);
    }

    /// 0x4AD830…0x4AD89C: `None` — off; `Some(position)` — the song the swap starts:
    /// `|draw| % n` (`cdq ; xor ; sub` then `idiv`). (A draw of exactly 0x80000000 stays negative
    /// through the `abs` and gives the original a negative position; the port takes its
    /// magnitude.)
    fn toggle_step(advanced: bool, n: usize, draw: u32) -> Option<usize> {
        if advanced {
            None
        } else if n == 1 {
            Some(0)
        } else {
            Some((draw as i32).unsigned_abs() as usize % n)
        }
    }

    pub fn stop(&mut self, out: &AudioOutput) {
        self.pending = None;
        if let Some(m) = self.current.take() {
            out.stop(m.voice);
        }
    }

    fn play_current(&mut self, out: &AudioOutput) {
        // a swap (state 1) clears `+0x10` (0x4ADB6E); the advance sets it back after this
        self.advanced = false;
        // a new start replaces whatever an `I_MUSIC` press left waiting
        self.pending = None;
        let Some(&idx) = self.playlist.get(self.pos) else { return };
        let Some(name) = self.names.get(idx).cloned() else { return };
        let vol = self.volume_for(&name);
        if let Some(m) = self.current.take() {
            out.stop(m.voice);
        }
        match Music::play(out, &self.dir, idx, vol) {
            Ok(m) => {
                let (title, artist) = split_title(&m.title);
                tracing::info!(title, artist, index = idx, volume = vol, "music");
                self.current = Some(m);
            }
            Err(e) => tracing::warn!(index = idx, error = %e, "music stream failed"),
        }
    }
}

impl Music {
    pub fn play(out: &AudioOutput, music_dir: &Path, index: usize, volume: f32) -> anyhow::Result<Self> {
        let data = std::fs::read(music_dir.join("zzzymus.viv"))?;
        let viv = Bigf::parse(&data)?;
        let entry = viv.entries.get(index).ok_or_else(|| anyhow::anyhow!("song index {index} out of range"))?.clone();
        let bytes = viv.data(&data, &entry).to_vec();
        let stream = SchlStream::parse(&bytes)?;
        let (rate, channels) = (stream.header.sample_rate, stream.header.channels);
        let src = StreamSource::new(rate, channels);
        let voice = out.play_stream(src.clone(), volume);
        let title = entry.name.clone();
        let feeder = src.clone();
        let thread = std::thread::spawn(move || {
            let stream = match SchlStream::parse(&bytes) {
                Ok(s) => s,
                Err(_) => return,
            };
            for block in stream.blocks(&bytes) {
                let Ok(block) = block else { break };
                let nfs_formats::schl::Block::Data(block) = block else { continue };
                let mut pcm = Vec::new();
                if block.decode_into(&mut pcm).is_err() {
                    break;
                }
                feeder.push(&pcm);
                // keep ~2 s buffered ahead
                while feeder.queued_frames() > rate as usize * 2 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            feeder.finished.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        Ok(Self { voice, title, src, _thread: thread })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare `MusicPlayer` with no on-disk `zzzymus.viv` (bypassing `MusicPlayer::new`'s file
    /// read) — `names` only needs to be long enough for `playlist`'s and `saved`'s indices.
    fn bare(playlist: Vec<usize>, saved: Vec<usize>) -> MusicPlayer {
        MusicPlayer {
            dir: PathBuf::new(),
            names: vec!["a".into(), "b".into()],
            playlist,
            saved,
            pos: 0,
            shuffle: false,
            front_end: false,
            level: 127,
            trim: 52,
            current: None,
            rng: 1,
            advanced: false,
            pending: None,
        }
    }

    #[test]
    fn playsong_with_no_current_song_does_not_reset_the_playlist() {
        // 0x5120D0's `None` arm (0x4BEB40 found no current entry, returning 0) calls
        // `MusicPlay(NULL, 0)` — 0x4AE4D0 with the candidate pool exhausted (`[edx+8] == 0`,
        // 0x4ae61c…0x4ae898) does **nothing**: no refill, no `MusicSetPlaylist`, `[edx+8]` stays 0.
        // `MusicPlayer::play_next` (the port's rule for this call, matched 2026-09-27, §18.12) must
        // leave an exhausted `playlist` empty and report no song started — unlike `start_race`
        // (the port's rule for `AudioLoadTrack`'s own `MusicPlay(NULL, 0)` after `MusicSetPlaylist`
        // rebuilds the list from `saved`), which is what the port's `PLAYSONG` handler wrongly
        // called here before this fix.
        let Ok(out) = AudioOutput::new() else { return }; // no audio device on this host
        let mut exhausted = bare(vec![], vec![0, 1]);
        assert!(!exhausted.play_next(&out), "no candidates left: MusicPlay(NULL, 0) plays nothing");
        assert!(exhausted.playlist.is_empty(), "play_next never refills from `saved`");

        // Contrast: `start_race` (the wrong call the port's `PLAYSONG` used to make for this arm)
        // rebuilds the list from `saved` and would have picked from it — the divergence this test
        // guards against regressing.
        let mut same_state = bare(vec![], vec![0, 1]);
        same_state.start_race(&out);
        assert_eq!(same_state.playlist, vec![0, 1], "start_race resets the list from `saved` — not PLAYSONG's rule");
    }

    #[test]
    fn a_pause_brings_the_effects_back_after_f10() {
        let mut s = Settings { sfx_level: 100, engine_level: 90, speech_level: 80, ambient_level: 70, ..Settings::default() };
        let mut saved = None;
        // no F10: the pause's and the resume's copies change nothing
        s.pause_levels(&saved);
        s.resume_levels(&mut saved);
        assert_eq!((s.sfx_level, saved), (100, None));
        // F10 off (0x4AB529…0x4AB56F), then a pause: live := registry (0x4AB1BF…0x4AB1F1)
        s.toggle_effects(&mut saved);
        assert_eq!((s.sfx_level, s.ambient_level), (0, 0));
        s.pause_levels(&saved);
        assert_eq!([s.sfx_level, s.engine_level, s.speech_level, s.ambient_level], [100, 90, 80, 70]);
        s.resume_levels(&mut saved);
        assert_eq!(saved, Some([100, 90, 80, 70]));
        // `[0x656840]` still clear: the next F10 copies the registry back — on, no change
        s.toggle_effects(&mut saved);
        assert_eq!((s.sfx_level, saved), (100, None));
    }

    #[test]
    fn patterns_select_the_right_groups() {
        let names = ["game13-Activator-Morphadron", "vic01-Congrats-CaptainGinger", "lose01-Aquadelic-Captain Ginger", "menu02-Aircon-Cypher", "win04-Victory-Cypher"];
        let matching = |p: &str| names.iter().filter(|n| pattern_match(n, p)).count();
        assert_eq!(matching("win*"), 1);
        assert_eq!(matching("lose*"), 1);
        assert_eq!(matching("vic*"), 1);
        assert_eq!(matching("menu*"), 1);
        assert_eq!(matching("game*"), 1);
        assert_eq!(matching("*"), 5);
        // the "*-<pattern>" retry is what makes a bare song name work
        assert!(!pattern_match(names[0], "Activator-Morphadron"));
        assert!(pattern_match(names[0], "*-Activator-Morphadron"));
        assert!(pattern_match(names[3], "MENU0?-*"));
    }

    #[test]
    fn titles_split_on_the_dashes() {
        assert_eq!(split_title("game13-Activator-Morphadron"), ("Activator".into(), "Morphadron".into()));
        assert_eq!(split_title("lose01-Aquadelic-Captain Ginger"), ("Aquadelic".into(), "Captain Ginger".into()));
    }

    #[test]
    fn settings_default_to_the_shipped_playlist() {
        let s = Settings::default();
        assert_eq!(s.playlist, (0..11).collect::<Vec<_>>());
        assert!(!s.shuffle, "the shipped fe.txt has no randomplaylist, so a race plays in order");
        assert_eq!(s.music_level, 127);
    }

    #[test]
    fn jingles_are_louder_than_the_race_playlist() {
        let p = MusicPlayer { dir: PathBuf::new(), names: Vec::new(), playlist: Vec::new(), saved: Vec::new(), pos: 0, shuffle: false, front_end: false, level: 127, trim: 52, current: None, rng: 1, advanced: false, pending: None };
        // the shipped game.spl[0] = 52: 52 · 127 >> 7 = 51 of 127 for a race track, the raw 127 for a jingle
        assert!((p.volume_for("game13-Activator-Morphadron") - 51.0 / 127.0).abs() < 1e-6);
        assert_eq!(p.volume_for("win04-Victory-Cypher"), 1.0);
        assert_eq!(p.volume_for("LOSE01-Aquadelic"), 1.0);
    }
}

#[cfg(test)]
mod toggle_tests {
    use super::{MusicPlayer, Settings};

    #[test]
    fn f10_keeps_the_levels_and_gives_them_back() {
        let mut s = Settings { sfx_level: 100, engine_level: 90, speech_level: 80, ambient_level: 70, music_level: 60, ..Settings::default() };
        let mut saved = None;
        s.toggle_effects(&mut saved);
        assert_eq!((s.sfx_level, s.engine_level, s.speech_level, s.ambient_level, s.music_level), (0, 0, 0, 0, 60));
        s.toggle_effects(&mut saved);
        assert_eq!((s.sfx_level, s.engine_level, s.speech_level, s.ambient_level), (100, 90, 80, 70));
        assert_eq!(saved, None);
    }

    #[test]
    fn the_resume_picks_any_other_entry_of_the_list_as_it_stands() {
        for r in 0..20 {
            let p = MusicPlayer::next_pos(12, 11, r);
            assert!(p < 12 && p != 11, "{r} -> {p}");
        }
        assert_eq!(MusicPlayer::next_pos(1, 0, 7), 0);
    }

    #[test]
    fn f9_is_off_after_an_advance_and_a_new_song_otherwise() {
        assert_eq!(MusicPlayer::toggle_step(true, 5, 7), None);
        assert_eq!(MusicPlayer::toggle_step(false, 5, 7), Some(2));
        assert_eq!(MusicPlayer::toggle_step(false, 1, 7), Some(0));
        // the sum is taken as a signed int and its magnitude divided: −7 → 7 % 5
        assert_eq!(MusicPlayer::toggle_step(false, 5, (-7i32) as u32), Some(2));
    }
}

#[cfg(test)]
mod real_data {
    use super::*;

    fn game_root() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
            return Some(PathBuf::from(p));
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let p = home.join("Games/NFS-Porsche-Unleashed/game_info/data");
        p.join("GameData").is_dir().then_some(p)
    }

    /// The "все треки" total, against the genuine capture (`pass6_options_audio.jpg`, 35:46) —
    /// guards the 2026-09-29 fix (35:52 vs 35:46): the port had been summing the
    /// decode-accurate `sample_count/sample_rate` length (35:52) instead of the original's own
    /// byte-size estimate (`⌊size×10/348⌋` per track, `0x4adee6..0x4adefc`).
    #[test]
    fn playlist_total_matches_the_genuine_capture() {
        let Some(root) = game_root() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let lens = song_lengths_ms(&root.join("GameData/Music")).unwrap();
        let names = song_titles(&root.join("GameData/Music")).unwrap();
        let total_ms: u64 = names.iter().zip(&lens).filter(|(n, _)| pattern_match(n, "game*")).map(|(_, &ms)| ms as u64).sum();
        let secs = total_ms / 1000;
        assert_eq!((secs / 60, secs % 60), (35, 46), "total {total_ms} ms, pass6_options_audio.jpg shows 35:46");
    }
}
