//! Music streaming from `Music/zzzymus.viv` (`SCHl` streams, `RE/notes/audio.md` §3).
//! A background thread decodes blocks ahead of playback; which song plays when is not yet
//! recovered from the frontend data (`FEAvailableSongsAccessor`), so the caller picks by index.

use crate::mixer::{AudioOutput, StreamSource, VoiceId};
use nfs_formats::{bigf::Bigf, schl::SchlStream};
use std::path::Path;
use std::sync::Arc;

pub struct Music {
    pub voice: VoiceId,
    pub title: String,
    _thread: std::thread::JoinHandle<()>,
}

pub fn song_titles(music_dir: &Path) -> anyhow::Result<Vec<String>> {
    let data = std::fs::read(music_dir.join("zzzymus.viv"))?;
    Ok(Bigf::parse(&data)?.entries.iter().map(|e| e.name.clone()).collect())
}

impl Music {
    pub fn play(out: &AudioOutput, music_dir: &Path, index: usize, volume: f32) -> anyhow::Result<Self> {
        let data = std::fs::read(music_dir.join("zzzymus.viv"))?;
        let viv = Bigf::parse(&data)?;
        let entry = viv.entries.get(index).ok_or_else(|| anyhow::anyhow!("song index {index} out of range"))?.clone();
        let bytes = viv.data(&data, &entry).to_vec();
        let stream = SchlStream::parse(&bytes)?;
        let (rate, channels) = (stream.header.sample_rate, stream.header.channels);
        let src = StreamSource::new(rate, channels);
        let voice = out.play_stream(src.clone(), volume);
        let title = entry.name.clone();
        let thread = std::thread::spawn(move || {
            let stream = match SchlStream::parse(&bytes) {
                Ok(s) => s,
                Err(_) => return,
            };
            for block in stream.blocks(&bytes) {
                let Ok(block) = block else { break };
                let nfs_formats::schl::Block::Data(block) = block else { continue };
                let mut pcm = Vec::new();
                if block.decode_into(&mut pcm).is_err() {
                    break;
                }
                src.push(&pcm);
                // keep ~2 s buffered ahead
                while src.queued_frames() > rate as usize * 2 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            src.finished.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        Ok(Self { voice, title, _thread: thread })
    }
}
