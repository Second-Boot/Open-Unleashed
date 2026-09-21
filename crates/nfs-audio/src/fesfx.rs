//! The front end's one-shot sounds — **0x4AADB0**, `FE_playSfx(n)`.
//!
//! ```text
//! 0x4AADB0  if [0x657C94] return            ; set when AUDIOQUALITY [0x657C98] is 3, or there is no
//!                                           ; sound device (0x4B5143…0x4B5164, 0x4A6854)
//! 0x4AADC6  vol = (game.spl[2] · SfxLevel · SfxLevel · 85) >> 21      ; [0x5D14B8], [0x5D1744]
//! 0x4AADFF  pan = 0x40, the centre ; pitch field 0x40 ; no effect
//! 0x4AAE18  0x565E70(bank [0x656510], slot n, params)
//! ```
//!
//! The bank is `fesfx.bnk`, loaded by 0x4A8B80 as bank kind 7 — `lilfesfx.bnk` when the sound-detail
//! setting `[0x657C98] > 1`, the same rule as `SND_coll` (`RE/notes/sfx.md` §1). The slot is the
//! caller's number itself; the callers the notes have read push 5 (a button's click), 6, 8 (a
//! scrollbar arrow), 9 (an input box taking a key, the hover of a HUD box), 0x0D and 0x30…0x35 (the
//! front-end car's parts).

use crate::mixer::{AudioOutput, Clip};
use nfs_formats::bnk::Bnk;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub struct FeSfx {
    bank: Bnk,
    data: Vec<u8>,
    clips: HashMap<usize, Arc<Clip>>,
    /// `game.spl[2]` (`GameSpl::fe_sfx`)
    trim: u32,
}

/// The level 0x4AADB0 computes: `(trim · SfxLevel · SfxLevel · 85) >> 21` — the master really is
/// applied twice. 0…127 for the shipped trim.
pub fn level(trim: u32, sfx_level: u8) -> u32 {
    let s = sfx_level as u32;
    (trim * s * s * 85) >> 21
}

impl FeSfx {
    /// `fesfx.bnk`, or `lilfesfx.bnk` for `audio_quality > 1`.
    pub fn load(sounds_dir: &Path, audio_quality: u8) -> anyhow::Result<Self> {
        let name = if audio_quality > 1 { "lilfesfx.bnk" } else { "fesfx.bnk" };
        let path = sounds_dir.join(name);
        let data = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let bank = Bnk::parse(&data)?;
        tracing::info!(slots = bank.slot_count(), name, "front-end sound bank loaded");
        let trim = nfs_formats::spl::GameSpl::load(sounds_dir).fe_sfx();
        Ok(Self { bank, data, clips: HashMap::new(), trim })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&slot) {
            return Some(c.clone());
        }
        let pcm = self.bank.decode_slot(&self.data, slot).ok()?;
        let clip = Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: pcm.loop_range, gain: pcm.gain });
        self.clips.insert(slot, clip.clone());
        Some(clip)
    }

    /// 0x4AADB0(`slot`): nothing at sound detail 3; otherwise the slot once, centred, at
    /// [`level`].
    pub fn play(&mut self, out: &AudioOutput, slot: usize, sfx_level: u8, audio_quality: u8) {
        if audio_quality == 3 {
            return;
        }
        let v = level(self.trim, sfx_level);
        if let Some(c) = self.clip(slot) {
            out.play(c, v as f32 / 127.0, 1.0, false);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_master_is_squared() {
        assert_eq!(super::level(127, 127), 83);
        assert_eq!(super::level(127, 64), 21);
        assert_eq!(super::level(127, 0), 0);
    }
}
