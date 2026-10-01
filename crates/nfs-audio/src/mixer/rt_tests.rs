//! PERF F5: the game thread's calls reach the mixer through the command queue and the device
//! callback allocates nothing. Two proofs: (1) the callback's whole run — drain, mix, publish —
//! is counted by a global allocator over a long script that starts, steals, frees, ramps, filters,
//! streams and changes the mix rate; (2) the same script through the queue and applied straight
//! to a second mixer, the way the mutex-locked methods did, mixes sample for sample the same.

use super::*;
use nfs_formats::bnk::{PatchRecord, RecordTables};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}

fn bump() {
    let _ = COUNT.try_with(|c| {
        if let Some(n) = c.get() {
            c.set(Some(n + 1));
        }
    });
}

// SAFETY: forwards to the system allocator; the counter is a const thread-local `Cell`, which
// neither allocates nor runs a destructor.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        bump();
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Allocations and frees `f` makes on this thread.
fn counted<R>(f: impl FnOnce() -> R) -> (R, usize) {
    COUNT.with(|c| c.set(Some(0)));
    let r = f();
    (r, COUNT.with(|c| c.take()).unwrap())
}

fn clips() -> [Arc<Clip>; 4] {
    let plain = Clip { rate: 22050, channels: 1, samples: Vec::new(), loop_range: None, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), layers: Vec::new(), tables: Vec::new() };
    let wave = |n: usize, k: i16| (0..n).map(|i| ((i as i16 % 97) - 48) * k).collect::<Vec<i16>>();
    // a loop that sends to the reverb, a short one-shot, a stereo one-shot
    let looped = Arc::new(Clip { samples: wave(5000, 120), loop_range: Some((100, 4000)), fx_send: 100, priority: 10, ..plain.clone() });
    let shot = Arc::new(Clip { samples: wave(1500, 90), fx_send: 60, priority: 30, ..plain.clone() });
    let stereo = Arc::new(Clip { channels: 2, rate: 44100, samples: wave(6000, 70), priority: 20, ..plain.clone() });
    // a three-record patch with spreads (the generator is drawn at every start)
    let rec = |lo: i32, hi: i32| PatchRecord::from_codes(&[(0x01, lo), (0x02, hi), (0x0D, 20), (0x0F, 10), (0x11, 30), (0x07, 60)]);
    let layer = |n: usize| Arc::new(Clip { samples: wave(n, 100), priority: 26, fx_send: 50, ..plain.clone() });
    let tables = RecordTables { envelope: nfs_formats::bnk::DEFAULT_ENVELOPE.to_vec(), ..Default::default() };
    let patch = Arc::new(Clip { records: vec![rec(72, 97), rec(98, 115), rec(0, 78)], layers: vec![layer(300), layer(5000), layer(2000)], tables: vec![tables.clone(), tables.clone(), tables], ..(*layer(10)).clone() });
    [looped, shot, stereo, patch]
}

/// What a script drives: [`AudioOutput`], or the mixer itself with the old locked bodies.
trait Api {
    fn quality(&mut self, q: u8);
    fn play(&mut self, c: &Arc<Clip>, vol: f32, looping: bool, sel: u8) -> VoiceId;
    fn word(&mut self, id: VoiceId, vol: f32, word: u16, bend: u8);
    fn pan(&mut self, id: VoiceId, pan: u16);
    fn aux(&mut self, id: VoiceId, aux: u8);
    fn ramp(&mut self, id: VoiceId, ticks: i32, target: i32);
    fn filter(&mut self, id: VoiceId, v: i32);
    fn stop(&mut self, id: VoiceId);
    fn env(&mut self, a: i32, b: i32);
    fn silence(&mut self);
    fn fade(&mut self, id: VoiceId, ms: i32, target: i32);
    fn stream(&mut self, src: &Arc<StreamSource>, vol: f32) -> VoiceId;
}

impl Api for AudioOutput {
    fn quality(&mut self, q: u8) {
        self.set_audio_quality(q);
    }
    fn play(&mut self, c: &Arc<Clip>, vol: f32, looping: bool, sel: u8) -> VoiceId {
        self.play_word_sel(c.clone(), vol, 4096, 0x50, 0x2000, looping, sel)
    }
    fn word(&mut self, id: VoiceId, vol: f32, word: u16, bend: u8) {
        self.set_word(id, vol, word, bend);
    }
    fn pan(&mut self, id: VoiceId, pan: u16) {
        self.set_pan(id, pan);
    }
    fn aux(&mut self, id: VoiceId, aux: u8) {
        self.set_fx_aux(id, aux);
    }
    fn ramp(&mut self, id: VoiceId, ticks: i32, target: i32) {
        AudioOutput::ramp(self, id, ticks, target);
    }
    fn filter(&mut self, id: VoiceId, v: i32) {
        self.set_filter(id, v);
    }
    fn stop(&mut self, id: VoiceId) {
        AudioOutput::stop(self, id);
    }
    fn env(&mut self, a: i32, b: i32) {
        self.set_env(a, b);
    }
    fn silence(&mut self) {
        self.silence_voices();
    }
    fn fade(&mut self, id: VoiceId, ms: i32, target: i32) {
        AudioOutput::fade(self, id, ms, target);
    }
    fn stream(&mut self, src: &Arc<StreamSource>, vol: f32) -> VoiceId {
        self.play_stream(src.clone(), vol)
    }
}

/// The mixer called directly, each call the body the mutex-locked method had before the queue.
struct Direct {
    core: Core,
    cfg_next_id: u32,
}

impl Api for Direct {
    fn quality(&mut self, q: u8) {
        let (budget, rate, interp, sleep) = (voice_budget(q), mix_rate(q), Interp::for_quality(q), pump_sleep_ms(q));
        let g = &mut self.core.shared;
        g.sleep_ms = Some(sleep);
        g.interp = interp;
        g.budget = Some(budget);
        if g.mix_rate != Some(rate) {
            g.mix_rate = Some(rate);
            g.tail = None;
        }
    }
    fn play(&mut self, c: &Arc<Clip>, vol: f32, looping: bool, sel: u8) -> VoiceId {
        let id = VoiceId(self.cfg_next_id);
        self.cfg_next_id += 1;
        self.core.shared.start_patch_as(id, c.clone(), vol, 4096, looping, sel, true);
        for (_, v) in self.core.shared.voices.iter_mut().filter(|(i, _)| *i == id) {
            v.bend = 0x50u8 as i8;
            v.pan = pan_byte(0x2000);
        }
        id
    }
    fn word(&mut self, id: VoiceId, vol: f32, word: u16, bend: u8) {
        for (_, v) in self.core.shared.voices.iter_mut().filter(|(i, _)| *i == id) {
            v.set_volume(vol);
            v.word = word;
            v.bend = bend as i8;
        }
    }
    fn pan(&mut self, id: VoiceId, pan: u16) {
        for (_, v) in self.core.shared.voices.iter_mut().filter(|(i, _)| *i == id) {
            v.pan = pan_byte(pan);
        }
    }
    fn aux(&mut self, id: VoiceId, aux: u8) {
        for (_, v) in self.core.shared.voices.iter_mut().filter(|(i, _)| *i == id) {
            v.fx_scale = 0x7F - aux.min(0x7F);
        }
    }
    fn ramp(&mut self, id: VoiceId, ticks: i32, target: i32) {
        self.core.shared.ramp(id, ticks, target);
    }
    fn filter(&mut self, id: VoiceId, value: i32) {
        let rate = self.core.shared.mix_rate();
        for (_, v) in self.core.shared.voices.iter_mut().filter(|(i, v)| *i == id && !v.finished && !v.held) {
            LowPass::set(&mut v.lowpass, value, rate);
        }
    }
    fn stop(&mut self, id: VoiceId) {
        self.core.shared.voices.retain(|(i, _)| *i != id);
        self.core.shared.streams.retain(|(i, _)| *i != id);
    }
    fn env(&mut self, a: i32, b: i32) {
        self.core.shared.bus_c = a as i8;
        self.core.shared.bus_d = b as i8;
    }
    fn silence(&mut self) {
        self.core.shared.silence();
    }
    fn fade(&mut self, id: VoiceId, ms: i32, target: i32) {
        if let Some((_, sv)) = self.core.shared.streams.iter_mut().find(|(i, _)| *i == id) {
            sv.fade = Some(Fade::new(sv.volume, ms, target));
        }
    }
    fn stream(&mut self, src: &Arc<StreamSource>, vol: f32) -> VoiceId {
        let id = VoiceId(self.cfg_next_id);
        self.cfg_next_id += 1;
        if self.core.shared.allocate(src.channels.max(1) as usize, STREAM_PRIORITY, None) {
            self.core.shared.streams.push((id, StreamVoice { src: src.clone(), volume: vol, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: None }));
        }
        id
    }
}

/// A deterministic mix of calls, step `i` of the script; `live` the handles given so far.
fn step(a: &mut dyn Api, i: usize, live: &mut Vec<VoiceId>, clips: &[Arc<Clip>; 4], stream: &Arc<StreamSource>) {
    let pick = |live: &Vec<VoiceId>, k: usize| (!live.is_empty()).then(|| live[(k * 7 + i) % live.len()]);
    if i == 0 {
        a.quality(2);
    }
    if i == 1 {
        let id = a.stream(stream, 0.6);
        live.push(id);
    }
    if i == 3 {
        a.env(100, 70);
    }
    if i == 60 {
        a.quality(1);
    }
    if i == 90 {
        a.silence();
    }
    if i == 120 {
        a.quality(0);
    }
    if i == 130 {
        a.fade(live[0], 500, 0);
    }
    if i % 3 == 0 {
        let id = a.play(&clips[0], 0.8, true, 0x7F);
        live.push(id);
    }
    if i % 4 == 0 {
        let id = a.play(&clips[1], 1.0, false, 0x7F);
        live.push(id);
    }
    if i % 5 == 0 {
        let id = a.play(&clips[2], 0.7, false, 0x7F);
        live.push(id);
    }
    if i % 6 == 0 {
        let id = a.play(&clips[3], 0.9, false, 75);
        live.push(id);
    }
    if let Some(id) = pick(live, 1) {
        a.word(id, 0.5 + (i % 5) as f32 * 0.1, 4096 + (i as u16 * 13) % 900, 0x40 + (i % 30) as u8);
    }
    if let Some(id) = pick(live, 2) {
        a.pan(id, (i as u16).wrapping_mul(1237));
    }
    if i % 9 == 0 {
        if let Some(id) = pick(live, 3) {
            a.aux(id, (i % 100) as u8);
        }
    }
    if i % 8 == 0 {
        if let Some(id) = pick(live, 4) {
            a.ramp(id, RELEASE_TICKS, RELEASE_TARGET);
        }
    }
    if i % 11 == 0 {
        if let Some(id) = pick(live, 5) {
            a.filter(id, 800 + (i as i32 % 7) * 600);
        }
    }
    if i % 17 == 0 && i > 1 {
        if let Some(id) = pick(live, 6) {
            a.stop(id);
        }
    }
}

#[test]
fn the_callback_allocates_nothing() {
    let (tx, core) = Tx::new(44100, false);
    let mut core = core.unwrap();
    let mut out = AudioOutput { _stream: None, tx, rate: 44100 };
    let clips = clips();
    let stream = StreamSource::new(22050, 2);
    stream.push(&(0..40000).map(|i| ((i % 200) as i16 - 100) * 60).collect::<Vec<_>>());
    let mut live = Vec::new();
    let mut buf = vec![0.0f32; 1024];
    let mut worst = 0;
    for i in 0..300 {
        step(&mut out, i, &mut live, &clips, &stream);
        // the status calls the game makes while it is at it
        for id in live.iter().take(4) {
            out.is_playing(*id);
            out.progress(*id);
        }
        let ((), n) = counted(|| {
            core.drain();
            core.fill(&mut buf, 2, 44100.0);
            core.publish();
        });
        worst = worst.max(n);
        assert_eq!(n, 0, "step {i}: the callback allocated or freed {n} times");
    }
    assert_eq!(worst, 0);
    // it really mixed: voices, the reverb, the stream
    assert!(buf.iter().any(|x| *x != 0.0));
    assert!(!core.shared.voices.is_empty() && !core.shared.streams.is_empty());
}

/// The queue applies each call at the top of the next run, in call order — what the lock did — so
/// a mixer driven through it equals one driven directly, run for run.
#[test]
fn the_queue_mixes_what_the_direct_calls_mix() {
    let (tx, core) = Tx::new(44100, false);
    let mut core = core.unwrap();
    let mut out = AudioOutput { _stream: None, tx, rate: 44100 };
    // the old mixer made its reverb on the first send; the new one has a fresh one from the start
    let (_, rcore) = Tx::new(44100, false);
    let mut direct = Direct { core: rcore.unwrap(), cfg_next_id: 0 };
    direct.core.shared.tail = None;
    let clips = clips();
    let wave = (0..60000).map(|i| ((i % 200) as i16 - 100) * 60).collect::<Vec<_>>();
    let (s1, s2) = (StreamSource::new(22050, 2), StreamSource::new(22050, 2));
    s1.push(&wave);
    s2.push(&wave);
    let (mut live1, mut live2) = (Vec::new(), Vec::new());
    let (mut b1, mut b2) = (vec![0.0f32; 882], vec![0.0f32; 882]);
    for i in 0..300 {
        step(&mut out, i, &mut live1, &clips, &s1);
        step(&mut direct, i, &mut live2, &clips, &s2);
        assert_eq!(live1, live2, "the handles are numbered alike");
        core.drain();
        core.fill(&mut b1, 2, 44100.0);
        // the direct mixer renders with the same driver
        direct.core.fill(&mut b2, 2, 44100.0);
        assert!(b1.iter().zip(&b2).all(|(a, b)| a.to_bits() == b.to_bits()), "step {i}: the mix differs");
    }
    assert!(b1.iter().any(|x| *x != 0.0));
}

#[test]
fn a_status_call_sees_the_calls_the_callback_has_not_yet_applied() {
    let (tx, core) = Tx::new(44100, false);
    let mut core = core.unwrap();
    let out = AudioOutput { _stream: None, tx, rate: 44100 };
    let clips = clips();
    let id = out.play_word(clips[0].clone(), 1.0, 4096, 0x40, 0x4000, true);
    assert!(out.is_playing(id), "started, queued");
    assert_eq!(out.progress(id), Some(0.0));
    let mut buf = vec![0.0f32; 512];
    core.drain();
    core.fill(&mut buf, 2, 44100.0);
    core.publish();
    assert!(out.is_playing(id), "applied, published");
    out.stop(id);
    assert!(!out.is_playing(id), "stopped, queued");
    core.drain();
    core.publish();
    assert!(!out.is_playing(id), "stopped, applied");
    assert_eq!(out.progress(id), None);
}

#[test]
fn the_counter_sees_an_allocation() {
    let (v, n) = counted(|| vec![1u8; 100]);
    assert!(n >= 1 && v.len() == 100);
    assert_eq!(counted(|| 1 + 1).1, 0);
}

/// The device's buffer size only cuts the same stream into callbacks: a mix taken in 256-frame
/// callbacks equals one taken in 512-frame or odd-sized ones.
#[test]
fn the_mix_does_not_depend_on_the_callback_size() {
    let clips = clips();
    let run = |chunks: &[usize]| -> Vec<f32> {
        let stream = StreamSource::new(22050, 2);
        stream.push(&(0..40000).map(|i| ((i % 200) as i16 - 100) * 60).collect::<Vec<_>>());
        let (tx, core) = Tx::new(48000, false);
        let mut core = core.unwrap();
        let mut out = AudioOutput { _stream: None, tx, rate: 48000 };
        let mut live = Vec::new();
        for i in 0..40 {
            step(&mut out, i, &mut live, &clips, &stream);
        }
        core.drain();
        let mut all = Vec::new();
        let mut k = 0;
        while all.len() < 2 * 20000 {
            let n = chunks[k % chunks.len()];
            let mut buf = vec![0.0f32; n * 2];
            core.fill(&mut buf, 2, 48000.0);
            all.extend(buf);
            k += 1;
        }
        all.truncate(2 * 20000);
        all
    };
    let a = run(&[512]);
    let b = run(&[256]);
    let c = run(&[100, 333, 7, 1024]);
    assert!(a.iter().any(|x| *x != 0.0));
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()), "256 vs 512");
    assert!(a.iter().zip(&c).all(|(x, y)| x.to_bits() == y.to_bits()), "odd sizes vs 512");
}
