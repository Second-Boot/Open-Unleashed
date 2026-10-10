//! The "keep-alive loop voice" of `Voice_play` 0x4A9C10, written once.
//!
//! A looping voice (the squeal, the road roll, the wind, the turbo, the AI cars' loops) is driven
//! every audio update the same way: when the voice is playing the sample the update wants,
//! `Voice_play`'s update arm (0x4A9E1F…0x4AA03B) does `setVolume` / `setBend` / `setPitch`
//! ([`AudioOutput::set_word`]), the pan ([`AudioOutput::set_pan`]) and the strip hook
//! ([`AudioOutput::voice_play_filter`]), in that order; when the voice plays another sample the
//! old one gets the 25-tick release (0x4A9DA4, [`AudioOutput::release_replaced`]) before the new
//! start; when it plays nothing, it starts. The call sites keep their RE-derived arithmetic and
//! pass the numbers in [`LoopCall`]; what differs between the sites is carried exactly:
//!
//! * the sample the voice is on (`slot`): a site with one fixed sample (wind, turbo) passes a
//!   constant, which never differs;
//! * the bend byte on an update: the squeal / road / wind / turbo pass the fresh one, an AI car's
//!   loop the one latched when it started (`0x5D17E8[voice]`, `latched`);
//! * the release at the end of a loop is always [`LoopVoice::release`] (0x565DD0(h, 5, −1)); a
//!   shutdown is [`LoopVoice::stop`].

use crate::mixer::{AudioOutput, Clip, PlayArgs, VoiceId};
use std::sync::Arc;

/// A started loop voice: its handle, the sample it plays and the bend it started with.
#[derive(Clone, Copy)]
pub struct LoopVoice {
    id: VoiceId,
    slot: usize,
    bend: u8,
}

/// One update's numbers for [`LoopVoice::drive`].
#[derive(Clone, Copy)]
pub struct LoopCall {
    /// the sample the voice should be playing
    pub slot: usize,
    pub amp: f32,
    pub word: u16,
    pub bend: u8,
    pub pan: u16,
    /// the strip hook's value for the update arm ([`crate::mixer::voice_play_filter`])
    pub filter: Option<i32>,
}

impl LoopVoice {
    /// One update of the loop in `cur`. `latched` is true for a site whose update keeps the bend
    /// the voice started with. `clip` is asked only when a start is needed; a start whose clip is
    /// missing leaves `cur` empty (after the old sample's release, if there was one).
    pub fn drive(cur: &mut Option<LoopVoice>, out: &AudioOutput, c: LoopCall, latched: bool, clip: impl FnOnce() -> Option<Arc<Clip>>) {
        match *cur {
            Some(v) if v.slot == c.slot => {
                out.set_word(v.id, c.amp, c.word, if latched { v.bend } else { c.bend });
                out.set_pan(v.id, c.pan);
                out.voice_play_filter(v.id, c.filter);
            }
            old => {
                // `Voice_play` with another sample on the voice: the old one's 25-tick release
                // (0x4A9DA4) before the start
                if let Some(v) = old {
                    out.release_replaced(v.id);
                }
                *cur = clip().map(|clip| LoopVoice {
                    id: out.play(PlayArgs::new(clip, c.amp, c.word, c.bend, c.pan, true)),
                    slot: c.slot,
                    bend: c.bend,
                });
            }
        }
    }

    /// 0x4A9890: the loop ends with 0x565DD0(h, 5, −1), the five-tick release.
    pub fn release(cur: &mut Option<LoopVoice>, out: &AudioOutput) {
        if let Some(v) = cur.take() {
            out.release(v.id);
        }
    }

    /// The shutdown: the voice is cut at once.
    pub fn stop(cur: &mut Option<LoopVoice>, out: &AudioOutput) {
        if let Some(v) = cur.take() {
            out.stop(v.id);
        }
    }
}
