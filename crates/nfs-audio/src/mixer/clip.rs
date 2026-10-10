//! The decoded sample a voice plays ([`Clip`]) and the cache the callers keep them in.

use super::*;

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
    /// The patch's bend range in cents (`PcmSample::bend_cents`, code 0x0A · 100) — `voice+0x66`, which
    /// scales the caller's bend byte `voice+0x3F` ([`Voice::pitch_word`]).
    pub bend_cents: i32,
    /// The patch's effect send, code 0x13 → `voice+0x44` (`PcmSample::fx_send`).
    pub fx_send: u8,
    /// The patch's voice priority, code 0x06 → `voice+0x1A` (`PcmSample::priority`): below
    /// [`STEAL_BELOW`] the voice can be taken by a later start when the budget is full.
    pub priority: u8,
    /// The patch's pan byte, code 0x0C (`PcmSample::pan`) — `voice+0x3E`.
    pub pan: u8,
    /// The patch's pan scale, code 0x25 (`PcmSample::pan_scale`) — `voice+0x43`.
    pub pan_scale: i8,
    /// The patch's records (`PcmSample::records`): what the start tests and draws from
    /// ([`crate::librand::patch_start`]). Empty for a clip that is not a bank patch.
    pub records: Vec<nfs_formats::bnk::PatchRecord>,
    /// For a patch of more than one record, each record's own clip (`PcmSample::parts`), in
    /// record order: 0x590420 starts every matching record as a voice of its own. Empty for a
    /// one-record patch, whose sample is this clip's.
    pub layers: Vec<Arc<Clip>>,
    /// The records' tables (`PcmSample::tables`), in record order: the envelope, the volume and
    /// pitch step tables and the level curve the library service walks ([`Shared::service`]).
    pub tables: Vec<nfs_formats::bnk::RecordTables>,
}

impl Default for Clip {
    /// Silence at the bank default: 22050 Hz mono, unit gain, centre pan, no records.
    fn default() -> Self {
        Self {
            rate: 22050,
            channels: 1,
            samples: Vec::new(),
            loop_range: None,
            gain: 1.0,
            bend_cents: 0,
            fx_send: 0,
            priority: 0,
            pan: 64,
            pan_scale: 1,
            records: Vec::new(),
            layers: Vec::new(),
            tables: Vec::new(),
        }
    }
}

/// The decoded clips of one bank, made on first use. With `remember_misses` a
/// slot that does not decode is remembered as missing too; without, it is tried again at its
/// next call.
pub struct ClipCache<K> {
    clips: std::collections::HashMap<K, Option<Arc<Clip>>>,
    remember_misses: bool,
}

impl<K: Eq + std::hash::Hash> ClipCache<K> {
    pub fn new(remember_misses: bool) -> Self {
        Self { clips: std::collections::HashMap::new(), remember_misses }
    }

    /// The clip under `key`, from `load` when it is not cached yet.
    pub fn get_or_load(&mut self, key: K, load: impl FnOnce() -> Option<Arc<Clip>>) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&key) {
            return c.clone();
        }
        let clip = load();
        if clip.is_some() || self.remember_misses {
            self.clips.insert(key, clip.clone());
        }
        clip
    }
}

impl Clip {
    /// The clip's length in frames.
    pub fn frames(&self) -> u32 {
        (self.samples.len() / self.channels.max(1) as usize) as u32
    }

    /// The clip as the sound loops it: its own loop, else the whole sample.
    pub fn with_whole_loop(self) -> Clip {
        Clip { loop_range: self.loop_range.or(Some((0, self.frames()))), ..self }
    }

    /// The clip without a loop: it plays once.
    pub fn one_shot(self) -> Clip {
        Clip { loop_range: None, ..self }
    }

    /// A clip out of its `Arc`: moved when this is the only handle, copied otherwise.
    pub fn from_arc(c: Arc<Clip>) -> Clip {
        Arc::try_unwrap(c).unwrap_or_else(|c| (*c).clone())
    }

    /// A bank patch's decoded sample as a clip, its records' own samples as its layers.
    pub fn from_pcm(p: nfs_formats::bnk::PcmSample) -> Clip {
        Clip {
            rate: p.rate,
            channels: p.channels,
            samples: p.samples,
            loop_range: p.loop_range,
            gain: p.gain,
            bend_cents: p.bend_cents,
            fx_send: p.fx_send,
            priority: p.priority,
            pan: p.pan,
            pan_scale: p.pan_scale,
            records: p.records,
            layers: p.parts.into_iter().map(|q| Arc::new(Clip::from_pcm(q))).collect(),
            tables: p.tables,
        }
    }

    /// The clip record `i` plays: its layer, or the clip itself for a one-record patch.
    pub(super) fn layer(self: &Arc<Self>, i: usize) -> Arc<Clip> {
        self.layers.get(i).cloned().unwrap_or_else(|| Arc::clone(self))
    }
}
