//! Track ambience — `Ambience_updateAll` 0x4AEF30 / `Ambience_updateElement` 0x4AEAC0
//! (`RE/notes/engine-audio.md` §7, CONFIRMED).
//!
//! Each track ships a `<track>_audio.scn` full of `AUDIO_ELEMENT` records: a world position, a
//! **PatchID** that names a bank in `Sounds/track.viv` (`<name>_<NNN>.bnk`, the id is literally the
//! `atoi` of the last three digits), a range and fade-in radius, a retrigger delay with a random
//! extra, a repeat count and a volume percentage. Sixteen voices serve the whole list; the driver
//! visits **one quarter of the elements per update** and steals the farthest run-out voice when a nearer
//! element needs one.
//!
//! **Type 1 is the triggered kind** (read 2026-09-26, 0x4AEAC0 0x4AED43…0x4AED8F): its voice starts
//! only on an update that finds the element's byte `+0xA2` (the record's `+0x26`) set, and clears
//! it; with the byte clear the update only prefetches the bank (`SndBank_get(0, patch, 0)`) and
//! returns — so a playing Type-1 voice keeps the level and pan of the update that started or last
//! re-flagged it. Two things set the byte, both through `0x4AC0E0(1)`: a type-4 `CTriggerElem`
//! (`0x470BB0` → 0x470D5F…0x470DB3, [`Ambience::fire`]) and a smackable's first move
//! (`0x476810` → `0x4AF440`, [`Ambience::fire_nearest`]). A Type-1 voice that has run out while
//! the listener is still inside `Range` keeps its slot and is not started again until the
//! listener has left `Range` (0x4AECFC…0x4AED0F, 0x4AED5A…0x4AED6D).
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
    /// 0/2/3/4 = periodic one-shot, 1 = triggered (see the module doc), 2 = non-positional, 3/4 = hard left/right
    pub kind: i32,
    /// `rec+0xE`, an `i16`: 0x470DA8 and 0x4AEB16 refuse a negative one
    pub patch: i16,
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
    /// `CAudioElem+0xA2` (= the record's `+0x26`): set by `0x4AC0E0(1)`, consumed by the next
    /// update that would start a Type-1 voice (0x4AED73…0x4AED8F)
    pub triggered: bool,
    /// the element's index in the combined scene list `[0x628168]` (the trigger links index it)
    pub scene_index: usize,
    pub label: String,
}

impl Element {
    /// `AUDIO_ELEMENT` v4: `<Type> <PatchID> <Range> <FadeIn> <Trigger> <FadeOut> <MinRepeat>
    /// <RandomRepeat> <nextTrigger> <slot> <Volume> <x78>`. Fields 9 and 10 are editor scratch.
    /// `base` is the number of elements the scenes before this one put into `[0x628168]`.
    pub fn from_scene(scene: &Scene, base: usize) -> Vec<Element> {
        scene
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == ElementKind::Audio)
            .filter_map(|(k, e)| {
                let p = e.params.first()?;
                if p.len() < 11 {
                    return None;
                }
                Some(Element {
                    position: e.position,
                    kind: p[0] as i32,
                    patch: p[1] as i16,
                    range: p[2],
                    fade_in: p[3],
                    trigger: p[4] as i32,
                    fade_out: p[5] as i32,
                    min_repeat: p[6] as i32,
                    random_repeat: p[7] as i32,
                    volume: p[10] as i32,
                    next_trigger: 0,
                    slot: -1,
                    triggered: false,
                    scene_index: base + k,
                    label: e.name.clone().unwrap_or_default(),
                })
            })
            .collect()
    }

    /// §7.5: the level for a listener `d` metres away, the volume percentage applied
    /// (0x4AED93…0x4AEEB9). Only inside `Range` (`d < Range`, 0x4AED9C `test ah, 1`) is there a
    /// level; outside it is 0. Types 1, 6, 7: `_ftol((R² − d²)/R² · 0x80 · 127 · (1/128))`
    /// (0x4AEE08…0x4AEE2B, the `0x80` the scene elements' `fadeScale`); the others 127 inside
    /// `FadeIn` (`d < FadeIn`) else `_ftol(((R−r0)² − (d−r0)²)/(R−r0)² · 127)`. Then
    /// `level · Volume / 100` in integers, toward zero (the magic 0x51EB851F, `sar 5`).
    /// *(Until 2026-09-26 the port applied the percentage in floats before the truncation.)*
    pub fn level(&self, d: f32) -> i32 {
        if d >= self.range {
            return 0;
        }
        let r = self.range;
        let base = if matches!(self.kind, 1 | 6 | 7) {
            (((r * r - d * d) / (r * r)) as f64 * 128.0 * 127.0 * 0.0078125) as i32
        } else if d < self.fade_in {
            127
        } else {
            let r0 = self.fade_in;
            let span = (r - r0) * (r - r0);
            ((span - (d - r0) * (d - r0)) / span * 127.0) as i32
        };
        base * self.volume / 100
    }
}

#[derive(Default, Clone, Copy)]
struct Slot {
    element: Option<usize>,
    voice: Option<VoiceId>,
    patch: i16,
    repeats_left: i32,
}

/// What `Ambience_updateElement` 0x4AEAC0 does with an element that holds a slot, once the
/// patch check is past (0x4AECCF…0x4AED8F): the pure part of the fire / stop path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// stop the voice, free the slot (0x4AED11…0x4AED31)
    Release,
    /// return with nothing done: a Type-1 voice that has run out inside `Range` (0x4AED6D)
    Hold,
    /// a Type-1 element not flagged: `SndBank_get(0, patch, 0)` and return (0x4AED7A…0x4AED8E)
    Prefetch,
    /// `Voice_play(0x39 + slot, …)` — a start when the handle is −1 (a replay sets it so,
    /// 0x4AECF3), else an update of the playing voice; `consume` clears `+0xA2` (0x4AED8F)
    Play { replay: bool, consume: bool },
}

/// The voice as 0x566380 sees it: none (`handle == −1`), playing, or run out (`0x589D20 < 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    None,
    Playing,
    Finished,
}

/// 0x4AECCF…0x4AED8F. `repeats_left` is the pool entry's `+0xE`, `triggered` the element's
/// `+0xA2`. A finished voice with repeats left replays (`+0xE − 1`, handle −1, `bl = 1`), with
/// none it gives the slot back unless it is Type 1 with `d <= Range` (`test ah, 0x41`); a Type-1
/// element that did not just replay plays only when flagged, and holds a finished voice.
pub fn step(kind: i32, voice: VoiceState, repeats_left: i32, d: f32, range: f32, triggered: bool) -> Step {
    let mut replay = false;
    let mut voice = voice;
    if voice == VoiceState::Finished {
        if repeats_left != 0 {
            replay = true;
            voice = VoiceState::None;
        } else if !(kind == 1 && d <= range) {
            return Step::Release;
        }
    }
    if kind == 1 && !replay {
        if voice == VoiceState::Finished {
            return Step::Hold;
        }
        if !triggered {
            return Step::Prefetch;
        }
        return Step::Play { replay, consume: true };
    }
    Step::Play { replay, consume: false }
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
        // `[0x628168]` holds every element of every scene in load order: a scene's own indices
        // are re-based by the count before it (0x46C417…0x46C46A)
        let mut base = 0;
        let mut elements = Vec::new();
        for s in scenes {
            elements.extend(Element::from_scene(s, base));
            base += s.elements.len();
        }
        tracing::info!(elements = elements.len(), banks = index.len(), ambient_level, "ambience loaded");
        Ok(Self { elements, index, viv, clips: HashMap::new(), pool: [Slot::default(); VOICES], rng: 0x1234_5678, ambient_level, game_trim })
    }

    fn rand(&mut self) -> i32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        ((self.rng >> 16) & 0x7FFF) as i32
    }

    /// The patch's first sample as the bank has it — its own loop points or none. *(Until
    /// 2026-09-26 the port forced a whole-sample loop on Type 1, which it took for "continuous";
    /// `Voice_play` 0x4A9C10 plays the patch as it is.)*
    fn clip(&mut self, patch: u16) -> Option<Arc<Clip>> {
        if let Some(c) = self.clips.get(&patch) {
            return c.clone();
        }
        let clip = self.index.get(&patch).and_then(|&(o, l)| {
            let bytes = self.viv.get(o..o + l)?;
            let bank = Bnk::parse(bytes).ok()?;
            let p = (0..bank.slot_count().max(1)).find_map(|s| bank.decode_slot(bytes, s).ok())?;
            Some(Arc::new(Clip { rate: p.rate, channels: p.channels, samples: p.samples, loop_range: p.loop_range, gain: p.gain, bend_cents: p.bend_cents, fx_send: p.fx_send }))
        });
        if clip.is_none() {
            tracing::debug!(patch, "ambience bank missing or unreadable");
        }
        self.clips.insert(patch, clip.clone());
        clip
    }

    /// `0x4AC0E0(1)` from a type-4 trigger (0x470D5F…0x470DB3): the `[0x628168]` element
    /// `scene_index` through the `CAudioElem` cast (0x5A134B), when 0x4AC0D0 (`rec+0xC`, the
    /// Type) answers 1 and its `rec+0xE` (PatchID) is not negative, gets `+0xA2 = 1`. Answers
    /// whether it did.
    pub fn fire(&mut self, scene_index: usize) -> bool {
        match self.elements.iter_mut().find(|e| e.scene_index == scene_index) {
            Some(e) if e.kind == 1 && e.patch >= 0 => {
                e.triggered = true;
                true
            }
            _ => false,
        }
    }

    /// `0x4AF440(pos)`, a smackable's first move (0x476810 → 0x4768A6): of the Type-1 audio
    /// elements, the first nearest by the octagonal x–z distance `max + ¼·min` (0x4AF509…0x4AF536)
    /// that is within 5 m (`[0x5B2400]`, `<=`: `test ah, 0x41 ; je`) and nearer than the best so
    /// far (`<`, from 10000) gets `+0xA2 = 1` (0x4AF5AC) — no PatchID test here. Answers its index.
    pub fn fire_nearest(&mut self, pos: [f32; 3]) -> Option<usize> {
        let k = nearest_triggered(&self.elements, pos)?;
        self.elements[k].triggered = true;
        Some(k)
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
            self.update_element(out, i, tick8, d, listener, [dx, dy, dz], right);
        }
    }

    /// `Ambience_updateElement` 0x4AEAC0.
    #[allow(clippy::too_many_arguments)]
    fn update_element(&mut self, out: &AudioOutput, i: usize, tick8: i32, d: f32, listener: [f32; 3], rel: [f32; 3], right: [f32; 3]) {
        let e = self.elements[i].clone();
        // 0x4AEAE1…0x4AEB28: `d > Range + 100` (`test ah, 0x41`), a PatchID out of the index or
        // negative, or an empty bank
        if d > e.range + 100.0 || e.patch < 0 || !self.index.get(&(e.patch as u16)).is_some_and(|&(_, l)| l != 0) {
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
                    // 0x4AEB5F…0x4AEBCF: the farthest entry whose voice **has run out** (a
                    // handle, and 0x566380 answers 1), its element measured afresh (0x5323A0), and
                    // only for a nearer element (`d < far`, `test ah, 1`); the old element's slot
                    // is not cleared here — the driver's ownership check does it
                    let mut best = (0usize, 0.0f32);
                    for (k, s) in self.pool.iter().enumerate() {
                        let (Some(el), Some(v)) = (s.element, s.voice) else { continue };
                        if out.is_playing(v) {
                            continue;
                        }
                        let p = self.elements[el].position;
                        let dk = ((p[0] - listener[0]).powi(2) + (p[1] - listener[1]).powi(2) + (p[2] - listener[2]).powi(2)).sqrt();
                        if dk > best.1 {
                            best = (k, dk);
                        }
                    }
                    if d >= best.1 {
                        return;
                    }
                    best.0
                }
            };
            let repeats = e.min_repeat + if e.random_repeat > 0 { self.rand() % (e.random_repeat + 1) } else { 0 };
            // 0x4AEBE6…0x4AEC94: the entry's element, `+8 = −1`, the repeats and the element's
            // slot; the handle and the patch stay what they were
            let s = &mut self.pool[slot];
            s.element = Some(i);
            s.repeats_left = repeats;
            self.elements[i].slot = slot as i32;
        }
        let slot = self.elements[i].slot as usize;
        // 0x4AECA0…0x4AECCB: another patch stops the voice
        if self.pool[slot].patch != e.patch {
            if let Some(v) = self.pool[slot].voice.take() {
                out.stop(v);
            }
            self.pool[slot].patch = e.patch;
        }
        let state = match self.pool[slot].voice {
            None => VoiceState::None,
            Some(v) if out.is_playing(v) => VoiceState::Playing,
            Some(_) => VoiceState::Finished,
        };
        match step(e.kind, state, self.pool[slot].repeats_left, d, e.range, e.triggered) {
            Step::Release => {
                self.release(out, i);
                return;
            }
            Step::Hold | Step::Prefetch => return,
            Step::Play { replay, consume } => {
                if replay {
                    self.pool[slot].repeats_left -= 1;
                    self.pool[slot].voice = None;
                }
                if consume {
                    self.elements[i].triggered = false;
                    tracing::debug!(element = e.scene_index, label = e.label, patch = e.patch, d, "ambience: triggered element plays");
                }
            }
        }
        let level = e.level(d);
        let vol = self.voice_volume(level);
        // §7.5 pan, folded to the mixer's left/right: Type 2 is non-positional, 3 and 4 are
        // pinned; outside `Range` the pan stays 0 with the level (0x4AED39, 0x4AED9F)
        let pan = if d >= e.range {
            0.0
        } else {
            match e.kind {
                2 => 0.0,
                3 => -1.0,
                4 => 1.0,
                _ => {
                    let len = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt().max(0.001);
                    ((rel[0] * right[0] + rel[1] * right[1] + rel[2] * right[2]) / len).clamp(-1.0, 1.0)
                }
            }
        };
        match self.pool[slot].voice {
            Some(v) if out.is_playing(v) => {
                out.set(v, vol, 1.0);
                out.set_pan(v, pan);
            }
            _ => {
                if let Some(c) = self.clip(e.patch as u16) {
                    let looping = c.loop_range.is_some();
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

/// 0x4AF440's search (see [`Ambience::fire_nearest`]).
pub fn nearest_triggered(elements: &[Element], pos: [f32; 3]) -> Option<usize> {
    let mut best = 10000.0f32;
    let mut found = None;
    for (k, e) in elements.iter().enumerate() {
        let a = (e.position[0] - pos[0]).abs();
        let b = (e.position[2] - pos[2]).abs();
        let est = if a > b { b * 0.25 + a } else { a * 0.25 + b };
        if est <= 5.0 && est < best && e.kind == 1 {
            best = est;
            found = Some(k);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(kind: i32, range: f32, fade_in: f32, volume: i32) -> Element {
        Element { position: [0.0; 3], kind, patch: 1, range, fade_in, trigger: 0, fade_out: 0, min_repeat: 0, random_repeat: 0, volume, next_trigger: 0, slot: -1, triggered: false, scene_index: 0, label: String::new() }
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
        let e = &Element::from_scene(&scene, 7)[0];
        assert_eq!(e.scene_index, 7);
        assert_eq!(e.patch, 338);
        assert_eq!(e.kind, 0);
        assert_eq!(e.range, 200.0);
        assert_eq!(e.trigger, 40);
        assert_eq!(e.fade_out, 108);
        assert_eq!(e.random_repeat, 2);
        assert_eq!(e.volume, 88, "field 11 is the volume, not the editor scratch at 9/10");
        assert_eq!(e.label, "Ships Bell 3 733");
    }

    #[test]
    fn a_type_1_element_plays_only_when_flagged() {
        use VoiceState::*;
        // not flagged: the bank is fetched and nothing plays (0x4AED7A)
        assert_eq!(step(1, None, 0, 10.0, 100.0, false), Step::Prefetch);
        assert_eq!(step(1, Playing, 0, 10.0, 100.0, false), Step::Prefetch);
        // flagged: it plays (a start, or an update of the playing voice) and the flag is consumed
        assert_eq!(step(1, None, 0, 10.0, 100.0, true), Step::Play { replay: false, consume: true });
        assert_eq!(step(1, Playing, 0, 10.0, 100.0, true), Step::Play { replay: false, consume: true });
        // run out inside Range: the slot is held and a new flag does not restart it
        assert_eq!(step(1, Finished, 0, 100.0, 100.0, true), Step::Hold);
        // run out outside Range: the slot goes back
        assert_eq!(step(1, Finished, 0, 100.5, 100.0, true), Step::Release);
        // repeats replay without the flag
        assert_eq!(step(1, Finished, 2, 10.0, 100.0, false), Step::Play { replay: true, consume: false });
        // the periodic kinds play on every visit and give the slot back when done
        assert_eq!(step(0, None, 0, 10.0, 100.0, false), Step::Play { replay: false, consume: false });
        assert_eq!(step(0, Finished, 0, 10.0, 100.0, false), Step::Release);
    }

    #[test]
    fn the_shipped_type_4_triggers_reach_type_1_elements() {
        // castle_audio.scn's four type-4 triggers link elements 294…297 ("Bad Dog 1592a"), forest's
        // one element 102 ("treefall"); every one is Type 1 with a PatchID
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let dir = paths.game_data().join("Track");
        for (file, links) in [("castle_audio.scn", &[294usize, 295, 296, 297][..]), ("forest_audio.scn", &[102][..])] {
            let Ok(text) = std::fs::read_to_string(dir.join(file)) else { return };
            let scene = Scene::parse(&text).unwrap();
            let mut elements = Element::from_scene(&scene, 0);
            for &l in links {
                let e = elements.iter_mut().find(|e| e.scene_index == l).expect("an audio element");
                assert_eq!(e.kind, 1, "{file} {l}");
                assert!(e.patch >= 0);
            }
        }
    }

    #[test]
    fn a_smackable_flags_the_nearest_type_1_within_5_m() {
        let mut a = elem(1, 100.0, 0.0, 100);
        a.position = [3.0, 50.0, 0.0];
        let mut b = elem(1, 100.0, 0.0, 100);
        b.position = [0.0, 0.0, 2.0];
        let mut c = elem(0, 100.0, 0.0, 100);
        c.position = [0.0, 0.0, 0.5];
        let v = vec![a, b, c];
        // octagonal x–z only: b (2.0) beats a (3.0) whatever the height; c is not Type 1
        assert_eq!(nearest_triggered(&v, [0.0, 0.0, 0.0]), Some(1));
        // exactly 5 m still counts (`test ah, 0x41 ; je`), past it nothing
        assert_eq!(nearest_triggered(&v[..1], [-2.0, 0.0, 0.0]), Some(0));
        assert_eq!(nearest_triggered(&v[..1], [-2.01, 0.0, 0.0]), None);
        // max + ¼·min
        assert_eq!(nearest_triggered(&v[..1], [0.0, 0.0, 4.0]), Some(0));
        assert_eq!(nearest_triggered(&v[..1], [0.0, 0.0, 4.5]), None);
    }

    #[test]
    fn the_level_truncates_before_the_percentage() {
        // 0x4AEE63…0x4AEEB9: `_ftol` first, then `level · Volume / 100` in integers
        let e = elem(0, 200.0, 0.0, 88);
        let raw = (((200.0f32 * 200.0 - 150.0 * 150.0) / (200.0 * 200.0)) * 127.0) as i32;
        assert_eq!(e.level(150.0), raw * 88 / 100);
        assert_eq!(e.level(200.0), 0);
        assert_eq!(e.level(250.0), 0);
    }
}
