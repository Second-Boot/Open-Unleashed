//! Replays (`RE/notes/replay.md`, CONFIRMED unless noted).
//!
//! The original records **inputs, not state**: not a byte of position, orientation or physics state
//! is ever stored. Per human car, once per 32 Hz input frame, `Replay_recordInput` 0x4143F0
//! captures the four-byte controller packet plus three booleans, and playback
//! (`Replay_playbackInput` 0x4144B0) simply overwrites the same four bytes before the ordinary
//! local-human branch of `Car_applyInput` 0x4991E0 reads them. The replayed car keeps the normal
//! physics handler and the normal 64 Hz integrators; the AI, traffic and police are not recorded at
//! all — they are re-simulated. That only works because the simulation is a pure function of the
//! race setup, the RNG seed and the input stream (§11).
//!
//! What is ported here is the stream: the four interleaved channels, the 8-sample block, the
//! length-prefixed RLE of §3, the 102 400-byte buffer with its 128-byte head-room guard, the
//! `+0x40` steering bias that makes a zero channel-0 byte impossible, and the state machine of §1.
//! [`Replay::step`] is the hook: call it where the port latches the 32 Hz controls.
//!
//! **Not the original's `.rpl` file.** That file (§6.1) is a raw dump of the registry region
//! `0x6573E8` — 0x3E24 bytes of setup, the final tick, then the stream, 117 800 bytes exactly — and
//! reproducing it means reproducing the whole registry, which this port does not have. §14.5 says
//! to use a native format until then, so [`ReplayFile`] carries the port's own setup block and the
//! same stream, under its own extension (`.prpl`) and its own magic, so neither game can mistake
//! one for the other.

use nfs_physics::vehicle::Controls;

/// `0x5ED848`, `u8[0x19000]`.
pub const STREAM_LEN: usize = 0x19000;
/// `Replay_recordBlock`'s guard: `cursor + 0x80 >= 0x19000` stops recording.
pub const HEAD_ROOM: usize = 0x80;
/// Samples per flushed block; one block is 8 input frames = 16 game ticks = 0.25 s.
pub const BLOCK_SAMPLES: usize = 8;
/// The steering bias of channel 0 (`b[slot] = steer + 0x40`), which keeps the byte non-zero.
pub const STEER_BIAS: u8 = 0x40;

/// `[0x5E99F4]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 0 — recording, the buffer has room.
    Recording,
    /// 1 — recording stopped, the stream is full.
    Full,
    /// 2 — playing back.
    Playing,
    /// 3 — playback ran off the end of the stream.
    Exhausted,
}

/// The live controller packet `0x5E9664..0x5E9667` (§2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Packet {
    /// +0: bit 0 handbrake, bit 1 the second held button, bits 3..7 the command code.
    pub buttons: u8,
    /// +1: steering, −63..+63.
    pub steer: i8,
    /// +2: throttle, 0..0x7E.
    pub throttle: u8,
    /// +3: brake, 0..0x7E.
    pub brake: u8,
}

impl Packet {
    /// `Car_applyInput` 0x4992CF doubles every axis on the way into the car, so the port's controls
    /// are the packet shifted left by one.
    pub fn from_controls(c: &Controls) -> Self {
        Self {
            buttons: u8::from(c.handbrake),
            steer: (c.steer / 2).clamp(-63, 63) as i8,
            throttle: (c.throttle >> 1).min(0x7E),
            brake: (c.brake >> 1).min(0x7E),
        }
    }
    pub fn to_controls(self) -> Controls {
        Controls {
            throttle: self.throttle << 1,
            brake: self.brake << 1,
            steer: (self.steer as i32) << 1,
            handbrake: self.buttons & 1 != 0,
            gear_request: None,
        }
    }
}

/// The three car-info booleans that travel with a sample (§2.1): a digital axis has to be ramped,
/// an analogue one does not, so the same raw byte means different things without them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ramp {
    /// `.RAMPS`, car-info +0x14.
    pub steer: bool,
    /// `.RAMPG`, +0x18.
    pub gas: bool,
    /// `.RAMPB`, +0x1C.
    pub brake: bool,
}

/// `RLE8` 0x413C50: runs of 3 or more become `0xFF n value`, everything else is literal. `out[0]`
/// is the total length **including itself**; the original also writes a 0 terminator past the end.
pub fn rle8(src: &[u8; BLOCK_SAMPLES]) -> Vec<u8> {
    let mut out = vec![0u8; 1];
    let mut i = 0usize;
    while i < BLOCK_SAMPLES {
        let v = src[i];
        // The original counts the run by looking at src[i+1], reading one byte past the channel on
        // the last iteration (§15); the count it produces for the bytes inside the channel is the
        // same, so the port stops at the boundary.
        let mut run = 0usize;
        loop {
            run += 1;
            i += 1;
            if i >= BLOCK_SAMPLES || src[i] != v {
                break;
            }
        }
        if run >= 3 {
            out.extend_from_slice(&[0xFF, run as u8, v]);
        } else {
            out.extend(std::iter::repeat_n(v, run));
        }
    }
    out[0] = out.len() as u8;
    out
}

/// The inverse of [`rle8`] (`Replay_readBlock` 0x4141A0's inner loop). Returns the decoded channel
/// and how many bytes of `src` were consumed.
pub fn unrle8(src: &[u8]) -> Option<([u8; BLOCK_SAMPLES], usize)> {
    let len = *src.first()? as usize;
    if len < 1 || len > src.len() {
        return None;
    }
    let mut out = [0u8; BLOCK_SAMPLES];
    let (mut q, mut i) = (0usize, 1usize);
    while i < len && q < BLOCK_SAMPLES {
        if src[i] == 0xFF {
            let (n, v) = (*src.get(i + 1)? as usize, *src.get(i + 2)?);
            for _ in 0..n.min(BLOCK_SAMPLES - q) {
                out[q] = v;
                q += 1;
            }
            i += 3;
        } else {
            out[q] = src[i];
            q += 1;
            i += 1;
        }
    }
    Some((out, len))
}

/// One human player's recorder/player.
#[derive(Debug, Clone)]
pub struct Replay {
    pub mode: Mode,
    /// `0x5ED848`.
    stream: Vec<u8>,
    /// `[0x5E99F8]` / `[0x5E99FC]`.
    write: usize,
    read: usize,
    /// `0x606880 + p*0x20`: four channels of eight samples.
    block: [u8; 4 * BLOCK_SAMPLES],
    /// `[0x5E9A00 + p*4]`, 0..7.
    slot: usize,
    /// `[0x606A80]` — the tick recording stopped at, i.e. the replay's length.
    pub final_tick: u64,
    /// The flags the last played-back sample carried.
    pub ramp: Ramp,
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl Replay {
    pub fn new() -> Self {
        Self { mode: Mode::Recording, stream: vec![0; STREAM_LEN], write: 0, read: 0, block: [0; 32], slot: 0, final_tick: 0, ramp: Ramp::default() }
    }

    /// A replay loaded for playback.
    pub fn playing(stream: Vec<u8>, final_tick: u64) -> Self {
        let mut s = stream;
        s.resize(STREAM_LEN, 0);
        Self { mode: Mode::Playing, stream: s, write: 0, read: 0, block: [0; 32], slot: 0, final_tick, ramp: Ramp::default() }
    }

    /// `Replay_startRace` 0x413EA0: while recording, clear the buffer and both cursors; while
    /// playing, only the read side is rewound. Note the original does **not** put an overflowed
    /// recording back into mode 0 here (§1).
    pub fn start_race(&mut self) {
        match self.mode {
            Mode::Recording | Mode::Full => {
                self.stream.iter_mut().for_each(|b| *b = 0);
                self.write = 0;
            }
            _ => {}
        }
        self.read = 0;
        self.slot = 0;
        self.block = [0; 32];
    }

    pub fn stream(&self) -> &[u8] {
        &self.stream[..self.write.max(0)]
    }
    pub fn written(&self) -> usize {
        self.write
    }

    /// The hook. During recording it stores `controls` and hands them straight back; during
    /// playback it returns the recorded packet instead. `tick` is the current 64 Hz game tick and
    /// is only used to remember where an overflowing recording stopped.
    pub fn step(&mut self, controls: Controls, ramp: Ramp, tick: u64) -> Controls {
        match self.mode {
            Mode::Recording | Mode::Full => {
                self.record(Packet::from_controls(&controls), ramp, tick);
                controls
            }
            Mode::Playing | Mode::Exhausted => self.playback().to_controls(),
        }
    }

    /// `Replay_recordInput` 0x4143F0.
    pub fn record(&mut self, p: Packet, ramp: Ramp, tick: u64) {
        let s = self.slot;
        self.block[s] = (p.steer as i16 + STEER_BIAS as i16) as u8 | if ramp.steer { 0x80 } else { 0 };
        self.block[s + 8] = p.throttle | if ramp.gas { 0x80 } else { 0 };
        self.block[s + 16] = p.brake | if ramp.brake { 0x80 } else { 0 };
        self.block[s + 24] = p.buttons;
        self.slot += 1;
        if self.slot == BLOCK_SAMPLES {
            self.slot = 0;
            self.flush(tick);
        }
    }

    /// `Replay_recordBlock` 0x414060: channel 0 raw, channels 1..3 RLE'd, with the head-room guard
    /// that stops recording and freezes the final tick.
    fn flush(&mut self, tick: u64) {
        if self.write + HEAD_ROOM >= STREAM_LEN {
            if self.mode == Mode::Recording {
                self.mode = Mode::Full;
                self.final_tick = tick;
            }
            return;
        }
        let w = self.write;
        self.stream[w..w + BLOCK_SAMPLES].copy_from_slice(&self.block[..BLOCK_SAMPLES]);
        self.write += BLOCK_SAMPLES;
        for c in 1..4 {
            let mut ch = [0u8; BLOCK_SAMPLES];
            ch.copy_from_slice(&self.block[c * BLOCK_SAMPLES..(c + 1) * BLOCK_SAMPLES]);
            let enc = rle8(&ch);
            let w = self.write;
            self.stream[w..w + enc.len()].copy_from_slice(&enc);
            self.write += enc.len();
        }
    }

    /// `Replay_playbackInput` 0x4144B0.
    pub fn playback(&mut self) -> Packet {
        if self.slot == 0 {
            // A recorded channel-0 byte is never zero (the +0x40 bias), so a zero is the end.
            if self.stream.get(self.read).copied().unwrap_or(0) == 0 {
                self.mode = Mode::Exhausted;
            } else {
                self.read_block();
            }
        }
        let out = if self.mode == Mode::Playing {
            let s = self.slot;
            self.ramp = Ramp { steer: self.block[s] & 0x80 != 0, gas: self.block[s + 8] & 0x80 != 0, brake: self.block[s + 16] & 0x80 != 0 };
            self.block[s] &= 0x7F;
            self.block[s + 8] &= 0x7F;
            self.block[s + 16] &= 0x7F;
            Packet {
                steer: (self.block[s] as i16 - STEER_BIAS as i16) as i8,
                throttle: self.block[s + 8],
                brake: self.block[s + 16],
                buttons: self.block[s + 24],
            }
        } else {
            // Mode 3: coast, no input at all.
            Packet::default()
        };
        self.slot = (self.slot + 1) % BLOCK_SAMPLES;
        out
    }

    /// `Replay_readBlock` 0x4141A0.
    fn read_block(&mut self) {
        let r = self.read;
        if r + BLOCK_SAMPLES > self.stream.len() {
            self.mode = Mode::Exhausted;
            return;
        }
        self.block[..BLOCK_SAMPLES].copy_from_slice(&self.stream[r..r + BLOCK_SAMPLES]);
        self.read += BLOCK_SAMPLES;
        for c in 1..4 {
            let Some((ch, used)) = unrle8(&self.stream[self.read..]) else {
                self.mode = Mode::Exhausted;
                return;
            };
            self.block[c * BLOCK_SAMPLES..(c + 1) * BLOCK_SAMPLES].copy_from_slice(&ch);
            self.read += used;
        }
    }
}

/// Playback speed `[0x606864]` (§5.1): the original decimates **ticks**, it never scales `dt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    /// 0 — tick on even ticks only.
    Half,
    /// 1 — tick every fourth.
    Quarter,
    /// 2 — normal.
    Normal,
    /// 3 — two game ticks per loop iteration.
    Double,
}

impl Speed {
    /// How many game ticks this loop iteration runs at tick counter `t` (0 = render only).
    pub fn ticks_at(self, t: u64) -> u32 {
        match self {
            Speed::Half => u32::from(t % 2 == 0),
            Speed::Quarter => u32::from(t % 4 == 0),
            Speed::Normal => 1,
            Speed::Double => 2,
        }
    }
    /// The camera lag coefficient that goes with it (0x464D7E): 0.01 normal and double, 0.005 half,
    /// 0.0025 quarter.
    pub fn camera_lag(self) -> f32 {
        match self {
            Speed::Half => 0.005,
            Speed::Quarter => 0.0025,
            _ => 0.01,
        }
    }
    pub fn next(self) -> Self {
        match self {
            Speed::Half => Speed::Quarter,
            Speed::Quarter => Speed::Normal,
            Speed::Normal => Speed::Double,
            Speed::Double => Speed::Half,
        }
    }
}

/// The nine cycleable camera ids of §8 (`0x5CC654`); 0x14 is the track-side "TV" mode and 0x0B is
/// skidpad-only.
pub const CAMERA_IDS: [u8; 9] = [0x14, 0x0A, 0x0B, 0x08, 0x09, 0x06, 0x04, 0x0F, 0x00];

/// Ticks the race keeps running after the stream is exhausted (0x415CB3: one second).
pub const END_GRACE_TICKS: u64 = 64;

// ---------------------------------------------------------------------------------------------
// The port's replay file — NOT the original's `.rpl` (see the module docs and notes §6.1/§14.5)
// ---------------------------------------------------------------------------------------------

/// Magic of the port's own replay file.
pub const FILE_MAGIC: &[u8; 8] = b"NFS5PRPL";
pub const FILE_VERSION: u32 = 1;

/// Everything the port needs to set the race up again, plus the stream. The original stores the
/// whole `0x6573E8` registry region instead (§6.1); these are the same knobs the port's race setup
/// actually reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplaySetup {
    pub track: String,
    /// Car model (`GameData/CarModel/<model>`) and simulation variant.
    pub car: String,
    pub sim: String,
    /// Body colour.
    pub paint: [u8; 3],
    pub laps: u32,
    pub opponents: u32,
    pub traffic: u32,
    pub cops: bool,
    pub backwards: bool,
    pub mirror: bool,
    /// The seed the traffic/AI RNG was started from (`RAND_SEED`'s role in §11).
    pub seed: u32,
    pub skill: u8,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayFile {
    pub setup: ReplaySetup,
    /// `[0x606A80]`.
    pub final_tick: u64,
    pub stream: Vec<u8>,
}

impl ReplayFile {
    pub fn write(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.stream.len() + 128);
        o.extend_from_slice(FILE_MAGIC);
        o.extend_from_slice(&FILE_VERSION.to_le_bytes());
        let s = &self.setup;
        for t in [&s.track, &s.car, &s.sim] {
            o.extend_from_slice(&(t.len() as u32).to_le_bytes());
            o.extend_from_slice(t.as_bytes());
        }
        o.extend_from_slice(&s.paint);
        o.push(s.skill);
        o.extend_from_slice(&s.laps.to_le_bytes());
        o.extend_from_slice(&s.opponents.to_le_bytes());
        o.extend_from_slice(&s.traffic.to_le_bytes());
        o.push(u8::from(s.cops));
        o.push(u8::from(s.backwards));
        o.push(u8::from(s.mirror));
        o.push(0);
        o.extend_from_slice(&s.seed.to_le_bytes());
        o.extend_from_slice(&self.final_tick.to_le_bytes());
        o.extend_from_slice(&(self.stream.len() as u32).to_le_bytes());
        o.extend_from_slice(&self.stream);
        o
    }

    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.len() < 16 || &d[..8] != FILE_MAGIC || u32::from_le_bytes(d[8..12].try_into().ok()?) != FILE_VERSION {
            return None;
        }
        let mut p = 12usize;
        let u32at = |p: &mut usize| -> Option<u32> {
            let v = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?);
            *p += 4;
            Some(v)
        };
        let text = |p: &mut usize| -> Option<String> {
            let n = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?) as usize;
            *p += 4;
            let s = String::from_utf8_lossy(d.get(*p..*p + n)?).to_string();
            *p += n;
            Some(s)
        };
        let (track, car, sim) = (text(&mut p)?, text(&mut p)?, text(&mut p)?);
        let paint = [*d.get(p)?, *d.get(p + 1)?, *d.get(p + 2)?];
        let skill = *d.get(p + 3)?;
        p += 4;
        let laps = u32at(&mut p)?;
        let opponents = u32at(&mut p)?;
        let traffic = u32at(&mut p)?;
        let (cops, backwards, mirror) = (*d.get(p)? != 0, *d.get(p + 1)? != 0, *d.get(p + 2)? != 0);
        p += 4;
        let seed = u32at(&mut p)?;
        let final_tick = u64::from_le_bytes(d.get(p..p + 8)?.try_into().ok()?);
        p += 8;
        let len = u32at(&mut p)? as usize;
        let stream = d.get(p..p + len)?.to_vec();
        Some(Self { setup: ReplaySetup { track, car, sim, paint, laps, opponents, traffic, cops, backwards, mirror, seed, skill }, final_tick, stream })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_matches_the_originals_shape_and_costs() {
        // A constant channel is the best case: FF n v behind the length byte.
        let e = rle8(&[7; 8]);
        assert_eq!(e, [4, 0xFF, 8, 7]);
        // Eight distinct bytes are the worst case: all literal.
        let e = rle8(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(e, [9, 1, 2, 3, 4, 5, 6, 7, 8]);
        // A run of exactly three is worth encoding; two are not.
        assert_eq!(rle8(&[1, 1, 2, 2, 2, 3, 4, 5]), [9, 1, 1, 0xFF, 3, 2, 3, 4, 5]);
        for ch in [[0u8; 8], [1, 1, 1, 1, 2, 2, 2, 2], [9, 9, 3, 3, 3, 3, 3, 1]] {
            let e = rle8(&ch);
            let (back, used) = unrle8(&e).unwrap();
            assert_eq!(back, ch);
            assert_eq!(used, e.len());
        }
    }

    #[test]
    fn a_block_costs_between_20_and_35_bytes() {
        let mut r = Replay::new();
        for _ in 0..8 {
            r.record(Packet { buttons: 0, steer: 0, throttle: 0x40, brake: 0 }, Ramp::default(), 0);
        }
        assert_eq!(r.written(), 20, "three constant channels");
        let mut r = Replay::new();
        for i in 0..8 {
            r.record(Packet { buttons: i, steer: i as i8, throttle: i, brake: 0x7E - i }, Ramp::default(), 0);
        }
        assert_eq!(r.written(), 35, "nothing compresses");
    }

    #[test]
    fn record_then_play_returns_the_same_inputs() {
        let samples: Vec<Packet> = (0..24).map(|i| Packet { buttons: u8::from(i % 7 == 0), steer: (i as i32 % 127 - 63) as i8, throttle: (i * 5) % 0x7F, brake: (i * 3) % 0x7F }).collect();
        let ramp = Ramp { steer: true, gas: false, brake: true };
        let mut rec = Replay::new();
        for p in &samples {
            rec.record(*p, ramp, 0);
        }
        let mut play = Replay::playing(rec.stream().to_vec(), 0);
        for p in &samples {
            assert_eq!(play.playback(), *p);
            assert_eq!(play.ramp, ramp, "the .RAMP* flags travel with the sample");
        }
        // Past the end the stream reports exhaustion and the car coasts.
        assert_eq!(play.playback(), Packet::default());
        assert_eq!(play.mode, Mode::Exhausted);
    }

    #[test]
    fn steering_is_never_a_zero_byte() {
        // The +0x40 bias is what makes the end-of-stream test work.
        let mut r = Replay::new();
        for s in [-63i8, 0, 63] {
            for _ in 0..8 {
                r.record(Packet { steer: s, ..Default::default() }, Ramp::default(), 0);
            }
        }
        assert!(r.stream().iter().take(8).all(|b| *b != 0));
        assert!(r.stream()[20..28].iter().all(|b| *b != 0));
    }

    #[test]
    fn the_buffer_guard_stops_recording_and_keeps_the_tick() {
        let mut r = Replay::new();
        let mut tick = 0u64;
        // Worst-case blocks (35 B) until the guard trips.
        while r.mode == Mode::Recording {
            for i in 0..8u8 {
                r.record(Packet { buttons: i, steer: i as i8, throttle: i, brake: 0x7E - i }, Ramp::default(), tick);
            }
            tick += 16;
        }
        assert_eq!(r.mode, Mode::Full);
        assert!(r.written() + HEAD_ROOM >= STREAM_LEN);
        assert_eq!(r.final_tick, tick - 16, "the final tick is frozen at the block that would not fit");
        // 102 272 usable bytes / 35 per block ≈ 2 922 blocks ≈ 730 s of race.
        assert!((700..780).contains(&(r.final_tick / 64)), "{}", r.final_tick / 64);
    }

    #[test]
    fn controls_round_trip_through_the_packet() {
        let c = Controls { throttle: 200, brake: 40, steer: -100, handbrake: true, gear_request: None };
        let p = Packet::from_controls(&c);
        assert_eq!((p.throttle, p.brake, p.steer, p.buttons), (100, 20, -50, 1));
        let back = p.to_controls();
        assert_eq!((back.throttle, back.brake, back.steer, back.handbrake), (200, 40, -100, true));
    }

    #[test]
    fn playback_speed_is_tick_decimation() {
        assert_eq!((0..8).map(|t| Speed::Half.ticks_at(t)).sum::<u32>(), 4);
        assert_eq!((0..8).map(|t| Speed::Quarter.ticks_at(t)).sum::<u32>(), 2);
        assert_eq!((0..8).map(|t| Speed::Normal.ticks_at(t)).sum::<u32>(), 8);
        assert_eq!((0..8).map(|t| Speed::Double.ticks_at(t)).sum::<u32>(), 16);
        assert_eq!(Speed::Quarter.camera_lag(), 0.0025);
    }

    #[test]
    fn the_port_file_round_trips() {
        let f = ReplayFile {
            setup: ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993turbo".into(), paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, cops: true, backwards: true, mirror: false, seed: 12345, skill: 1 },
            final_tick: 40_000,
            stream: (0..500).map(|i| (i % 251 + 1) as u8).collect(),
        };
        let d = f.write();
        assert_eq!(&d[..8], FILE_MAGIC, "a port replay can never be mistaken for the original's .rpl");
        assert_eq!(ReplayFile::parse(&d), Some(f));
        assert_eq!(ReplayFile::parse(&d[..20]), None);
        assert_eq!(ReplayFile::parse(b"not a replay file at all"), None);
    }
}
