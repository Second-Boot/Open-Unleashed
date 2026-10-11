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

use crate::mixer::{byte_gain, pan_angle, AudioOutput, Clip, ClipCache, VoiceId, BEND_ROOT, LEVEL_MAX};
use nfs_formats::{bigf::Bigf, bnk::Bnk, scn::{ElementKind, Scene}};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The 16 ambience voices are 0x39..0x48 in the original.
pub const VOICES: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub position: [f32; 3],
    /// 0/2/3/4 = periodic one-shot, 1 = triggered (see the module doc), 2 = non-positional, 3/4 = `panBase − 0x4000` / `panBase + 0x3FFF`
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
    /// `emitterVel` of [`doppler_pitch_word`] — `[0.0;0.0;0.0]` for every plain `.scn`
    /// `AUDIO_ELEMENT` (`engine-audio.md` §7.4: `Ambience_updateElement(rec, tick>>3, NULL, …)`,
    /// so static ambience always plays at exactly 1.0×), refreshed every frame for an
    /// animated-object emitter by [`Ambience::sync_animated`] (§7.4's `p1`/`p2`/`vel`).
    pub velocity: [f32; 3],
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
                    velocity: [0.0; 3],
                })
            })
            .collect()
    }

    /// An animated-object emitter from its `SoundDef` (`engine-audio.md` §7.3/§7.4,
    /// `0x477A8B..0x477AD4`): `Type` is always 0 — never written by that field-copy, which leaves
    /// the destination record's `+0xC` at the zeroed allocation's 0 — so it uses the same
    /// "127 inside `FadeIn`, falloff to `Range`" law as every plain `Type 0/2/3/4/5` scene element
    /// ([`Element::level`]); nothing here is `fadeScale`-dependent (that only matters for `Type`
    /// 1/6/7). `scene_index` only needs to be unique among the port's own animated emitters — the
    /// original's `[0x628960+0x20]` object list is a separate array from the `.scn` one
    /// `scene_index` otherwise indexes, so any id disjoint from the `.scn` list's own is faithful.
    pub fn from_sound_def(def: &nfs_formats::animdefs::SoundDef, label: String, scene_index: usize) -> Element {
        Element {
            position: [0.0; 3],
            kind: 0,
            patch: def.patch_id as i16,
            range: def.range,
            fade_in: def.fade_in,
            trigger: def.trigger as i32,
            fade_out: def.fade_out as i32,
            min_repeat: def.min_repeat as i32,
            random_repeat: def.random_repeat as i32,
            volume: def.volume as i32,
            next_trigger: 0,
            slot: -1,
            triggered: false,
            scene_index,
            label,
            velocity: [0.0; 3],
        }
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

/// `Amb_dopplerPitch` `0x4A6F10` (`engine-audio.md` §7.5, `COVERAGE-2026-09-26.md` §14.38):
/// `pitch16_16 = clamp(340.0 / (dot(unit(v), emitterVel) − dot(unit(v), listenerVel) + 340.0) ·
/// 65536.0, 1, 0xA0000)`, `v = elemPos − listener` (`rel`, **not** normalised by the caller —
/// `unit(v)` is this function's own job, matching `0x4A6F10`'s internal normalise-then-dot). The
/// port's mixer takes a **pitch word** where `0x1000` (4096) is 1.0×, not the original's 16.16
/// (`0x10000` is 1.0×) — `engine-audio.md` line 258's own "playback ratio = pitchWord / 0x1000" —
/// so the final `>> 4` (`0x10000 >> 4 == 0x1000`) is the port's own unit conversion, not part of
/// `0x4A6F10` itself. `listenerVel` (`[0x655E44]`) is always `NULL` in this build (§7.5), so it is
/// hard-coded to zero here rather than threaded through every caller. With `emitterVel = [0;3]`
/// (every plain `.scn` element) this is `340/(0+340)·65536 = 65536`, `>> 4 = 4096 =
/// `[resample::PITCH_UNIT`] exactly — the original's own "static ambience always plays at exactly
/// 1.0" (§7.5), reproduced rather than special-cased.
pub fn doppler_pitch_word(rel: [f32; 3], emitter_vel: [f32; 3]) -> u16 {
    let len = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
    let dot = if len > 0.0 {
        (rel[0] * emitter_vel[0] + rel[1] * emitter_vel[1] + rel[2] * emitter_vel[2]) / len
    } else {
        0.0
    };
    let denom = dot + 340.0;
    let pitch16_16: i64 = if denom > 0.0 { (340.0 / denom * 65536.0) as i64 } else { 0xA0000 };
    (pitch16_16.clamp(1, 0xA0000) >> 4) as u16
}

/// Which of [`Ambience`]'s two parallel emitter lists a pool slot's owner lives in — the port's
/// own disambiguator for sharing one 16-voice pool between `elements` (the `.scn` list) and
/// `animated` (the `[0x628960+0x20]` list §7.4 walks separately); the original tells the two
/// apart by pointer identity (`CAudioElem*`), which a plain `usize` index cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Static,
    Animated,
}

#[derive(Default, Clone, Copy)]
struct Slot {
    element: Option<(List, usize)>,
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
    /// What `Voice_play`'s update of a playing voice hands the strip hook this pass
    /// ([`crate::mixer::voice_play_filter`], 0x4A9FA8…0x4AA03B): set by the caller every audio update.
    pub filter: Option<i32>,
    pub elements: Vec<Element>,
    /// The animated-object emitters (`engine-audio.md` §7.4's second half, after the `.scn`-element
    /// loop above): built once from `AnimDefs::sounds()` by [`Ambience::add_animated`], refreshed
    /// every frame by [`Ambience::sync_animated`] and shared the pool with `elements` — walked in
    /// full every update ([`Ambience::update_animated`]), unlike `elements`' own one-quarter-a-tick
    /// visitation, matching `0x4AEF30`'s own separate, unconditional second loop.
    pub animated: Vec<Element>,
    /// PatchID → bank bytes in `track.viv`
    index: HashMap<u16, (usize, usize)>,
    viv: Vec<u8>,
    clips: ClipCache<u16>,
    pool: [Slot; VOICES],
    rng: u32,
    /// `[0x5D1750]` AmbientLevel 0..127
    pub ambient_level: u8,
    /// `game.spl[3]` (100 shipped)
    game_trim: u32,
    /// the sky lightning's thunder ([`Thunder`])
    pub thunder: Thunder,
}

/// The thunder of the sky's lightning (`RE/RENDERING.md` "Lightning", `sfx.md` "The thunder"): three
/// globals — `[0x656494]` the countdown, `[0x656834]` the patch, `[0x656498]` the pan angle — that
/// the race audio's init 0x4A8660 sets to 0, −1, 0 (0x4A87B7…0x4A87C3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thunder {
    pub delay: i32,
    pub patch: i32,
    pub pan: i32,
}

impl Default for Thunder {
    fn default() -> Self {
        Self { delay: 0, patch: -1, pan: 0 }
    }
}

/// What one pass of 0x4A8380 does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThunderStep {
    /// nothing pending
    Idle,
    /// counting down: `SndBank_get(0, patch, 0)` — the patch's bank asked for (0x4A8570, the
    /// streamed-patch cache), the answer dropped
    Fetch(i32),
    /// the countdown is out: `Bank_playOneShot` 0x4A98E0 on bank `[0x6564F0]` (−2, 0x4A8DA0: the
    /// same streamed-patch lookup, kind 0, 0x4A9B8D…0x4A9BAE) with the patch, the pan angle, level
    /// 0x7F, bend 0x40, pitch 0x10000 (`>> 4`: the word 4096) and aux 0x7F; the patch back to −1
    Play { patch: i32, pan: i32 },
}

impl Thunder {
    /// **0x4A8340**(lead, pan) as a strike calls it: the patch `0x3DF + r % 9` (991…999,
    /// `thunder_991.bnk`…`thunder_999.bnk` in `track.viv`; `div` unsigned) of the `0x538040`
    /// random the sky drew, the countdown `0x87 − lead` (95 / 55 / 9 updates for the modes' 0x28 /
    /// 0x50 / 0x7E), the pan kept. A second call before the play replaces all three.
    pub fn call(&mut self, lead: i32, pan: i32, draw: u32) {
        self.patch = 0x3DF + (draw % 9) as i32;
        self.delay = 0x87 - lead;
        self.pan = pan;
    }

    /// **0x4A8380**, once an audio update (0x4A813F, after the speech service): a positive countdown
    /// goes down by one and fetches the patch; at 0 a patch above −1 plays and is cleared.
    pub fn step(&mut self) -> ThunderStep {
        if self.delay > 0 {
            self.delay -= 1;
            return ThunderStep::Fetch(self.patch);
        }
        if self.patch > -1 {
            let patch = self.patch;
            self.patch = -1;
            return ThunderStep::Play { patch, pan: self.pan };
        }
        ThunderStep::Idle
    }
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
        Ok(Self { filter: None, elements, animated: Vec::new(), index, viv, clips: ClipCache::new(true), pool: [Slot::default(); VOICES], rng: 0x1234_5678, ambient_level, game_trim, thunder: Thunder::default() })
    }

    fn rand(&mut self) -> i32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        ((self.rng >> 16) & 0x7FFF) as i32
    }

    /// The patch's first sample as the bank has it — its own loop points or none. *(Until
    /// 2026-09-26 the port forced a whole-sample loop on Type 1, which it took for "continuous";
    /// `Voice_play` 0x4A9C10 plays the patch as it is.)*
    fn clip(&mut self, patch: u16) -> Option<Arc<Clip>> {
        let (index, viv) = (&self.index, &self.viv);
        self.clips.get_or_load(patch, || {
            let clip = index.get(&patch).and_then(|&(o, l)| {
                let bytes = viv.get(o..o + l)?;
                let bank = Bnk::parse(bytes).ok()?;
                let p = (0..bank.slot_count().max(1)).find_map(|s| bank.decode_slot(bytes, s).ok())?;
                Some(Arc::new(Clip::from_pcm(p)))
            });
            if clip.is_none() {
                tracing::debug!(patch, "ambience bank missing or unreadable");
            }
            clip
        })
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
        byte_gain(((self.ambient_level as i32 * self.game_trim as i32 * level * 85) >> 21).clamp(0, LEVEL_MAX))
    }

    fn list_ref(&self, list: List) -> &[Element] {
        match list {
            List::Static => &self.elements,
            List::Animated => &self.animated,
        }
    }

    fn list_mut(&mut self, list: List) -> &mut Vec<Element> {
        match list {
            List::Static => &mut self.elements,
            List::Animated => &mut self.animated,
        }
    }

    /// The animated-object emitters' per-frame position/velocity refresh (§7.4: `p1 =
    /// obj.EvalPos(tick)`, `p2 = obj.EvalPos(tick + 0x20)`, `vel = 2·(p1 − p2)` — read directly off
    /// `0x4AEF30`'s disassembly (2026-09-30), which **corrects** an earlier note's
    /// `(p2 − p1)·64/dt`: the actual subtraction order is `p1 − p2`, doubled (`64/0x20 == 2`), not
    /// `p2 − p1`). The record's own position field gets **`p2`** (the *later* sample, not `p1`) —
    /// `0x477a2e`'s copy writes `[esp+0x1c..0x24]` (still holding `p2` at that point) into the
    /// record, matching `0x4AEF30`'s own field order. The caller supplies both samples (typically
    /// `nfs_formats::crp::AnimTrack::sample` on the object's own path — see that call site's own
    /// `APPROXIMATION` note for the frame-rate/loop caveat) so this method stays free of any
    /// track/animation-format dependency.
    pub fn sync_animated(&mut self, index: usize, p1: [f32; 3], p2: [f32; 3]) {
        if let Some(e) = self.animated.get_mut(index) {
            e.position = p2;
            e.velocity = [2.0 * (p1[0] - p2[0]), 2.0 * (p1[1] - p2[1]), 2.0 * (p1[2] - p2[2])];
        }
    }

    /// Registers one `SoundDef` as an animated-object emitter and returns its index for
    /// [`Ambience::sync_animated`] (`AnimObj_buildAudioRecords`'s `0x477a8b..0x477ad4` field copy,
    /// `engine-audio.md` §7.3 — one call per animated object whose `Base` tag has a
    /// `SECTION:Audio`, `AnimDefs::sound_by_fourcc`).
    pub fn add_animated(&mut self, def: &nfs_formats::animdefs::SoundDef, label: String) -> usize {
        let index = self.animated.len();
        self.animated.push(Element::from_sound_def(def, label, index));
        index
    }

    /// One update (`0x4AEF30`'s first loop). `tick` is the 64 Hz game tick; `side` and `forward`
    /// are the listener's rows `+0xC` / `+0x24` of `mixer::pan_angle` (`side` negated under
    /// `MIRROR`), and `pan_base` the angle 0x4A7A4E…0x4A7AAC hands 0x4AEF30: `pan_angle` of the
    /// player car's forward `car+0x3D0` dotted with the same two rows (not an offset — a
    /// direction).
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, out: &AudioOutput, tick: u64, listener: [f32; 3], side: [f32; 3], forward: [f32; 3], pan_base: u16) {
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
            if slot >= 0 && self.pool[slot as usize].element != Some((List::Static, i)) {
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
            self.update_element(out, List::Static, i, tick8, d, listener, [dx, dy, dz], (side, forward, pan_base));
        }
    }

    /// `0x4AEF30`'s **second** loop (§7.4, "then the animated-object emitters"): every entry of
    /// [`Ambience::animated`] visited **every** frame — no one-quarter phasing, unlike
    /// [`Ambience::update`]'s `elements` — with `rec.nextTrigger` a fixed phase (`= Trigger`,
    /// re-armed every frame rather than accumulated with jitter) and `tickDiv8 = (tick % 100) >>
    /// 3`, both distinct from the static loop's own `tick >> 3`. Call [`Ambience::sync_animated`]
    /// first each frame so `position`/`velocity` are this tick's.
    #[allow(clippy::too_many_arguments)]
    pub fn update_animated(&mut self, out: &AudioOutput, tick: u64, listener: [f32; 3], side: [f32; 3], forward: [f32; 3], pan_base: u16) {
        if self.ambient_level == 0 {
            return;
        }
        let tick8 = ((tick % 100) >> 3) as i32;
        for i in 0..self.animated.len() {
            let slot = self.animated[i].slot;
            if slot >= 0 && self.pool[slot as usize].element != Some((List::Animated, i)) {
                self.animated[i].slot = -1;
            }
            self.animated[i].next_trigger = self.animated[i].trigger;
            let e = &self.animated[i];
            let (dx, dy, dz) = (e.position[0] - listener[0], e.position[1] - listener[1], e.position[2] - listener[2]);
            let est = dx.abs().max(dz.abs()) + 0.25 * dx.abs().min(dz.abs());
            if est > e.range + 100.0 && e.slot <= -1 {
                continue;
            }
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            self.update_element(out, List::Animated, i, tick8, d, listener, [dx, dy, dz], (side, forward, pan_base));
        }
    }

    /// `Ambience_updateElement` 0x4AEAC0.
    #[allow(clippy::too_many_arguments)]
    fn update_element(&mut self, out: &AudioOutput, list: List, i: usize, tick8: i32, d: f32, listener: [f32; 3], rel: [f32; 3], (side, forward, pan_base): ([f32; 3], [f32; 3], u16)) {
        let e = self.list_ref(list)[i].clone();
        // 0x4AEAE1…0x4AEB28: `d > Range + 100` (`test ah, 0x41`), a PatchID out of the index or
        // negative, or an empty bank
        if d > e.range + 100.0 || e.patch < 0 || self.index.get(&(e.patch as u16)).is_none_or(|&(_, l)| l == 0) {
            self.release(out, list, i);
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
                        let (Some((el_list, el)), Some(v)) = (s.element, s.voice) else { continue };
                        if out.is_playing(v) {
                            continue;
                        }
                        let p = self.list_ref(el_list)[el].position;
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
            s.element = Some((list, i));
            s.repeats_left = repeats;
            self.list_mut(list)[i].slot = slot as i32;
        }
        let slot = self.list_ref(list)[i].slot as usize;
        // 0x4AECA0…0x4AECCB: another patch stops the voice
        if self.pool[slot].patch != e.patch {
            // 0x4AECB8: 0x4A9890, the five-tick release 0x565DD0(h, 5, −1)
            if let Some(v) = self.pool[slot].voice.take() {
                out.release(v);
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
                self.release(out, list, i);
                return;
            }
            Step::Hold | Step::Prefetch => return,
            Step::Play { replay, consume } => {
                if replay {
                    self.pool[slot].repeats_left -= 1;
                    self.pool[slot].voice = None;
                }
                if consume {
                    self.list_mut(list)[i].triggered = false;
                    tracing::debug!(element = e.scene_index, label = e.label, patch = e.patch, d, "ambience: triggered element plays");
                }
            }
        }
        let level = e.level(d);
        let vol = self.voice_volume(level);
        // §7.5 pan, the 16-bit angle: Type 2 is non-positional (0), 3 is `panBase − 0x4000` and 4
        // `panBase + 0x3FFF` (0x4AEDAF…0x4AEDC5), the rest 0x4A6FE0 of the element's offset;
        // outside `Range` the pan stays 0 with the level (0x4AED39, 0x4AED9F). (Until 2026-09-27
        // Types 3 / 4 were pinned hard to the two sides, the other way round at `panBase` 0.)
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let pan: u16 = if d >= e.range {
            0
        } else {
            match e.kind {
                2 => 0,
                3 => pan_base.wrapping_sub(0x4000),
                4 => pan_base.wrapping_add(0x3FFF),
                _ => pan_angle(dot(rel, side), dot(rel, forward)),
            }
        };
        // §7.5 doppler (0x4A6F10): `emitterVel = NULL` for every plain scene element (`e.velocity`
        // stays `[0.0;0.0;0.0]`), which [`doppler_pitch_word`] collapses to exactly `PITCH_UNIT` —
        // so this is a strict extension, not a behaviour change, for every element but an
        // animated-object one ([`Element::from_sound_def`]/[`Ambience::sync_animated`]).
        let pitch = doppler_pitch_word(rel, e.velocity);
        match self.pool[slot].voice {
            Some(v) if out.is_playing(v) => {
                out.set_word(v, vol, pitch, BEND_ROOT);
                out.set_pan(v, pan);
                // 0x4AEED6 is `Voice_play` on voice `slot + 0x39` (bank kind 5)
                out.voice_play_filter(v, self.filter);
            }
            _ => {
                if let Some(c) = self.clip(e.patch as u16) {
                    let looping = c.loop_range.is_some();
                    tracing::debug!(patch = e.patch, label = e.label, d = d as i32, level, looping, pitch, "ambience voice");
                    self.pool[slot].voice = Some(out.play_word(c, vol, pitch, BEND_ROOT, pan, looping));
                } else {
                    self.release(out, list, i);
                }
            }
        }
    }

    fn release(&mut self, out: &AudioOutput, list: List, i: usize) {
        let slot = self.list_ref(list)[i].slot;
        if slot >= 0 {
            self.release_slot(out, slot as usize);
        }
        self.list_mut(list)[i].slot = -1;
    }

    /// Every release of an ambience voice is 0x4A9890 (0x4AE9F3, 0x4AED19, 0x4AEF0C, 0x4AF0A8,
    /// and the shutdown 0x4AF760's 0x4AF782): 0x565DD0(h, 5, −1), the five-tick ramp.
    fn release_slot(&mut self, out: &AudioOutput, slot: usize) {
        if let Some(v) = self.pool[slot].voice.take() {
            out.release(v);
        }
        if let Some((list, e)) = self.pool[slot].element.take()
            && self.list_ref(list)[e].slot == slot as i32
        {
            self.list_mut(list)[e].slot = -1;
        }
        self.pool[slot] = Slot::default();
    }

    pub fn stop(&mut self, out: &AudioOutput) {
        for s in 0..VOICES {
            self.release_slot(out, s);
        }
    }

    /// The thunder's pass 0x4A8380 ([`Thunder::step`]). The play is `Bank_playOneShot` 0x4A98E0's
    /// kind-5 arm: the level `([0x5D1750] AmbientLevel · [0x5D14BC] game.spl[3] · 0x7F · 85) >> 21`
    /// (0x4A9969…0x4A9A30, the ambience's own [`Self::voice_volume`]), the pan byte of the angle
    /// ([`crate::mixer::pan_byte`], 0x4A9B4B…0x4A9B84), no bend, the patch as the bank has it.
    /// (Was an APPROXIMATION, settled below:) the original plays only if the streamed patch has loaded during the
    /// countdown (0x4A8570 answers −1 until then, and 0x4A9BC7 then plays nothing); the port's
    /// banks are in memory, so it always plays. **Observed on the original (2026-10-01 SV, Wine, industrial
    /// with `skyrendering = 2`, gdb at 0x4A8340 / 0x4A98E0 / 0x4A9BC7, two separate races):** a strike's
    /// call 0x4A8340 (mode 0x50), the countdown 0x4A8380 running 55 → 1, `playOneShot` 0x4A98E0 with the
    /// thunder patch (995 / 998) and the bank −2, and then 0x4A9BC7 — the streamed patch had NOT loaded, so
    /// both first thunders were **silent**. The game runs at well under one frame a second under the
    /// breakpoints, so the disk / streaming race is far slower than on a real machine: whether a real
    /// first thunder is silent depends on streaming speed. **MEASURED at real speed 2026-10-02 (SRV-B,
    /// `RE/notes/REFERENCE-2026-10-02-SRVB.md` §6): non-stopping logging breakpoints, industrial, 3 races, 5 strikes — 0x4A8570 answered
    /// handles 0x18…0x1A (never 0xFF) and every first thunder was started.** The port's "always plays" is
    /// the original's behaviour at real speed; SV's silence was the debugger's slowdown.
    pub fn thunder_update(&mut self, out: &AudioOutput) {
        match self.thunder.step() {
            ThunderStep::Idle => {}
            ThunderStep::Fetch(patch) => {
                let _ = self.clip(patch as u16);
            }
            ThunderStep::Play { patch, pan } => {
                let vol = self.voice_volume(LEVEL_MAX);
                match self.clip(patch as u16) {
                    Some(c) => {
                        let looping = c.loop_range.is_some();
                        let id = out.play_word(c, vol, crate::resample::PITCH_UNIT, BEND_ROOT, pan as u16, looping);
                        tracing::info!(patch, pan, vol, playing = out.is_playing(id), "thunder");
                    }
                    None => tracing::info!(patch, "thunder: patch missing from track.viv"),
                }
            }
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
        Element {
            position: [0.0; 3],
            kind,
            patch: 1,
            range,
            fade_in,
            trigger: 0,
            fade_out: 0,
            min_repeat: 0,
            random_repeat: 0,
            volume,
            next_trigger: 0,
            slot: -1,
            triggered: false,
            scene_index: 0,
            label: String::new(),
            velocity: [0.0; 3],
        }
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

    /// `Amb_dopplerPitch` `0x4A6F10`/§7.5: zero emitter velocity is exactly `PITCH_UNIT` — the
    /// "static ambience always plays at exactly 1.0" fact, reproduced as a special case of the
    /// general formula rather than a separate code path.
    #[test]
    fn zero_velocity_is_exactly_pitch_unit() {
        assert_eq!(doppler_pitch_word([10.0, 0.0, 0.0], [0.0; 3]), crate::resample::PITCH_UNIT);
        assert_eq!(doppler_pitch_word([0.0, 5.0, -3.0], [0.0; 3]), crate::resample::PITCH_UNIT);
        // the degenerate `rel == 0` guard (listener exactly at the emitter) also gives 1.0×
        assert_eq!(doppler_pitch_word([0.0; 3], [1.0, 2.0, 3.0]), crate::resample::PITCH_UNIT);
    }

    /// An emitter closing on the listener (`v` points from listener to emitter, `dot(unit(v),
    /// vel) < 0` for an emitter moving toward the listener) raises the pitch; one receding lowers
    /// it — `0x4A6F10`'s formula, `340 / (dot(unit(v), vel) + 340) · 65536`, `>> 4` for the port's
    /// pitch word. Hand-computed: emitter 100 m in +z, closing at 20 m/s along −z
    /// (`dot(unit(v),vel) = −20`): `340/(−20+340) = 1.0625`, `·65536 = 69632`, `>>4 = 4352`.
    #[test]
    fn doppler_pitch_matches_the_hand_computed_formula() {
        let rel = [0.0, 0.0, 100.0];
        let closing = doppler_pitch_word(rel, [0.0, 0.0, -20.0]);
        assert_eq!(closing, 4352, "closing emitter should raise the pitch above PITCH_UNIT (4096)");
        assert!(closing > crate::resample::PITCH_UNIT);
        // receding at the same speed: 340/(20+340)=0.9444.., ·65536=61895 (truncated), >>4=3868
        let receding = doppler_pitch_word(rel, [0.0, 0.0, 20.0]);
        assert!(receding < crate::resample::PITCH_UNIT);
        assert_eq!(receding, ((340.0f64 / 360.0 * 65536.0) as i64 >> 4) as u16);
        // the 0x4A6F10 clamp, [1, 0xA0000] before the port's own `>> 4`: an extreme closing speed
        // saturates at 0xA0000 >> 4 = 0xA000, never overflows or goes negative
        let extreme = doppler_pitch_word(rel, [0.0, 0.0, -339.999]);
        assert_eq!(extreme, 0xA000);
    }

    /// The animated-object emitter path end to end: [`Ambience::add_animated`] registers a
    /// `SoundDef`, [`Ambience::sync_animated`] feeds it a moving path's `p1`/`p2` samples each
    /// frame (`AnimTrack::sample`'s job in `nfs-game`, stood in here by a straight-line stub so
    /// this test stays free of `nfs-formats`), and the emitter's position/velocity/pitch come out
    /// exactly as the static-element path's formulas would give them for the same numbers —
    /// proving the doppler wiring without needing to play or hear anything.
    #[test]
    fn animated_emitter_position_velocity_and_pitch_over_a_scripted_path() {
        let def = nfs_formats::animdefs::SoundDef { patch_id: 833, range: 300.0, fade_in: 17.0, trigger: 0, fade_out: 0, min_repeat: 0, random_repeat: 0, volume: 100 };
        let mut amb = Ambience {
            filter: None,
            elements: Vec::new(),
            animated: Vec::new(),
            index: HashMap::new(),
            viv: Vec::new(),
            clips: ClipCache::new(true),
            pool: [Slot::default(); VOICES],
            rng: 1,
            ambient_level: 127,
            game_trim: 100,
            thunder: Thunder::default(),
        };
        let idx = amb.add_animated(&def, "train1_833".to_string());
        assert_eq!(idx, 0);
        assert_eq!(amb.animated.len(), 1);
        assert_eq!(amb.animated[0].kind, 0, "Type is always 0 for a SoundDef-derived record — 0x477a8b..0x477ad4 never writes +0xC");
        assert_eq!(amb.animated[0].patch, 833);

        // a straight-line "train" moving along +x at 40 m/s (2560 m per 64-tick second — a plain
        // stand-in for `AnimTrack::sample`, logged in a scripted-run trace)
        let speed = 40.0f32;
        let sample = |tick: u64| [speed * (tick as f32) / 64.0, 0.0, 200.0];
        let mut log = Vec::new();
        for tick in [0u64, 64, 128, 192] {
            let (p1, p2) = (sample(tick), sample(tick + 0x20));
            amb.sync_animated(idx, p1, p2);
            let e = &amb.animated[idx];
            // p2 is the position the record carries (0x477a2e's copy — the *later* sample)
            assert_eq!(e.position, p2);
            // vel = 2·(p1 − p2): for a constant velocity path this is exactly −2× the true
            // per-tick displacement, i.e. `[-2·speed/2, 0, 0]` for the 0x20-tick gap here
            let expected_vel_x = 2.0 * (p1[0] - p2[0]);
            assert!((e.velocity[0] - expected_vel_x).abs() < 1e-4);
            let listener = [0.0, 0.0, 0.0];
            let rel = [e.position[0] - listener[0], e.position[1] - listener[1], e.position[2] - listener[2]];
            let pitch = doppler_pitch_word(rel, e.velocity);
            log.push((tick, e.position, e.velocity, pitch));
        }
        tracing::debug!(?log, "scripted autobahn train emitter state (position, vel, pitch word)");
        // `vel = 2·(p1 − p2)` is the *reverse* of the object's true per-tick displacement
        // (confirmed by re-disassembling `0x4AEF30` this pass — it is `p1 − p2`, not `p2 − p1` as
        // an earlier note had it), so for a train genuinely receding in +x while the listener sits
        // near its path, `dot(unit(rel), vel)` comes out **negative** and the pitch rises above
        // `PITCH_UNIT` throughout — the disassembled formula's own behaviour, not the physically
        // "expected" direction, and reproduced exactly rather than corrected: every rule here is
        // traced to `0x4A6F10`/`0x4AEF30`, not invented.
        for &(_, _, _, pitch) in &log {
            assert!(pitch > crate::resample::PITCH_UNIT, "{log:?}");
        }
        // pitch grows monotonically as the (constant-speed) train pulls away — the ratio `rel_x /
        // |rel|` shrinks each tick, weakening the closing dot product toward zero
        assert!(log.windows(2).all(|w| w[0].3 < w[1].3), "{log:?}");
        // and no two consecutive ticks are numerically identical — the path really is moving
        assert!(log.windows(2).all(|w| w[0].1 != w[1].1), "{log:?}");
    }
}

#[cfg(test)]
mod thunder_tests {
    use super::{Thunder, ThunderStep};

    #[test]
    fn the_thunder_counts_down_then_plays_once() {
        let mut t = Thunder::default();
        assert_eq!(t.step(), ThunderStep::Idle);
        // mode 2's lead 0x7E: 9 updates of countdown, the play on the tenth; 0x538040 → 20 % 9 = 2
        t.call(0x7E, 0x4000, 20);
        assert_eq!((t.patch, t.delay, t.pan), (993, 9, 0x4000));
        for left in (0..9).rev() {
            assert_eq!(t.step(), ThunderStep::Fetch(993));
            assert_eq!(t.delay, left);
        }
        assert_eq!(t.step(), ThunderStep::Play { patch: 993, pan: 0x4000 });
        assert_eq!(t.step(), ThunderStep::Idle);
        // the other modes wait 95 and 55 updates; a second call replaces the first
        t.call(0x28, 1, 0);
        assert_eq!((t.patch, t.delay), (991, 95));
        t.call(0x50, 2, 8);
        assert_eq!((t.patch, t.delay, t.pan), (999, 55, 2));
        // the patch draw is unsigned
        t.call(0x7E, 0, u32::MAX);
        assert_eq!(t.patch, 0x3DF + (u32::MAX % 9) as i32);
    }
}
