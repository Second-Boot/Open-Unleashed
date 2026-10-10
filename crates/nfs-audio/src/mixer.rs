use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::resample::{self, Cursor, Gains, Interp};
use crate::rt::{self, Consumer, Producer, SampleRing};
use std::sync::atomic::{AtomicI8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod rt_cmd;
use self::rt_cmd::*;

mod output;
pub use self::output::*;

mod stream;
pub use self::stream::*;

mod voice;
pub use self::voice::*;

mod clock;
pub use self::clock::*;

mod clip;
pub use self::clip::*;

mod pan;
pub use self::pan::*;

mod budget;
pub use self::budget::*;

mod reverb;
use self::reverb::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u32);

/// The bend byte of "no bend": `Voice_play`'s `0x40`, the root of the pitch (`Voice::pitch_word`
/// takes `bend − 0x40`).
pub const BEND_ROOT: u8 = 0x40;

/// The top of the level byte, `0x7F` (`Voice_play` clamps its volume argument to 0…0x7F).
pub const LEVEL_MAX: i32 = 0x7F;

/// A level byte as a gain, `byte / 127` (the f32 divide the callers did inline).
#[inline]
pub fn byte_gain(byte: i32) -> f32 {
    byte as f32 / 127.0
}

/// The audio quality whose row is the tables' `_` arm: the library's own defaults before the
/// quality is given ([`mix_rate`] 22050, [`pump_sleep_ms`] 10, [`voice_budget`] 24).
pub const DEFAULT_QUALITY: u8 = 3;

/// The software voice count before the quality is given: the library's default 24 (0x58A5B5).
pub const DEFAULT_BUDGET: usize = 24;

const _: () = {
    assert!(BEND_ROOT == 0x40 && LEVEL_MAX == 0x7F && DEFAULT_QUALITY == 3 && DEFAULT_BUDGET == 24);
};

/// A `Send + Sync` handle to the game thread's side of the mixer, without the output stream
/// itself — see [`AudioOutput::stop_handle`].
#[derive(Clone)]
pub struct StopHandle(Arc<Tx>);

impl StopHandle {
    pub fn stop(&self, id: VoiceId) {
        self.0.stop(id);
    }
}

#[derive(Default)]
struct Shared {
    voices: Vec<(VoiceId, Voice)>,
    streams: Vec<(VoiceId, StreamVoice)>,
    #[cfg(test)]
    next_id: u32,
    /// the environment buses of record 0, `[0x6B4A0C]` (`Mixer_setEnvParam(0x20, ·)`, the tunnel
    /// reverb's level) and `[0x6B4A0D]` (`(0x80, ·)`, half of it) — `engine-audio.md` §4.7
    bus_c: i8,
    bus_d: i8,
    tail: Option<Box<ReverbTail>>,
    /// the software voice count `[0x6B48DD]` ([`voice_budget`]); `None` until the quality is
    /// given — the library's default 24 (0x58A5B5)
    budget: Option<usize>,
    /// the most voices in use at once (a debug log line only)
    peak: usize,
    /// the mix rate `[0x6B48DA]` ([`mix_rate`]); `None` until the quality is given
    mix_rate: Option<u32>,
    /// the mixed frames on their way to the device's rate
    bridge: Bridge,
    /// the driver thread's writes ([`Pump`]) the mix is rendered in
    pump: Pump,
    /// the driver thread's `Sleep` `[0x6B48E1]` ([`pump_sleep_ms`]); `None` until the quality is
    /// given — the library's default 10 (0x58A570)
    sleep_ms: Option<u32>,
    /// the resampler's interpolator, `[0x6B48E5]` ([`Interp::for_quality`]); linear until the
    /// quality is given (the library's 0, 0x58A5D2)
    interp: Interp,
    /// the library's random generator 0x598FF0 ([`crate::librand`]), which every bank-patch start
    /// draws from
    rand: crate::librand::LibRand,
    /// the service ticks the mix is counted in (0x58FBC0)
    clock: ServiceClock,
    /// **`[0x6B49A8]`**, the library service's own tick count (0x589531…0x589538: `inc`ed at the
    /// top of every call to [`Shared::service`], before anything else): 0 at start-up (`mov
    /// [0x6B49A8], ebx` with `ebx` 0, 0x5657D1) and never reset — unlike [`ServiceClock::tick`]
    /// (`[0x6AD7AC]`, 0x58FBC0's own count of mixed frames, which wraps at 30000): the two
    /// increment in lockstep (0x58FBC0 calls the service once, then counts the next tick's frames,
    /// [`Shared::render`]) but are different variables, byte-scan confirmed (`bscan.py 0x6b49a8`
    /// finds no other writer). This is `voice+0x20`'s stamp ([`Voice::stamp`], [`pick_victims`]):
    /// every start between two service calls reads the same value, so two voices started in the
    /// same tick tie on it, as the original's do.
    service_tick: u32,
    /// the callback's reusable buffers (PERF F5: nothing in the device callback allocates)
    scratch: Scratch,
    /// voices and streams taken out of the mix, waiting for [`Core::publish`] to hand them to the
    /// game thread to be freed there
    /// 
    dead: Vec<Gc>,
}

/// What the callback gives back to the game thread to free (the fields are never read: they exist
/// to be dropped over there).
#[allow(dead_code)]
enum Gc {
    Reverb(Box<ReverbTail>),
    Voice(Voice),
    Stream(StreamVoice),
}

impl Shared {
    /// Take out the voices `gone` names (in `Vec::retain`'s order, once each); they are freed on
    /// the game thread, not here.
    fn remove_voices(&mut self, mut gone: impl FnMut(&mut (VoiceId, Voice)) -> bool) {
        for (_, v) in self.voices.extract_if(.., |e| gone(e)) {
            bury(&mut self.dead, Gc::Voice(v));
        }
    }

    /// Take out the streams `gone` names, likewise.
    fn remove_streams(&mut self, mut gone: impl FnMut(&mut (VoiceId, StreamVoice)) -> bool) {
        for (_, v) in self.streams.extract_if(.., |e| gone(e)) {
            bury(&mut self.dead, Gc::Stream(v));
        }
    }
}

/// Keep `g` for the game thread; a full list (it holds as many as the mixer has voices) frees it
/// here instead, which only costs the callback what it cost before.
fn bury(dead: &mut Vec<Gc>, g: Gc) {
    if dead.len() < dead.capacity() {
        dead.push(g);
    }
}

/// Buffers the mixer reuses from one call to the next instead of allocating each time; made
/// once ([`Shared::reserve_rt`]), emptied and refilled.
#[derive(Default)]
struct Scratch {
    /// the frames [`Shared::render_into`] mixes a driver pass in
    mix: Vec<[f32; 2]>,
    stopped: Vec<VoiceId>,
    stop: Vec<VoiceId>,
    failed: Vec<usize>,
    live: Vec<(u8, u16, u32)>,
    live_ids: Vec<VoiceId>,
    live_held: Vec<bool>,
    live_at: Vec<usize>,
    order: Vec<usize>,
    victims: Vec<usize>,
}

impl Shared {
    /// The library's mix rate `[0x6B48DA]`: the table's once 0x4A66B0 has configured the sound
    /// ([`mix_rate`]), the library's own 22050 (`mov word [0x6B48DA], 0x5622`, 0x58A5C2) before.
    fn mix_rate(&self) -> u32 {
        self.mix_rate.unwrap_or(mix_rate(DEFAULT_QUALITY))
    }

    /// One pass of the driver thread ([`Pump`]): its one or two parts, each rendered as 0x58FBC0
    /// renders it.
    #[cfg(test)]
    fn pump(&mut self, mix_rate: u32) -> Vec<[f32; 2]> {
        let mut out = std::collections::VecDeque::new();
        self.pump_to(mix_rate, &mut out);
        out.into()
    }

    /// [`Shared::pump`] appending to `dst` (the [`Bridge`]'s queue) — no allocation once the
    /// buffers have their size.
    fn pump_to(&mut self, mix_rate: u32, dst: &mut std::collections::VecDeque<[f32; 2]>) {
        let sleep = self.sleep_ms.unwrap_or(pump_sleep_ms(DEFAULT_QUALITY));
        let mut buf = std::mem::take(&mut self.scratch.mix);
        for part in self.pump.next_write(mix_rate, sleep) {
            if part > 0 {
                self.render_into(&mut buf, part as usize, mix_rate);
                dst.extend(buf.iter().copied());
            }
        }
        self.scratch.mix = buf;
    }

    /// The software mixer's output for `n` stereo frames at `mix_rate`, as **0x58FBC0** drives it:
    /// whenever the frames of the last service tick are used up, the library service 0x589520
    /// runs ([`Shared::service`]) and the next tick's frames are counted ([`ServiceClock`]); the
    /// frames are mixed in blocks of at most [`BLOCK`] (0x598940 → 0x598760), never across a
    /// tick. Every voice is resampled to the mix rate by the library's kernel ([`resample`]: the
    /// 16.16 step of 0x596B60 / 0x596A50, the cubic or linear interpolator of `[0x6B48E5]`), mixed
    /// at its channel gains with their ramp (0x598760 / 0x5980C0) into floats of 16-bit scale,
    /// then the reverb, the streams and the 16-bit store (0x59A350). [`Bridge`] takes these frames
    /// on to the device's rate.
    ///
    /// 0x58FBC0 is handed one part of the driver thread's write, and a block ends where that part
    /// ends too: the port renders the same parts ([`Pump`], [`Shared::pump`]). *(Until 2026-09-27
    /// a block ended where the device callback's request did.)*
    #[cfg(test)]
    fn render(&mut self, n: usize, mix_rate: u32) -> Vec<[f32; 2]> {
        let mut out = Vec::new();
        self.render_into(&mut out, n, mix_rate);
        out
    }

    /// [`Shared::render`] into `out`, which is cleared and sized to `n` frames.
    fn render_into(&mut self, out: &mut Vec<[f32; 2]>, n: usize, mix_rate: u32) {
        out.clear();
        out.resize(n, [0.0f32; 2]);
        let mut at = 0;
        while at < n {
            if self.clock.left == 0 {
                // 0x58FBF6…0x58FC3A: the service first, then the tick's frames (0 for a tick
                // that rounds to nothing, which only runs the service again)
                self.service();
                self.clock.left = self.clock.next_tick(mix_rate);
                continue;
            }
            let len = (n - at).min(self.clock.left as usize).min(BLOCK);
            self.mix_block(&mut out[at..at + len], mix_rate);
            self.clock.left -= len as u32;
            at += len;
        }
        // the 16-bit store of the mix (0x59A350), back to the port's float scale
        for frame in out.iter_mut() {
            for s in frame.iter_mut() {
                *s = resample::to_i16(*s) as f32 / 32768.0;
            }
        }
    }

    /// **0x589520**, the library service, once a tick of mixed frames ([`ServiceClock`]), over
    /// every voice playing (state 1, a handle of its own, 0x589581…0x58958D), in this order:
    ///
    /// * a **pitch table** (`voice+0x60`): its step `+0x4B` moves on one, back to 0 at the
    ///   length `+0x49` (code 0x21, unsigned `jb`), and the pitch is made again (0x589593…0x5895B7);
    /// * a **volume table** (`+0x5C`): its step `+0x4A` likewise at `+0x48` (code 0x1E)
    ///   (0x5895BF…0x5895DE);
    /// * the ramp `voice+0x24` (0x5895E1…0x589619, [`Ramp`]) — below 0 it stops the handle; a
    ///   stream's volume ramp is [`Fade`];
    /// * the **envelope** ([`Envelope::tick`], 0x58961E…0x589687), which stops the voice's handle
    ///   — its group — once its segments have run (0x58964D → 0x565D60);
    /// * the level made again (0x589DF0) when any of these changed it.
    ///
    /// The port makes the level and the pitch word from the voice's state at every block
    /// ([`Voice::level_byte`], [`Voice::pitch_word`]), which is the same between two ticks —
    /// except that a caller's volume or pitch counts from the next block, where the library's
    /// `setVolume` ramp waits for the next tick.
    fn service(&mut self) {
        // 0x589531…0x589538, the very first thing the service does: `[0x6B49A8] += 1`
        self.service_tick = self.service_tick.wrapping_add(1);
        let mut stopped = std::mem::take(&mut self.scratch.stopped);
        stopped.clear();
        for (id, sv) in self.streams.iter_mut() {
            if let Some(f) = sv.fade.as_mut() {
                if f.tick() {
                    stopped.push(*id);
                }
                sv.volume = f.volume();
                if f.done() {
                    sv.fade = None;
                }
            }
            // **a stream whose song has been played is gone**: the stream's position callback
            // 0x566E40 adds the frames played to the item's `+0x14` and, once it reaches the
            // item's length `+0x18` (0x566E7C…0x566E84), takes the item off (0x566D80 at 0x566E8A),
            // so the stream object's item count `+0x16` (0x567550 adds one per queued song) drops
            // to 0 — the running flag 0x566A40 hands `MusicUpdate` (`+0x8C`), whose state 0 then
            // advances the playlist (0x4ADD6D). Here: the feeder has decoded the last block and
            // the mix has taken every queued frame. *(Until 2026-10-09 a drained stream stayed in
            // the mix for ever, so the music player never saw it end and the race went on in
            // silence after the first song — AUDIO-RACEMUSIC.)*
            if sv.src.finished.load(Ordering::Acquire) && sv.src.queue.len() < sv.src.channels.max(1) as usize && !stopped.contains(id) {
                stopped.push(*id);
            }
        }
        self.remove_streams(|(i, _)| stopped.contains(i));
        self.scratch.stopped = stopped;
        let mut stop = std::mem::take(&mut self.scratch.stop);
        stop.clear();
        for (id, v) in self.voices.iter_mut() {
            // a voice whose group a stop earlier in this pass took is not in state 1 any more
            if v.finished || v.held || stop.contains(id) {
                continue;
            }
            let Some(r) = v.clip.records.first().copied() else {
                // not a bank patch: no tables, no envelope — the ramp only
                let ramping = v.ramp.step != 0;
                if v.ramp.tick(&mut v.vol16) {
                    stop.push(*id);
                } else {
                    v.hook |= ramping;
                }
                continue;
            };
            // 0x589598…0x58962D: the tick recomputes the level byte and runs the volume hook
            // 0x590D10 (0x589699) when `edi` was raised — a volume table (`+0x5C`, 0x5895C8), a
            // release ramp under way (`+0x24 ≠ 0`, 0x5895EB) or an envelope step `+0x30 ≠ 0`
            // (0x589628, tested before the segment is reloaded) — whether or not the byte changed
            let edi = v.clip.tables.first().is_some_and(|t| t.volume.is_some()) || v.ramp.step != 0 || v.env.step != 0;
            let tables = v.clip.tables.first();
            if tables.is_some_and(|t| t.pitch.is_some()) {
                v.pitch_phase = v.pitch_phase.wrapping_add(1);
                if v.pitch_phase >= r.pitch_table_len {
                    v.pitch_phase = 0;
                }
            }
            if tables.is_some_and(|t| t.volume.is_some()) {
                v.volume_phase = v.volume_phase.wrapping_add(1);
                if v.volume_phase >= r.volume_table_len as u8 {
                    v.volume_phase = 0;
                }
            }
            // the release ramp (0x5895E1…0x589619): below 0 stops the handle, and the envelope
            // is not stepped
            if v.ramp.tick(&mut v.vol16) {
                stop.push(*id);
                continue;
            }
            let Some(t) = tables else {
                v.hook |= edi;
                continue;
            };
            if v.env.tick(&t.envelope) {
                stop.push(*id);
            } else {
                v.hook |= edi;
            }
        }
        if !stop.is_empty() {
            self.remove_voices(|(i, _)| stop.contains(i));
        }
        self.scratch.stop = stop;
    }

    /// One block of 0x598760 (≤ [`BLOCK`] frames): each voice pulls its chain once — a one-shot
    /// whose decoder has nothing left fails the pull and is freed before it mixes anything
    /// ([`Voice::pull_fails`], 0x5987DC → 0x598670 → 0x589C00) — then the streams.
    fn mix_block(&mut self, out: &mut [[f32; 2]], mix_rate: u32) {
        let n = out.len();
        let bus_c = self.bus_c;
        let interp = self.interp;
        let mut failed = std::mem::take(&mut self.scratch.failed);
        failed.clear();
        for (k, (_, v)) in self.voices.iter_mut().enumerate() {
            if v.finished || v.held {
                continue;
            }
            // the clip is only read here: a borrow of the voice's own `Arc`, no clone per voice
            // per block
            let c: &Clip = &v.clip;
            let cc = c.channels.max(1) as usize;
            // the 16.16 step, 0x596B60 → 0x596A50 → 0x599830
            let step = resample::step_16_16(v.pitch_word(), c.rate, mix_rate);
            if v.pull_fails(step, n, interp) {
                failed.push(k);
                continue;
            }
            v.pulled = true;
            // the level byte (0x589DF0) · `[0x5C1788]` (f32 1/127) as an f32 (0x59697A)
            let vol = (v.level_byte() as f64 * (1.0f32 / 127.0) as f64) as f32;
            let pg = pan_gains(voice_pan(v.pan, v.patch_pan, c.pan_scale));
            // N10 (0x598721): a hook call since the last block raised the ramp flag, changed or not
            if std::mem::take(&mut v.hook) {
                v.gains.touch();
            }
            v.gains.set_vol(vol);
            v.gains.set([resample::channel_gain(vol, pg[0]), resample::channel_gain(vol, pg[1])]);
            // the send (0x599800, at the voice start and at every bus change: 0x596A10) and the
            // wet gain from the volume as it is now. 0x598700 raises the ramp flag on every call —
            // the hooks' calls are tracked in `v.hook` (service tick, setVolume, pan), so the ramp
            // (and the wet gain from it) also runs for a call that changed nothing
            let send = reverb_input(v.fx_scale, c.fx_send, bus_c);
            if v.send != Some(send) {
                v.send = Some(send);
                v.wet = wet_gain(v.gains.vol[0], send);
            }
            let ramp = v.gains.ramp(n);
            // 0x59C170's `fld dword [edi]` for the block
            if let Some(lp) = v.lowpass.as_mut() {
                lp.begin_block();
            }
            // the looped range and the position are the block's: taken once, not per tap
            let (lo, hi) = v.range();
            let looping = v.looping;
            for (fi, frame) in out.iter_mut().enumerate() {
                let (frac, pos) = (v.cur.frac, v.cur.pos as i64);
                let at = |sc: usize| {
                    resample::interpolate(interp, frac, pos, |k| {
                        Voice::frame_in(lo, hi, looping, k).map_or(0.0, |f| c.samples[f * cc + sc] as f32)
                    })
                };
                let x0 = at(0);
                let mut xs = [x0, if cc > 1 { at(1) } else { x0 }];
                // the strip's low-pass at the chain's head, each channel of the voice its own
                // (a mono voice's one channel feeds both sides)
                if let Some(lp) = v.lowpass.as_mut() {
                    xs[0] = lp.run(0, xs[0]);
                    xs[1] = if cc > 1 { lp.run(1, xs[1]) } else { xs[0] };
                }
                let ramping = ramp.filter(|r| fi < r.len);
                for (ch, o) in frame.iter_mut().enumerate() {
                    let g = match ramping {
                        Some(r) => v.gains.ramp_next(&r, ch),
                        // a channel at gain 0 is skipped (0x59881E)
                        None if v.gains.cur[ch] == 0.0 => continue,
                        None => v.gains.cur[ch],
                    };
                    *o = resample::mix_into(*o, xs[ch], g);
                }
                // the reverb send: the voice's pulled buffer (`[0x6B1758]`, the one every channel
                // mixes from) at the wet gain into the reverb's input (0x598A90); in the ramp the
                // volume steps first, in the last channel's pass (0x5981BF…0x5981CF)
                if let Some(r) = ramping {
                    let vc = v.gains.ramp_vol_next(&r);
                    v.wet = wet_gain(vc, send);
                }
                if v.wet != 0.0 {
                    self.tail.get_or_insert_with(|| Box::new(ReverbTail::new(mix_rate))).send(fi, xs[0], v.wet);
                }
                if ramp.is_some_and(|r| fi + 1 == r.len) {
                    v.gains.ramp_done();
                }
                v.cur.advance(step);
            }
            // 0x59C170's `fstp dword [edi]`
            if let Some(lp) = v.lowpass.as_mut() {
                lp.end_block();
            }
        }
        // 0x598760 frees each failing strip as it comes to it (0x598670 → 0x589C00)
        for &k in &failed {
            self.free(k);
        }
        self.scratch.failed = failed;
        self.remove_voices(|(_, v)| v.finished);
        // the reverb (0x598760 → [0x6B16B8] = 0x598AE0), after the voices and before the streams
        if let Some(rv) = self.tail.as_mut() {
            rv.process(out);
        }
        // streamed sources: a stream is a voice of the library too (0x5612A0 → 0x5898F0), so it
        // takes the same resampler, at the pitch word 4096: 0x5612A0 sets `voice+0x68` (cents)
        // from the descriptor's word `+0`, which 0x561900 clears and the stream header parser
        // 0x590F70 never writes, `voice+0x6C` from the request's `+0xC`, 0x561960's 0x1000 (the
        // music start 0x4AE087…0x4AE0A9 writes only `+8` and `+0xA`, the movie's 0x4E213B nothing
        // there), and the bend `+0x3F` from the request's 0x40 (0x561429…0x561458 → 0x589F40);
        // the library's `setPitch` 0x565F80 is called only by `Voice_play`'s update (0x4A9FA3) and
        // the engine mixer (0x4AC608, 0x4AC6F9), never on a stream. *(Until 2026-09-27 an
        // approximation "the stream player's own pitch call is not read".)* `window` holds
        // `s[P−1] … s[P+2]` of each channel
        for (_, sv) in self.streams.iter_mut() {
            let cc = sv.src.channels.max(1) as usize;
            let step = resample::step_16_16(4096, sv.src.rate, mix_rate);
            let q = &sv.src.queue;
            // the next source frame, zeros when the feeder is behind
            let pop = |q: &SampleRing| -> [f32; 2] { q.pop_frame(cc).unwrap_or([0.0, 0.0]) };
            if !sv.primed {
                sv.primed = true;
                for j in 1..4 {
                    let f = pop(q);
                    for c in 0..2 {
                        sv.window[c][j] = f[c];
                    }
                }
            }
            for frame in out.iter_mut() {
                for (ch, o) in frame.iter_mut().enumerate() {
                    let w = sv.window[ch];
                    let x = resample::interpolate(interp, sv.frac, 0, |k| w[(k + 1) as usize]);
                    *o = resample::mix_into(*o, x, sv.volume);
                }
                let f = sv.frac + step;
                sv.frac = f & 0xFFFF;
                for _ in 0..(f >> 16) {
                    let nf = pop(q);
                    for c in 0..2 {
                        sv.window[c].rotate_left(1);
                        sv.window[c][3] = nf[c];
                    }
                }
            }
        }
    }

    /// [`AudioOutput::silence_voices`]: only a voice tagged `table` — the pause's own walk never
    /// reaches the others.
    fn silence(&mut self) {
        for (_, v) in self.voices.iter_mut() {
            if !v.table {
                continue;
            }
            if v.ramp.step != 0 && v.ramp.goal < 0 {
                continue;
            }
            v.set_volume(0.0);
        }
    }

    /// [`AudioOutput::ramp`].
    fn ramp(&mut self, id: VoiceId, ticks: i32, target: i32) {
        for (_, v) in self.voices.iter_mut().filter(|(i, v)| *i == id && !v.finished && !v.held) {
            v.ramp = Ramp::start(v.vol16, ticks, target);
        }
    }

    /// [`AudioOutput::play_word`]'s start (0x5907B0). `tabled` is `false` only for the callers
    /// listed on the `table` field of [`Voice`] — a bare `Voice_start` 0x565E70 in the original,
    /// never `Voice_play` 0x4A9C10's voice-numbered table.
    #[cfg(test)]
    fn start_patch(&mut self, clip: Arc<Clip>, volume: f32, word: u16, looping: bool, sel: u8, tabled: bool) -> VoiceId {
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.start_patch_as(id, clip, volume, word, looping, sel, tabled);
        id
    }

    /// [`Shared::start_patch`] with the handle the game thread gave the start ([`AudioOutput`]
    /// numbers them in call order and queues the start, [`Cmd::Start`]).
    #[allow(clippy::too_many_arguments)]
    fn start_patch_as(&mut self, id: VoiceId, clip: Arc<Clip>, volume: f32, word: u16, looping: bool, sel: u8, tabled: bool) -> VoiceId {
        let g = self;
        if clip.records.is_empty() {
            // not a bank patch: one voice, no draw, the clip's own values scaled by `sel`
            if g.allocate(clip.channels.max(1) as usize, clip.priority, Some(id)) {
                let mut v = Voice::new(clip.clone(), word, volume, looping, g.service_tick);
                v.level = ((clip.gain * 127.0).round() as i32 * sel as i8 as i32 / 127) as i8;
                v.table = tabled;
                g.voices.push((id, v));
            }
            return id;
        }
        let patch_cents = crate::librand::patch_draw(&mut g.rand, &clip.records);
        let mut started = 0;
        for (i, r) in clip.records.iter().enumerate() {
            if !r.matches(sel as i8, crate::librand::REQUEST_NOTE) {
                continue;
            }
            let pan = crate::librand::record_pan(&mut g.rand, r);
            let layer = clip.layer(i);
            if !g.allocate(layer.channels.max(1) as usize, layer.priority, Some(id)) {
                // 0x5908F1 → 0x5909A6: every handle started so far is stopped again
                g.remove_voices(|(j, _)| *j == id);
                tracing::trace!(record = i, sel, "patch start refused");
                return id;
            }
            let d = crate::librand::record_draws(&mut g.rand, r, sel as i8, patch_cents, pan);
            let mut v = Voice::new(layer, word, volume, looping, g.service_tick);
            v.patch_pan = d.pan;
            v.level = d.level as i8;
            v.cents = base_cents(r).wrapping_add(d.cents as i16);
            (v.volume_phase, v.pitch_phase) = (d.volume_phase, d.pitch_phase);
            v.table = tabled;
            tracing::trace!(record = i, ?d, sel, "patch start");
            g.voices.push((id, v));
            started += 1;
        }
        if started > 1 {
            // 0x590A5C: the last one still standing is the anchor
            if let Some((_, v)) = g.voices.iter_mut().rev().find(|(j, _)| *j == id) {
                v.anchor = true;
            }
        }
        id
    }

    fn budget(&self) -> usize {
        self.budget.unwrap_or(DEFAULT_BUDGET)
    }

    /// **0x589C00**, a voice freed when its sample has ended (the failed pull's callback,
    /// 0x58EC19): a voice outside a group (`voice+0x1E` 0, every one-record start) is freed.
    /// In a group — the voices of one patch start of several records (0x590A04…0x590A5C) — the
    /// live members are counted (0x589C39…0x589C79) and the anchor found (`voice+0x1B`):
    /// the anchor ending while others play is **held** (state 2, 0x589CF8) instead of freed; the
    /// second-last member ending beside a held anchor frees both (0x589CB6…0x589CE6); any other
    /// member is freed. So the group's handle — the anchor's — stays good, and its voice stays
    /// in the budget, until every record's sample has ended. Marks the freed voices `finished`.
    fn free(&mut self, k: usize) {
        let id = self.voices[k].0;
        // the group's live members: their count and the first anchor among them
        let live = |j: &usize| self.voices[*j].0 == id && !self.voices[*j].1.finished;
        let members = (0..self.voices.len()).filter(live).count();
        let anchor = (0..self.voices.len()).filter(live).find(|&j| self.voices[j].1.anchor);
        match anchor {
            Some(a) if members > 1 => {
                if self.voices[a].1.held && a != k && members == 2 {
                    self.voices[k].1.finished = true;
                    self.voices[a].1.finished = true;
                } else if !self.voices[a].1.held && a == k {
                    self.voices[a].1.held = true;
                } else {
                    self.voices[k].1.finished = true;
                }
            }
            _ => self.voices[k].1.finished = true,
        }
    }

    /// 0x5898F0 for a start needing `need` voices: `true` with the victims stopped, `false` for the
    /// refusal (−9) with nothing touched. A victim playing is stopped by its handle (0x589B26,
    /// 0x565D60), which stops its whole group; a held anchor (state 2) is taken without a stop
    /// (0x589B09 `cmp cl, 1 ; jne`), and so is a voice of the start in progress, whose group is not
    /// made yet (`voice+0x1E` is set after every record has started, 0x590A04).
    fn allocate(&mut self, need: usize, priority: u8, starting: Option<VoiceId>) -> bool {
        // the lists are the callback's reusable buffers ([`Scratch`])
        let sc = &mut self.scratch;
        let (mut live_ids, mut live_held, mut live, mut live_at) =
            (std::mem::take(&mut sc.live_ids), std::mem::take(&mut sc.live_held), std::mem::take(&mut sc.live), std::mem::take(&mut sc.live_at));
        let (mut order, mut victims) = (std::mem::take(&mut sc.order), std::mem::take(&mut sc.victims));
        live_ids.clear();
        live_held.clear();
        live.clear();
        live_at.clear();
        for (j, (i, v)) in self.voices.iter().enumerate().filter(|(_, (_, v))| !v.finished) {
            live_ids.push(*i);
            live_held.push(v.held);
            live.push((v.clip.priority, v.clip.channels, v.stamp));
            live_at.push(j);
        }
        live.extend(self.streams.iter().map(|(id, sv)| (STREAM_PRIORITY, sv.src.channels, id.0)));
        let served = pick_victims_into(&live, self.budget(), need, &mut order, &mut victims);
        let ok = served && {
            if !victims.is_empty() {
                // streams sit after the voices in `live` and are never victims; `live` lists the
                // voices in order, so a victim's index is its place among the unfinished voices
                self.steal(&victims, &live_ids, &live_held, &live_at, need, priority, starting);
            }
            let ch = |k: usize| live[k].1.max(1) as usize;
            let in_use = (0..live.len()).map(ch).sum::<usize>() - victims.iter().map(|&k| ch(k)).sum::<usize>() + need;
            if in_use > self.peak {
                self.peak = in_use;
                tracing::debug!(voices = in_use, budget = self.budget(), "voices in use: a new peak");
            }
            true
        };
        if !served {
            tracing::debug!(need, priority, budget = self.budget(), "voice start refused: no voice free or below 0x65");
        }
        let sc = &mut self.scratch;
        (sc.live_ids, sc.live_held, sc.live, sc.live_at, sc.order, sc.victims) = (live_ids, live_held, live, live_at, order, victims);
        ok
    }

    /// The victims of [`Shared::allocate`] stopped (0x589B26): see there.
    #[allow(clippy::too_many_arguments)]
    fn steal(&mut self, victims: &[usize], live_ids: &[VoiceId], live_held: &[bool], live_at: &[usize], need: usize, priority: u8, starting: Option<VoiceId>) {
        tracing::debug!(need, priority, stolen = victims.len(), "voice budget full: the lowest priority, oldest voice stolen");
        for &k in victims {
            let one = live_held[k] || Some(live_ids[k]) == starting;
            for j in 0..self.voices.len() {
                if (one && j == live_at[k]) || (!one && self.voices[j].0 == live_ids[k]) {
                    self.voices[j].1.finished = true;
                }
            }
        }
        self.remove_voices(|(_, v)| v.finished);
    }
}

impl Shared {
    /// The state the status calls read, into `s` (whose buffers are reused).
    fn snapshot_into(&self, s: &mut Snapshot) {
        s.voices.clear();
        s.voices.extend(self.voices.iter().map(|(id, v)| VoiceStat {
            id: *id,
            anchor: v.anchor,
            held: v.held,
            finished: v.finished,
            position: v.position(),
            frames: v.clip.samples.len() / v.clip.channels.max(1) as usize,
            rate: v.clip.rate,
        }));
        s.streams.clear();
        s.streams.extend(self.streams.iter().map(|(id, sv)| (*id, sv.volume)));
    }

    /// Make the buffers the callback uses, so that it allocates nothing.
    fn reserve_rt(&mut self) {
        self.voices.reserve(RT_VOICES);
        self.streams.reserve(32);
        self.dead.reserve_exact(RT_VOICES + 32);
        self.bridge.pending.reserve(RT_FRAMES);
        let sc = &mut self.scratch;
        sc.mix.reserve(RT_FRAMES);
        sc.stopped.reserve(32);
        sc.stop.reserve(RT_VOICES);
        sc.failed.reserve(RT_VOICES);
        sc.live.reserve(RT_VOICES + 32);
        sc.live_ids.reserve(RT_VOICES);
        sc.live_held.reserve(RT_VOICES);
        sc.live_at.reserve(RT_VOICES);
        sc.order.reserve(RT_VOICES);
        sc.victims.reserve(RT_VOICES);
        // the reverb the first send would make (at the default mix rate, before any quality is
        // given): a fresh one holds nothing and only runs once something is sent to it
        let rate = self.mix_rate();
        self.tail = Some(Box::new(ReverbTail::new(rate)));
    }
}

#[cfg(test)]
mod rate_tests {
    use super::{mix_rate, Bridge, Shared};
    use crate::resample;

    fn ramp(sign: f32) -> impl FnMut(usize) -> Vec<[f32; 2]> {
        let mut next = 0.0f32;
        move |n| {
            (0..n)
                .map(|_| {
                    next += 1.0;
                    [next, sign * next]
                })
                .collect()
        }
    }

    /// The device frames (44.1 kHz callbacks of 512) from a sound started right after the
    /// driver's sleep changed from the library's 10 ms to `AUDIOQUALITY` 1's 20 ms, `intro_s`
    /// seconds of output after the device opened, until the sound is heard.
    fn frames_until_heard_after_sleep_change(intro_s: f64) -> usize {
        let mut s = Shared::default();
        let mut bridge = Bridge::default();
        let step = 22050.0 / 44100.0;
        let callback = |s: &mut Shared, bridge: &mut Bridge| -> bool {
            let mut heard = false;
            for k in 0..512 {
                let lr = bridge.next(step, 512 - k, |_, pending| s.pump_to(22050, pending));
                heard |= lr[0] != 0.0;
            }
            heard
        };
        for _ in 0..(intro_s * 44100.0 / 512.0) as usize {
            callback(&mut s, &mut bridge);
        }
        // the front end's first `fe_music`: 0x4A66B0's configuration for AUDIOQUALITY 1 (20 ms)
        s.sleep_ms = Some(20);
        // one callback with it (the music stream opens silent, state 2), then a sound starts
        callback(&mut s, &mut bridge);
        let clip = std::sync::Arc::new(super::Clip { samples: vec![1000; 22050], loop_range: Some((0, 22050)), ..Default::default() });
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, true, 0)));
        let mut n = 0;
        while !callback(&mut s, &mut bridge) {
            n += 512;
            assert!(n < 44100 * 120, "never heard");
        }
        n
    }

    #[test]
    fn a_driver_sleep_change_queues_no_backlog() {
        // AUDIO-START (2026-10-09): the intro opened the output with the library's 10 ms
        // pass, the front end's first `set_audio_quality` made it 20 ms, and a pass wrote
        // `passes · 20 ms` less what was written — the whole intro once more, mixed at once and
        // played before anything newer
        for intro in [3.0, 10.0, 30.0] {
            let n = frames_until_heard_after_sleep_change(intro);
            eprintln!("intro {intro} s: first sound after {n} device frames ({} ms)", n * 1000 / 44100);
            assert!(n <= 2 * 512, "intro {intro} s: {n} frames of backlog");
        }
    }

    #[test]
    fn the_driver_writes_a_sleep_of_frames_in_sixteens() {
        use super::{Pump, pump_sleep_ms, ring_frames};
        assert_eq!([0, 1, 2, 3].map(pump_sleep_ms), [8, 20, 50, 10]);
        // 0.4 s of the mix rate, in sixteens
        assert_eq!([32000, 22050, 16000].map(ring_frames), [12800, 8816, 6400]);
        // 22050 Hz, 20 ms: 441 frames a pass, written as 432 / 448 (the cursor's sixteens)
        let mut p = Pump::default();
        let w: Vec<u32> = (0..4).map(|_| p.next_write(22050, 20).iter().sum()).collect();
        assert_eq!(w, [432, 448, 432, 448]);
        // 32000 Hz, 8 ms: 256 a pass, the ring of 12800 = 50 passes, so no pass is cut
        let mut p = Pump::default();
        assert!((0..200).all(|_| p.next_write(32000, 8)[1] == 0));
        // a pass across the ring's end is two parts (0x58F9DB…0x58F9F5): the shipped qualities'
        // passes land on the ring's end (20 · 441 → 8816, 8 · 800 = 6400, 50 · 256 = 12800), the
        // library's 10 ms at 22050 does not — its 160th pass is 208 + 16
        let mut p = Pump::default();
        let parts: Vec<[u32; 2]> = (0..160).map(|_| p.next_write(22050, 10)).collect();
        assert_eq!(parts[159], [208, 16]);
        assert!(parts[..159].iter().all(|x| x[1] == 0));
        assert_eq!(parts.iter().map(|x| x[0] + x[1]).sum::<u32>(), (160 * 2205 / 10) & !15);
    }

    #[test]
    fn the_mix_is_rendered_in_the_driver_passes() {
        // each pass is one or two 0x58FBC0 calls: at 22050 Hz / 20 ms the first pass of 432
        // frames spans the first tick (208) and 224 of the second (224), so the service runs twice
        let mut s = Shared::default();
        s.sleep_ms = Some(20);
        assert_eq!(s.pump(22050).len(), 432);
        assert_eq!(s.clock.tick, 2);
        assert_eq!(s.clock.left, 0);
        assert_eq!(s.pump(22050).len(), 448);
    }

    #[test]
    fn the_service_tick_is_a_hundredth_of_a_second_in_sixteens() {
        use super::ServiceClock;
        // 0x58FBC0: rate · tick / 100 less what was counted, rounded down to 16
        let mut c = ServiceClock::default();
        let ticks: Vec<u32> = (0..6).map(|_| c.next_tick(22050)).collect();
        assert_eq!(ticks, [208, 224, 224, 224, 208, 224]);
        let mut c = ServiceClock::default();
        assert!((0..100).all(|_| c.next_tick(32000) == 320));
        // on average the rate: 100 ticks of 22050 are 22048 frames (the last 16 not yet due)
        let mut c = ServiceClock::default();
        assert_eq!((0..100).map(|_| c.next_tick(22050)).sum::<u32>(), 22048);
        // both counters start over after tick 30000
        let mut c = ServiceClock { tick: 30000, counted: 30000 * 160, left: 0 };
        assert_eq!(c.next_tick(16000), 160);
        assert_eq!((c.tick, c.counted), (0, 0));
    }

    #[test]
    fn a_one_shot_is_freed_at_the_first_failing_pull() {
        use crate::resample::Interp;
        // 1000 frames at the mix rate (step 0x10000), linear: the decoder runs 2 ahead
        let clip = std::sync::Arc::new(super::Clip { samples: vec![100; 1000], ..Default::default() });
        let mut s = Shared::default();
        s.interp = Interp::Linear;
        s.voices.push((super::VoiceId(0), super::Voice::new(clip.clone(), resample::PITCH_UNIT, 1.0, false, 0)));
        // blocks: the ticks 208, 224, 224, 224, 208; before the fifth the position is 880:
        // 880 + 2 < 1000, so the fifth block (to 1088) pulls and mixes the tail and the zeros
        s.render(208 + 224 * 3 + 208, 22050);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].1.cur.pos, 1088);
        // the sixth block's pull fails: 1088 + 2 ≥ 1000 — the voice is gone before it mixes
        s.render(1, 22050);
        assert!(s.voices.is_empty());
        // a looped voice never fails
        let mut s = Shared::default();
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, true, 0)));
        s.render(5000, 22050);
        assert_eq!(s.voices.len(), 1);
    }

    #[test]
    fn a_stream_level_is_set_at_once_and_ends_a_running_fade() {
        // 0x567810 → 0x5662B0: `+0x2C = level << 16`, the ramp step 0 — the level at once
        let src = super::StreamSource::new(22050, 1);
        let mut sv = super::StreamVoice { src, volume: 1.0, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: Some(super::Fade::new(1.0, 500, 0)) };
        sv.set_level(40);
        assert_eq!((sv.volume, sv.fade), (40.0 / 127.0, None));
        // the same word (0x5662FE `je`): nothing written, a fade running from there goes on
        sv.fade = Some(super::Fade::new(sv.volume, 500, 0));
        sv.set_level(40);
        assert!(sv.fade.is_some());
        // once the fade has stepped the word is no longer `40 << 16`: the call ends it
        sv.fade.as_mut().unwrap().tick();
        sv.set_level(40);
        assert_eq!((sv.volume, sv.fade), (40.0 / 127.0, None));
    }

    #[test]
    fn a_stream_fade_steps_once_a_tick() {
        let src = super::StreamSource::new(22050, 1);
        let mut s = Shared::default();
        s.streams.push((super::VoiceId(0), super::StreamVoice { src, volume: 1.0, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: Some(super::Fade::new(1.0, 30, 0)) }));
        // the first render runs the service before its first frame: one of the three steps,
        // 127 << 16 less 2774357 — level 84
        s.render(1, 22050);
        assert_eq!(s.streams[0].1.volume, 84.0 / 127.0);
        // the rest of the first tick (208 frames) has no service; the next tick's has
        s.render(207, 22050);
        assert_eq!(s.streams[0].1.volume, 84.0 / 127.0);
        s.render(1, 22050);
        assert_eq!(s.streams[0].1.volume, 42.0 / 127.0);
    }

    #[test]
    fn the_mix_rate_is_the_quality_table() {
        // 0x5B4AE4: 32000 / 22050 / 16000; quality 3 keeps the library's 22050 (0x58A5C2)
        assert_eq!([0, 1, 2, 3].map(mix_rate), [32000, 22050, 16000, 22050]);
        assert_eq!(Shared::default().mix_rate(), 22050);
    }

    #[test]
    fn the_bridge_interpolates_the_mix_to_the_device_rate() {
        // a ramp mixed at 16000 heard at 48000: three device frames per mix frame
        let mut r = ramp(-1.0);
        let mut b = Bridge::default();
        let got: Vec<f32> = (0..7).map(|k| b.next(16000.0 / 48000.0, 7 - k, |n, p| p.extend(r(n)))[0]).collect();
        let want = [1.0, 4.0 / 3.0, 5.0 / 3.0, 2.0, 7.0 / 3.0, 8.0 / 3.0, 3.0];
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{got:?}");
        }
        // at the same rate the mix passes through unchanged
        let mut r = ramp(1.0);
        let mut b = Bridge::default();
        assert_eq!((0..4).map(|k| b.next(1.0, 4 - k, |n, p| p.extend(r(n)))[1]).collect::<Vec<_>>(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_voice_steps_through_its_sample_at_the_mix_rate() {
        // a 22050 Hz sample mixed at 16000 (quality 2): 22050/16000 sample frames a mix frame
        let clip = std::sync::Arc::new(super::Clip { samples: vec![0; 100_000], ..Default::default() });
        let mut s = Shared::default();
        s.voices.push((super::VoiceId(0), super::Voice::new(clip, resample::PITCH_UNIT, 1.0, false, 0)));
        s.render(16000, 16000);
        // the step 22050·16/16000 = 22.05 (an f32) · 4096 → 90316 (truncated): 16000 of them
        let want = 90316u64 * 16000;
        assert_eq!((s.voices[0].1.cur.pos, s.voices[0].1.cur.frac as u64), (want >> 16, want & 0xFFFF));
    }

    #[test]
    fn a_known_sample_through_the_kernel_the_gains_and_the_store() {
        use crate::resample::{channel_gain, cubic, linear, mix_into, to_i16, Interp};
        // an 11025 Hz sample at the 22050 mix: step 0x8000, half a sample a frame
        let samples: Vec<i16> = vec![1000, -2000, 3000, 12000, -500];
        let clip = std::sync::Arc::new(super::Clip { rate: 11025, samples: samples.clone(), ..Default::default() });
        let s_at = |k: i64| if (0..5).contains(&k) { samples[k as usize] as f32 } else { 0.0 };
        let g = channel_gain(1.0, super::pan_gains(0x40)[0]);
        for interp in [Interp::Cubic, Interp::Linear] {
            let mut s = Shared::default();
            s.interp = interp;
            s.voices.push((super::VoiceId(0), super::Voice::new(clip.clone(), resample::PITCH_UNIT, 1.0, false, 0)));
            let out = s.render(12, 22050);
            for (k, frame) in out.iter().enumerate() {
                let (p, frac) = ((k / 2) as i64, if k % 2 == 1 { 0x8000 } else { 0 });
                // past the end the decoder's zeros go on through the kernel (0x597BF2)
                let x = if interp == Interp::Cubic {
                    cubic(s_at(p - 1), s_at(p), s_at(p + 1), s_at(p + 2), frac)
                } else {
                    linear(s_at(p), s_at(p + 1), frac)
                };
                let want = to_i16(mix_into(0.0, x, g)) as f32 / 32768.0;
                assert_eq!(frame[0], want, "{interp:?} frame {k}");
                assert_eq!(frame[1], want);
            }
            // the zeros past the end were mixed, and the voice is still held: its decoder fails
            // only at the next block's pull (cubic: 6 + 3 ≥ 5 samples; linear: 6 + 2)
            assert_eq!(s.voices.len(), 1);
            // a one-frame block asks the decoder for nothing (half a sample, 0x59BF88): no failure
            s.render(1, 22050);
            assert_eq!(s.voices.len(), 1);
            let out = s.render(2, 22050);
            assert_eq!(out, [[0.0, 0.0]; 2]);
            assert!(s.voices.is_empty());
        }
        // the cubic's first half-step reads the zeroed s[−1]: (−2000 − 0)/2 · t …, not s0 again
        assert_ne!(cubic(0.0, 1000.0, -2000.0, 3000.0, 0x8000), linear(1000.0, -2000.0, 0x8000));
    }
}

#[cfg(test)]
mod group_tests {
    use super::{Clip, Shared, Voice, VoiceId};
    use crate::resample;
    use nfs_formats::bnk::PatchRecord;
    use std::sync::Arc;

    fn layer(frames: usize, priority: u8) -> Arc<Clip> {
        Arc::new(Clip { samples: vec![100; frames], priority, ..Default::default() })
    }

    /// A patch of three records like `SND_coll`'s 0xD34: 72…97, 98…115 and 0…78 — a level of
    /// 75 starts the first and the third.
    fn patch(frames: [usize; 3]) -> Arc<Clip> {
        let rec = |lo: i32, hi: i32| PatchRecord::from_codes(&[(0x01, lo), (0x02, hi)]);
        let layers: Vec<Arc<Clip>> = frames.iter().map(|&n| layer(n, 26)).collect();
        Arc::new(Clip { records: vec![rec(72, 97), rec(98, 115), rec(0, 78)], layers, ..(*layer(10, 26)).clone() })
    }

    fn start(s: &mut Shared, clip: &Arc<Clip>, sel: u8) -> VoiceId {
        s.start_patch(clip.clone(), 1.0, resample::PITCH_UNIT, false, sel, true)
    }

    #[test]
    fn every_matching_record_starts_its_own_voice() {
        let clip = patch([300, 5000, 2000]);
        let mut s = Shared::default();
        let id = start(&mut s, &clip, 75);
        // records 0 and 2, each on its own sample, the last the anchor
        let lens: Vec<usize> = s.voices.iter().map(|(_, v)| v.clip.samples.len()).collect();
        assert_eq!(lens, [300, 2000]);
        assert!(s.voices.iter().all(|(i, _)| *i == id));
        assert_eq!(s.voices.iter().map(|(_, v)| v.anchor).collect::<Vec<_>>(), [false, true]);
        // a level of 100 starts record 1 alone: no group
        let mut s = Shared::default();
        start(&mut s, &clip, 100);
        assert_eq!(s.voices.len(), 1);
        assert!(!s.voices[0].1.anchor);
        assert_eq!(s.voices[0].1.clip.samples.len(), 5000);
    }

    #[test]
    fn the_anchor_is_held_until_the_group_ends() {
        // the anchor (record 2, 300 frames) ends first: held, in the budget, silent
        let clip = patch([2000, 10, 300]);
        let mut s = Shared::default();
        s.interp = crate::resample::Interp::Linear;
        start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert_eq!(s.voices.len(), 2);
        assert!(s.voices[1].1.held && !s.voices[0].1.held);
        // the other one's end frees both (0x589CB6)
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
        // the other way round: the first record ends first and is freed alone
        let clip = patch([300, 10, 2000]);
        let mut s = Shared::default();
        start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert_eq!(s.voices.len(), 1);
        assert!(s.voices[0].1.anchor && !s.voices[0].1.held);
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
    }

    #[test]
    fn a_held_anchor_answers_zero_never_a_siblings_position() {
        // 0x566910: the call always reads the handle's own entry (0x590e20's self-match), never a
        // sibling's; a held anchor's own entry stays in the group's array (it is not freed) but
        // its remaining-frames field is negative — state 0, every field left the zero the call
        // started with — and `None` only once the group is fully gone (state 3, 0x566a1e)
        let clip = patch([2000, 10, 300]);
        let mut s = Shared::default();
        s.interp = crate::resample::Interp::Linear;
        let id = start(&mut s, &clip, 75);
        s.render(1000, 22050);
        assert!(s.voices[1].1.held && s.voices[1].1.anchor && !s.voices[0].1.held);
        let status = |s: &Shared| {
            let mut snap = super::Snapshot::default();
            s.snapshot_into(&mut snap);
            (snap.status_voice(id).copied(), snap)
        };
        let v = status(&s).0.expect("the group is still alive");
        assert!(v.held, "answers the held anchor, not the still-playing sibling at voices[0]");
        s.render(2000, 22050);
        assert!(s.voices.is_empty());
        assert!(status(&s).0.is_none(), "state 3: gone");
    }

    #[test]
    fn a_refused_record_stops_the_ones_started() {
        // one voice free: record 0 takes it, record 2 finds only the patch's own voice at 26 —
        // stealable, so it takes it: the first is stopped by the steal, the second plays
        let clip = patch([300, 10, 2000]);
        let mut s = Shared::default();
        s.budget = Some(1);
        start(&mut s, &clip, 75);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].1.clip.samples.len(), 2000);
        // two voices, one held by a voice at 0x65: record 0 (at 0x65 too) takes the free one,
        // record 2 finds nothing it may steal — refused, and record 0 is stopped again
        let mut s = Shared::default();
        s.budget = Some(2);
        s.voices.push((VoiceId(900), Voice::new(layer(10, 0x65), resample::PITCH_UNIT, 1.0, true, 0)));
        let unstealable = Arc::new(Clip { layers: clip.layers.iter().map(|l| Arc::new(Clip { priority: 0x65, ..(**l).clone() })).collect(), ..(*clip).clone() });
        let id = start(&mut s, &unstealable, 75);
        // record 0 took the free voice, record 2 was refused: record 0 is stopped again
        assert!(s.voices.iter().all(|(i, _)| *i != id));
        assert_eq!(s.voices.len(), 1);
    }

    #[test]
    fn two_starts_before_the_next_service_tick_share_the_voice_steal_stamp() {
        // 0x589531…0x589538 increments [0x6B49A8] once per call to the service, not once per
        // start (0x589B3F…0x589B54 just reads it) — so every patch started between two service
        // calls carries the same `voice+0x20` stamp
        let clip = layer(10, 50);
        let mut s = Shared::default();
        start(&mut s, &clip, 0x7F);
        start(&mut s, &clip, 0x7F);
        assert_eq!(s.voices.iter().map(|(_, v)| v.stamp).collect::<Vec<_>>(), [0, 0]);
        s.service();
        start(&mut s, &clip, 0x7F);
        assert_eq!(s.voices[2].1.stamp, 1);
    }
}

#[cfg(test)]
mod service_tests {
    use super::{cents_word, div_127, div_127_cubed, Clip, Envelope, Shared, Voice, VoiceId};
    use crate::resample;
    use nfs_formats::bnk::{PatchRecord, RecordTables};
    use std::sync::Arc;

    #[test]
    fn cents_to_the_pitch_word() {
        // 0x589E80 against the image's table (worked in scratch `audio6/chk_tbl.py`)
        let want = [(0, 4096), (1, 4096), (100, 4336), (-100, 3864), (600, 5792), (-600, 2904), (1199, 8176), (1200, 8192), (-1200, 2048), (2400, 16384), (-1600, 1624), (-1, 4088), (5, 4112), (-5, 4080)];
        for (c, w) in want {
            assert_eq!(cents_word(c), w, "{c} cents");
        }
    }

    #[test]
    fn the_root_and_the_fine_tune_start_the_pitch() {
        use super::base_cents;
        let rec = |codes: &[(u8, i32)]| PatchRecord::from_codes(codes);
        // no code 0x07 / 0x10: middle C, 0 cents
        assert_eq!(base_cents(&rec(&[])), 0);
        // sd6a's first slot: root 66 → six semitones down
        assert_eq!(base_cents(&rec(&[(0x07, 66)])), -600);
        // SND_coll's wind: root 76; a fine tune of −12 (the byte 244) on top
        assert_eq!(base_cents(&rec(&[(0x07, 76), (0x10, -12)])), -1612);
        // the start adds the draws to it (a record without spreads draws nothing)
        let mut s = Shared::default();
        let c = Arc::new(Clip { records: vec![rec(&[(0x07, 48), (0x10, 25)])], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, resample::PITCH_UNIT, false, 0x7F, true);
        assert_eq!(s.voices[0].1.cents, 1225);
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(1225));
    }

    #[test]
    fn an_engine_layer_pitch_end_to_end() {
        use super::base_cents;
        use crate::resample;
        // sd7a's layer 0x51 (44100 Hz, root 61, fine −43) at half the redline, spl[8] = 80, no
        // doppler — worked from the image in scratch `audio7/worked.py`: the record's word 3583
        // (0x4AC7F6…0x4AC81F), −143 cents → 3776 (0x589E80), +0x6E = 3583 · 3776 >> 12 = 3303
        // (0x589FCA), the step 3303 · f32(44100 · 16 / 32000) truncated = 72831 (0x596A50)
        let spl = nfs_formats::spl::Spl::default();
        let word = spl.pitch_word(nfs_formats::spl::rpm_frac_16(3000.0, 6000.0), 0x10000);
        assert_eq!(word, 3583);
        let r = PatchRecord::from_codes(&[(0x07, 61), (0x10, -43)]);
        assert_eq!(base_cents(&r), -143);
        assert_eq!(cents_word(-143), 3776);
        let mut s = Shared::default();
        let c = Arc::new(Clip { rate: 44100, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, word, true, 0x7F, true);
        assert_eq!(s.voices[0].1.pitch_word(), 3303);
        assert_eq!(resample::step_16_16(3303, 44100, 32000), 72831);
    }

    #[test]
    fn an_ai_engine_loop_pitch_end_to_end() {
        use super::base_cents;
        use crate::resample;
        // aud1ldex.bnk slot 0 (16000 Hz, root 61, fine +44, bend range 700 at the byte 0x40) at
        // half the redline, no doppler — worked from the image in scratch `audio8/worked.py`:
        // 0x4AAAAC…0x4AAAD1 give 3583, `<< 4` to Voice_play and `>> 4` back into `voice+0x6C`;
        // −56 cents → 3968 (0x589E80), `+0x6E` = 3583 · 3968 >> 12 = 3471, the step
        // 3471 · f32(16000 · 16 / 32000) = 27768 (0x596A50) at 32 kHz
        let word = crate::ai_engine::ai_pitch_word(crate::ai_engine::ai_rpm_frac(3000.0, 6000.0), crate::ai_engine::PITCH_BASE_STILL);
        assert_eq!(word, 3583);
        let r = PatchRecord::from_codes(&[(0x07, 61), (0x10, 44)]);
        assert_eq!(base_cents(&r), -56);
        assert_eq!(cents_word(-56), 3968);
        let mut s = Shared::default();
        let c = Arc::new(Clip { rate: 16000, bend_cents: 700, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, word, true, 0x7F, true);
        assert_eq!(s.voices[0].1.bend, 0x40);
        assert_eq!(s.voices[0].1.pitch_word(), 3471);
        assert_eq!(resample::step_16_16(3471, 16000, 32000), 27768);
    }

    #[test]
    fn the_bend_byte_is_cents_on_the_voice() {
        // SND_coll 0x10 (root 57, bend range 1200 cents) at the AI road loop's byte 127 and the
        // word 0x1000: 300 + ((127 − 64) · 1200 >> 6) = 1481 cents → 9632 (0x589E80), and
        // `+0x6E` = 0x1000 · 9632 >> 12 (0x589FCA; scratch `audio8/worked.py`)
        let r = PatchRecord::from_codes(&[(0x07, 57)]);
        let mut s = Shared::default();
        let c = Arc::new(Clip { bend_cents: 1200, records: vec![r], tables: vec![tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())], ..(*clip(PatchRecord::from_codes(&[]), RecordTables::default())).clone() });
        s.start_patch(c, 1.0, resample::PITCH_UNIT, true, 0x7F, true);
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(300), "0x40 is no bend");
        s.voices[0].1.bend = 127;
        assert_eq!(s.voices[0].1.pitch_word(), 9632);
        // (0 − 0x40) · 1200 >> 6 = −1200, an octave down; `sar` rounds a negative term down
        s.voices[0].1.bend = 0;
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(-900));
        s.voices[0].1.bend = 0x3F;
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(300 - 19));
    }

    #[test]
    fn the_magic_divides_go_toward_zero() {
        for x in [-5_000_000, -2_048_383, -2_048_382, -1, 0, 1, 2_048_382, 2_048_383, 16_387_064, 260_144_641] {
            assert_eq!(div_127_cubed(x), x / 2_048_383, "{x}");
        }
        for x in (-16129..=16129).step_by(7) {
            assert_eq!(div_127(x), x / 127, "{x}");
        }
    }

    #[test]
    fn the_envelope_steps_once_a_tick_and_runs_a_segment_one_tick_over() {
        // SND_coll's common table: 22 ticks to 127, 20 to 126, hold at 121, 24 to 0; from 0
        let table = [(22, 127), (20, 126), (0x7FFF_FFFF, 121), (24, 0)];
        let mut e = Envelope::start(0, 4, &table);
        assert_eq!((e.level, e.step, e.left), (0, 378_321, 22));
        for _ in 0..22 {
            assert!(!e.tick(&table));
        }
        assert_eq!(e.level >> 16, 126);
        // the 23rd tick steps once more — 132 — and then loads the second segment
        assert!(!e.tick(&table));
        assert_eq!((e.level >> 16, e.seg, e.step, e.left), (132, 1, ((126 << 16) - 8_701_383) / 20, 19));
        // the hold (0x7FFFFFFF ticks) keeps it for good: the step rounds to 0 there
        for _ in 0..200 {
            assert!(!e.tick(&table));
        }
        assert_eq!(e.seg, 2);
        // a table that runs out stops the voice: 2 ticks to 50, then 3 to 0, two segments
        let table = [(2, 50), (3, 0)];
        let mut e = Envelope::start(0, 2, &table);
        // (the first segment steps on ticks 1–3, the second is loaded on tick 3 and has run
        // out by tick 6)
        let stops: Vec<bool> = (0..6).map(|_| e.tick(&table)).collect();
        assert_eq!(stops, [false, false, false, false, false, true]);
        // the default (no code 0x19): 127 for good
        let mut e = Envelope::start(0x7F, 1, &nfs_formats::bnk::DEFAULT_ENVELOPE);
        assert_eq!((e.level, e.step), (127 << 16, 0));
        assert!(!e.tick(&nfs_formats::bnk::DEFAULT_ENVELOPE));
    }

    fn clip(r: PatchRecord, t: RecordTables) -> Arc<Clip> {
        Arc::new(Clip { rate: 22050, channels: 1, samples: vec![100; 50_000], loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: vec![r], layers: Vec::new(), tables: vec![t] })
    }

    fn tables(envelope: Vec<(i32, i32)>) -> RecordTables {
        RecordTables { envelope, ..Default::default() }
    }

    #[test]
    fn the_level_byte_takes_the_envelope_the_table_and_the_curve() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, volume_table_len: 3, ..PatchRecord::from_codes(&[]) };
        let t = RecordTables { volume: Some(vec![127, 64, -10]), ..tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()) };
        let mut v = Voice::new(clip(r, t.clone()), resample::PITCH_UNIT, 100.0 / 127.0, false, 0);
        v.level = 99;
        // 127 · 100 · 99 · 127 / 127³ = 77.95 → 77, then · 127 / 127
        assert_eq!(v.level_byte(), 77);
        v.volume_phase = 1;
        assert_eq!(v.level_byte() as i32, 77 * 64 / 127);
        v.volume_phase = 2;
        assert_eq!(v.level_byte() as i32, (77 * -10) / 127);
        // a curve maps the byte: entry `level + 128`
        let mut curve = vec![0u8; 256];
        curve[128 + 77] = 120;
        let t = RecordTables { volume: None, curve: Some(curve), ..t };
        let mut v = Voice::new(clip(r, t), resample::PITCH_UNIT, 100.0 / 127.0, false, 0);
        v.level = 99;
        assert_eq!(v.level_byte(), 120);
    }

    #[test]
    fn the_service_walks_the_tables_once_a_tick() {
        // a pitch table of 3 entries (depth 128: two cents a step from 0x40) and a volume table of 2
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, volume_table_len: 2, pitch_table_len: 3, pitch_depth: 128, ..PatchRecord::from_codes(&[]) };
        let t = RecordTables { volume: Some(vec![127, 0]), pitch: Some(vec![0x40, 0x40 + 50, 0x40 - 50]), ..tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(0), Voice::new(clip(r, t), resample::PITCH_UNIT, 1.0, false, 0)));
        let phases = |s: &Shared| (s.voices[0].1.volume_phase, s.voices[0].1.pitch_phase);
        assert_eq!(phases(&s), (0, 0));
        // the first render runs the service before its first frame
        s.render(1, 22050);
        assert_eq!(phases(&s), (1, 1));
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(100));
        assert_eq!(s.voices[0].1.level_byte(), 0);
        s.render(207 + 1, 22050);
        assert_eq!(phases(&s), (0, 2));
        assert_eq!(s.voices[0].1.pitch_word(), cents_word(-100));
        s.render(224, 22050);
        assert_eq!(phases(&s), (1, 0));
        assert_eq!(s.voices[0].1.pitch_word(), 4096);
    }

    #[test]
    fn a_run_out_envelope_stops_the_group() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(3), Voice::new(clip(r, tables(vec![(1, 127)])), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices.push((VoiceId(3), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        // tick 1: one tick left → 0; tick 2: past the only segment → 0x565D60 on the handle
        s.render(1, 22050);
        assert_eq!(s.voices.len(), 2);
        s.render(208, 22050);
        assert!(s.voices.is_empty());
    }

    #[test]
    fn a_release_is_a_five_tick_ramp_below_zero() {
        use super::{Ramp, RELEASE_TARGET, RELEASE_TICKS};
        // 0x565DD0(h, 5, −1) from 127: step (−0x10000 − 0x7F0000) / 5 toward zero
        let mut v = 127 << 16;
        let mut r = Ramp::start(v, RELEASE_TICKS, RELEASE_TARGET);
        assert_eq!(r, Ramp { step: -1677721, goal: -0x10000 });
        let mut words = Vec::new();
        let mut stopped = false;
        for _ in 0..5 {
            stopped = r.tick(&mut v);
            words.push(v >> 16);
        }
        assert_eq!(words, [101, 75, 50, 24, -1]);
        assert!(stopped);
        assert_eq!(v, -65533);
        // from 40 the fifth tick lands past the goal: clamped to it, the step 0, still below 0
        let mut v = 40 << 16;
        let mut r = Ramp::start(v, 5, -1);
        assert!(!(0..4).any(|_| r.tick(&mut v)));
        assert!(r.tick(&mut v));
        // ticks ≤ 0 are 1 (0x565E03); an upward ramp stops at its goal and does nothing more
        let mut v = 0;
        let mut r = Ramp::start(v, 0, 100);
        assert!(!r.tick(&mut v));
        assert_eq!((v, r.step), (100 << 16, 0));
        assert!(!r.tick(&mut v));
    }

    #[test]
    fn a_released_voice_holds_its_budget_until_the_ramp_stops_it() {
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(4), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices.push((VoiceId(5), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.ramp(VoiceId(4), super::RELEASE_TICKS, super::RELEASE_TARGET);
        // ticks 1…4 fade it; the fifth (0x589611) stops the handle
        s.render(1, 22050);
        assert_eq!(s.voices[0].1.vol16 >> 16, 101);
        s.render(207 + 224 * 2 + 1, 22050);
        assert_eq!(s.voices.len(), 2);
        assert_eq!(s.voices[0].1.vol16 >> 16, 24);
        // tick 4 is 224 frames, one of them mixed
        s.render(223, 22050);
        assert_eq!(s.voices.len(), 2);
        s.render(1, 22050);
        assert_eq!(s.voices.len(), 1);
        assert_eq!(s.voices[0].0, VoiceId(5));
        // a setVolume (0x5662B0) cancels a ramp: the step zeroed with the new volume
        s.ramp(VoiceId(5), 5, -1);
        s.voices[0].1.set_volume(0.5);
        assert_eq!(s.voices[0].1.ramp.step, 0);
        assert_eq!(s.voices[0].1.vol16, 64 << 16);
    }

    #[test]
    fn the_pause_silences_the_table_but_not_a_released_voice() {
        // 0x4AB150 → 0x566340(h, 0) over 0x656180…0x6563C8: a released handle is not there any
        // more (the release forgets it), so its ramp runs on; every other voice goes to 0, its
        // ramp step zeroed (0x566303…0x566306), and keeps running
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        for id in 1..=3 {
            s.voices.push((VoiceId(id), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        }
        s.ramp(VoiceId(1), super::RELEASE_TICKS, super::RELEASE_TARGET);
        // an upward ramp (a stream's fade-in shape) is a table voice's: cancelled
        s.ramp(VoiceId(3), 5, 100);
        s.silence();
        assert_eq!(s.voices[0].1.vol16, 127 << 16);
        assert_eq!(s.voices[0].1.ramp.step, -1677721);
        assert_eq!((s.voices[1].1.vol16, s.voices[1].1.ramp.step), (0, 0));
        assert_eq!((s.voices[2].1.vol16, s.voices[2].1.ramp.step), (0, 0));
        // the released one ramps out and stops; the silenced ones still play
        s.render(22050, 22050);
        assert_eq!(s.voices.iter().map(|(i, _)| i.0).collect::<Vec<_>>(), [2, 3]);
        // the next update's setVolume brings a loop back at its level
        s.voices[0].1.set_volume(0.5);
        assert_eq!(s.voices[0].1.vol16, 64 << 16);
    }

    #[test]
    fn the_pause_leaves_an_untabled_voice_sounding() {
        // 0x4AB150 walks only 0x656180…0x6563C8; a bare Voice_start 0x565E70 voice — the HUD's
        // fe_sound, the front end's fesfx, the collision one-shots (voice 0x33), the checkpoint
        // chime / wrong-way cue / pylon-hit cue and the cop siren's start — is never in it
        // (`RE/notes/sfx.md` "Audit 2026-09-27, audio port 11")
        let r = PatchRecord { env_level: 0x7F, env_segments: 1, ..PatchRecord::from_codes(&[]) };
        let mut s = Shared::default();
        s.voices.push((VoiceId(1), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.voices[0].1.table = false;
        s.voices.push((VoiceId(2), Voice::new(clip(r, tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec())), resample::PITCH_UNIT, 1.0, true, 0)));
        s.silence();
        assert_eq!(s.voices[0].1.vol16, 127 << 16, "not in the table: untouched");
        assert_eq!(s.voices[1].1.vol16, 0, "in the table: silenced");
    }

    #[test]
    fn the_strip_hook_is_a_one_pole_low_pass_at_the_value_in_hz() {
        use super::{voice_play_filter, LowPass};
        // 0x596AC0: 3000 / (rate · 0.5); 0xFFFF is ≥ 1 at every mix rate — no node
        assert_eq!(LowPass::coefficient(0xBB8, 22050), Some((3000.0f64 / 11025.0) as f32));
        assert_eq!(LowPass::coefficient(0xBB8, 32000), Some(0.1875));
        assert_eq!(LowPass::coefficient(0xFFFF, 32000), None);
        assert_eq!(LowPass::coefficient(16000, 32000), None);
        // the node starts at 0 and follows `y = y·(1 − f) + f·x` (0x59C170)
        let mut n = None;
        LowPass::set(&mut n, 0xBB8, 32000);
        let lp = n.as_mut().unwrap();
        assert_eq!((lp.a, lp.b), (0.8125, 0.1875));
        lp.begin_block();
        assert_eq!(lp.run(0, 1000.0), 187.5);
        assert_eq!(lp.run(0, 1000.0), 187.5 * 0.8125 + 187.5);
        lp.end_block();
        assert_eq!(lp.y[1], 0.0);
        // a new value keeps the state; 0xFFFF takes the node away (0x596D10)
        LowPass::set(&mut n, 0xBB8, 22050);
        assert_eq!(n.unwrap().y[0], 187.5 * 0.8125 + 187.5);
        LowPass::set(&mut n, 0xFFFF, 22050);
        assert!(n.is_none());
        // 0x4A9FA8…0x4AA03B: the player's engine banks and AUDIOQUALITY ≥ 2 are left alone
        assert_eq!(voice_play_filter(true, 0, true), None);
        assert_eq!(voice_play_filter(false, 2, true), None);
        assert_eq!(voice_play_filter(false, 1, true), Some(0xBB8));
        assert_eq!(voice_play_filter(false, 0, false), Some(0xFFFF));
    }

    #[test]
    fn the_low_pass_keeps_y_in_the_fpu_register_inside_a_block() {
        use super::LowPass;
        // 0x59C170: `y` is loaded once (`fld dword [edi]`), every sample is computed in the
        // register's 53-bit precision (only the handed-on sample is an f32, `fst dword`), and
        // `fstp dword [edi]` rounds `y` to f32 at the block's end
        let mut n = None;
        LowPass::set(&mut n, 1234, 22050);
        let lp = n.as_mut().unwrap();
        let (a, b) = (lp.a as f64, lp.b as f64);
        let xs = [1000.37f32, -733.91, 12345.6, 0.001, 77.7];
        let mut y = 0.0f64;
        lp.begin_block();
        for &x in &xs {
            y = y * a + b * x as f64;
            assert_eq!(lp.run(0, x), y as f32);
        }
        lp.end_block();
        assert_eq!(lp.y[0], y as f32);
        // the next block starts from the f32, not from the register
        lp.begin_block();
        let y2 = (y as f32 as f64) * a + b * 5.0f64;
        assert_eq!(lp.run(0, 5.0), y2 as f32);
    }

    #[test]
    fn a_hook_call_raises_the_ramp_flag_even_for_an_unchanged_value() {
        use crate::resample::Gains;
        // 0x598721: `mov byte [eax+1], 1` before any compare
        let mut g = Gains::default();
        g.touch();
        assert!(!g.pending, "the voice's first set is taken at once, flag clear (0x598630)");
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        assert!(g.ramp(256).is_none());
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        assert!(g.ramp(256).is_none(), "no event, no ramp");
        g.touch();
        g.set_vol(0.5);
        g.set([0.25, 0.25]);
        let r = g.ramp(256).expect("a hook call ramps");
        assert_eq!((r.len, r.step, r.vol_step), (32, [0.0, 0.0], 0.0));
    }

    #[test]
    fn a_filtered_voice_mixes_its_low_passed_samples() {
        use super::LowPass;
        // a square wave at the Nyquist rate (step 0x10000): the one-pole filter smooths it
        let alt = Arc::new(Clip { samples: (0..4000).map(|i| if i % 2 == 0 { 8000 } else { -8000 }).collect(), rate: 22050, ..(*clip(PatchRecord::from_codes(&[]), tables(nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec()))).clone() });
        let peak = |filter: bool| {
            let mut s = Shared::default();
            s.interp = crate::resample::Interp::Linear;
            let mut v = Voice::new(alt.clone(), resample::PITCH_UNIT, 1.0, true, 0);
            if filter {
                LowPass::set(&mut v.lowpass, 0xBB8, 22050);
            }
            s.voices.push((VoiceId(1), v));
            s.render(2000, 22050)[1000..].iter().map(|f| f[0].abs()).fold(0.0f32, f32::max)
        };
        let (open, closed) = (peak(false), peak(true));
        assert!(open > 0.05, "{open}");
        assert!(closed < open * 0.25, "{closed} vs {open}");
    }
}

#[cfg(test)]
mod rt_tests;
