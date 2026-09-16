//! Engine sound from a car's sound set (see crate docs for the model and its confidence).

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk, crdl::Crdl};
use std::path::Path;
use std::sync::Arc;

struct Layer {
    voice: VoiceId,
    entry: nfs_formats::crdl::EngineEntry,
    load: bool,
}

pub struct EngineSound {
    layers: Vec<Layer>,
    /// Engine bank slot 2: the gear-shift click (`RE/notes/sfx.md` §4.4, player only).
    shift_click: Option<Arc<Clip>>,
}

/// HYPOTHESIS: 512 bins over 0..10240 rpm.
const RPM_PER_BIN: f32 = 20.0;

impl EngineSound {
    /// `set` = catalogue sound set name (e.g. `sd7a`); loads `Sounds/<set>.viv`.
    pub fn load(out: &AudioOutput, sounds_dir: &Path, set: &str) -> anyhow::Result<Self> {
        let viv_path = sounds_dir.join(format!("{set}.viv"));
        let data = std::fs::read(&viv_path).map_err(|e| anyhow::anyhow!("{}: {e}", viv_path.display()))?;
        let viv = Bigf::parse(&data)?;
        let bank_bytes = viv.get(&data, &format!("{set}.bnk")).ok_or_else(|| anyhow::anyhow!("{set}.bnk missing in {}", viv_path.display()))?;
        let bank = Bnk::parse(bank_bytes)?;
        let mut layers = Vec::new();
        for (ext, load) in [("elt", true), ("ect", false)] {
            let Some(t) = viv.get(&data, &format!("{set}.{ext}")) else { continue };
            let crdl = Crdl::parse(t)?;
            for e in crdl.entries {
                let Ok(pcm) = bank.decode_slot(bank_bytes, e.slot as usize) else { continue };
                let clip = Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: pcm.loop_range });
                let voice = out.play(clip, 0.0, 1.0, true);
                layers.push(Layer { voice, entry: e, load });
            }
        }
        let shift_click = bank.decode_slot(bank_bytes, 2).ok().map(|pcm| Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: None }));
        tracing::info!(set, layers = layers.len(), shift_click = shift_click.is_some(), "engine sound loaded");
        Ok(Self { layers, shift_click })
    }

    /// Stop every layer (leaving the race).
    pub fn stop(&self, out: &AudioOutput) {
        for l in &self.layers {
            out.stop(l.voice);
        }
    }

    /// Gear-shift click (engine bank slot 2) at full level.
    pub fn play_shift(&self, out: &AudioOutput, master: f32) {
        if let Some(c) = &self.shift_click {
            out.play(c.clone(), master, 1.0, false);
        }
    }

    /// Update per frame: `rpm`, `throttle` 0..1 selects load vs coast tables, `master` volume.
    pub fn update(&self, out: &AudioOutput, rpm: f32, throttle: f32, master: f32) {
        let bin = (rpm / RPM_PER_BIN).clamp(0.0, 511.0);
        let b = bin as usize;
        let load_mix = throttle.clamp(0.0, 1.0);
        for l in &self.layers {
            let vol = l.entry.table_a[b] as f32 / 127.0 * if l.load { load_mix } else { 1.0 - load_mix } * master;
            let pitch = (bin.max(1.0) / l.entry.centre_bin().max(1.0)).clamp(0.3, 3.0);
            out.set(l.voice, vol, pitch);
        }
    }
}
