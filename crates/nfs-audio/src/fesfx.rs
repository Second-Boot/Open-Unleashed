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

use crate::mixer::{byte_gain, AudioOutput, Clip, ClipCache, BEND_ROOT};
use nfs_formats::bnk::Bnk;
use std::path::Path;
use std::sync::Arc;

pub struct FeSfx {
    bank: Bnk,
    data: Vec<u8>,
    clips: ClipCache<usize>,
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
        Ok(Self { bank, data, clips: ClipCache::new(false), trim })
    }

    fn clip(&mut self, slot: usize) -> Option<Arc<Clip>> {
        let (bank, data) = (&self.bank, &self.data);
        self.clips.get_or_load(slot, || Some(Arc::new(Clip::from_pcm(bank.decode_slot(data, slot).ok()?))))
    }

    /// 0x4AADB0(`slot`): nothing at sound detail 3; otherwise the slot once, centred, at
    /// [`level`]. A bare `Voice_start` 0x565E70, no voice number — not in the pause's table
    /// (`AudioOutput::silence_voices`, `RE/notes/sfx.md` "Audit 2026-09-27, audio port 11").
    pub fn play(&mut self, out: &AudioOutput, slot: usize, sfx_level: u8, audio_quality: u8) {
        if audio_quality == 3 {
            return;
        }
        let v = level(self.trim, sfx_level);
        if let Some(c) = self.clip(slot) {
            out.play_word_untabled(c, byte_gain(v as i32), crate::resample::PITCH_UNIT, BEND_ROOT, 0, false);
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
