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
    level: u8,
    /// `game.spl[0]` — the music trim (52 shipped, 80 compiled in).
    trim: u32,
    current: Option<Music>,
    rng: u32,
}

impl MusicPlayer {
    /// `sounds_dir` is `GameData/Sounds`, for `game.spl`; a missing file leaves the compiled-in trim.
    pub fn new(music_dir: &Path, sounds_dir: &Path, settings: &Settings) -> anyhow::Result<Self> {
        let names = song_titles(music_dir)?;
        let playlist: Vec<usize> = settings.playlist.iter().copied().filter(|i| *i < names.len()).collect();
        Ok(Self {
            dir: music_dir.to_path_buf(),
            names,
            saved: playlist.clone(),
            playlist,
            pos: 0,
            shuffle: settings.shuffle,
            level: settings.music_level,
            trim: nfs_formats::spl::GameSpl::load(sounds_dir).music(),
            current: None,
            rng: 0x2545_F491,
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

    /// End-of-stream advance (0x4ADD6D): sequential, or "any other entry" when shuffling.
    pub fn update(&mut self, out: &AudioOutput) {
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
        } else if self.shuffle {
            (self.rand() % (n - 1) + self.pos + 1) % n
        } else {
            (self.pos + 1) % n
        };
        self.play_current(out);
    }

    pub fn stop(&mut self, out: &AudioOutput) {
        if let Some(m) = self.current.take() {
            out.stop(m.voice);
        }
    }

    fn play_current(&mut self, out: &AudioOutput) {
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
        let p = MusicPlayer { dir: PathBuf::new(), names: Vec::new(), playlist: Vec::new(), saved: Vec::new(), pos: 0, shuffle: false, level: 127, trim: 52, current: None, rng: 1 };
        // the shipped game.spl[0] = 52: 52 · 127 >> 7 = 51 of 127 for a race track, the raw 127 for a jingle
        assert!((p.volume_for("game13-Activator-Morphadron") - 51.0 / 127.0).abs() < 1e-6);
        assert_eq!(p.volume_for("win04-Victory-Cypher"), 1.0);
        assert_eq!(p.volume_for("LOSE01-Aquadelic"), 1.0);
    }
}
