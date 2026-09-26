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
    /// The patch's effect send, code 0x13 → `voice+0x44` (`PcmSample::fx_send`).
    pub fx_send: u8,
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
    /// −1 = hard left, 0 = centre, +1 = hard right: the sine of the source's angle off the
    /// listener's forward (see `pan_gains`)
    pan: f32,
    looping: bool,
    finished: bool,
    /// `voice+0x45`: the request's `+0xA`, 0x7F unless the caller passes an aux byte — 0x561960
    /// fills every request with 0x7F (0x56199F) and 0x4A9A5F writes `0x7F − aux` for the
    /// collision one-shots (slots from 0x30). 0x5906D7 copies it into the voice.
    fx_scale: u8,
}

/// **0x589D70**: a voice's two effect sends are `e = voice+0x45 · voice+0x44` (both signed bytes)
/// times each environment bus over 127·127 — `0x82061029` then `add edx`, `sar 13` and the sign
/// bit added back: a divide by 16129 toward zero — stored as the bytes `voice+0x46` (bus `+0xC`,
/// `Mixer_setEnvParam(0x20, ·)`) and `voice+0x47` (bus `+0xD`, `(0x80, ·)`), set by 0x565CD0.
/// Recomputed at the voice start (0x5906DA) and on every bus change (0x565D28…0x565D51).
fn fx_send(fx_scale: u8, patch: u8, bus: i8) -> i32 {
    (fx_scale as i8 as i32 * patch as i8 as i32) * bus as i32 / 16129
}

/// The software path's reverb input for a voice (`[drv+0xD8]` = 0x596A10 when the voice start's
/// flag 0x10 is clear, 0x58A455…0x58A468): `voice+0x46 / 127` per channel (0x599800), which
/// 0x5980A0 turns into the channel's wet gain `volume · send · 1.5`. The DirectSound path (flag
/// 0x10, 0x596400) sends `voice+0x47 / 127` — the `0x80` bus — as the EAX buffer reverb mix
/// (property set {4A4E6FC0-C341-11D1-B73A-444553540000}) instead; like the pan law, the port
/// takes the software path.
fn reverb_input(fx_scale: u8, patch: u8, bus_c: i8) -> f32 {
    fx_send(fx_scale, patch, bus_c) as f32 / 127.0 * 1.5
}

/// The port's return level from its own reverb unit — APPROXIMATION: the library's reverb
/// (0x6B17D0's channels) is not reproduced, only what goes into it.
const REVERB_RETURN: f32 = 0.5;

/// The software mixer's stereo law (`RE/notes/engine-audio.md`, "The pan law"): 0x58E0E0 builds a
/// 256-step table over the voice's angle with the two speakers at 0x4000 (left) and 0xC000
/// (right); between them the angle splits **linearly**, and each speaker's share `x` (0…127,
/// integer division) becomes `f(x) = 2x − x²/127` (0x58E0B0), scaled by 1/127 (`[0x5C1788]`).
/// Front and back mirror. `pan` is the sine of the angle off forward, as this port computes it,
/// so it is taken back to the angle first.
///
/// This is the path of a voice the driver cannot give hardware 3D (`[0x6B48DF]` hardware 3D
/// voices, 0 on a card without DirectSound3D buffers, drops every 3D type from the table at
/// 0x58A7C4…0x58A7D6); a card with them positions the voice with `SetPosition` instead
/// (0x596370) and the stereo image is the card's — APPROXIMATION that the software path is the one.
/// (Until 2026-09-25 this was an equal-power cos / sin law of the port's own.)
fn pan_gains(pan: f32) -> [f32; 2] {
    let p = pan.clamp(-1.0, 1.0).asin() / std::f32::consts::FRAC_PI_2;
    // the angle on the table's 256-step grid: 128 steps across the half turn between the speakers
    let steps = ((1.0 + p) * 0.5 * 128.0).round() as i32;
    let from_left = steps * 256;
    let from_right = 0x8000 - from_left;
    let f = |x: i32| (2 * x - x * x / 127) as f32 / 127.0;
    // each speaker's share grows with the distance to the other one
    [f(127 * from_right / 0x8000), f(127 * from_left / 0x8000)]
}

#[derive(Default)]
struct Shared {
    voices: Vec<(VoiceId, Voice)>,
    streams: Vec<(VoiceId, StreamVoice)>,
    next_id: u32,
    /// the environment buses of record 0, `[0x6B4A0C]` (`Mixer_setEnvParam(0x20, ·)`, the tunnel
    /// reverb's level) and `[0x6B4A0D]` (`(0x80, ·)`, half of it) — `engine-audio.md` §4.7
    bus_c: i8,
    bus_d: i8,
    tail: Option<Reverb>,
    /// what the last callback left queued for the device, in output frames (its delay to the
    /// playback of the buffer plus the buffer), and when it ran — for [`AudioOutput::latency_ms`]
    queued: Option<(u64, std::time::Instant)>,
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
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                out.fill(0.0);
                let mut g = sh.lock().unwrap();
                let ts = info.timestamp();
                let delay = ts.playback.duration_since(&ts.callback).unwrap_or_default();
                let frames = (out.len() / channels.max(1)) as u64 + (delay.as_secs_f64() * out_rate).round() as u64;
                g.queued = Some((frames, std::time::Instant::now()));
                // the reverb's input: each voice's output times its effect send (0x589D70) — the
                // bus the environment level drives (`[0x6B4A0C]`, 0x565CD0), not the whole mix
                let mut send = vec![0.0f32; out.len() / channels.max(1)];
                let bus_c = g.bus_c;
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
                    let fx = reverb_input(v.fx_scale, c.fx_send, bus_c);
                    for (fi, frame) in out.chunks_exact_mut(channels).enumerate() {
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
                            let x = (a + (b - a) * f) / 32768.0 * vol * g[ch.min(1)];
                            *o += x;
                            if ch < 2 && fx != 0.0 {
                                send[fi] += x * fx * 0.5;
                            }
                        }
                        v.pos += step;
                    }
                }
                g.voices.retain(|(_, v)| !v.finished);
                // streamed sources: pull frames at src.rate, hold the last frame between source frames
                let fade_frames = (out_rate as u32 * Fade::STEP_MS / 1000).max(1) as usize;
                for (_, sv) in g.streams.iter_mut() {
                    let cc = sv.src.channels.max(1) as usize;
                    let step = sv.src.rate as f64 / out_rate;
                    let mut q = sv.src.queue.lock().unwrap();
                    for frame in out.chunks_exact_mut(channels) {
                        if let Some(f) = sv.fade.as_mut() {
                            sv.fade_frames += 1;
                            if sv.fade_frames >= fade_frames {
                                sv.fade_frames = 0;
                                sv.volume = f.advance(sv.volume);
                                if f.steps_left == 0 {
                                    sv.fade = None;
                                }
                            }
                        }
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
                if g.bus_c != 0 && !g.muted {
                    let rate_hz = out_rate as u32;
                    let rv = g.tail.get_or_insert_with(|| Reverb::new(rate_hz));
                    for (frame, dry) in out.chunks_exact_mut(channels).zip(send.iter()) {
                        let w = rv.tick(*dry);
                        for o in frame.iter_mut() {
                            *o += w * REVERB_RETURN;
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
        g.voices.push((id, Voice { clip, pos: 0.0, pitch, volume, pan: 0.0, looping, finished: false, fx_scale: 0x7F }));
        id
    }

    /// `play` with a stereo position (−1..1).
    pub fn play_panned(&self, clip: Arc<Clip>, volume: f32, pitch: f32, pan: f32, looping: bool) -> VoiceId {
        let id = self.play(clip, volume, pitch, looping);
        self.set_pan(id, pan);
        id
    }

    /// The aux byte of `Voice_play` for a collision one-shot: `voice+0x45 = 0x7F − aux`
    /// (0x4A9A5F…0x4A9A6C → 0x5906D7), so a loud hit sends less to the reverb.
    pub fn set_fx_aux(&self, id: VoiceId, aux: u8) {
        let mut g = self.shared.lock().unwrap();
        if let Some((_, v)) = g.voices.iter_mut().find(|(i, _)| *i == id) {
            v.fx_scale = 0x7F - aux.min(0x7F);
        }
    }

    /// Every one-shot / loop voice at volume 0, the streams (music) left alone — `I_SFX`'s
    /// `0x566340(h, 0)` over the voice table `0x656180…0x6563C8` and the player's engine voices
    /// (0x4AF900(0, 0)) at 0x4AB4FB…0x4AB527. The voices the audio update rebuilds each pass
    /// come back with the levels; a one-shot stays silent to its end.
    pub fn silence_voices(&self) {
        for (_, v) in self.shared.lock().unwrap().voices.iter_mut() {
            v.volume = 0.0;
        }
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

    /// The voice's position in ms — what the sound library's status call 0x566910 answers at `+4`:
    /// the frames played `· 1000 / rate` (0x5669B9…0x5669D5, a 64-bit multiply and divide), or
    /// `None` once the voice has ended (its state 3, 0x566A1E).
    pub fn position_ms(&self, id: VoiceId) -> Option<i32> {
        let g = self.shared.lock().unwrap();
        g.voices.iter().find(|(i, v)| *i == id && !v.finished).map(|(_, v)| played_ms(v.pos as u64, v.clip.rate))
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

    /// The race pause's audio side, 0x4AB150: `Voice_setVolume(h, 0)` on the player's engine voices
    /// (0x4AF900) and on every voice of the table `0x656180` — they keep running, silent — until the
    /// resume 0x4AB200 puts the saved levels back. Here the whole mix is held at zero instead.
    pub fn set_muted(&self, muted: bool) {
        self.shared.lock().unwrap().muted = muted;
    }

    /// `0x565CD0(id, level)`: bit 0x20 of the id sets the bus `[0x6B4A0C]`, bit 0x80
    /// `[0x6B4A0D]` (an id with none of 0xE0 sets all, 0x565CDD…0x565CE1), stored as bytes; the
    /// voices' sends follow at once (0x565D28…0x565D51 → 0x5902A0). `p20` / `p80` are the tunnel
    /// reverb's two parameters (`reverb::EnvParams`).
    pub fn set_env(&self, p20: i32, p80: i32) {
        let mut g = self.shared.lock().unwrap();
        g.bus_c = p20 as i8;
        g.bus_d = p80 as i8;
        if g.bus_c == 0 {
            g.tail = None;
        }
    }

    /// The two buses as `set_env` left them.
    pub fn env(&self) -> (i8, i8) {
        let g = self.shared.lock().unwrap();
        (g.bus_c, g.bus_d)
    }

    /// **0x56AFA0** → 0x592E40 → 0x58F5D0, the driver's latency in ms: the software mixer's write
    /// position `[0x6AD78C]` less the primary buffer's play cursor (`GetCurrentPosition`, bytes
    /// `/ 2 / channels`), wrapped by the buffer length `[0x6AD7B4]`, `· 1000 / rate` (`[0x6B48DA]`,
    /// `idiv`) — what has been mixed and not yet heard; 0 with no buffer or on a failed call. Here:
    /// what the last callback queued ([`latency_ms`]) less the time since.
    pub fn latency_ms(&self) -> i32 {
        let g = self.shared.lock().unwrap();
        match g.queued {
            Some((frames, at)) => latency_ms(frames, at.elapsed(), self.rate),
            None => 0,
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
    /// `0x566AE0(stream, ms, target)`: ramp a stream's volume to `target` over `ms` ([`Fade`]).
    pub fn fade(&self, id: VoiceId, ms: i32, target: f32) {
        let mut g = self.shared.lock().unwrap();
        if let Some((_, sv)) = g.streams.iter_mut().find(|(i, _)| *i == id) {
            sv.fade = Some(Fade::new(sv.volume, ms, target));
            sv.fade_frames = 0;
        }
    }

    /// A stream's volume now (0x567850 answers the voice's current level, 0x591320), `None` when
    /// it is not playing.
    pub fn stream_volume(&self, id: VoiceId) -> Option<f32> {
        let g = self.shared.lock().unwrap();
        g.streams.iter().find(|(i, _)| *i == id).map(|(_, sv)| sv.volume)
    }

    /// Play a streamed source at its native rate (simple nearest-frame resampling), returns a stop handle.
    pub fn play_stream(&self, src: Arc<StreamSource>, volume: f32) -> VoiceId {
        let mut g = self.shared.lock().unwrap();
        let id = VoiceId(g.next_id);
        g.next_id += 1;
        g.streams.push((id, StreamVoice { src, volume, frac: 0.0, last: [0.0, 0.0], fade: None, fade_frames: 0 }));
        id
    }
}

pub(crate) struct StreamVoice {
    pub(crate) src: Arc<StreamSource>,
    pub(crate) volume: f32,
    pub(crate) frac: f64,
    pub(crate) last: [f32; 2],
    pub(crate) fade: Option<Fade>,
    /// output frames since the fade's last step
    pub(crate) fade_frames: usize,
}

/// A volume ramp, **0x565DD0(voice, steps, target)** as `0x566AE0(stream, ms, target)` calls it:
/// `steps = ms / 10` toward zero (the magic 0x66666667, `sar 2`), at least 1 (0x565DFF…0x565E03);
/// the goal `voice+0x28 = target << 16` and the step `voice+0x24 = (goal − volume) / steps`
/// (`idiv`). One step per 10 ms of output — the divisor's unit; the library's service period
/// itself is not read, so the 10 ms is HIGH, not CONFIRMED.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub step: f32,
    pub goal: f32,
    pub steps_left: u32,
}

impl Fade {
    pub const STEP_MS: u32 = 10;

    pub fn new(volume: f32, ms: i32, goal: f32) -> Self {
        let steps = (ms / 10).max(1) as u32;
        Self { step: (goal - volume) / steps as f32, goal, steps_left: steps }
    }

    /// One 10 ms step: the new volume, `None` once the goal is reached.
    pub fn advance(&mut self, volume: f32) -> f32 {
        if self.steps_left <= 1 {
            self.steps_left = 0;
            return self.goal;
        }
        self.steps_left -= 1;
        volume + self.step
    }
}

/// The frames still queued `since` the callback that left `frames` for the device, as 0x58F5D0
/// answers them: `frames · 1000 / rate`, toward zero, never below 0.
pub fn latency_ms(frames: u64, since: std::time::Duration, rate: u32) -> i32 {
    let gone = (since.as_secs_f64() * rate as f64) as u64;
    (frames.saturating_sub(gone) * 1000 / rate.max(1) as u64) as i32
}

/// `played · 1000 / rate`, truncated (0x590D80 / 0x590DF0 are the 64-bit multiply and unsigned
/// divide).
pub fn played_ms(frames: u64, rate: u32) -> i32 {
    (frames * 1000 / rate.max(1) as u64) as i32
}

#[cfg(test)]
mod fx_tests {
    #[test]
    fn the_send_is_the_two_bytes_over_127_squared() {
        // every voice's default 0x7F with the engine banks' 127: the whole bus level
        assert_eq!(super::fx_send(0x7F, 127, 100), 100);
        assert_eq!(super::fx_send(0x7F, 127, 8), 8);
        // a patch without code 0x13 is never in the reverb
        assert_eq!(super::fx_send(0x7F, 0, 100), 0);
        // a full-level collision (aux 127) is dry; a light one (aux 66): 61·127·100 / 16129 = 48.03
        assert_eq!(super::fx_send(0x7F - 127, 127, 100), 0);
        assert_eq!(super::fx_send(0x7F - 66, 127, 100), 48);
        // toward zero, not rounded: 61·127·68 / 16129 = 32.66 → 32; the 0x80 bus 34 → 16.33 → 16
        assert_eq!(super::fx_send(0x7F - 66, 127, 68), 32);
        assert_eq!(super::fx_send(0x7F - 66, 127, 34), 16);
        // the software path's wet gain: `send / 127 · 1.5` (0x596A10, 0x5980A0)
        assert!((super::reverb_input(0x7F, 127, 100) - 100.0 / 127.0 * 1.5).abs() < 1e-6);
    }
}

#[cfg(test)]
mod fade_tests {
    use super::Fade;

    #[test]
    fn a_500_ms_fade_is_fifty_steps_to_the_goal() {
        // F9's 0x566AE0(stream, 0x1F4, 0): 500 / 10 = 50 steps
        let mut f = Fade::new(0.5, 500, 0.0);
        assert_eq!(f.steps_left, 50);
        let mut v = 0.5;
        let mut n = 0;
        while f.steps_left > 0 {
            v = f.advance(v);
            n += 1;
        }
        assert_eq!(n, 50);
        assert_eq!(v, 0.0);
        // a 5 ms ramp (the song start's 0x566AE0(stream, 5, level)) is one step
        let mut f = Fade::new(0.0, 5, 0.4);
        assert_eq!(f.steps_left, 1);
        assert_eq!(f.advance(0.0), 0.4);
    }
}

#[cfg(test)]
mod position_tests {
    #[test]
    fn the_latency_is_what_is_queued_and_not_yet_heard() {
        use std::time::Duration;
        // 1024 frames of buffer and 1024 of device delay at 48 kHz: 42 ms, toward zero
        assert_eq!(super::latency_ms(2048, Duration::ZERO, 48000), 42);
        // 10 ms later 480 frames have gone
        assert_eq!(super::latency_ms(2048, Duration::from_millis(10), 48000), 32);
        // never negative
        assert_eq!(super::latency_ms(2048, Duration::from_secs(1), 48000), 0);
    }

    #[test]
    fn the_position_is_whole_milliseconds() {
        assert_eq!(super::played_ms(22050, 22050), 1000);
        assert_eq!(super::played_ms(22049, 22050), 999);
        assert_eq!(super::played_ms(0, 22050), 0);
    }
}

#[cfg(test)]
mod bend_tests {
    use super::Clip;

    #[test]
    fn the_bend_byte_moves_the_pitch_about_0x40() {
        // (byte − 0x40) · bend >> 6 cents: a 4-semitone patch at 127 is 393 cents up, at 0 400 down
        let c = Clip { rate: 22050, channels: 1, samples: Vec::new(), loop_range: None, gain: 1.0, bend_cents: 400, fx_send: 0 };
        assert_eq!(c.bend(0x40), 1.0);
        assert!((c.bend(127) - (393.0f32 / 1200.0).exp2()).abs() < 1e-6);
        assert!((c.bend(0) - (-400.0f32 / 1200.0).exp2()).abs() < 1e-6);
        // clamped to 0…127 like Voice_play's argument (0x4A9C1F)
        assert_eq!(c.bend(1000), c.bend(127));
    }
}

#[cfg(test)]
mod pan_tests {
    use super::pan_gains;

    #[test]
    fn the_pan_law_is_the_software_mixers() {
        // hard left / right: one speaker at 127/127, the other silent (0x58E0E0)
        assert_eq!(pan_gains(-1.0), [1.0, 0.0]);
        assert_eq!(pan_gains(1.0), [0.0, 1.0]);
        // centre: x = 63, f = 126 − 31 = 95 → 95/127 on both (0x58E0B0)
        let c = pan_gains(0.0);
        assert!((c[0] - 95.0 / 127.0).abs() < 1e-6 && (c[1] - 95.0 / 127.0).abs() < 1e-6);
    }
}
