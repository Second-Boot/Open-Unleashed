//! The game thread's side: the command channel, the snapshot back, `Tx`, and the device callback's `Core`.

use super::*;

/// What the game thread asks of the mixer — one command per [`AudioOutput`] call that changes
/// something, queued ([`rt::channel`]) and applied by the device callback ([`Core::drain`]) at the
/// top of its next run, in call order. That is where the mutex used to put them: a call took the
/// lock between two callbacks, so it counted from the next one's first block — and the library's
/// generator ([`Shared::rand`]) is still drawn by the start, on the callback's side, in call order.
pub(super) enum Cmd {
    Quality { budget: usize, rate: u32, interp: Interp, sleep: u32, reverb: Option<Box<ReverbTail>> },
    FxAux { id: VoiceId, aux: u8 },
    Silence,
    Pan { id: VoiceId, pan: u16 },
    Start { id: VoiceId, clip: Arc<Clip>, volume: f32, word: u16, bend: u8, pan: u16, looping: bool, sel: u8, tabled: bool },
    SetWord { id: VoiceId, volume: f32, word: u16, bend: u8 },
    Ramp { id: VoiceId, ticks: i32, target: i32 },
    Filter { id: VoiceId, value: i32 },
    Stop(VoiceId),
    Env { p20: i32, p80: i32 },
    Fade { id: VoiceId, ms: i32, target: i32 },
    StreamLevel { id: VoiceId, level: i32 },
    Stream { id: VoiceId, src: Arc<StreamSource>, volume: f32 },
}

/// A command queued and not yet seen applied (its sequence number is above the snapshot's
/// `applied`): what a status call on the game thread must already know of it.
#[derive(Clone, Copy)]
pub(super) enum Pend {
    Start(VoiceId),
    Stream(VoiceId, f32),
    Stop(VoiceId),
}

impl Pend {
    pub(super) fn id(self) -> VoiceId {
        match self {
            Pend::Start(i) | Pend::Stream(i, _) | Pend::Stop(i) => i,
        }
    }
}

/// One voice as the status calls read it ([`AudioOutput::progress`], [`AudioOutput::is_playing`]).
#[derive(Clone, Copy)]
pub(super) struct VoiceStat {
    pub(super) id: VoiceId,
    pub(super) anchor: bool,
    pub(super) held: bool,
    pub(super) finished: bool,
    /// [`Voice::position`]
    pub(super) position: f64,
    /// the clip's frames
    pub(super) frames: usize,
    pub(super) rate: u32,
}

/// The mixer's state as the callback last published it ([`Core::publish`]) — what the game
/// thread's status calls read instead of the voices themselves.
#[derive(Default)]
pub(super) struct Snapshot {
    /// the commands applied when it was taken
    pub(super) applied: u64,
    pub(super) voices: Vec<VoiceStat>,
    /// `(handle, volume)` of the playing streams
    pub(super) streams: Vec<(VoiceId, f32)>,
}

impl Snapshot {
    /// The voice a status call answers for — **settled 2026-09-27** (read to the instruction:
    /// 0x566910 whole, 0x590e20, 0x566d60): the call takes a handle and, via 0x590e20, searches
    /// the group's own small array for the entry whose *own* handle matches — never a sibling's —
    /// answering "ended" (state 3, `mov [ebp], 3` at 0x566a1e) only once that entry is gone from
    /// the array. The game only ever holds the handle 0x590420 answered for a start — the anchor's
    /// (or the one voice of a solo patch) — so the call always reads the **anchor's own** entry
    /// while the group has any; there is no reading "the first playing member" or "the lowest
    /// slot" instead, whatever this note's own preliminary framing said. Found, the entry's own
    /// remaining-frames field decides 0x566986's `jge`: negative — the held anchor, its sample
    /// already having run out while the array keeps its bookkeeping ([`Voice::held`], the group
    /// note) — answers **state 0** with every field left the zero the call started with
    /// (0x566921…0x56692a): position 0, not the anchor's real (finished) cursor; non-negative
    /// answers state 1 or 2 (0x566999's `[handle] == the found entry`, an array-index test with no
    /// bearing on which data is read) from the anchor's own fields either way. The port's `held`
    /// on the anchor already matches "kept, not freed, no strip" ([`Shared::free`]); this settles
    /// what a status call answers for it: 0, not a sibling's position and not `None`.
    pub(super) fn status_voice(&self, id: VoiceId) -> Option<&VoiceStat> {
        let mut group = self.voices.iter().filter(|v| v.id == id);
        let first = group.clone().next();
        let v = group.find(|v| v.anchor).or(first)?;
        (!v.finished).then_some(v)
    }
}

/// What the last callback left queued for the device, in output frames (its delay to the playback
/// of the buffer plus the buffer), and when it ran (ns since `epoch`, plus 1; 0 before the first
/// callback) — for [`AudioOutput::latency_ms`].
pub(super) struct Latency {
    pub(super) epoch: std::time::Instant,
    pub(super) frames: AtomicU64,
    pub(super) at: AtomicU64,
}

/// The game thread's side: the command queue's producer and the bookkeeping the calls need
/// without asking the callback.
pub(super) struct TxState {
    pub(super) prod: Producer<Cmd>,
    /// what the callback gave back to be freed here (the reverb a rate change replaced)
    pub(super) gc: Consumer<Gc>,
    /// the next handle ([`VoiceId`]), numbered in call order as the library's start does
    pub(super) next_id: u32,
    /// the commands queued so far
    pub(super) seq: u64,
    /// the mix rate the queued commands leave the mixer at ([`Shared::mix_rate`]'s mirror)
    pub(super) cfg_rate: Option<u32>,
    pub(super) pending: Vec<(u64, Pend)>,
}

pub(super) struct Tx {
    pub(super) state: Mutex<TxState>,
    pub(super) snap: Arc<Mutex<Snapshot>>,
    pub(super) lat: Arc<Latency>,
    /// the buses `set_env` left, `[0x6B4A0C]` / `[0x6B4A0D]`
    pub(super) bus: [AtomicI8; 2],
    /// TEST GLUE: `NFS_AUDIO_DUMP` — no device, the mixer runs on the calling thread
    pub(super) offline: Option<Mutex<Offline>>,
}

/// The device callback's side: the mixer itself, which nothing else touches.
pub(super) struct Core {
    pub(super) shared: Shared,
    pub(super) rx: Consumer<Cmd>,
    pub(super) gc: Producer<Gc>,
    pub(super) applied: u64,
    pub(super) snap: Arc<Mutex<Snapshot>>,
    pub(super) lat: Arc<Latency>,
    pub(super) device_rate: u32,
}

/// The command queue's size, in commands: a game frame sends a few dozen.
pub(super) const QUEUE: usize = 4096;

/// The return ring's size, in voices / streams / reverbs waiting to be freed on the game thread.
pub(super) const GC_QUEUE: usize = 256;

/// What a push waits for a full queue (a callback that has stopped) before the command is dropped.
pub(super) const PUSH_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

/// The callback's buffers, made before it runs ([`Shared::reserve_rt`]).
pub(super) const RT_VOICES: usize = 512;

pub(super) const RT_FRAMES: usize = 1 << 15;

impl Core {
    pub(super) fn apply(&mut self, cmd: Cmd) {
        let g = &mut self.shared;
        match cmd {
            Cmd::Quality { budget, rate, interp, sleep, reverb } => {
                g.sleep_ms = Some(sleep);
                if g.interp != interp {
                    g.interp = interp;
                    tracing::info!(?interp, "mix interpolator");
                }
                if g.budget != Some(budget) {
                    g.budget = Some(budget);
                    tracing::info!(budget, "voice budget");
                }
                if g.mix_rate != Some(rate) {
                    g.mix_rate = Some(rate);
                    // the reverb's delay lines are sized in frames of the mix rate
                    if let Some(old) = std::mem::replace(&mut g.tail, reverb) {
                        // freed on the game thread; a full return queue frees it here
                        let _ = self.gc.push(Gc::Reverb(old));
                    }
                    tracing::info!(mix_rate = rate, device_rate = self.device_rate, "mix rate");
                }
            }
            Cmd::FxAux { id, aux } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.fx_scale = 0x7F - aux.min(0x7F);
                }
            }
            Cmd::Silence => g.silence(),
            Cmd::Pan { id, pan } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.pan = pan_byte(pan);
                    // 0x5661F0 stores the angle and calls 0x590CC0 unconditionally (no compare)
                    v.hook = true;
                }
            }
            Cmd::Start { id, clip, volume, word, bend, pan, looping, sel, tabled } => {
                g.start_patch_as(id, clip, volume, word, looping, sel, tabled);
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.bend = bend as i8;
                    v.pan = pan_byte(pan);
                }
            }
            Cmd::SetWord { id, volume, word, bend } => {
                for (_, v) in g.voices.iter_mut().filter(|(i, _)| *i == id) {
                    v.set_volume(volume);
                    v.word = word;
                    v.bend = bend as i8;
                }
            }
            Cmd::Ramp { id, ticks, target } => g.ramp(id, ticks, target),
            Cmd::Filter { id, value } => {
                let rate = g.mix_rate();
                for (_, v) in g.voices.iter_mut().filter(|(i, v)| *i == id && !v.finished && !v.held) {
                    LowPass::set(&mut v.lowpass, value, rate);
                }
            }
            Cmd::Stop(id) => {
                g.remove_voices(|(i, _)| *i == id);
                g.remove_streams(|(i, _)| *i == id);
            }
            Cmd::Env { p20, p80 } => {
                g.bus_c = p20 as i8;
                g.bus_d = p80 as i8;
            }
            Cmd::Fade { id, ms, target } => {
                if let Some((_, sv)) = g.streams.iter_mut().find(|(i, _)| *i == id) {
                    sv.fade = Some(Fade::new(sv.volume, ms, target));
                }
            }
            Cmd::StreamLevel { id, level } => {
                if let Some((_, sv)) = g.streams.iter_mut().find(|(i, _)| *i == id) {
                    sv.set_level(level);
                }
            }
            Cmd::Stream { id, src, volume } => {
                if g.allocate(src.channels.max(1) as usize, STREAM_PRIORITY, None) {
                    g.streams.push((id, StreamVoice { src, volume, frac: 0, window: [[0.0; 4]; 2], primed: false, fade: None }));
                }
            }
        }
        self.applied += 1;
    }

    /// Everything queued so far, in order.
    pub(super) fn drain(&mut self) {
        while let Some(cmd) = self.rx.pop() {
            self.apply(cmd);
        }
    }

    /// Hand the game thread the state its status calls read. Never waits: if the game thread is
    /// reading the last one at this instant, this one is skipped and the next callback's goes.
    pub(super) fn publish(&mut self) {
        // what left the mix since the last run goes to the game thread to be freed (a full ring
        // frees it here)
        for g in self.shared.dead.drain(..) {
            let _ = self.gc.push(g);
        }
        let Ok(mut s) = self.snap.try_lock() else { return };
        s.applied = self.applied;
        self.shared.snapshot_into(&mut s);
    }

    /// The device callback's run for `out` (interleaved, `channels` wide) at `out_rate`: the mix at
    /// the library's rate `[0x6B48DA]` into a buffer of that rate, and from there to the device's
    /// ([`Bridge`]).
    pub(super) fn fill(&mut self, out: &mut [f32], channels: usize, out_rate: f64) {
        let g = &mut self.shared;
        let mix_rate = g.mix_rate();
        let step = mix_rate as f64 / out_rate;
        let n = out.len() / channels.max(1);
        let mut bridge = std::mem::take(&mut g.bridge);
        for (k, frame) in out.chunks_exact_mut(channels.max(1)).enumerate() {
            let lr = bridge.next(step, n - k, |_, pending| g.pump_to(mix_rate, pending));
            // a mono device takes the left channel, a third and later one the right
            for (ch, o) in frame.iter_mut().enumerate() {
                *o = lr[ch.min(1)];
            }
        }
        g.bridge = bridge;
    }
}

impl Tx {
    /// The two halves of a mixer. With `offline` the callback's half is the glue's, and none is
    /// handed back.
    pub(super) fn new(device_rate: u32, offline: bool) -> (Arc<Tx>, Option<Core>) {
        let (prod, rx) = rt::channel::<Cmd>(QUEUE);
        let (gc_tx, gc) = rt::channel::<Gc>(GC_QUEUE);
        let snap = Arc::new(Mutex::new(Snapshot { applied: 0, voices: Vec::with_capacity(RT_VOICES), streams: Vec::with_capacity(32) }));
        let lat = Arc::new(Latency { epoch: std::time::Instant::now(), frames: AtomicU64::new(0), at: AtomicU64::new(0) });
        let mut shared = Shared::default();
        shared.reserve_rt();
        let core = Core { shared, rx, gc: gc_tx, applied: 0, snap: snap.clone(), lat: lat.clone(), device_rate };
        let state = Mutex::new(TxState { prod, gc, next_id: 0, seq: 0, cfg_rate: None, pending: Vec::with_capacity(256) });
        let mut tx = Tx { state, snap, lat, bus: [AtomicI8::new(0), AtomicI8::new(0)], offline: None };
        if offline {
            tx.offline = Some(Mutex::new(Offline { core, n: 0 }));
            return (Arc::new(tx), None);
        }
        (Arc::new(tx), Some(core))
    }

    /// Queue `cmd` (and what a status call must know of it until it is applied).
    pub(super) fn push(&self, st: &mut TxState, cmd: Cmd, pend: Option<Pend>) {
        // what the callback gave back is freed here, off its thread
        while let Some(old) = st.gc.pop() {
            drop(old);
        }
        let mut cmd = cmd;
        let t0 = std::time::Instant::now();
        loop {
            match st.prod.push(cmd) {
                Ok(()) => break,
                Err(c) => {
                    if t0.elapsed() > PUSH_WAIT {
                        tracing::warn!("audio command queue full: command dropped");
                        return;
                    }
                    cmd = c;
                    std::thread::yield_now();
                }
            }
        }
        st.seq += 1;
        if let Some(p) = pend {
            if st.pending.len() >= QUEUE {
                st.pending.remove(0);
            }
            st.pending.push((st.seq, p));
        }
        if let Some(o) = &self.offline {
            o.lock().unwrap().step();
        }
    }

    /// Queue `cmd` for a call that has no handle to give back.
    pub(super) fn send(&self, cmd: Cmd) {
        let mut st = self.state.lock().unwrap();
        self.push(&mut st, cmd, None);
    }

    pub(super) fn stop(&self, id: VoiceId) {
        let mut st = self.state.lock().unwrap();
        self.push(&mut st, Cmd::Stop(id), Some(Pend::Stop(id)));
    }

    /// The last queued, not yet applied command about `id`, if any — it is newer than the
    /// snapshot, which is returned locked with it.
    pub(super) fn overlay(&self, id: VoiceId) -> (Option<Pend>, std::sync::MutexGuard<'_, Snapshot>) {
        let mut st = self.state.lock().unwrap();
        let snap = self.snap.lock().unwrap();
        st.pending.retain(|(seq, _)| *seq > snap.applied);
        let last = st.pending.iter().rev().map(|(_, p)| *p).find(|p| p.id() == id);
        (last, snap)
    }
}
