//! Track ambience — `Ambience_updateAll` 0x4AEF30 / `Ambience_updateElement` 0x4AEAC0
//! (`RE/notes/engine-audio.md` §7, CONFIRMED).
//!
//! Each track ships a `<track>_audio.scn` full of `AUDIO_ELEMENT` records: a world position, a
//! **PatchID** that names a bank in `Sounds/track.viv` (`<name>_<NNN>.bnk`, the id is literally the
//! `atoi` of the last three digits), a range and fade-in radius, a retrigger delay with a random
//! extra, a repeat count and a volume percentage. Sixteen voices serve the whole list; the driver
//! visits **one quarter of the elements per update** and steals the farthest live voice when a nearer
//! element needs one.
//!
//! Not ported: the animated-object emitters of §7.4 (their `animdefs.txt` `SoundDef` records and the
//! doppler that goes with them) — the port has no animated objects yet. Static elements are never
//! doppler-shifted in the original either, so the rest plays at exactly 1.0.

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk, scn::{ElementKind, Scene}};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The 16 ambience voices are 0x39..0x48 in the original.
pub const VOICES: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub position: [f32; 3],
    /// 0/2/3/4 = periodic one-shot, 1 = continuous loop, 2 = non-positional, 3/4 = hard left/right
    pub kind: i32,
    pub patch: u16,
    /// outer radius; the element is dropped past `range + 100`
    pub range: f32,
    /// inner radius — full level inside it
    pub fade_in: f32,
    /// base retrigger delay, in units of 8 game ticks (0.125 s)
    pub trigger: i32,
    /// random extra delay, same units
    pub fade_out: i32,
    pub min_repeat: i32,
    pub random_repeat: i32,
    /// percent
    pub volume: i32,
    /// runtime: the tick (in eighths) this element next fires on
    next_trigger: i32,
    /// runtime: the pool slot holding it, −1 when idle
    slot: i32,
    pub label: String,
}

impl Element {
    /// `AUDIO_ELEMENT` v4: `<Type> <PatchID> <Range> <FadeIn> <Trigger> <FadeOut> <MinRepeat>
    /// <RandomRepeat> <nextTrigger> <slot> <Volume> <x78>`. Fields 9 and 10 are editor scratch.
    pub fn from_scene(scene: &Scene) -> Vec<Element> {
        scene
            .elements
            .iter()
            .filter(|e| e.kind == ElementKind::Audio)
            .filter_map(|e| {
                let p = e.params.first()?;
                if p.len() < 11 {
                    return None;
                }
                Some(Element {
                    position: e.position,
                    kind: p[0] as i32,
                    patch: p[1] as u16,
                    range: p[2],
                    fade_in: p[3],
                    trigger: p[4] as i32,
                    fade_out: p[5] as i32,
                    min_repeat: p[6] as i32,
                    random_repeat: p[7] as i32,
                    volume: p[10] as i32,
                    next_trigger: 0,
                    slot: -1,
                    label: e.name.clone().unwrap_or_default(),
                })
            })
            .collect()
    }

    /// §7.5: the level 0..127 before the volume percentage, for a listener `d` metres away.
    pub fn level(&self, d: f32) -> i32 {
        let r = self.range.max(0.001);
        let base = if matches!(self.kind, 1 | 6 | 7) {
            127.0 * (1.0 - d * d / (r * r))
        } else if d < self.fade_in {
            127.0
        } else {
            let r0 = self.fade_in;
            let span = (r - r0).max(0.001);
            127.0 * (1.0 - ((d - r0) / span).powi(2))
        };
        ((base * self.volume as f32 / 100.0) as i32).clamp(0, 127)
    }
}

#[derive(Default, Clone, Copy)]
struct Slot {
    element: Option<usize>,
    voice: Option<VoiceId>,
    patch: u16,
    repeats_left: i32,
    distance: f32,
}

pub struct Ambience {
    pub elements: Vec<Element>,
    /// PatchID → bank bytes in `track.viv`
    index: HashMap<u16, (usize, usize)>,
    viv: Vec<u8>,
    clips: HashMap<u16, Option<Arc<Clip>>>,
    pool: [Slot; VOICES],
    rng: u32,
    /// `[0x5D1750]` AmbientLevel 0..127
    pub ambient_level: u8,
    /// `game.spl[3]` (100 shipped)
    game_trim: u32,
}

impl Ambience {
    pub fn load(sounds_dir: &Path, scenes: &[&Scene], ambient_level: u8) -> anyhow::Result<Self> {
        let viv = std::fs::read(sounds_dir.join("track.viv"))?;
        let dir = Bigf::parse(&viv)?;
        let mut index = HashMap::new();
        for e in &dir.entries {
            // `id = atoi(name + strlen(name) − 7)` — the NNN of `<name>_<NNN>.bnk`
            if e.name.len() >= 7 && let Ok(id) = e.name[e.name.len() - 7..e.name.len() - 4].parse::<u16>() {
                index.insert(id, (e.offset as usize, e.size as usize));
            }
        }
        let game_trim = nfs_formats::spl::GameSpl::load(sounds_dir).ambience();
        let elements: Vec<Element> = scenes.iter().flat_map(|s| Element::from_scene(s)).collect();
        tracing::info!(elements = elements.len(), banks = index.len(), ambient_level, "ambience loaded");
        Ok(Self { elements, index, viv, clips: HashMap::new(), pool: [Slot::default(); VOICES], rng: 0x1234_5678, ambient_level, game_trim })
    }

    fn rand(&mut self) -> i32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        ((self.rng >> 16) & 0x7FFF) as i32
    }

    fn clip(&mut self, patch: u16, looping: bool) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&patch) {
            return c.clone().map(|c| Self::with_loop(&c, looping));
        }
        let clip = self.index.get(&patch).and_then(|&(o, l)| {
            let bytes = self.viv.get(o..o + l)?;
            let bank = Bnk::parse(bytes).ok()?;
            let p = (0..bank.slot_count().max(1)).find_map(|s| bank.decode_slot(bytes, s).ok())?;
            Some(Arc::new(Clip { rate: p.rate, channels: p.channels, samples: p.samples, loop_range: p.loop_range, gain: p.gain, bend_cents: p.bend_cents }))
        });
        if clip.is_none() {
            tracing::debug!(patch, "ambience bank missing or unreadable");
        }
        self.clips.insert(patch, clip.clone());
        clip.map(|c| Self::with_loop(&c, looping))
    }

    fn with_loop(c: &Arc<Clip>, looping: bool) -> Arc<Clip> {
        if !looping {
            return c.clone();
        }
        let frames = (c.samples.len() / c.channels.max(1) as usize) as u32;
        Arc::new(Clip { loop_range: c.loop_range.or(Some((0, frames))), ..(**c).clone() })
    }

    /// Mixer volume `(AmbientLevel · game.spl[3] · level · 85) >> 21`, which saturates at 83 of 127.
    fn voice_volume(&self, level: i32) -> f32 {
        ((self.ambient_level as i32 * self.game_trim as i32 * level * 85) >> 21).clamp(0, 127) as f32 / 127.0
    }

    /// One update (`0x4AEF30`). `tick` is the 64 Hz game tick; `right` and `forward` are the
    /// listener's axes.
    pub fn update(&mut self, out: &AudioOutput, tick: u64, listener: [f32; 3], right: [f32; 3], _forward: [f32; 3]) {
        if self.ambient_level == 0 || self.elements.is_empty() {
            return;
        }
        let n = self.elements.len();
        let q = n / 4 + 1;
        let phase = ((tick >> 1) & 3) as usize;
        let tick8 = (tick >> 3) as i32;
        for i in phase * q..(phase * q + q).min(n) {
            // a slot the pool has since given to someone else is not ours any more
            let slot = self.elements[i].slot;
            if slot >= 0 && self.pool[slot as usize].element != Some(i) {
                self.elements[i].slot = -1;
            }
            let e = &self.elements[i];
            let (dx, dy, dz) = (e.position[0] - listener[0], e.position[1] - listener[1], e.position[2] - listener[2]);
            let est = dx.abs().max(dz.abs()) + 0.25 * dx.abs().min(dz.abs());
            if (e.trigger != 0 || e.fade_out != 0) && e.next_trigger < tick8 && e.slot < 0 {
                let fo = e.fade_out;
                let extra = if fo > 1 { self.rand() % fo } else { 0 };
                let e = &mut self.elements[i];
                e.next_trigger = e.trigger + tick8 + extra;
            }
            let e = &self.elements[i];
            if est > e.range + 100.0 && e.slot <= -1 {
                continue;
            }
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            self.update_element(out, i, tick8, d, [dx, dy, dz], right);
        }
    }

    /// `Ambience_updateElement` 0x4AEAC0.
    fn update_element(&mut self, out: &AudioOutput, i: usize, tick8: i32, d: f32, rel: [f32; 3], right: [f32; 3]) {
        let e = self.elements[i].clone();
        if d > e.range + 100.0 || !self.index.contains_key(&e.patch) {
            self.release(out, i);
            return;
        }
        if e.slot < 0 {
            // an armed element fires on its exact tick and no other
            if e.next_trigger > 0 && e.next_trigger != tick8 {
                return;
            }
            let free = self.pool.iter().position(|s| s.element.is_none());
            let slot = match free {
                Some(s) => s,
                None => {
                    // steal the farthest live voice, but only for a nearer element
                    let (far, fd) = self.pool.iter().enumerate().fold((0usize, -1.0f32), |acc, (k, s)| if s.distance > acc.1 { (k, s.distance) } else { acc });
                    if d >= fd {
                        return;
                    }
                    self.release_slot(out, far);
                    far
                }
            };
            let repeats = e.min_repeat + if e.random_repeat > 0 { self.rand() % (e.random_repeat + 1) } else { 0 };
            self.pool[slot] = Slot { element: Some(i), voice: None, patch: e.patch, repeats_left: repeats, distance: d };
            self.elements[i].slot = slot as i32;
        }
        let slot = self.elements[i].slot as usize;
        self.pool[slot].distance = d;
        if self.pool[slot].patch != e.patch {
            self.release_slot(out, slot);
            self.pool[slot] = Slot { element: Some(i), voice: None, patch: e.patch, repeats_left: 0, distance: d };
            self.elements[i].slot = slot as i32;
        }
        // a finished one-shot either repeats or gives the slot back
        let looping = e.kind == 1;
        if let Some(v) = self.pool[slot].voice
            && !out.is_playing(v)
        {
            self.pool[slot].voice = None;
            if self.pool[slot].repeats_left > 0 {
                self.pool[slot].repeats_left -= 1;
            } else if !(looping && d <= e.range) {
                self.release(out, i);
                return;
            }
        }
        let level = e.level(d);
        let vol = self.voice_volume(level);
        // §7.5 pan, folded to the mixer's left/right: Type 2 is non-positional, 3 and 4 are pinned
        let pan = match e.kind {
            2 => 0.0,
            3 => -1.0,
            4 => 1.0,
            _ => {
                let len = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt().max(0.001);
                ((rel[0] * right[0] + rel[1] * right[1] + rel[2] * right[2]) / len).clamp(-1.0, 1.0)
            }
        };
        match self.pool[slot].voice {
            Some(v) => {
                out.set(v, vol, 1.0);
                out.set_pan(v, pan);
            }
            None => {
                if let Some(c) = self.clip(e.patch, looping) {
                    tracing::debug!(patch = e.patch, label = e.label, d = d as i32, level, looping, "ambience voice");
                    self.pool[slot].voice = Some(out.play_panned(c, vol, 1.0, pan, looping));
                } else {
                    self.release(out, i);
                }
            }
        }
    }

    fn release(&mut self, out: &AudioOutput, i: usize) {
        let slot = self.elements[i].slot;
        if slot >= 0 {
            self.release_slot(out, slot as usize);
        }
        self.elements[i].slot = -1;
    }

    fn release_slot(&mut self, out: &AudioOutput, slot: usize) {
        if let Some(v) = self.pool[slot].voice.take() {
            out.stop(v);
        }
        if let Some(e) = self.pool[slot].element.take()
            && self.elements[e].slot == slot as i32
        {
            self.elements[e].slot = -1;
        }
        self.pool[slot] = Slot::default();
    }

    pub fn stop(&mut self, out: &AudioOutput) {
        for s in 0..VOICES {
            self.release_slot(out, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(kind: i32, range: f32, fade_in: f32, volume: i32) -> Element {
        Element { position: [0.0; 3], kind, patch: 1, range, fade_in, trigger: 0, fade_out: 0, min_repeat: 0, random_repeat: 0, volume, next_trigger: 0, slot: -1, label: String::new() }
    }

    #[test]
    fn the_two_attenuation_laws_differ_inside_the_fade_radius() {
        // a one-shot is at full level inside FadeIn and falls to nothing at Range
        let e = elem(0, 200.0, 50.0, 100);
        assert_eq!(e.level(0.0), 127);
        assert_eq!(e.level(49.0), 127);
        assert!(e.level(125.0) < 127 && e.level(125.0) > 0);
        assert_eq!(e.level(200.0), 0);
        // a loop uses the plain 1 − d²/R² law, so it is already below full level at the centre+1
        let l = elem(1, 200.0, 50.0, 100);
        assert_eq!(l.level(0.0), 127);
        assert!(l.level(49.0) < 127);
        assert_eq!(l.level(200.0), 0);
        // and the level scales by the volume percentage
        assert_eq!(elem(0, 200.0, 50.0, 50).level(0.0), 63);
        assert_eq!(elem(0, 200.0, 50.0, 0).level(0.0), 0);
    }

    #[test]
    fn parses_the_shipped_record_shape() {
        let text = "AUDIO_ELEMENT 4\n2562.9 8.1 -968.7\n0 338 200.000000 0.000000 40 108 0 2 1489 -1 88 0\nShips Bell 3 733\n";
        let scene = Scene::parse(text).unwrap();
        let e = &Element::from_scene(&scene)[0];
        assert_eq!(e.patch, 338);
        assert_eq!(e.kind, 0);
        assert_eq!(e.range, 200.0);
        assert_eq!(e.trigger, 40);
        assert_eq!(e.fade_out, 108);
        assert_eq!(e.random_repeat, 2);
        assert_eq!(e.volume, 88, "field 11 is the volume, not the editor scratch at 9/10");
        assert_eq!(e.label, "Ships Bell 3 733");
    }
}
