//! One-shot speech banks from `Speech/zzzxeng.viv` (`RE/notes/sfx.md` §7): banks are decoded on
//! demand; requests go through a FIFO like `Speech_enqueue` 0x4B05A0 (no cool-down, phrases play back
//! to back, more than 32 pending requests are dropped).

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

pub struct Speech {
    data: Vec<u8>,
    viv: Bigf,
    clips: Mutex<HashMap<String, Option<Arc<Clip>>>>,
    queue: Mutex<VecDeque<(String, bool)>>,
    current: Mutex<Option<VoiceId>>,
    /// `SpeechLevel [0x5D174C]`, 0..127
    level: AtomicU8,
    /// `game.spl[1]` / `game.spl[4]` — the trims for speech ids ≥ 4 and < 4 (the countdown)
    trims: [u32; 2],
}

impl Speech {
    /// Open the speech archive; banks are decoded the first time they are requested.
    pub fn load(speech_dir: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read(speech_dir.join("zzzxeng.viv"))?;
        let viv = Bigf::parse(&data)?;
        let spl = nfs_formats::spl::GameSpl::load(&speech_dir.join("..").join("Sounds"));
        Ok(Self {
            data,
            viv,
            clips: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            current: Mutex::new(None),
            level: AtomicU8::new(127),
            trims: [spl.speech(false), spl.speech(true)],
        })
    }

    fn clip(&self, name: &str) -> Option<Arc<Clip>> {
        let key = name.to_lowercase();
        let mut cache = self.clips.lock().unwrap();
        if let Some(c) = cache.get(&key) {
            return c.clone();
        }
        let decoded = self.viv.get(&self.data, &key).or_else(|| self.viv.get(&self.data, name)).and_then(|bytes| {
            let bank = Bnk::parse(bytes).ok()?;
            let slot = bank.slots.iter().position(|s| s.is_some())?;
            let pcm = bank.decode_slot(bytes, slot).ok()?;
            Some(Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: None, gain: pcm.gain, bend_cents: pcm.bend_cents }))
        });
        if decoded.is_none() {
            tracing::warn!(name, "speech bank missing");
        }
        cache.insert(key, decoded.clone());
        decoded
    }

    /// The `SpeechLevel` slider, read when a phrase starts.
    pub fn set_level(&self, level: u8) {
        self.level.store(level, Ordering::Relaxed);
    }

    /// The volume byte 0x4B04E0 builds for a queued phrase of the announcer's bank (record `+0x10`
    /// equal to `[0x657328]`): `(SpeechLevel · game.spl[k] · 85) >> 14` with `k = 1` for speech ids
    /// ≥ 4 and 4 for ids 0…3 — the countdown (0x4B052A…0x4B055A; helpers 0x4A8300 / 0x4A8320) — at
    /// bend byte 0x40. At the top of the slider with the shipped trims that is 83 of 127.
    pub fn volume(level: u8, trim: u32) -> f32 {
        (((level as u32 * trim * 85) >> 14).min(127)) as f32 / 127.0
    }

    /// Enqueue a phrase (bank name, e.g. `Lap-Final.bnk`). The port had fixed volumes 0.8 / 0.9 at
    /// each call until 2026-09-24.
    pub fn say(&self, _out: &AudioOutput, name: &str) {
        self.enqueue(name, false);
    }

    /// Enqueue one of the countdown phrases (speech ids 0…3, which take `game.spl[4]`).
    pub fn say_countdown(&self, _out: &AudioOutput, name: &str) {
        self.enqueue(name, true);
    }

    fn enqueue(&self, name: &str, countdown: bool) {
        let mut q = self.queue.lock().unwrap();
        if q.len() >= 32 {
            return;
        }
        q.push_back((name.to_string(), countdown));
    }

    /// Per frame: start the next queued phrase once the current one has ended.
    pub fn update(&self, out: &AudioOutput) {
        let mut cur = self.current.lock().unwrap();
        if let Some(id) = *cur {
            if out.is_playing(id) {
                return;
            }
            *cur = None;
        }
        let next = self.queue.lock().unwrap().pop_front();
        if let Some((name, countdown)) = next && let Some(c) = self.clip(&name) {
            // the volume is built when the phrase starts (0x4B04E0)
            let volume = Self::volume(self.level.load(Ordering::Relaxed), self.trims[countdown as usize]);
            *cur = Some(out.play(c, volume, 1.0, false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Speech;

    #[test]
    fn announcer_volume_is_the_speech_category() {
        // (127 · 127 · 85) >> 14 = 83 of 127 at the top of the slider with the shipped trim
        assert_eq!(Speech::volume(127, 127), 83.0 / 127.0);
        assert_eq!(Speech::volume(0, 127), 0.0);
    }
}
