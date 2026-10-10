//! Streamed sources (music, movie sound): the sample ring, the stream voice and the fade.

use super::*;

/// The samples a stream's ring holds (1 Mi: ~12 s of 44.1 kHz stereo).
pub(super) const STREAM_QUEUE: usize = 1 << 20;

/// Streamed stereo/mono source fed from another thread (music).
pub struct StreamSource {
    pub rate: u32,
    pub channels: u16,
    pub(super) queue: SampleRing,
    pub finished: std::sync::atomic::AtomicBool,
    /// Set by the owner when nobody will play this stream any more: the feeder
    /// thread stops decoding and drops its handle.
    cancelled: std::sync::atomic::AtomicBool,
}

impl StreamSource {
    pub fn new(rate: u32, channels: u16) -> Arc<Self> {
        Arc::new(Self {
            rate,
            channels,
            queue: SampleRing::new(STREAM_QUEUE),
            finished: std::sync::atomic::AtomicBool::new(false),
            cancelled: std::sync::atomic::AtomicBool::new(false),
        })
    }
    /// The stream is not wanted any more: its feeder stops (the voice is stopped separately).
    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }
    /// Queue decoded samples (the feeder thread). The queue is a lock-free ring the callback reads
    /// without waiting; the feeder keeps itself ~2 s ahead, far below its size.
    pub fn push(&self, samples: &[i16]) {
        let n = self.queue.push(samples);
        if n < samples.len() {
            tracing::warn!(dropped = samples.len() - n, "music stream queue full");
        }
    }
    pub fn queued_frames(&self) -> usize {
        self.queue.len() / self.channels.max(1) as usize
    }
}

impl AudioOutput {
    /// `0x566AE0(stream, ms, target)`: ramp a stream's volume to the byte `target` over `ms`
    /// ([`Fade`]).
    pub fn fade(&self, id: VoiceId, ms: i32, target: i32) {
        self.tx.send(Cmd::Fade { id, ms, target });
    }

    /// **0x567810(stream, level)**: the byte `stream+0xE4 = level` and `Voice_setVolume`
    /// 0x566340 on the stream's voice — the level at once, a running fade cancelled
    /// ([`StreamVoice::set_level`]).
    pub fn stream_level(&self, id: VoiceId, level: i32) {
        self.tx.send(Cmd::StreamLevel { id, level });
    }

    /// A stream's volume now (0x567850 answers the voice's current level, 0x591320), `None` when
    /// it is not playing.
    pub fn stream_volume(&self, id: VoiceId) -> Option<f32> {
        let (pend, snap) = self.tx.overlay(id);
        match pend {
            Some(Pend::Stream(_, v)) => Some(v),
            Some(_) => None,
            None => snap.streams.iter().find(|(i, _)| *i == id).map(|(_, v)| *v),
        }
    }

    /// Play a streamed source at its native rate (the voices' resampler, [`resample`]), returns a stop
    /// handle. Its channels take voices of the budget at [`STREAM_PRIORITY`] (0x5612A0), so it is
    /// never stolen; a refused start (the budget full of unstealable voices) returns a dead handle.
    pub fn play_stream(&self, src: Arc<StreamSource>, volume: f32) -> VoiceId {
        let mut st = self.tx.state.lock().unwrap();
        let id = VoiceId(st.next_id);
        st.next_id += 1;
        self.tx.push(&mut st, Cmd::Stream { id, src, volume }, Some(Pend::Stream(id, volume)));
        id
    }
}

pub(crate) struct StreamVoice {
    pub(crate) src: Arc<StreamSource>,
    pub(crate) volume: f32,
    /// the resampler's 16-bit fraction ([`resample::Cursor`])
    pub(crate) frac: u32,
    /// per channel `s[P−1], s[P], s[P+1], s[P+2]` around the stream's position, 16-bit scale
    pub(crate) window: [[f32; 4]; 2],
    /// the first three frames are in `window`
    pub(crate) primed: bool,
    pub(crate) fade: Option<Fade>,
}

impl StreamVoice {
    /// **0x567810(stream, level)** → `Voice_setVolume` 0x566340 → **0x5662B0** on the stream's
    /// voice: the int `<< 16` into `voice+0x2C` and the ramp step `+0x24` zeroed — the level is
    /// there at once and a running fade (0x566AE0) is over — unless `+0x2C` already holds that
    /// value (0x5662FE `je`), when nothing is written and a running fade goes on. `+0x2C` is the
    /// fade's word while one runs, else the stream's byte level.
    pub(crate) fn set_level(&mut self, level: i32) {
        let vol16 = self.fade.map_or_else(|| volume_16(self.volume), |f| f.vol16);
        let v = level.wrapping_shl(16);
        if vol16 != v {
            self.volume = level.clamp(0, 127) as f32 / 127.0;
            self.fade = None;
        }
    }
}

/// **0x566AE0(stream, ms, target)**: the byte `stream+0xE4 = target` and **0x565DD0**(`stream+8`,
/// `ms / 10`, target) on the stream's voice — the same [`Ramp`] as a release: `ms / 10` toward
/// zero (`imul` 0x66666667, `sar 2`, the sign added), at least 1 (0x565DFF…0x565E03); the goal
/// `target << 16`, the step `(goal − voice+0x2C) / ticks` (`idiv`, toward zero), one step a
/// service tick, a hundredth of a second of the mix ([`ServiceClock`], 0x58FBC0), until the
/// volume is past the goal — then it is the goal and the step 0. The voice's level is the word
/// `>> 16` (0x589DF0), which 0x567850 answers (its `+0x3D`, 0x591320). So F9's
/// 0x566AE0(stream, 0x1F4, 0) (0x4AD80A) from level 80 steps 50 times by −104857 in 16.16: the
/// level runs 78, 76, 75 … 1, 0 — 0 on the fiftieth tick with 30 left in the word, the goal
/// on the fifty-first. A goal below 0 would stop the handle (0x5895E1…0x589619); no stream
/// caller passes one. *(Until 2026-09-27 the port stepped the float volume in exactly `ms / 10`
/// even steps, not byte levels.)*
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade {
    pub ramp: Ramp,
    /// the stream voice's `+0x2C`, 16.16
    pub vol16: i32,
}

impl Fade {
    /// `volume` the stream's current `byte / 127`, `target` the call's byte.
    pub fn new(volume: f32, ms: i32, target: i32) -> Self {
        let vol16 = volume_16(volume);
        Self { ramp: Ramp::start(vol16, ms / 10, target), vol16 }
    }

    /// One service tick: `true` when the ramp went below 0 and stops the handle.
    pub fn tick(&mut self) -> bool {
        self.ramp.tick(&mut self.vol16)
    }

    /// The level byte over 127, as the mix takes the stream's volume.
    pub fn volume(&self) -> f32 {
        (self.vol16 >> 16).max(0) as f32 / 127.0
    }

    /// The ramp has reached its goal.
    pub fn done(&self) -> bool {
        self.ramp.step == 0
    }
}

#[cfg(test)]
mod fade_tests {
    use super::Fade;

    #[test]
    fn the_f9_fade_is_the_library_ramp_in_byte_levels() {
        // F9's 0x566AE0(stream, 0x1F4, 0) on a stream at level 80: 500 / 10 = 50 ticks, step
        // (0 − 0x500000) / 50 = −104857 toward zero
        let mut f = Fade::new(80.0 / 127.0, 500, 0);
        assert_eq!((f.vol16, f.ramp.step, f.ramp.goal), (80 << 16, -104857, 0));
        let mut levels = Vec::new();
        while !f.done() {
            assert!(!f.tick(), "a goal of 0 never stops the stream");
            levels.push(f.vol16 >> 16);
        }
        assert_eq!(levels.len(), 51);
        assert_eq!(&levels[..3], &[78, 76, 75]);
        assert_eq!(&levels[48..], &[1, 0, 0]);
        // the fiftieth tick leaves 30 in the word (level 0), the fifty-first the goal
        assert_eq!(f.vol16, 0);
        assert_eq!(f.volume(), 0.0);
        // `ms / 10` toward zero, at least one tick: a 5 ms ramp (the song start's) is one step
        let mut f = Fade::new(0.0, 5, 51);
        assert!(!f.tick());
        assert!(f.done());
        assert_eq!(f.volume(), 51.0 / 127.0);
        // a negative goal stops the handle when the word goes below 0
        let mut f = Fade::new(1.0 / 127.0, 10, -1);
        assert!(f.tick());
    }
}
