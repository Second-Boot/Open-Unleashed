//! One-shot speech banks from `Speech/zzzxeng.viv` (`RE/notes/sfx.md` §7): banks are decoded on
//! demand; requests go through a FIFO like `Speech_enqueue` 0x4B05A0 (no cool-down, phrases play back
//! to back, more than 32 pending requests are dropped).

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct Speech {
    data: Vec<u8>,
    viv: Bigf,
    clips: Mutex<HashMap<String, Option<Arc<Clip>>>>,
    queue: Mutex<VecDeque<(String, f32)>>,
    current: Mutex<Option<VoiceId>>,
}

impl Speech {
    /// Open the speech archive; banks are decoded the first time they are requested.
    pub fn load(speech_dir: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read(speech_dir.join("zzzxeng.viv"))?;
        let viv = Bigf::parse(&data)?;
        Ok(Self { data, viv, clips: Mutex::new(HashMap::new()), queue: Mutex::new(VecDeque::new()), current: Mutex::new(None) })
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
            Some(Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: None, gain: pcm.gain }))
        });
        if decoded.is_none() {
            tracing::warn!(name, "speech bank missing");
        }
        cache.insert(key, decoded.clone());
        decoded
    }

    /// Enqueue a phrase (bank name, e.g. `Lap-Final.bnk`).
    pub fn say(&self, _out: &AudioOutput, name: &str, volume: f32) {
        let mut q = self.queue.lock().unwrap();
        if q.len() >= 32 {
            return;
        }
        q.push_back((name.to_string(), volume));
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
        if let Some((name, volume)) = next && let Some(c) = self.clip(&name) {
            *cur = Some(out.play(c, volume, 1.0, false));
        }
    }
}
