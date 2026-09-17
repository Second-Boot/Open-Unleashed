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
                            *o += (a + (b - a) * f) / 32768.0 * v.volume * g[ch.min(1)];
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
