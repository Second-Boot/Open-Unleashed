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
//! `0x6573E8` — 0x3E24 bytes of setup, the final tick, then the stream, 118 312 bytes exactly — and
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
    /// The `nfs5.car` record id, which is what the original's snapshot carries (`.TYPE` of the car
    /// slot, §16.2); the port resolves the model and sim from it when it reads an original `.rpl`.
    pub car_id: u16,
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
        o.extend_from_slice(&s.car_id.to_le_bytes());
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
        let car_id = u16::from_le_bytes(d.get(p + 4..p + 6)?.try_into().ok()?);
        p += 6;
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
        Some(Self { setup: ReplaySetup { track, car, sim, car_id, paint, laps, opponents, traffic, cops, backwards, mirror, seed, skill }, final_tick, stream })
    }
}

// ---------------------------------------------------------------------------------------------
// The original's `.rpl` (§6.1 + §16)
// ---------------------------------------------------------------------------------------------

/// The size of the setup snapshot a `.rpl` starts with (`0x6573E8`, 0xF89 dwords).
pub const SNAPSHOT_LEN: usize = 0x3E24;
/// `0x3E24 + 4 + 0x19000` — every `.rpl` the original writes is exactly this long.
pub const RPL_LEN: usize = SNAPSHOT_LEN + 4 + STREAM_LEN;
/// Offsets of the snapshot fields the port can fill, from `replay.md` §16.1 (CONFIRMED).
pub mod snapshot {
    pub const FEGAME_TYPE: usize = 0x0000;
    pub const RACE_TYPE: usize = 0x0004;
    pub const NUM_LAPS: usize = 0x0008;
    pub const SKILL: usize = 0x000C;
    pub const NOCOLLISION: usize = 0x0024;
    pub const COPS: usize = 0x002C;
    pub const TRAFFIC_DENSITY: usize = 0x0030;
    pub const LOCAL_CAR: usize = 0x0034;
    pub const CATCHUP_LOGIC: usize = 0x0038;
    pub const REPLAY: usize = 0x003C;
    pub const MIRROR: usize = 0x0044;
    pub const REVERSE: usize = 0x0048;
    pub const MEASUREMENT: usize = 0x004C;
    /// A NUL-terminated name; the next registered variable is at +0xAC.
    pub const TRACK_NAME: usize = 0x005C;
    pub const LINEAR_TRACK: usize = 0x00AC;
    pub const WEATHER: usize = 0x063C;
    pub const FOG: usize = 0x0640;
    pub const DAMAGE: usize = 0x0644;
    pub const TIME: usize = 0x0648;
    pub const RAND_SEED: usize = 0x064C;
    pub const NUMCARS: usize = 0x0C98;
    pub const NUMPLAYERRACECARS: usize = 0x0C9C;
    pub const NUMOPPONENTRACECARS: usize = 0x0CA0;
    /// The nine car slots, stride 0x580 (§16.2).
    pub const CARS: usize = 0x0CA4;
    pub const CAR_STRIDE: usize = 0x580;
    pub const CAR_TYPE: usize = 0x0000;
    pub const CAR_CLASS: usize = 0x0008;
    pub const CAR_TRANSMISSION: usize = 0x000C;
    pub const CAR_COLOUR: usize = 0x0010;
    pub const CAR_RAMPS: usize = 0x0014;
    pub const CAR_RAMPG: usize = 0x0018;
    pub const CAR_RAMPB: usize = 0x001C;
}

/// A `.rpl` in the original's own format: the setup snapshot, the final game tick and the stream
/// (§6.1). The port fills the registry variables it has values for (§16.1) and leaves the rest
/// zero; the original reads whatever a variable holds, so the fields it needs for the race setup
/// are exactly the ones named here.
///
/// **Never round-tripped through the original** — there is no copy of the game to load it into on
/// this machine, so this is written from the documented layout, not from an observed file. The
/// port's own [`ReplayFile`] stays the format it reads back itself.
pub fn original_rpl(setup: &ReplaySetup, final_tick: u64, stream: &[u8]) -> Vec<u8> {
    use snapshot as f;
    let mut out = vec![0u8; RPL_LEN];
    let put = |o: &mut Vec<u8>, at: usize, v: u32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut out, f::RACE_TYPE, 0);
    put(&mut out, f::NUM_LAPS, setup.laps);
    put(&mut out, f::SKILL, setup.skill as u32);
    put(&mut out, f::COPS, u32::from(setup.cops));
    put(&mut out, f::TRAFFIC_DENSITY, setup.traffic);
    put(&mut out, f::MIRROR, u32::from(setup.mirror));
    put(&mut out, f::REVERSE, u32::from(setup.backwards));
    put(&mut out, f::RAND_SEED, setup.seed);
    // `Replay_save` patches this to 2 so that loading the file lands straight in playback (§6.2)
    put(&mut out, f::REPLAY, 2);
    put(&mut out, f::NUMCARS, 1 + setup.opponents);
    put(&mut out, f::NUMPLAYERRACECARS, 1);
    put(&mut out, f::NUMOPPONENTRACECARS, setup.opponents);
    let name = setup.track.as_bytes();
    let n = name.len().min(0x4F);
    out[f::TRACK_NAME..f::TRACK_NAME + n].copy_from_slice(&name[..n]);
    // slot 0 is the human's car; the port drives everything else off its own setup
    put(&mut out, f::CARS + f::CAR_TYPE, setup.car_id as u32);
    put(&mut out, f::CARS + f::CAR_RAMPS, 1);
    out[SNAPSHOT_LEN..SNAPSHOT_LEN + 4].copy_from_slice(&(final_tick as u32).to_le_bytes());
    let n = stream.len().min(STREAM_LEN);
    out[SNAPSHOT_LEN + 4..SNAPSHOT_LEN + 4 + n].copy_from_slice(&stream[..n]);
    out
}

/// Read back what the port understands of an original `.rpl`: the setup fields of §16.1, the final
/// tick and the stream. The file has no header to check, so the only validation is its length —
/// which is what the original does too (§6.3 reads whatever size the file has).
pub fn parse_original_rpl(data: &[u8]) -> Option<(ReplaySetup, u64, Vec<u8>)> {
    use snapshot as f;
    if data.len() < SNAPSHOT_LEN + 4 {
        return None;
    }
    let u32at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let end = data[f::TRACK_NAME..f::LINEAR_TRACK].iter().position(|b| *b == 0).unwrap_or(0);
    let track: String = data[f::TRACK_NAME..f::TRACK_NAME + end].iter().map(|&b| b as char).collect();
    let setup = ReplaySetup {
        track,
        car: String::new(),
        sim: String::new(),
        car_id: u32at(f::CARS + f::CAR_TYPE) as u16,
        paint: [0, 0, 0],
        laps: u32at(f::NUM_LAPS),
        opponents: u32at(f::NUMOPPONENTRACECARS),
        traffic: u32at(f::TRAFFIC_DENSITY),
        cops: u32at(f::COPS) != 0,
        backwards: u32at(f::REVERSE) != 0,
        mirror: u32at(f::MIRROR) != 0,
        seed: u32at(f::RAND_SEED),
        skill: u32at(f::SKILL) as u8,
    };
    let final_tick = u32at(SNAPSHOT_LEN) as u64;
    let stream = data[SNAPSHOT_LEN + 4..].to_vec();
    Some((setup, final_tick, stream))
}

// -------------------------------------------------------------------------------------------------
// `HReplay`, the control bar (`RE/notes/hud.md` §5.2)
// -------------------------------------------------------------------------------------------------

/// One cell of the bar: which `DashHud/replay.fsh` glyph it shows and the **box** the original
/// centres that glyph in. The box is not the glyph's own size — three of the seven disagree with the
/// file, and these are the numbers 0x42C910 passes to `0x422DF0` (`hud.md` §5.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarCell {
    /// index into `replay.fsh`; the name is the file's own four-character tag
    pub glyph: usize,
    pub name: &'static str,
    pub box_w: f32,
    pub box_h: f32,
}

/// The seven cells, left to right, in the order 0x42DEF0 lays their positions out. Two of them have
/// alternatives the state picks between: cell 2 is `paus` or `next` by the ctor's byte argument and
/// `play` when playback is stopped (`this+0xC8` / `this+0xCC` share `+0x13C`), and cell 4 is one of
/// the four speed glyphs (`this+0xD8 … +0xE4` all share `+0x14C`).
pub const BAR: [BarCell; 7] = [
    BarCell { glyph: 0, name: "cam", box_w: 14.0, box_h: 12.0 },
    BarCell { glyph: 3, name: "rest", box_w: 11.0, box_h: 9.0 },
    BarCell { glyph: 1, name: "paus", box_w: 8.0, box_h: 9.0 },
    BarCell { glyph: 2, name: "quit", box_w: 9.0, box_h: 10.0 },
    BarCell { glyph: 5, name: "I.1X", box_w: 16.0, box_h: 13.0 },
    BarCell { glyph: 4, name: "Irac", box_w: 14.0, box_h: 13.0 },
    BarCell { glyph: 9, name: "save", box_w: 14.0, box_h: 13.0 },
];

/// The alternative glyphs cells 2 and 4 swap in: `play` 0xB and `next` 0xA for the pause cell, and
/// the four speed steps for cell 4 in the file's own order — `I.1X`, `I.2X`, `I.25`, `I.5`.
pub const BAR_PLAY: usize = 0xB;
pub const BAR_NEXT: usize = 0xA;
pub const BAR_SPEEDS: [usize; 4] = [5, 6, 7, 8];

impl Speed {
    /// Which of the four speed glyphs cell 4 shows. The `.fsh` names them `I.1X`, `I.2X`, `I.25`
    /// and `I.5`, which is one-for-one with [`Speed`]'s four values — normal, double, a quarter and
    /// a half — so the mapping is the file's, not a guess.
    pub fn bar_glyph(self) -> usize {
        match self {
            Speed::Normal => BAR_SPEEDS[0],
            Speed::Double => BAR_SPEEDS[1],
            Speed::Quarter => BAR_SPEEDS[2],
            Speed::Half => BAR_SPEEDS[3],
        }
    }
}

/// The cell, `[0x5B2EF0]` × `[0x5B2EF4]`.
pub const BAR_CELL: (f32, f32) = (21.0, 18.0);
/// Cell pitch: `+0x15 + 0x1E` at 0x42DF4C and its five twins.
pub const BAR_PITCH: f32 = 21.0 + 30.0;

/// **Where the bar goes** — `0x42DEF0`, which is the only thing that sets `this+0x12C … +0x160`.
///
/// ```text
/// 0x42DEFA  total = 8·cell − cell + 0xB4 = 7·21 + 180 = 327      ; and 6·pitch + cell is 327 too
/// 0x42DF0D  x0    = [0x619784]/2 − total/2                       ; screen width, halved signed
/// 0x42DF21  y     = [0x619788] − 0x1E                            ; thirty up from the bottom
/// 0x42DF34  cell i at (x0 + i·(cell + 30), y)
/// 0x42DFC4  the bar's own frame at (x0 − 3, y − 3)
/// ```
///
/// Every halving is `cdq ; sub ; sar 1` — signed, so an odd screen width biases the bar left by one
/// rather than wrapping. Returns the seven cell origins and the frame origin.
pub fn control_bar(screen_w: f32, screen_h: f32) -> ([(f32, f32); 7], (f32, f32)) {
    let half = |v: f32| ((v as i32) - if (v as i32) < 0 { 1 } else { 0 }) >> 1;
    let total = 7.0 * BAR_CELL.0 + 180.0;
    let x0 = (half(screen_w) - half(total)) as f32;
    let y = screen_h - 30.0;
    let mut out = [(0.0, 0.0); 7];
    for (i, c) in out.iter_mut().enumerate() {
        *c = (x0 + i as f32 * BAR_PITCH, y);
    }
    (out, (x0 - 3.0, y - 3.0))
}

/// Where one glyph lands inside its cell: `x = (21 − w)/2 + cellX + 1`, `y = (18 − h)/2 + cellY + 1`
/// (0x42C993 and its eleven twins), the halving signed again.
pub fn cell_glyph_origin(cell: (f32, f32), box_w: f32, box_h: f32) -> (f32, f32) {
    let half = |v: f32| ((v as i32) - if (v as i32) < 0 { 1 } else { 0 }) >> 1;
    (half(BAR_CELL.0 - box_w) as f32 + cell.0 + 1.0, half(BAR_CELL.1 - box_h) as f32 + cell.1 + 1.0)
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
            setup: ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993turbo".into(), car_id: 154, paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, cops: true, backwards: true, mirror: false, seed: 12345, skill: 1 },
            final_tick: 40_000,
            stream: (0..500).map(|i| (i % 251 + 1) as u8).collect(),
        };
        let d = f.write();
        assert_eq!(&d[..8], FILE_MAGIC, "a port replay can never be mistaken for the original's .rpl");
        assert_eq!(ReplayFile::parse(&d), Some(f));
        assert_eq!(ReplayFile::parse(&d[..20]), None);
        assert_eq!(ReplayFile::parse(b"not a replay file at all"), None);
    }

    #[test]
    fn the_original_rpl_has_the_documented_shape() {
        // §6.1: 0x3E24 of setup, the final tick, 0x19000 of stream — 118 312 bytes, always
        let setup = ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993coupe36".into(), car_id: 154, paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, cops: true, backwards: true, mirror: false, seed: 4321, skill: 2 };
        let stream: Vec<u8> = (0..1000).map(|i| (i % 251 + 1) as u8).collect();
        let f = original_rpl(&setup, 12_345, &stream);
        assert_eq!(f.len(), RPL_LEN);
        assert_eq!(RPL_LEN, 118_312, "0x1CE28 — the note's decimal said 117 800");
        let at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
        assert_eq!(at(snapshot::NUM_LAPS), 3);
        assert_eq!(at(snapshot::SKILL), 2);
        assert_eq!(at(snapshot::COPS), 1);
        assert_eq!(at(snapshot::TRAFFIC_DENSITY), 2);
        assert_eq!(at(snapshot::REVERSE), 1);
        assert_eq!(at(snapshot::MIRROR), 0);
        assert_eq!(at(snapshot::RAND_SEED), 4321);
        // the save patches REPLAY to 2 so a load lands in playback (§6.2)
        assert_eq!(at(snapshot::REPLAY), 2);
        assert_eq!(at(snapshot::NUMCARS), 6);
        assert_eq!(&f[snapshot::TRACK_NAME..snapshot::TRACK_NAME + 8], b"coastal\0");
        assert_eq!(at(SNAPSHOT_LEN), 12_345, "the final game tick sits between the block and the stream");
        assert_eq!(&f[SNAPSHOT_LEN + 4..SNAPSHOT_LEN + 4 + 1000], &stream[..]);
        // and the port reads its own file back
        let (back, tick, s2) = parse_original_rpl(&f).expect("parse");
        assert_eq!(tick, 12_345);
        assert_eq!(back.track, "coastal");
        assert_eq!(back.car_id, 154, "the car's nfs5.car id comes back from the slot's .TYPE");
        assert_eq!((back.laps, back.opponents, back.traffic, back.cops, back.backwards, back.seed, back.skill), (3, 5, 2, true, true, 4321, 2));
        assert_eq!(&s2[..1000], &stream[..]);
        assert_eq!(parse_original_rpl(&f[..100]), None);
    }

    /// The bar's geometry, against 0x42DEF0's own arithmetic: seven cells of pitch 51 spanning
    /// exactly the 327 the width formula computes, centred, thirty up from the bottom.
    #[test]
    fn the_replay_bar_is_seven_cells_of_pitch_fifty_one() {
        let (cells, frame) = control_bar(640.0, 480.0);
        // 8·21 − 21 + 0xB4 = 327, and 6·51 + 21 = 327: the width and the pitch agree
        let total = 6.0 * BAR_PITCH + BAR_CELL.0;
        assert_eq!(total, 327.0);
        assert_eq!(cells[0].0, 320.0 - 163.0, "centred with the signed halving: 327/2 is 163");
        assert_eq!(cells[0].1, 450.0);
        for i in 1..7 {
            assert_eq!(cells[i].0 - cells[i - 1].0, BAR_PITCH);
            assert_eq!(cells[i].1, cells[0].1);
        }
        assert_eq!(cells[6].0 + BAR_CELL.0 - cells[0].0, total, "the seven cells span the width");
        assert_eq!(frame, (cells[0].0 - 3.0, cells[0].1 - 3.0));
        // an odd width biases left rather than wrapping (both halvings truncate toward −∞ here
        // because both values are positive, which is what `cdq ; sub ; sar 1` does)
        let (odd, _) = control_bar(641.0, 480.0);
        assert_eq!(odd[0].0, 320.0 - 163.0);
    }

    /// And where a glyph sits inside its cell — `(21 − w)/2 + 1`, so the 16-wide speed glyphs sit
    /// one in from the left edge and the 8-wide pause glyph six in.
    #[test]
    fn a_bar_glyph_is_centred_in_its_cell() {
        let cell = (100.0, 200.0);
        assert_eq!(cell_glyph_origin(cell, 21.0, 18.0), (101.0, 201.0), "a full-cell glyph just insets by one");
        assert_eq!(cell_glyph_origin(cell, 8.0, 9.0), (107.0, 205.0), "`paus`: (21−8)/2 = 6");
        assert_eq!(cell_glyph_origin(cell, 16.0, 13.0), (103.0, 203.0), "a speed step: (21−16)/2 = 2");
        // the seven cells' boxes are the ctor's, not the file's — `rest` is 11×9 here and 12×9 there
        assert_eq!(BAR[1].name, "rest");
        assert_eq!((BAR[1].box_w, BAR[1].box_h), (11.0, 9.0));
    }

}
