use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u32);

/// A decoded mono/stereo clip at its native rate.
#[derive(Debug, Clone)]
pub struct Clip {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
    pub loop_range: Option<(u32, u32)>,
    /// The patch's own volume (`PcmSample::gain`, `RE/notes/audio.md` §2): the sound library
    /// multiplies the level the game asks for by it, so it belongs to the clip, not the call.
    pub gain: f32,
    /// The patch's bend range in cents (`PcmSample::bend_cents`, code 0x0A · 100).
    pub bend_cents: i32,
}

impl Clip {
    /// The pitch factor of the caller's **bend byte** — the third argument of `Voice_play` 0x4A9C10
    /// and the fifth of `Bank_playOneShot` 0x4A98E0, clamped to 0…127 (0x4A9C1F) and stored in
    /// `voice+0x3F` (0x5905FE): `(byte − 0x40) · bend_cents >> 6` cents on top of the sample's pitch
    /// (0x589F75…0x589F90, cents → ratio at 0x589E80, 1200 an octave). 0x40 leaves it alone.
    /// The port read this byte as a volume until 2026-09-24.
    pub fn bend(&self, byte: i32) -> f32 {
        let cents = ((byte.clamp(0, 127) - 0x40) * self.bend_cents) >> 6;
        (cents as f32 / 1200.0).exp2()
    }
}

struct Voice {
    clip: Arc<Clip>,
    /// position in frames (fractional)
    pos: f64,
    pitch: f32,
    volume: f32,
    /// −1 = hard left, 0 = centre, +1 = hard right (equal power)
    pan: f32,
    looping: bool,
    finished: bool,
}

/// Equal-power stereo gains for a pan in −1..1.
fn pan_gains(pan: f32) -> [f32; 2] {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * (std::f32::consts::FRAC_PI_4);
    [a.cos(), a.sin()]
}

#[derive(Default)]
struct Shared {
    voices: Vec<(VoiceId, Voice)>,
    streams: Vec<(VoiceId, StreamVoice)>,
    next_id: u32,
    /// wet level 0..1, set from the tunnel reverb state (`engine-audio.md` §4.7)
    reverb: f32,
    tail: Option<Reverb>,
    /// every voice at volume 0 — the race pause (see [`AudioOutput::set_muted`])
    muted: bool,
}

/// A small Schroeder reverb — four combs into two all-passes, per channel.
///
/// APPROXIMATION: the original hands a level to the sound driver's environment
/// (`Mixer_setEnvParam(0x80, level/2)` and `(0x20, level)`, `engine-audio.md` §4.7) and the driver
/// owns the DSP, which is not in the data. What is reproduced from the game is *when* the reverb
/// runs and how loud (`Reverb` in `lib.rs`); the filter itself is ours.
struct Reverb {
    combs: [(Vec<f32>, usize, f32); 4],
    aps: [(Vec<f32>, usize); 2],
}

impl Reverb {
    fn new(rate: u32) -> Self {
        let d = |ms: f32| ((rate as f32 * ms / 1000.0) as usize).max(1);
        Self {
            combs: [
                (vec![0.0; d(29.7)], 0, 0.805),
                (vec![0.0; d(37.1)], 0, 0.827),
                (vec![0.0; d(41.1)], 0, 0.783),
                (vec![0.0; d(43.7)], 0, 0.764),
            ],
            aps: [(vec![0.0; d(5.0)], 0), (vec![0.0; d(1.7)], 0)],
        }
    }

    fn tick(&mut self, x: f32) -> f32 {
        let mut y = 0.0;
        for (buf, i, g) in self.combs.iter_mut() {
            let v = buf[*i];
            buf[*i] = x + v * *g;
            *i = (*i + 1) % buf.len();
            y += v;
        }
        y *= 0.25;
        for (buf, i) in self.aps.iter_mut() {
            let v = buf[*i];
            let out = v - y * 0.7;
            buf[*i] = y + v * 0.7;
            *i = (*i + 1) % buf.len();
            y = out;
        }
        y
    }
}

pub struct AudioOutput {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    pub rate: u32,
}

impl AudioOutput {
    pub fn new() -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
        let config = device.default_output_config()?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let sh = shared.clone();
        let out_rate = rate as f64;
        let stream = device.build_output_stream(
            &config.into(),
            move |out: &mut [f32], _| {
                out.fill(0.0);
                let mut g = sh.lock().unwrap();
                for (_, v) in g.voices.iter_mut() {
                    if v.finished {
                        continue;
                    }
                    let c = &v.clip;
                    let cc = c.channels as usize;
                    let nframes = c.samples.len() / cc;
                    let (lo, hi) = match (v.looping, c.loop_range) {
                        (true, Some((a, b))) if b > a && (b as usize) <= nframes => (a as usize, b as usize),
                        (true, _) => (0, nframes),
                        _ => (0, nframes),
                    };
                    let step = v.pitch as f64 * c.rate as f64 / out_rate;
                    // the library scales the caller's level by the patch's own volume (0x5905A5)
                    let vol = v.volume * c.gain;
                    for frame in out.chunks_exact_mut(channels) {
                        if v.pos >= hi as f64 {
                            if v.looping && hi > lo {
                                v.pos = lo as f64 + (v.pos - hi as f64) % (hi - lo) as f64;
                            } else {
                                v.finished = true;
                                break;
                            }
                        }
                        let i = v.pos as usize;
                        let f = (v.pos - i as f64) as f32;
                        let j = if i + 1 < hi { i + 1 } else if v.looping { lo } else { i };
                        let g = pan_gains(v.pan);
                        for (ch, o) in frame.iter_mut().enumerate() {
                            let sc = ch.min(cc - 1);
                            let a = c.samples[i * cc + sc] as f32;
                            let b = c.samples[j * cc + sc] as f32;
                            *o += (a + (b - a) * f) / 32768.0 * vol * g[ch.min(1)];
                        }
                        v.pos += step;
                    }
                }
                g.voices.retain(|(_, v)| !v.finished);
                // streamed sources: pull frames at src.rate, hold the last frame between source frames
                for (_, sv) in g.streams.iter_mut() {
                    let cc = sv.src.channels.max(1) as usize;
                    let step = sv.src.rate as f64 / out_rate;
                    let mut q = sv.src.queue.lock().unwrap();
                    for frame in out.chunks_exact_mut(channels) {
                        sv.frac += step;
                        while sv.frac >= 1.0 {
                            sv.frac -= 1.0;
                            if q.len() >= cc {
                                for c in 0..cc.min(2) {
                                    sv.last[c] = q[c] as f32 / 32768.0;
                                }
                                if cc == 1 {
                                    sv.last[1] = sv.last[0];
                                }
                                q.drain(..cc);
                            } else {
                                sv.last = [0.0, 0.0];
                            }
                        }
                        for (ch, o) in frame.iter_mut().enumerate() {
                            *o += sv.last[ch.min(1)] * sv.volume;
                        }
                    }
                }
                if g.muted {
                    out.fill(0.0);
                }
                if g.reverb > 0.0 && !g.muted {
                    let wet = g.reverb;
                    let rate_hz = out_rate as u32;
                    let rv = g.tail.get_or_insert_with(|| Reverb::new(rate_hz));
                    for frame in out.chunks_exact_mut(channels) {
                        let dry: f32 = frame.iter().take(2).sum::<f32>() / 2.0;
                        let w = rv.tick(dry);
                        for o in frame.iter_mut() {
                            *o += w * wet;
                        }
                    }
                }
                for s in out.iter_mut() {
                    *s = s.clamp(-1.0, 1.0);
                }
            },
            |e| tracing::error!(error = %e, "audio stream"),
            None,
        )?;
        stream.play()?;
        tracing::info!(rate, channels, "audio output started");
        Ok(Self { _stream: stream, shared, rate })
    }

    pub fn play(&self, clip: Arc<Clip>, volume: f32, pitch: f32, looping: bool) -> VoiceId {
        let mut g = self.shared.lock().unwrap();
        let id = VoiceId(g.next_id);
        g.next_id += 1;
        g.voices.push((id, Voice { clip, pos: 0.0, pitch, volume, pan: 0.0, looping, finished: false }));
        id
    }

    /// `play` with a stereo position (−1..1).
    pub fn play_panned(&self, clip: Arc<Clip>, volume: f32, pitch: f32, pan: f32, looping: bool) -> VoiceId {
        let id = self.play(clip, volume, pitch, looping);
        self.set_pan(id, pan);
        id
    }

    pub fn set_pan(&self, id: VoiceId, pan: f32) {
        let mut g = self.shared.lock().unwrap();
        if let Some((_, v)) = g.voices.iter_mut().find(|(i, _)| *i == id) {
            v.pan = pan.clamp(-1.0, 1.0);
        }
    }

    /// Fraction of the clip already played (0..1), or None when the voice is gone.
    pub fn progress(&self, id: VoiceId) -> Option<f32> {
        let g = self.shared.lock().unwrap();
        g.voices.iter().find(|(i, _)| *i == id).map(|(_, v)| {
            let n = (v.clip.samples.len() / v.clip.channels.max(1) as usize).max(1) as f64;
            (v.pos / n).clamp(0.0, 1.0) as f32
        })
    }

    pub fn set(&self, id: VoiceId, volume: f32, pitch: f32) {
        let mut g = self.shared.lock().unwrap();
        if let Some((_, v)) = g.voices.iter_mut().find(|(i, _)| *i == id) {
            v.volume = volume;
            v.pitch = pitch.max(0.05);
        }
    }

    pub fn stop(&self, id: VoiceId) {
        let mut g = self.shared.lock().unwrap();
        g.voices.retain(|(i, _)| *i != id);
        g.streams.retain(|(i, _)| *i != id);
    }

    /// Wet level of the tunnel reverb, 0..1 (`Reverb::wet` from the game's 0..100 level).
    /// The race pause's audio side, 0x4AB150: `Voice_setVolume(h, 0)` on the player's engine voices
    /// (0x4AF900) and on every voice of the table `0x656180` — they keep running, silent — until the
    /// resume 0x4AB200 puts the saved levels back. Here the whole mix is held at zero instead.
    pub fn set_muted(&self, muted: bool) {
        self.shared.lock().unwrap().muted = muted;
    }

    pub fn set_reverb(&self, wet: f32) {
        let mut g = self.shared.lock().unwrap();
        g.reverb = wet.clamp(0.0, 1.0);
        if g.reverb == 0.0 {
            g.tail = None;
        }
    }

    /// Is a one-shot / loop voice still playing?
    pub fn is_playing(&self, id: VoiceId) -> bool {
        let g = self.shared.lock().unwrap();
        g.voices.iter().any(|(i, v)| *i == id && !v.finished) || g.streams.iter().any(|(i, _)| *i == id)
    }
}

/// Streamed stereo/mono source fed from another thread (music).
pub struct StreamSource {
    pub rate: u32,
    pub channels: u16,
    queue: Mutex<std::collections::VecDeque<i16>>,
    pub finished: std::sync::atomic::AtomicBool,
}

impl StreamSource {
    pub fn new(rate: u32, channels: u16) -> Arc<Self> {
        Arc::new(Self { rate, channels, queue: Mutex::new(std::collections::VecDeque::new()), finished: std::sync::atomic::AtomicBool::new(false) })
    }
    pub fn push(&self, samples: &[i16]) {
        self.queue.lock().unwrap().extend(samples.iter().copied());
    }
    pub fn queued_frames(&self) -> usize {
        self.queue.lock().unwrap().len() / self.channels.max(1) as usize
    }
}

impl AudioOutput {
    /// Play a streamed source at its native rate (simple nearest-frame resampling), returns a stop handle.
    pub fn play_stream(&self, src: Arc<StreamSource>, volume: f32) -> VoiceId {
        let mut g = self.shared.lock().unwrap();
        let id = VoiceId(g.next_id);
        g.next_id += 1;
        g.streams.push((id, StreamVoice { src, volume, frac: 0.0, last: [0.0, 0.0] }));
        id
    }
}

pub(crate) struct StreamVoice {
    pub(crate) src: Arc<StreamSource>,
    pub(crate) volume: f32,
    pub(crate) frac: f64,
    pub(crate) last: [f32; 2],
}

#[cfg(test)]
mod bend_tests {
    use super::Clip;

    #[test]
    fn the_bend_byte_moves_the_pitch_about_0x40() {
        // (byte − 0x40) · bend >> 6 cents: a 4-semitone patch at 127 is 393 cents up, at 0 400 down
        let c = Clip { rate: 22050, channels: 1, samples: Vec::new(), loop_range: None, gain: 1.0, bend_cents: 400 };
        assert_eq!(c.bend(0x40), 1.0);
        assert!((c.bend(127) - (393.0f32 / 1200.0).exp2()).abs() < 1e-6);
        assert!((c.bend(0) - (-400.0f32 / 1200.0).exp2()).abs() < 1e-6);
        // clamped to 0…127 like Voice_play's argument (0x4A9C1F)
        assert_eq!(c.bend(1000), c.bend(127));
    }
}
