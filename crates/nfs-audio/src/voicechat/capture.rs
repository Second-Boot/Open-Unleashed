//! **PLATFORM GLUE**: the microphone. The original opens `DirectSoundCaptureCreate` (0x58F39E, the
//! image's only use) through 0x561A40 → 0x58ACA0 → 0x58F2C0 with the format 8000 Hz, mono, 16-bit
//! (0x493380…0x4933C2) and lets DirectSound convert from the device; a failed open is reported
//! (0x516950) and the session goes on without capture. Here the default input device of the audio
//! backend (cpal) is opened at its own rate and the samples are brought to 8000 Hz mono by
//! averaging each 1/8000 s of input (the port's own converter; DirectSound's is not in the image).
//! No device, a refused permission (macOS: the app needs `NSMicrophoneUsageDescription`, see
//! `packaging/make_app.sh`) or an unsupported sample format all give `None`: no capture, as when
//! the original's open fails.
//!
//! **PORT-DEVIATION D39**: the microphone stays closed unless the player asks for it with `NFS_MIC=1`
//! (the original opens it in every established session; owner decision 2026-10-22, privacy: the voice
//! goes to every peer, strangers included). Receiving and playing the others' voice is unchanged.
//! TEST GLUE: a muted run (`NFS_MUTE`, or `NFS_NO_FOCUS` without `NFS_SOUND`, as the output's own
//! rule in `mixer/output.rs`) opens no microphone even with `NFS_MIC=1`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The capture rate (0x493383 `mov word [esp+..], 0x1F40`).
pub const RATE: u32 = 8000;
/// At most this many 8 kHz samples wait for the game (two seconds); older ones are dropped.
const KEEP: usize = 2 * RATE as usize;

/// The 8 kHz converter: every output sample is the mean of the input samples that fall in its
/// 1/8000 s.
struct Down {
    in_rate: u32,
    pos: u32,
    sum: f64,
    n: u32,
}

impl Down {
    fn new(in_rate: u32) -> Self {
        Down { in_rate: in_rate.max(1), pos: 0, sum: 0.0, n: 0 }
    }

    fn push(&mut self, x: f32, out: &mut VecDeque<i16>) {
        self.sum += x as f64;
        self.n += 1;
        self.pos += RATE;
        if self.pos >= self.in_rate {
            self.pos -= self.in_rate;
            let v = (self.sum / self.n as f64 * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            out.push_back(v);
            if out.len() > KEEP {
                out.pop_front();
            }
            self.sum = 0.0;
            self.n = 0;
        }
    }
}

/// An open microphone delivering 8 kHz mono samples.
pub struct Capture {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<i16>>>,
}

fn env_on(k: &str) -> bool {
    std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0")
}

impl Capture {
    /// The default input device, or `None` (see the module notes).
    pub fn open() -> Option<Capture> {
        if !env_on("NFS_MIC") || (!env_on("NFS_SOUND") && (env_on("NFS_MUTE") || env_on("NFS_NO_FOCUS"))) {
            tracing::info!("voice chat: no capture (off unless NFS_MIC=1, PORT-DEVIATION D39; or a muted run)");
            return None;
        }
        match Self::try_open() {
            Ok(c) => {
                tracing::info!("voice chat: capture open");
                Some(c)
            }
            Err(e) => {
                tracing::info!(error = %e, "voice chat: no capture");
                None
            }
        }
    }

    fn try_open() -> anyhow::Result<Capture> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input device"))?;
        let supported = device.default_input_config()?;
        let channels = supported.channels().max(1) as usize;
        let rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.config();
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(KEEP)));
        let err = |e| tracing::warn!(error = %e, "voice chat capture");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                let (q, mut d) = (queue.clone(), Down::new(rate));
                device.build_input_stream(
                    config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut q = q.lock().unwrap();
                        for f in data.chunks(channels) {
                            d.push(f.iter().sum::<f32>() / channels as f32, &mut q);
                        }
                    },
                    err,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                let (q, mut d) = (queue.clone(), Down::new(rate));
                device.build_input_stream(
                    config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mut q = q.lock().unwrap();
                        for f in data.chunks(channels) {
                            d.push(f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / channels as f32, &mut q);
                        }
                    },
                    err,
                    None,
                )?
            }
            f => anyhow::bail!("input sample format {f:?} not supported"),
        };
        stream.play()?;
        Ok(Capture { _stream: stream, queue })
    }

    /// The next 160 samples (one packet's worth), when there are that many.
    pub fn read(&self, out: &mut [i16; super::codec::FRAME]) -> bool {
        let mut q = self.queue.lock().unwrap();
        if q.len() < out.len() {
            return false;
        }
        for s in out.iter_mut() {
            *s = q.pop_front().unwrap_or(0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_converter_averages_each_eighth_millisecond() {
        let mut d = Down::new(48000);
        let mut q = VecDeque::new();
        for i in 0..48 {
            d.push(if i < 6 { 0.5 } else { 0.0 }, &mut q);
        }
        assert_eq!(q.len(), 8);
        assert_eq!(q[0], (0.5f64 * 32767.0).round() as i16);
        assert!(q.iter().skip(1).all(|&v| v == 0));
    }
}
