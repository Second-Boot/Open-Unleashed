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
            buttons: u8::from(c.handbrake) | (c.command << 3),
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
            command: self.buttons >> 3,
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
    /// The flags the last sample carried — recorded or played back. Either way they are what
    /// car-info `+0x14/+0x18/+0x1C` hold at the save: 0x4143F0 reads them from there and 0x4144B0
    /// writes them there (0x414546…0x4145A3).
    pub ramp: Ramp,
    /// The other human players' own state (`0x606880 + p·0x20`, `[0x5E9A00 + p·4]`), players
    /// `1 …` — the stream is shared, the block and the slot are per player (`replay.md` §4, §18.15):
    /// an input-exchange race has one human car per player and records every one.
    others: Vec<Lane>,
}

/// One player's part of the recorder: the 32-byte block being filled or read, the slot in it, the flags.
#[derive(Debug, Clone, Default)]
struct Lane {
    block: [u8; 4 * BLOCK_SAMPLES],
    slot: usize,
    ramp: Ramp,
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl Replay {
    pub fn new() -> Self {
        Self { mode: Mode::Recording, stream: vec![0; STREAM_LEN], write: 0, read: 0, block: [0; 32], slot: 0, final_tick: 0, ramp: Ramp::default(), others: Vec::new() }
    }

    /// A replay loaded for playback. The loaded bytes count as written, so [`Replay::stream`]
    /// hands them back: the de-init save 0x413FD0 dumps the buffer whatever the mode, and after a
    /// playback that is the stream it played.
    pub fn playing(stream: Vec<u8>, final_tick: u64) -> Self {
        let mut s = stream;
        let write = s.len().min(STREAM_LEN);
        s.resize(STREAM_LEN, 0);
        Self { mode: Mode::Playing, stream: s, write, read: 0, block: [0; 32], slot: 0, final_tick, ramp: Ramp::default(), others: Vec::new() }
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
        self.others.iter_mut().for_each(|l| *l = Lane::default());
    }

    /// The session loop's turn to the automatic replay (0x4B6EF4…0x4B6F18): `REPLAY = 2`,
    /// `[0x5E99F4] = 2`, back to 0x4B6D9F — the buffer is kept as it was recorded (or as far as it
    /// filled), and the race's restart, [`Replay::start_race`] (0x413EA0), rewinds only the read
    /// side. The samples of a block never flushed are not in the stream, as in the original.
    pub fn replay_in_place(&mut self) {
        self.mode = Mode::Playing;
    }

    pub fn stream(&self) -> &[u8] {
        &self.stream[..self.write]
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

    /// Player `p`'s lane made the current one (or put back): the per-player block, slot and flags
    /// swap with the ones the single-player methods work on. Player 0 is the current lane already.
    fn swap_lane(&mut self, p: usize) {
        if p == 0 {
            return;
        }
        while self.others.len() < p {
            self.others.push(Lane::default());
        }
        let l = &mut self.others[p - 1];
        std::mem::swap(&mut self.block, &mut l.block);
        std::mem::swap(&mut self.slot, &mut l.slot);
        std::mem::swap(&mut self.ramp, &mut l.ramp);
    }

    /// **`Replay_recordInput` 0x4143F0 for player `p`** (`car+0x524`): every human car's hook records into
    /// its own block of the shared stream (the packet sits at `0x606880 + p·0x20`, its slot at
    /// `[0x5E9A00 + p·4]`); a block is flushed when the player's slot reaches 8, so the players' blocks
    /// interleave in the order the cars' hooks run.
    pub fn record_player(&mut self, p: usize, packet: Packet, ramp: Ramp, tick: u64) {
        self.swap_lane(p);
        self.record(packet, ramp, tick);
        self.swap_lane(p);
    }

    /// **`Replay_playbackInput` 0x4144B0 for player `p`**: the block is read when the player's slot is 0,
    /// from wherever the shared stream's cursor stands.
    pub fn playback_player(&mut self, p: usize) -> Packet {
        self.swap_lane(p);
        let out = self.playback();
        self.swap_lane(p);
        out
    }

    /// The `.RAMP*` flags of player `p`'s last sample.
    pub fn player_ramp(&self, p: usize) -> Ramp {
        if p == 0 { self.ramp } else { self.others.get(p - 1).map_or_else(Ramp::default, |l| l.ramp) }
    }

    /// `Replay_recordInput` 0x4143F0.
    pub fn record(&mut self, p: Packet, ramp: Ramp, tick: u64) {
        self.ramp = ramp;
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
    /// that stops recording. The guard does not look at the mode: **every** refused flush writes
    /// `[0x5E99F4] = 1` and `[0x606A80] = [0x606AC4]` (0x414076…0x414086), so while the stream is
    /// full the final tick keeps following the block flushes, and the save (0x413FD0, which only
    /// takes the clock in mode 0) stores the tick of the last refused block. *(Until the audit of
    /// 2026-09-26 the port froze it at the first refusal.)*
    fn flush(&mut self, tick: u64) {
        if self.write + HEAD_ROOM >= STREAM_LEN {
            self.mode = Mode::Full;
            self.final_tick = tick;
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
            Speed::Half => u32::from(t.is_multiple_of(2)),
            Speed::Quarter => u32::from(t.is_multiple_of(4)),
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
}

/// The nine cycleable camera ids of §8 (`0x5CC654`); 0x14 is the track-side "TV" mode and 0x0B is
/// skidpad-only.
pub const CAMERA_IDS: [u8; 9] = [0x14, 0x0A, 0x0B, 0x08, 0x09, 0x06, 0x04, 0x0F, 0x00];

/// Applying a cycle entry, 0x414953…0x414ACD: **0x14** is the track-side "TV" camera only while
/// the track carries camera nodes (`0x46D960()`), and **id 3** when it carries none; every other
/// id goes through unchanged.
pub fn apply_camera(id: u8, has_camera_nodes: bool) -> u8 {
    if id == 0x14 && !has_camera_nodes { 3 } else { id }
}

/// Stepping the cycle, command 8 (0x414901…0x414ACD): `idx = (idx + 1) % 9`, and when the new
/// entry is **0x0B `SplineCam`** the name `TRACK_NAME [0x657444]` is compared with `"skidpad"`
/// (`_stricmp` 0x5AE3C0 at 0x4149A3): **equal** steps the index once more (0x4149B3…0x4149C4),
/// **different** keeps 0x0B (`jne 0x414AB7`). So 0x0B is skipped *on* skidpad and offered on every
/// other track. *(Until the audit of 2026-09-26 this was the other way round, from `replay.md` §8.)*
pub fn next_camera(index: usize, is_skidpad: bool) -> usize {
    let n = CAMERA_IDS.len();
    let mut i = (index + 1) % n;
    if CAMERA_IDS[i] == 0x0B && is_skidpad {
        i = (i + 1) % n;
    }
    i
}

/// The end of a playback, 0x415CB3…0x415CE2 in `RaceFrame`: once the stream has run out
/// (`[0x5E99F4] == 3`) the race is over as soon as `gGameTick [0x606AC4] > 0x40`
/// (`cmp …, 0x40 ; jle` skips) — `[0x606A8C] = 1`, `REPLAY = 3`. The 64 is an absolute tick of the
/// race, not a grace period after the stream ends. *(Until the audit of 2026-09-26 the port kept
/// the race running for 64 ticks after the exhaustion, from `replay.md` §5.2's "one second after".)*
pub const END_MIN_TICK: u64 = 0x40;

/// Whether a playback in `mode` at game tick `tick` is over (0x415CB3).
pub fn playback_over(mode: Mode, tick: u64) -> bool {
    mode == Mode::Exhausted && tick > END_MIN_TICK
}

// ---------------------------------------------------------------------------------------------
// The port's replay file — NOT the original's `.rpl` (see the module docs and notes §6.1/§14.5)
// ---------------------------------------------------------------------------------------------

/// Magic of the port's own replay file.
pub const FILE_MAGIC: &[u8; 8] = b"NFS5PRPL";
/// 2 carries `ReplaySetup::driver_name` (§16.2 / §18.11's gap) — a clean break: the format is the
/// port's own working file, not a distributed one, and a version-1 file fails [`ReplayFile::parse`]
/// rather than silently reading in an unset name. 3 adds the network race's own section
/// ([`NetSetup`], `replay.md` §18.15) after the stream.
pub const FILE_VERSION: u32 = 3;

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
    /// `Game.Damage` — the snapshot's `DAMAGE` word (`[0x657A2C]`, registry id 0x20 at +0x644,
    /// `replay.md` §6); the port's own file keeps it in the byte after `mirror` as "damage off"
    /// so that a file written before 2026-09-26 (that byte 0) still reads as on
    pub damage: bool,
    pub backwards: bool,
    pub mirror: bool,
    /// The seed the traffic/AI RNG was started from (`RAND_SEED`'s role in §11).
    pub seed: u32,
    pub skill: u8,
    /// `.DRIVERNAME` (id 0x13B, car slot `+0x17C`, §16.2): `Replay_begin` 0x413CF0's bulk
    /// `memcpy(0x6573E8, 0x5E9A20, …)` restores the whole recorded car-slot block on playback,
    /// this field among it, before the race is built — so an original `.rpl` shows the recording's
    /// driver, not the profile up now (§18.11's gap, closed here). Empty when the file carries
    /// none (the port's own `.prpl` before this field existed).
    pub driver_name: String,
    /// The race was an input-exchange network race: what the launch settled about the other humans
    /// and the road they share (`replay.md` §18.15). `None` for every other race.
    pub net: Option<NetSetup>,
    /// `RACE_TYPE` (`[0x6573EC]`, registry id 0x10 at +0x04): `Replay_begin` 0x413CF0's bulk
    /// restore hands the recording's back, so a playback of a Quick Knockout (3) runs the lap
    /// knockout, of the cops race (1) the cops race, and so on (0 normal, 1 cops, 2 series,
    /// 3 lap knockout, 4 skill test: `modes.md` section 1, 0x4EB470). `None` for a file written before
    /// the field existed: the playback then runs as a single race (APPROXIMATION, logged).
    pub race_type: Option<u8>,
    /// `FEGAME_TYPE` (`[0x6573E8]`, id 0x30 at +0x00): the launch's `Game.GameType`, also the
    /// recording's after the same restore (0x4EB4DE...0x4EB4E2 writes it). `None` as above: the
    /// playback then answers the front end's current `Game.GameType`.
    pub fegame_type: Option<u8>,
    /// `.CAMERA` (id 0x148, car slot 0 `+0x208`, four dwords, `Camera.1...4`: 0x4EF02A...0x4EF0F5):
    /// the recording's list, `None` for a file that carries none (the last launch's stays).
    pub camera: Option<Vec<u32>>,
}

/// An input-exchange race's setup beyond [`ReplaySetup`]'s single car: the original's snapshot block
/// (`0x6573E8`) holds nine car-info records (`+0xCA4`, stride 0x580) and the launch's per-slot table;
/// the port keeps what its own race build reads of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetSetup {
    /// this machine's own race index (`[0x65741C]`) — the car [`ReplaySetup`]'s car / paint / name are
    pub local_index: u32,
    /// `.STARTINGPOS` of the local car as the port's `grid_pos` (`g − 1`, `u32::MAX` for `g = 0`)
    pub grid: Option<u32>,
    /// every other human, in room order (the port's seat order)
    pub seats: Vec<NetSeat>,
    /// the traffic cars' records (the host's `.FILENAME`, `.FEMULTIPLAYERCARNAME`, `.COLOUR`)
    pub traffic: Vec<NetTraffic>,
    /// `.CAMERA` of race index 0 (`car[0].camera`, `network.md` §11.12c)
    pub camera0: Option<Vec<u32>>,
    /// the local car's own launch record (its per-slot words at `local_index`), for the original-format `.rpl`'s
    /// nine car-info records (`replay.md` §18.15); empty in a file written before the field existed
    pub own_words: Vec<(u32, Vec<u32>)>,
    /// the host's `fe.txt` options the stream carried that the race reads and the setup has no field for:
    /// `("weather" | "time" | "fog", value)` — `TIME` picks the sky, and the original's snapshot holds all three
    /// (`WEATHER` +0x63C, `FOG` +0x640, `TIME` +0x648)
    pub fe_txt: Vec<(String, u32)>,
}

/// One other human of a network race.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetSeat {
    pub race_index: u32,
    /// the room's name for the player (the plate when the record carries no licence)
    pub name: String,
    pub grid: Option<u32>,
    /// its per-slot car record: `(option id, the words after the slot)` (`launchcar::CarSlot`)
    pub words: Vec<(u32, Vec<u32>)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetTraffic {
    pub model: String,
    pub name: Vec<u8>,
    pub colour: u32,
}

impl NetSetup {
    fn write(&self, o: &mut Vec<u8>) {
        let u32w = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
        let opt = |o: &mut Vec<u8>, v: Option<u32>| match v {
            Some(v) => {
                o.push(1);
                o.extend_from_slice(&v.to_le_bytes());
            }
            None => o.push(0),
        };
        let text = |o: &mut Vec<u8>, t: &[u8]| {
            o.extend_from_slice(&(t.len() as u32).to_le_bytes());
            o.extend_from_slice(t);
        };
        u32w(o, self.local_index);
        opt(o, self.grid);
        u32w(o, self.seats.len() as u32);
        for s in &self.seats {
            u32w(o, s.race_index);
            text(o, s.name.as_bytes());
            opt(o, s.grid);
            u32w(o, s.words.len() as u32);
            for (id, w) in &s.words {
                u32w(o, *id);
                u32w(o, w.len() as u32);
                for x in w {
                    u32w(o, *x);
                }
            }
        }
        u32w(o, self.traffic.len() as u32);
        for t in &self.traffic {
            text(o, t.model.as_bytes());
            text(o, &t.name);
            u32w(o, t.colour);
        }
        match &self.camera0 {
            Some(c) => {
                o.push(1);
                u32w(o, c.len() as u32);
                for x in c {
                    u32w(o, *x);
                }
            }
            None => o.push(0),
        }
        // a trailing section: the local car's own record (absent in a file written before it existed)
        u32w(o, self.own_words.len() as u32);
        for (id, w) in &self.own_words {
            u32w(o, *id);
            u32w(o, w.len() as u32);
            for x in w {
                u32w(o, *x);
            }
        }
        u32w(o, self.fe_txt.len() as u32);
        for (k, v) in &self.fe_txt {
            text(o, k.as_bytes());
            u32w(o, *v);
        }
    }

    fn parse(d: &[u8], p: &mut usize) -> Option<Self> {
        let u32r = |p: &mut usize| -> Option<u32> {
            let v = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?);
            *p += 4;
            Some(v)
        };
        let opt = |p: &mut usize| -> Option<Option<u32>> {
            let f = *d.get(*p)?;
            *p += 1;
            if f == 0 {
                return Some(None);
            }
            let v = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?);
            *p += 4;
            Some(Some(v))
        };
        let text = |p: &mut usize| -> Option<Vec<u8>> {
            let n = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?) as usize;
            *p += 4;
            let t = d.get(*p..*p + n)?.to_vec();
            *p += n;
            Some(t)
        };
        let local_index = u32r(p)?;
        let grid = opt(p)?;
        let n = u32r(p)? as usize;
        let mut seats = Vec::new();
        for _ in 0..n {
            let race_index = u32r(p)?;
            let name = String::from_utf8_lossy(&text(p)?).to_string();
            let grid = opt(p)?;
            let m = u32r(p)? as usize;
            let mut words = Vec::new();
            for _ in 0..m {
                let id = u32r(p)?;
                let k = u32r(p)? as usize;
                let mut w = Vec::new();
                for _ in 0..k {
                    w.push(u32r(p)?);
                }
                words.push((id, w));
            }
            seats.push(NetSeat { race_index, name, grid, words });
        }
        let t = u32r(p)? as usize;
        let mut traffic = Vec::new();
        for _ in 0..t {
            let model = String::from_utf8_lossy(&text(p)?).to_string();
            let name = text(p)?;
            let colour = u32r(p)?;
            traffic.push(NetTraffic { model, name, colour });
        }
        let camera0 = if *d.get(*p)? == 0 {
            *p += 1;
            None
        } else {
            *p += 1;
            let n = u32r(p)? as usize;
            let mut c = Vec::new();
            for _ in 0..n {
                c.push(u32r(p)?);
            }
            Some(c)
        };
        let mut own_words = Vec::new();
        if *p < d.len() {
            let m = u32r(p)? as usize;
            for _ in 0..m {
                let id = u32r(p)?;
                let k = u32r(p)? as usize;
                let mut w = Vec::new();
                for _ in 0..k {
                    w.push(u32r(p)?);
                }
                own_words.push((id, w));
            }
        }
        let mut fe_txt = Vec::new();
        if *p < d.len() {
            let m = u32r(p)? as usize;
            for _ in 0..m {
                let k = String::from_utf8_lossy(&text(p)?).to_string();
                fe_txt.push((k, u32r(p)?));
            }
        }
        Some(Self { local_index, grid, seats, traffic, camera0, own_words, fe_txt })
    }
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
        for t in [&s.track, &s.car, &s.sim, &s.driver_name] {
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
        // "damage off": 0 keeps the files written before the field read as damage on
        o.push(u8::from(!s.damage));
        o.extend_from_slice(&s.seed.to_le_bytes());
        o.extend_from_slice(&self.final_tick.to_le_bytes());
        o.extend_from_slice(&(self.stream.len() as u32).to_le_bytes());
        o.extend_from_slice(&self.stream);
        match &s.net {
            Some(n) => {
                o.push(1);
                n.write(&mut o);
            }
            None => o.push(0),
        }
        // the launch block's words (`RACE_TYPE`, `FEGAME_TYPE`, `.CAMERA`): a trailing section after the
        // network one, so a file written before it still parses (its fields read `None`)
        if s.race_type.is_some() || s.fegame_type.is_some() || s.camera.is_some() {
            o.push(1);
            o.push(s.race_type.unwrap_or(0xFF));
            o.push(s.fegame_type.unwrap_or(0xFF));
            match &s.camera {
                Some(c) => {
                    o.push(c.len().min(4) as u8 + 1);
                    for v in c.iter().take(4) {
                        o.extend_from_slice(&v.to_le_bytes());
                    }
                }
                None => o.push(0),
            }
        }
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
        let (track, car, sim, driver_name) = (text(&mut p)?, text(&mut p)?, text(&mut p)?, text(&mut p)?);
        let paint = [*d.get(p)?, *d.get(p + 1)?, *d.get(p + 2)?];
        let skill = *d.get(p + 3)?;
        let car_id = u16::from_le_bytes(d.get(p + 4..p + 6)?.try_into().ok()?);
        p += 6;
        let laps = u32at(&mut p)?;
        let opponents = u32at(&mut p)?;
        let traffic = u32at(&mut p)?;
        let (cops, backwards, mirror) = (*d.get(p)? != 0, *d.get(p + 1)? != 0, *d.get(p + 2)? != 0);
        let damage = *d.get(p + 3)? == 0;
        p += 4;
        let seed = u32at(&mut p)?;
        let final_tick = u64::from_le_bytes(d.get(p..p + 8)?.try_into().ok()?);
        p += 8;
        let len = u32at(&mut p)? as usize;
        let stream = d.get(p..p + len)?.to_vec();
        p += len;
        let net = if *d.get(p)? == 0 {
            p += 1;
            None
        } else {
            p += 1;
            Some(NetSetup::parse(d, &mut p)?)
        };
        // the launch block's trailing section; a file written before it ends here (the fallbacks are the caller's)
        let (mut race_type, mut fegame_type, mut camera) = (None, None, None);
        if d.get(p) == Some(&1) {
            let some = |b: u8| (b != 0xFF).then_some(b);
            race_type = d.get(p + 1).copied().and_then(some);
            fegame_type = d.get(p + 2).copied().and_then(some);
            let n = *d.get(p + 3)? as usize;
            if n > 0 {
                let mut list = Vec::new();
                for k in 0..n - 1 {
                    let at = p + 4 + k * 4;
                    list.push(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?));
                }
                camera = Some(list);
            }
        }
        Some(Self { setup: ReplaySetup { track, car, sim, car_id, paint, laps, opponents, traffic, cops, damage, backwards, mirror, seed, skill, driver_name, net, race_type, fegame_type, camera }, final_tick, stream })
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
    /// A NUL-terminated name; the next registered variable, `FE_PLAYER_FILENAME`, is at +0x9C
    /// (§16.1), so the field is 0x40 bytes.
    pub const TRACK_NAME: usize = 0x005C;
    pub const FE_PLAYER_FILENAME: usize = 0x009C;
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
    /// `.DRIVERNAME` (id 0x13B, §16.2): a NUL-terminated name; the next registered field,
    /// `.LICENSE` at +0x1C0, bounds it to 0x44 bytes.
    pub const CAR_DRIVERNAME: usize = 0x017C;
    /// `.CAMERA` (id 0x148, replay.md 16.2): four dwords up to `.COLORCONFIG` at +0x218
    pub const CAR_CAMERA: usize = 0x0208;
    pub const CAR_CAMERA_LEN: usize = 4;
    pub const CAR_DRIVERNAME_LEN: usize = 0x44;
    /// `[0x65741C]` — this machine's own race index, at `LOCAL_CAR` (+0x34, above) — and the launch's two network
    /// words `COMM_MODE` (`[0x657404]`, +0x1C) and `COMMSPOSITIONAL` (`[0x657408]`, +0x20)
    pub const COMM_MODE: usize = 0x001C;
    pub const COMMSPOSITIONAL: usize = 0x0020;
    /// `FEGAME_TYPE` of a network race
    pub const GAME_TYPE_NETWORK: u32 = 3;
    /// The per-slot car-info record's registered fields, `(offset, option id)` (`replay.md` §16.2): what
    /// 0x4B4B80 writes a launch record's words into (`slot·0x580 + 0x65808C + offset`).
    pub const CAR_FIELDS: &[(usize, u32)] = &[
    (0x0000, 0x21f),
    (0x0008, 0x220),
    (0x000C, 0x221),
    (0x0010, 0x225),
    (0x0014, 0x222),
    (0x0018, 0x223),
    (0x001C, 0x224),
    (0x0020, 0x23b),
    (0x0024, 0x23a),
    (0x0028, 0x14b),
    (0x0078, 0x14c),
    (0x00C8, 0x14d),
    (0x00F0, 0x227),
    (0x00F4, 0x22e),
    (0x00FC, 0x228),
    (0x0100, 0x229),
    (0x0104, 0x22a),
    (0x0108, 0x22b),
    (0x010C, 0x22c),
    (0x0110, 0x22d),
    (0x0114, 0x22f),
    (0x0118, 0x230),
    (0x011C, 0x231),
    (0x0120, 0x232),
    (0x0124, 0x234),
    (0x0128, 0x235),
    (0x0130, 0x233),
    (0x0134, 0x236),
    (0x013C, 0x237),
    (0x0140, 0x238),
    (0x0144, 0x239),
    (0x0148, 0x23c),
    (0x014C, 0x23e),
    (0x0150, 0x23d),
    (0x0154, 0x23f),
    (0x0158, 0x240),
    (0x015C, 0x241),
    (0x0160, 0x242),
    (0x0164, 0x243),
    (0x0168, 0x244),
    (0x016C, 0x245),
    (0x0170, 0x246),
    (0x0174, 0x247),
    (0x017C, 0x13b),
    (0x01C0, 0x13c),
    (0x0200, 0x248),
    (0x0204, 0x249),
    (0x0208, 0x148),
    (0x0218, 0x149),
    (0x0228, 0x13d),
    (0x0268, 0x13e),
    (0x02A8, 0x13f),
    (0x02E8, 0x140),
    (0x0328, 0x24a),
    (0x032C, 0x14a),
    (0x050C, 0x24b),
    (0x0510, 0x24d),
    (0x0514, 0x24e),
    (0x0518, 0x24f),
    (0x051C, 0x250),
    (0x0520, 0x24c),
    (0x0524, 0x251),
    (0x0528, 0x252),
    (0x052C, 0x253),
    (0x0530, 0x254),
    (0x0534, 0x141),
    (0x0574, 0x226),
    (0x0578, 0x255),
    ];
    /// The field an option id lands in: its offset and the bytes up to the next registered field (the record ends at
    /// `CAR_STRIDE`) — the most a record's words can fill.
    pub fn car_field(id: u32) -> Option<(usize, usize)> {
        let k = CAR_FIELDS.iter().position(|f| f.1 == id)?;
        let next = CAR_FIELDS.get(k + 1).map_or(CAR_STRIDE, |f| f.0);
        Some((CAR_FIELDS[k].0, next - CAR_FIELDS[k].0))
    }
}

/// Write launch records — `(option id, the words after the slot)` — into car slot `slot` of the snapshot `out` the way
/// 0x4B4B80 fills the per-slot table: each id at its field, the words little-endian, cut at the field's end.
fn put_car_words(out: &mut [u8], slot: usize, words: &[(u32, Vec<u32>)]) {
    use snapshot as f;
    if slot >= 9 {
        return;
    }
    for (id, w) in words {
        let Some((off, cap)) = f::car_field(*id) else { continue };
        let at = f::CARS + slot * f::CAR_STRIDE + off;
        for (k, x) in w.iter().enumerate() {
            if k * 4 + 4 > cap {
                break;
            }
            out[at + k * 4..at + k * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
}

/// A `.rpl` in the original's own format: the setup snapshot, the final game tick and the stream
/// (§6.1). The port fills the registry variables it has values for (§16.1) and leaves the rest
/// zero; the original reads whatever a variable holds, so the fields it needs for the race setup
/// are exactly the ones named here.
///
/// **Never round-tripped through the original** — there is no copy of the game to load it into on
/// this machine, so this is written from the documented layout, not from an observed file. The
/// port's own [`ReplayFile`] stays the format it reads back itself.
///
/// `ramp` is the three `.RAMP*` flags car-info slot 0 holds at the save (the last sample's), and
/// `num_cars` is `NUMCARS` as `Race_setCars` 0x4ECF00 sets it: cops + players + opponents +
/// traffic, the traffic already cut to fit nine (0x4ED382…0x4ED399) — the caller knows the counts.
pub fn original_rpl(setup: &ReplaySetup, final_tick: u64, stream: &[u8], ramp: Ramp, num_cars: u32) -> Vec<u8> {
    use snapshot as f;
    let mut out = vec![0u8; RPL_LEN];
    let put = |o: &mut Vec<u8>, at: usize, v: u32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
    // `RACE_TYPE` / `FEGAME_TYPE` as the launch's 0x4EB470 wrote them (0x4EB54C, 0x4EB4E2); a setup that
    // carries none keeps the port's earlier file: both 0
    put(&mut out, f::RACE_TYPE, u32::from(setup.race_type.unwrap_or(0)));
    put(&mut out, f::FEGAME_TYPE, u32::from(setup.fegame_type.unwrap_or(0)));
    put(&mut out, f::NUM_LAPS, setup.laps);
    put(&mut out, f::SKILL, setup.skill as u32);
    put(&mut out, f::COPS, u32::from(setup.cops));
    put(&mut out, f::DAMAGE, u32::from(setup.damage));
    put(&mut out, f::TRAFFIC_DENSITY, setup.traffic);
    put(&mut out, f::MIRROR, u32::from(setup.mirror));
    put(&mut out, f::REVERSE, u32::from(setup.backwards));
    put(&mut out, f::RAND_SEED, setup.seed);
    // `Replay_save` patches this to 2 so that loading the file lands straight in playback (§6.2)
    put(&mut out, f::REPLAY, 2);
    put(&mut out, f::NUMCARS, num_cars);
    put(&mut out, f::NUMPLAYERRACECARS, 1);
    put(&mut out, f::NUMOPPONENTRACECARS, setup.opponents);
    let name = setup.track.as_bytes();
    // at most 0x3F characters and the NUL inside the 0x40-byte field; the port wrote up to 0x4F,
    // over the start of `FE_PLAYER_FILENAME` (until the audit of 2026-09-26)
    let n = name.len().min(f::FE_PLAYER_FILENAME - f::TRACK_NAME - 1);
    out[f::TRACK_NAME..f::TRACK_NAME + n].copy_from_slice(&name[..n]);
    // slot 0 is the human's car — a network race's is its own race index; the port drives everything else off
    // its own setup
    let me = setup.net.as_ref().map_or(0, |n| (n.local_index as usize).min(8));
    let base = f::CARS + me * f::CAR_STRIDE;
    // `.CAMERA` of the car slot, written first: a network race's own launch record (below) is the
    // same field and wins
    if let Some(c) = &setup.camera {
        for (k, v) in c.iter().take(f::CAR_CAMERA_LEN).enumerate() {
            put(&mut out, base + f::CAR_CAMERA + k * 4, *v);
        }
    }
    if let Some(net) = &setup.net {
        // **A network race's snapshot** (`replay.md` §18.15): `FEGAME_TYPE` 3, the launch's `COMM_MODE` 4 and
        // `COMMSPOSITIONAL` 0 (an input-exchange race), `[0x65741C]` the recorded machine's own race index, the
        // humans as the players — and **one car-info record per human**, each the launch record 0x4B4B80 wrote
        // into its slot, then the host's traffic cars' after them (`.TYPE` 0xB, `.CLASS` 4)
        put(&mut out, f::FEGAME_TYPE, f::GAME_TYPE_NETWORK);
        put(&mut out, f::COMM_MODE, 4);
        put(&mut out, f::COMMSPOSITIONAL, 0);
        put(&mut out, f::LOCAL_CAR, net.local_index);
        put(&mut out, f::NUMPLAYERRACECARS, 1 + net.seats.len() as u32);
        put(&mut out, f::NUMOPPONENTRACECARS, 0);
        for (k, v) in &net.fe_txt {
            match k.as_str() {
                "weather" => put(&mut out, f::WEATHER, *v),
                "fog" => put(&mut out, f::FOG, *v),
                "time" => put(&mut out, f::TIME, *v),
                _ => {}
            }
        }
        put_car_words(&mut out, me, &net.own_words);
        for s in &net.seats {
            put_car_words(&mut out, s.race_index as usize, &s.words);
        }
        let players = 1 + net.seats.len();
        for (k, t) in net.traffic.iter().enumerate() {
            let text = |b: &[u8]| -> Vec<u32> { b.chunks(4).map(|c| u32::from_le_bytes(std::array::from_fn(|i| c.get(i).copied().unwrap_or(0)))).collect() };
            put_car_words(
                &mut out,
                players + k,
                &[(0x13D, text(t.model.as_bytes())), (0x141, text(&t.name)), (0x225, vec![t.colour]), (0x21F, vec![0xB]), (0x220, vec![4])],
            );
        }
    }
    if setup.net.as_ref().is_none_or(|n| n.own_words.is_empty()) {
        put(&mut out, base + f::CAR_TYPE, setup.car_id as u32);
    }
    // the three flags as the car-info holds them — the port wrote `.RAMPS = 1` and nothing else
    // (until the audit of 2026-09-26)
    put(&mut out, base + f::CAR_RAMPS, u32::from(ramp.steer));
    put(&mut out, base + f::CAR_RAMPG, u32::from(ramp.gas));
    put(&mut out, base + f::CAR_RAMPB, u32::from(ramp.brake));
    // `.DRIVERNAME` (+0x17C, §16.2): `Replay_begin`'s bulk restore hands it back on playback, so
    // an original `.rpl` this port writes carries the recording driver's name too (§18.11)
    let dn = setup.driver_name.as_bytes();
    let n = dn.len().min(f::CAR_DRIVERNAME_LEN - 1);
    out[base + f::CAR_DRIVERNAME..base + f::CAR_DRIVERNAME + n].copy_from_slice(&dn[..n]);
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
    let end = data[f::TRACK_NAME..f::FE_PLAYER_FILENAME].iter().position(|b| *b == 0).unwrap_or(0);
    let track: String = data[f::TRACK_NAME..f::TRACK_NAME + end].iter().map(|&b| b as char).collect();
    // `.DRIVERNAME` (+0x17C, §16.2), restored by `Replay_begin`'s bulk `memcpy` on playback —
    // the same field the standings / summary / plate read (§18.11's gap, closed here)
    let dn_start = f::CARS + f::CAR_DRIVERNAME;
    let dn_end = data[dn_start..dn_start + f::CAR_DRIVERNAME_LEN].iter().position(|b| *b == 0).unwrap_or(0);
    let driver_name: String = data[dn_start..dn_start + dn_end].iter().map(|&b| b as char).collect();
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
        damage: u32at(f::DAMAGE) != 0,
        backwards: u32at(f::REVERSE) != 0,
        mirror: u32at(f::MIRROR) != 0,
        seed: u32at(f::RAND_SEED),
        skill: u32at(f::SKILL) as u8,
        driver_name,
        net: None,
        // the three words of the launch block `Replay_begin` restores: read as they stand (the file has
        // no way to say "absent": a port-written file from before them holds 0 / 0 / zeros)
        race_type: Some(u32at(f::RACE_TYPE) as u8),
        fegame_type: Some(u32at(f::FEGAME_TYPE) as u8),
        camera: Some((0..f::CAR_CAMERA_LEN).map(|k| u32at(f::CARS + f::CAR_CAMERA + k * 4)).collect()),
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

/// What a press on one of the bar's cells asks for — `HReplay`'s slot 9, **0x42D880**(x, y, event),
/// settled from the code (read 2026-09-26). Each cell is hit-tested against its own position field
/// (the same `+0x12C…+0x160` pairs 0x42DEF0 lays out), `x` in `[cx, cx + 21]` and `y` in
/// `[cy, cy + 18]` — `jl` / `jg` on integers, so both edges are inside — and answers only **event
/// 2**, the left button going down (`WM_LBUTTONDOWN` → 0x435180), once per press: a latch per
/// cell `[0x60CB50 + i]` is set by the answer and cleared by any other event with the pointer in
/// that cell. The answers, by cell (left to right, 0x42DEF0's order):
///
/// | cell | field | answer |
/// |---|---|---|
/// | 0 `cam` | `+0x12C` | **6** (0x42D8FC) → `RaceFrame` command 8, the next camera |
/// | 1 `rest` | `+0x134` | **7** with `+0x174 = 5` (0x42D984…0x42D98E) — or **0x17** when `+0x198` |
/// | 2 `paus` / `play` | `+0x13C` | **8** with `+0x199` flipped (0x42DA24…0x42DA34) — or **0x16** when `+0x198` |
/// | 3 `quit` | `+0x144` | **9** (0x42DB45), only without `+0x198` |
/// | 4 speed | `+0x14C` | `+0x174` stepped, then **0xA…0xD** (0x42DBCF…0x42DC2C), only without `+0x198` |
/// | 5 `Irac` | `+0x154` | **0xE** (0x42DABD) |
/// | 6 `save` | `+0x15C` | **0xF** (0x42DCA6), only without `+0x198` |
///
/// `+0x198` is the constructor's byte, 1 only for a network race (`FEGAME_TYPE 3`, 0x424F7B…
/// 0x424F90): there cell 2 shows `next` and the two cells step the car (0x16 → command 9, 0x17 →
/// command 10), and quit, speed and save are neither drawn (0x42D3CD) nor tested (0x42DAFC). The
/// in-race action table (`[0x415D9C + event − 1]`, 0x415520) maps 6 → 8, 7 → 1, 8 → 3, 9 → 2,
/// 0xA…0xD → 4…7, 0x16 → 9, 0x17 → 10; 0xE and 0xF reach no command — the HUD's pump takes them
/// (0x42664B, 0x4264EE), and it takes 7 as well before passing it on (0x4266BD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarEvent {
    /// 6 → command 8
    Camera,
    /// 7 → command 1: quit with `[0x657428] = 1`, which the session loop reads as "play it again"
    Restart,
    /// 8 → command 3, the pause toggle
    Pause,
    /// 9 → command 2, quit
    Quit,
    /// 0xA…0xD → command 4…7
    Speed(Speed),
    /// 0xE: the result pages again
    Results,
    /// 0xF: `HReplaySave`
    Save,
    /// 0x16 → command 9 (network only)
    NextCar,
    /// 0x17 → command 10 (network only)
    PrevCar,
}

impl BarEvent {
    /// The event number the slot returns.
    pub fn code(self) -> u8 {
        match self {
            BarEvent::Camera => 6,
            BarEvent::Restart => 7,
            BarEvent::Pause => 8,
            BarEvent::Quit => 9,
            BarEvent::Speed(Speed::Normal) => 0xA,
            BarEvent::Speed(Speed::Double) => 0xB,
            BarEvent::Speed(Speed::Quarter) => 0xC,
            BarEvent::Speed(Speed::Half) => 0xD,
            BarEvent::Results => 0xE,
            BarEvent::Save => 0xF,
            BarEvent::NextCar => 0x16,
            BarEvent::PrevCar => 0x17,
        }
    }
}

/// Which cell the pointer `(x, y)` (whole pixels, the HUD's 640 × 480) is on — 0x42D8CD…0x42D8EA
/// and its six twins: `x < cx` or `x > cx + 21` misses (`jl` / `jg`), the same for `y` with 18.
/// The cells are 30 apart, so at most one matches. The draw 0x42D4C0 lights a cell's outline by
/// the same test (0x42D6B0…0x42D6D3).
/// The cells the network bar (`+0x198`, built for `FEGAME_TYPE 3`, 0x424F7B) neither draws (0x42D3CD…0x42D3D5 jumps over
/// their draws to 0x42D476) nor tests (0x42D880's `+0x198` tests, 0x42D97A / 0x42DA1A / 0x42DAFC…): quit, speed and save
/// — so their hover counts stay at the constructor's 0 and no tooltip comes for them.
pub fn is_cell_off_in_network(cell: usize) -> bool {
    matches!(cell, 3 | 4 | 6)
}

pub fn bar_cell_at(cells: &[(f32, f32); 7], x: i32, y: i32) -> Option<usize> {
    cells.iter().position(|&(cx, cy)| {
        let (cx, cy) = (cx as i32, cy as i32);
        x >= cx && x <= cx + BAR_CELL.0 as i32 && y >= cy && y <= cy + BAR_CELL.1 as i32
    })
}

/// A press on `cell` (0x42D880 with event 2): the answer, given the bar's own state and `network`
/// (`+0x198`). The `rest` cell puts the speed step `+0x174` back to 5 (0x42D984) and the speed
/// cell steps it (0x42DBCF), both on the bar ([`BarState`]). See [`BarEvent`]. *(Until
/// 2026-09-27 the speed cell stepped from the playback's speed `[0x606864]` — `Speed::next`,
/// removed — which differs from `+0x174` after a playback start, 0x413ECF.)*
pub fn bar_event(cell: usize, bar: &mut BarState, network: bool) -> Option<BarEvent> {
    match (cell, network) {
        (0, _) => Some(BarEvent::Camera),
        (1, false) => {
            bar.step = 5;
            Some(BarEvent::Restart)
        }
        (1, true) => Some(BarEvent::PrevCar),
        (2, false) => Some(BarEvent::Pause),
        (2, true) => Some(BarEvent::NextCar),
        (3, false) => Some(BarEvent::Quit),
        (4, false) => Some(BarEvent::Speed(bar.press_speed())),
        (5, _) => Some(BarEvent::Results),
        (6, false) => Some(BarEvent::Save),
        _ => None,
    }
}

/// The bar's frame box for the auto-hide test, `+0x24 / +0x28` and `+0x10 / +0x14`: the tray's
/// origin is the frame origin of [`control_bar`] (0x42DFEC…0x42DFFC → 0x4342E0 → 0x434310) and its
/// size the constructor's `[0x60CB48]` × `[0x60CB4C]` (333 × 24, 0x42C96A → 0x42DEF0 → 0x4340A0).
pub const BAR_FRAME: (f32, f32) = (6.0 * BAR_PITCH + BAR_CELL.0 + 6.0, BAR_CELL.1 + 6.0);

/// `HReplay`'s own state beyond its cells — built once per **session** by the HUD factory (0x424F90
/// / 0x424FA6 → 0x42C910; a restart and the automatic replay keep it), read 2026-09-27:
///
/// * `step` is `+0x174`, the speed cell's step 5…8 (0x42D0B3 sets 5): the draw shows glyph
///   `+0xDC + 4·step` (0x42D40A…0x42D42F, the four speed widgets `+0xD8…+0xE4` copied to `+0xF0…`,
///   0x42D095…0x42D0C9) and a press steps it (0x42DBCF…0x42DBE3) and answers by it (table 0x42DCF8)
///   — **not** by the playback's speed `[0x606864]`, which a playback start puts back to normal
///   (0x413ECF) without touching `+0x174`. The `rest` cell puts it back to 5 (0x42D984).
/// * `away` is `+0x124`: slot 9 (0x42D880, the pump's call once per `RaceFrame`, 0x415520 →
///   0x4264CC) zeroes it with the pointer inside the bar's frame box (0x4344B0, edges inside) and
///   otherwise counts it up to 0x28 (0x42D8A0…0x42D8BB).
/// * `hover[i]` are `+0x108 + 4i` (cleared by the constructor, 0x42D076…0x42D093): the same count
///   per cell, the pointer in the cell (0x42D911…0x42D931 and its six twins) — the tooltip's
///   (0x42DE1D: drawn from 0x28 on).
/// * `pinned` is `+0x178`: the HUD's frame 0x425950 sets it while a dialog is queued
///   (`[0x60C9C4]` not empty, 0x425CF5…0x425D0C) and, once the queue is empty, clears it with
///   `away` (0x42D4A0 at 0x425D22).
/// * `state` / `stamp` are the tray's `+0xC` and `+0xB4`. The draw (0x42D22F…0x42D282) closes the
///   bar — slot 5(2) — when `away >= 0x28` and not `pinned` and the state is 3 or 1, and opens it —
///   slot 5(1) — otherwise when the state is 0 or 2; slot 5 stamps `[0x606AC8]` (0x434490). The
///   bar is built with `+0xBC = 1` (0x42C919), so its tray **fades** (slot 14 0x4349F0: `pct =
///   min((clock − stamp)·100 / 48, 100)`, signed `idiv`) and nothing moves it to 3 or 0 — only
///   the slide (0x4346A0, for `+0xBC == 0`) does that. So after its first opening the bar lives in
///   states 1 (fading in, then held at 100) and 2 (fading out, then held at 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarState {
    pub step: u8,
    pub away: i32,
    pub hover: [i32; 7],
    pub pinned: bool,
    pub state: u8,
    pub stamp: u64,
}

impl Default for BarState {
    fn default() -> Self {
        Self::new()
    }
}

impl BarState {
    /// The constructor 0x42C910: `+0x174 = 5` (0x42D0B3), `+0x178 = 0`, the counters 0
    /// (0x42D076…), the tray in state 0 with stamp 0 (0x42A369, 0x433FDD).
    pub fn new() -> Self {
        Self { step: 5, away: 0, hover: [0; 7], pinned: false, state: 0, stamp: 0 }
    }

    /// The speed the step stands for, through 0x42DCF8: 5 → 0xA normal, 6 → 0xB double, 7 → 0xC
    /// quarter, 8 → 0xD half (`RaceFrame`'s commands 4…7).
    pub fn speed(&self) -> Speed {
        match self.step {
            6 => Speed::Double,
            7 => Speed::Quarter,
            8 => Speed::Half,
            _ => Speed::Normal,
        }
    }

    /// Which of the four speed glyphs cell 4 shows: widget `+0xDC + 4·step` (0x42D413), i.e.
    /// `+0xD8…+0xE4` for 5…8, which the constructor built from `replay.fsh` entries 5, 6, 7, 8
    /// (0x42CC8E, 0x42CD01, 0x42CD74, 0x42CDE7) — `I.1X`, `I.2X`, `I.25`, `I.5`.
    pub fn glyph(&self) -> usize {
        BAR_SPEEDS[usize::from(self.step.clamp(5, 8) - 5)]
    }

    /// A press on the speed cell (0x42DBCF…0x42DBE3): `++step`, above 8 back to 5; the answer is
    /// the new step's.
    pub fn press_speed(&mut self) -> Speed {
        self.step += 1;
        if self.step > 8 {
            self.step = 5;
        }
        self.speed()
    }

    /// Slot 9's counting (0x42D892…0x42D8BB and 0x42D8C1…0x42DCE2), run once per frame with the
    /// pointer in whole pixels, or `None` when the port has none (the pointer off the window —
    /// counted as off the bar). The cells' latches are [`bar_cell_at`]'s caller's.
    pub fn count(&mut self, cells: &[(f32, f32); 7], frame: (f32, f32), pointer: Option<(i32, i32)>, network: bool) {
        let inside = pointer.is_some_and(|(x, y)| {
            let (fx, fy) = (frame.0 as i32, frame.1 as i32);
            x >= fx && x <= fx + BAR_FRAME.0 as i32 && y >= fy && y <= fy + BAR_FRAME.1 as i32
        });
        self.away = if inside { 0 } else { (self.away + 1).min(0x28) };
        let over = pointer.and_then(|(x, y)| bar_cell_at(cells, x, y)).filter(|&i| !(network && is_cell_off_in_network(i)));
        for (i, h) in self.hover.iter_mut().enumerate() {
            *h = if over == Some(i) { (*h + 1).min(0x28) } else { 0 };
        }
    }

    /// The draw's transition (0x42D22F…0x42D282) at `[0x606AC8]` = `clock`.
    pub fn transition(&mut self, clock: u64) {
        let target = if self.away >= 0x28 && !self.pinned {
            matches!(self.state, 3 | 1).then_some(2)
        } else {
            matches!(self.state, 0 | 2).then_some(1)
        };
        if let Some(s) = target {
            self.state = s;
            self.stamp = clock;
        }
    }

    /// The HUD frame's pin (0x425CF5…0x425D22), after the draw: a dialog queued pins the bar; the
    /// queue empty again unpins it and zeroes `away`.
    pub fn pin(&mut self, dialog_queued: bool) {
        if dialog_queued {
            self.pinned = true;
        } else if self.pinned {
            self.away = 0;
            self.pinned = false;
        }
    }

    /// The fade in per cent the draw scales everything by (0x42D29B…0x42D2D2): slot 14 in state
    /// 1, `100 − ` it in state 2; `None` in state 0 (0x42D28E: not drawn). State 3 would read a
    /// local the draw never wrote (0x42D2CE, the `push ecx` slot, i.e. `this`) — unreachable, see
    /// the struct.
    pub fn pct(&self, clock: u64) -> Option<i32> {
        let slot14 = || ((clock as i64 - self.stamp as i64) * 100 / 48).min(100) as i32;
        match self.state {
            1 => Some(slot14()),
            2 => Some(100 - slot14()),
            3 => Some(100),
            _ => None,
        }
    }
}

/// The tooltips 0x42DD10 (drawn at the end of the bar's draw, 0x42D478): the FE string of each
/// cell (`hud.loc` through 0x41E160 → 0x536DA0), `+0x17C…+0x194` rebuilt every draw — cell 2 is
/// 18149 (0x46E5) while `+0x199` is clear and 18173 (0x46FD) while it is set.
///
/// **The network bar** (`+0x198`, 0x42DD3A…0x42DD80): cell 1 reads 18163 (0x46F3) in place of 18148 and cell 2 reads 18164
/// (0x46F4) whatever `+0x199` says — «previous / next car»'s strings.
pub fn bar_tooltip(cell: usize, paused: bool, network: bool) -> u32 {
    match cell {
        0 => 18147,
        1 if network => 18163,
        1 => 18148,
        2 if network => 18164,
        2 if paused => 18173,
        2 => 18149,
        3 => 18194,
        4 => 18150,
        5 => 18151,
        _ => 18152,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The network bar (`+0x198`) neither draws nor tests quit / speed / save, so their hover counts never rise and no
    /// tooltip comes for them; cells 1 and 2 read 18163 / 18164 (0x42DD3A…0x42DD80).
    #[test]
    fn the_network_bar_counts_no_hover_on_quit_speed_and_save_and_has_its_own_tooltips() {
        let (cells, frame) = control_bar(640.0, 480.0);
        for (cell, counts) in [(1usize, true), (2, true), (3, false), (4, false), (6, false), (5, true)] {
            let mut b = BarState::new();
            let (cx, cy) = (cells[cell].0 as i32 + 2, cells[cell].1 as i32 + 2);
            for _ in 0..0x30 {
                b.count(&cells, frame, Some((cx, cy)), true);
            }
            assert_eq!(b.hover[cell] >= 0x28, counts, "cell {cell}");
        }
        assert_eq!((bar_tooltip(1, false, true), bar_tooltip(2, false, true), bar_tooltip(2, true, true)), (18163, 18164, 18164));
        assert_eq!((bar_tooltip(1, false, false), bar_tooltip(2, false, false), bar_tooltip(2, true, false)), (18148, 18149, 18173));
        assert!([3, 4, 6].iter().all(|&i| is_cell_off_in_network(i)) && ![0, 1, 2, 5].iter().any(|&i| is_cell_off_in_network(i)));
    }

    /// The bar's own state: the speed step, the auto-hide count, the pin and the fade (0x42D880,
    /// 0x42D22F, 0x425CF5, 0x4349F0).
    #[test]
    fn the_bar_hides_and_fades_as_0x42d1e0() {
        let (cells, frame) = control_bar(640.0, 480.0);
        let mut b = BarState::new();
        // +0x174: 5 → 6 → 7 → 8 → 5 answers double, quarter, half, normal
        let seen: Vec<Speed> = (0..4).map(|_| b.press_speed()).collect();
        assert_eq!(seen, [Speed::Double, Speed::Quarter, Speed::Half, Speed::Normal]);
        // the first frame opens it (state 0, away 0) and it fades in over 48 ticks
        b.count(&cells, frame, None, false);
        b.transition(100);
        assert_eq!((b.state, b.stamp), (1, 100));
        assert_eq!(b.pct(100), Some(0));
        assert_eq!(b.pct(124), Some(50));
        assert_eq!(b.pct(200), Some(100));
        // forty frames off the bar (the first counted above) close it; it never reaches state 3
        // or 0 again
        for _ in 0..0x26 {
            b.count(&cells, frame, None, false);
            b.transition(300);
        }
        assert_eq!(b.state, 1);
        b.count(&cells, frame, None, false);
        b.transition(300);
        assert_eq!((b.state, b.away), (2, 0x28));
        assert_eq!(b.pct(300 + 48), Some(0));
        assert_eq!(b.pct(1000), Some(0));
        // a dialog pins it open; its end zeroes the count
        b.pin(true);
        b.transition(400);
        assert_eq!(b.state, 1);
        b.pin(false);
        assert_eq!((b.pinned, b.away), (false, 0));
        // the pointer back on the frame (edges inside) zeroes the count, a cell counts its hover
        b.count(&cells, frame, Some((frame.0 as i32 + 333, frame.1 as i32 + 24)), false);
        assert_eq!(b.away, 0);
        let (cx, cy) = (cells[4].0 as i32 + 3, cells[4].1 as i32 + 3);
        for _ in 0..50 {
            b.count(&cells, frame, Some((cx, cy)), false);
        }
        assert_eq!(b.hover, [0, 0, 0, 0, 0x28, 0, 0]);
        b.count(&cells, frame, Some((frame.0 as i32 + 334, frame.1 as i32)), false);
        assert_eq!((b.away, b.hover[4]), (1, 0));
    }

    /// 0x42D880's answers, cell by cell, and the speed cell's cycle through the jump table 0x42DCF8.
    #[test]
    fn the_bar_cells_answer_as_0x42d880() {
        let codes: Vec<Option<u8>> = (0..7).map(|c| bar_event(c, &mut BarState::new(), false).map(BarEvent::code)).collect();
        assert_eq!(codes, [Some(6), Some(7), Some(8), Some(9), Some(0xB), Some(0xE), Some(0xF)]);
        let net: Vec<Option<u8>> = (0..7).map(|c| bar_event(c, &mut BarState::new(), true).map(BarEvent::code)).collect();
        assert_eq!(net, [Some(6), Some(0x17), Some(0x16), None, None, Some(0xE), None]);
        // +0x174: 5 → 6 → 7 → 8 → 5 is 0xB, 0xC, 0xD, 0xA
        let mut b = BarState::new();
        let seen: Vec<u8> = (0..4).map(|_| bar_event(4, &mut b, false).map_or(0, BarEvent::code)).collect();
        assert_eq!(seen, [0xB, 0xC, 0xD, 0xA]);
        // the step is the bar's, not the playback's speed: a playback start leaves it (0x413ECF
        // writes `[0x606864]` only), the `rest` cell puts it back to 5 (0x42D984)
        bar_event(4, &mut b, false);
        assert_eq!(b.step, 6);
        assert_eq!(bar_event(1, &mut b, false), Some(BarEvent::Restart));
        assert_eq!((b.step, bar_event(4, &mut b, false)), (5, Some(BarEvent::Speed(Speed::Double))));
        // both edges are inside the cell (`jl` / `jg`), the gap between cells is not
        let (cells, _) = control_bar(640.0, 480.0);
        let (x0, y0) = (cells[0].0 as i32, cells[0].1 as i32);
        assert_eq!(bar_cell_at(&cells, x0, y0), Some(0));
        assert_eq!(bar_cell_at(&cells, x0 + 21, y0 + 18), Some(0));
        assert_eq!(bar_cell_at(&cells, x0 + 22, y0), None);
        assert_eq!(bar_cell_at(&cells, x0 + 51, y0 + 9), Some(1));
        assert_eq!(bar_cell_at(&cells, x0 + 6 * 51 + 3, y0 - 1), None);
    }

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
        assert_eq!(play.stream(), rec.stream(), "a playback saves the stream it plays (0x413FD0)");
        for p in &samples {
            assert_eq!(play.playback(), *p);
            assert_eq!(play.ramp, ramp, "the .RAMP* flags travel with the sample");
        }
        // Past the end the stream reports exhaustion and the car coasts.
        assert_eq!(play.playback(), Packet::default());
        assert_eq!(play.mode, Mode::Exhausted);
    }

    #[test]
    fn the_automatic_replay_plays_the_buffer_in_place() {
        // 20 samples: two blocks flushed, four samples of a third never are
        let samples: Vec<Packet> = (0..20).map(|i| Packet { buttons: 0, steer: i as i8 - 10, throttle: i as u8, brake: 0 }).collect();
        let mut r = Replay::new();
        r.start_race();
        for p in &samples {
            r.record(*p, Ramp::default(), 0);
        }
        // 0x4B6F0C `[0x5E99F4] = 2`, then the race's restart 0x413EA0
        r.replay_in_place();
        r.start_race();
        assert_eq!(r.mode, Mode::Playing);
        for p in &samples[..16] {
            assert_eq!(r.playback(), *p);
        }
        assert_eq!(r.playback(), Packet::default());
        assert_eq!(r.mode, Mode::Exhausted);
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
        assert_eq!(r.final_tick, tick - 16, "the final tick is the block that would not fit");
        // 102 272 usable bytes / 35 per block ≈ 2 922 blocks ≈ 730 s of race.
        assert!((700..780).contains(&(r.final_tick / 64)), "{}", r.final_tick / 64);
        // …and every later refused block moves it on (0x414086 has no mode test)
        let written = r.written();
        for i in 0..8u8 {
            r.record(Packet { buttons: i, ..Default::default() }, Ramp::default(), tick);
        }
        assert_eq!((r.mode, r.written(), r.final_tick), (Mode::Full, written, tick));
    }

    #[test]
    fn controls_round_trip_through_the_packet() {
        let c = Controls { throttle: 200, brake: 40, steer: -100, handbrake: true, command: 0 };
        let p = Packet::from_controls(&c);
        assert_eq!((p.throttle, p.brake, p.steer, p.buttons), (100, 20, -50, 1));
        let back = p.to_controls();
        assert_eq!((back.throttle, back.brake, back.steer, back.handbrake), (200, 40, -100, true));
        // the command code rides in bits 3..7 (a shift-down, code 3, is 0x18)
        let p = Packet::from_controls(&Controls { command: 3, ..c });
        assert_eq!(p.buttons, 0x19);
        assert_eq!(p.to_controls().command, 3);
    }

    #[test]
    fn playback_ends_once_exhausted_past_tick_64() {
        assert!(!playback_over(Mode::Exhausted, 64), "0x415CC2: jle 0x415CE8 at exactly 0x40");
        assert!(playback_over(Mode::Exhausted, 65));
        assert!(!playback_over(Mode::Playing, 5000));
    }

    #[test]
    fn playback_speed_is_tick_decimation() {
        assert_eq!((0..8).map(|t| Speed::Half.ticks_at(t)).sum::<u32>(), 4);
        assert_eq!((0..8).map(|t| Speed::Quarter.ticks_at(t)).sum::<u32>(), 2);
        assert_eq!((0..8).map(|t| Speed::Normal.ticks_at(t)).sum::<u32>(), 8);
        assert_eq!((0..8).map(|t| Speed::Double.ticks_at(t)).sum::<u32>(), 16);
        assert_eq!(Speed::Quarter.camera_lag(), 0.0025);
    }

    /// §8's two rules for the playback camera cycle.
    #[test]
    fn playback_camera_cycle() {
        // the "TV" camera falls back to id 3 on a track with no camera nodes
        assert_eq!(apply_camera(0x14, true), 0x14);
        assert_eq!(apply_camera(0x14, false), 3);
        assert_eq!(apply_camera(0x0A, false), 0x0A);
        // 0x0B `SplineCam` is stepped over on skidpad only (the `_stricmp` at 0x4149A3 equal)
        assert_eq!(CAMERA_IDS[1], 0x0A);
        assert_eq!(next_camera(0, false), 1, "0x14 -> 0x0A");
        assert_eq!(next_camera(1, false), 2, "off skidpad 0x0B is offered");
        assert_eq!(next_camera(1, true), 3, "on skidpad 0x0A skips 0x0B to 0x08");
        assert_eq!(next_camera(8, false), 0, "the ninth wraps to the first");
    }

    #[test]
    fn the_port_file_round_trips() {
        let f = ReplayFile {
            setup: ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993turbo".into(), car_id: 154, paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, cops: true, damage: true, backwards: true, mirror: false, seed: 12345, skill: 1, driver_name: "Kostya".into(), net: None, race_type: None, fegame_type: None, camera: None },
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
    fn the_launch_block_words_round_trip_and_an_old_file_reads_none() {
        let mut setup = ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993".into(), car_id: 154, paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, ..Default::default() };
        // a file written before the trailing section: no race type, game type or camera
        let old = ReplayFile { setup: setup.clone(), final_tick: 9, stream: vec![1, 2, 3] };
        let d = old.write();
        let back = ReplayFile::parse(&d).unwrap();
        assert_eq!((back.setup.race_type, back.setup.fegame_type, back.setup.camera), (None, None, None));
        // with the section (Quick Knockout, a career event's camera list)
        setup.race_type = Some(3);
        setup.fegame_type = Some(1);
        setup.camera = Some(vec![1, 0, 2, 7]);
        let f = ReplayFile { setup: setup.clone(), final_tick: 9, stream: vec![1, 2, 3] };
        let d = f.write();
        assert_eq!(ReplayFile::parse(&d), Some(f));
        // a reader of the old layout stops after the network byte: the new section only appends
        assert_eq!(d.len(), old.write().len() + 1 + 2 + 1 + 16);
        // the original's block: RACE_TYPE +0x04, FEGAME_TYPE +0x00, `.CAMERA` slot 0 +0xCA4 +0x208
        let rpl = original_rpl(&setup, 9, &[1, 2, 3], Ramp::default(), 7);
        let at = |o: usize| u32::from_le_bytes(rpl[o..o + 4].try_into().unwrap());
        assert_eq!((at(0x04), at(0x00)), (3, 1));
        assert_eq!([at(0xCA4 + 0x208), at(0xCA4 + 0x20C), at(0xCA4 + 0x210), at(0xCA4 + 0x214)], [1, 0, 2, 7]);
        assert_eq!(at(0xCA4 + 0x218), 0, "the next field (`.COLORCONFIG`) is untouched");
        let (p, _, _) = parse_original_rpl(&rpl).unwrap();
        assert_eq!((p.race_type, p.fegame_type, p.camera), (Some(3), Some(1), Some(vec![1, 0, 2, 7])));
    }

    #[test]
    fn the_original_rpl_has_the_documented_shape() {
        // §6.1: 0x3E24 of setup, the final tick, 0x19000 of stream — 118 312 bytes, always
        let setup = ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993coupe36".into(), car_id: 154, paint: [1, 2, 3], laps: 3, opponents: 5, traffic: 2, cops: true, damage: false, backwards: true, mirror: false, seed: 4321, skill: 2, driver_name: "Kostya".into(), net: None, race_type: None, fegame_type: None, camera: None };
        let stream: Vec<u8> = (0..1000).map(|i| (i % 251 + 1) as u8).collect();
        let f = original_rpl(&setup, 12_345, &stream, Ramp { steer: true, gas: false, brake: true }, 11);
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
        assert_eq!(at(snapshot::NUMCARS), 11);
        let slot0 = |o: usize| at(snapshot::CARS + o);
        assert_eq!((slot0(snapshot::CAR_RAMPS), slot0(snapshot::CAR_RAMPG), slot0(snapshot::CAR_RAMPB)), (1, 0, 1));
        // a long name stays inside TRACK_NAME's 0x40 bytes, NUL included
        let long = ReplaySetup { track: "x".repeat(100), ..setup.clone() };
        let g = original_rpl(&long, 0, &[], Ramp::default(), 1);
        assert_eq!(g[snapshot::FE_PLAYER_FILENAME - 1], 0);
        assert_eq!(g[snapshot::FE_PLAYER_FILENAME - 2], b'x');
        assert_eq!(&f[snapshot::TRACK_NAME..snapshot::TRACK_NAME + 8], b"coastal\0");
        assert_eq!(at(SNAPSHOT_LEN), 12_345, "the final game tick sits between the block and the stream");
        assert_eq!(&f[SNAPSHOT_LEN + 4..SNAPSHOT_LEN + 4 + 1000], &stream[..]);
        // `.DRIVERNAME` (+0x17C, §16.2): the recording driver's name sits in the same slot 0 the
        // car's `.TYPE` does, so `Replay_begin`'s bulk restore hands both back together (§18.11)
        assert_eq!(&f[snapshot::CARS + snapshot::CAR_DRIVERNAME..snapshot::CARS + snapshot::CAR_DRIVERNAME + 7], b"Kostya\0");
        // and the port reads its own file back
        let (back, tick, s2) = parse_original_rpl(&f).expect("parse");
        assert_eq!(tick, 12_345);
        assert_eq!(back.track, "coastal");
        assert_eq!(back.car_id, 154, "the car's nfs5.car id comes back from the slot's .TYPE");
        assert_eq!((back.laps, back.opponents, back.traffic, back.cops, back.backwards, back.seed, back.skill), (3, 5, 2, true, true, 4321, 2));
        assert_eq!(back.driver_name, "Kostya");
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


    #[test]
    fn every_human_records_its_own_block_and_the_blocks_interleave_in_hook_order() {
        // 0x4143F0 per player: two humans (race indices 1 and 3), 24 samples each. A block is flushed
        // when the player's own slot reaches 8, so the stream is p1 b0, p3 b0, p1 b1, p3 b1, ...
        let sample = |p: u8, n: u8| Packet { buttons: p, steer: n as i8, throttle: 0x10 + n, brake: p + n };
        let mut rec = Replay::new();
        for n in 0..24u8 {
            rec.record_player(1, sample(1, n), Ramp::default(), n as u64);
            rec.record_player(3, sample(3, n), Ramp { steer: n % 2 == 0, ..Ramp::default() }, n as u64);
        }
        let mut play = Replay::playing(rec.stream().to_vec(), 24);
        for n in 0..24u8 {
            assert_eq!(play.playback_player(1), sample(1, n), "player 1, sample {n}");
            assert_eq!(play.playback_player(3), sample(3, n), "player 3, sample {n}");
            assert_eq!(play.player_ramp(3).steer, n % 2 == 0, "the flags travel per player");
        }
        // and past the end both coast (0x4144B0's mode 3)
        assert_eq!(play.playback_player(1), Packet::default());
        assert_eq!(play.playback_player(3), Packet::default());
    }

    #[test]
    fn a_network_race_carries_its_seats_and_its_traffic_through_the_file() {
        let net = NetSetup {
            local_index: 1,
            grid: Some(u32::MAX),
            seats: vec![
                NetSeat { race_index: 0, name: "HOST".into(), grid: Some(2), words: vec![(0x13D, vec![0x3939_3339, 0]), (0x225, vec![0xFF11_2233])] },
                NetSeat { race_index: 2, name: "THIRD".into(), grid: None, words: vec![] },
            ],
            traffic: vec![NetTraffic { model: "EAS10".into(), name: b"Bus".to_vec(), colour: 0xFF8C_8C90 }],
            camera0: Some(vec![0, 1, 2, 3]),
            own_words: vec![(0x13D, vec![0x3132_3334, 0]), (0x24F, vec![1])],
            fe_txt: vec![("time".into(), 1), ("weather".into(), 0)],
        };
        let setup = ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993turbo".into(), car_id: 154, paint: [1, 2, 3], laps: 1, opponents: 2, traffic: 1, cops: false, damage: true, backwards: false, mirror: false, seed: 8165, skill: 1, driver_name: "ANNA".into(), net: Some(net.clone()), race_type: None, fegame_type: None, camera: None };
        let f = ReplayFile { setup, final_tick: 640, stream: vec![0x41; 40] };
        let back = ReplayFile::parse(&f.write()).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.setup.net.unwrap(), net);
    }

    /// The original-format `.rpl` of a network race holds one car-info record per human — each launch record at its
    /// field of `slot·0x580 + 0xCA4` (§16.2) — the host's traffic after them, `FEGAME_TYPE` 3 and the recorded
    /// machine's own race index at `[0x65741C]`.
    #[test]
    fn the_original_rpl_of_a_network_race_has_a_car_info_record_per_human() {
        let net = NetSetup {
            local_index: 1,
            grid: None,
            seats: vec![NetSeat { race_index: 0, name: "HOST".into(), grid: None, words: vec![(0x13D, vec![0x3939_3339, 0]), (0x225, vec![0xFF11_2233]), (0x24F, vec![1])] }],
            traffic: vec![NetTraffic { model: "sbus".into(), name: b"Bus".to_vec(), colour: 0xFF8C_8C90 }],
            camera0: None,
            own_words: vec![(0x13D, vec![0x3132_3334, 0]), (0x24A, vec![2])],
            fe_txt: vec![("time".into(), 1)],
        };
        let setup = ReplaySetup { track: "coastal".into(), car: "993".into(), sim: "993".into(), car_id: 154, paint: [0; 3], laps: 1, opponents: 1, traffic: 1, cops: false, damage: true, backwards: false, mirror: false, seed: 1, skill: 1, driver_name: "ANNA".into(), net: Some(net), race_type: None, fegame_type: None, camera: None };
        let f = original_rpl(&setup, 10, &[], Ramp::default(), 3);
        let u = |at: usize| u32::from_le_bytes(f[at..at + 4].try_into().unwrap());
        let slot = |i: usize| snapshot::CARS + i * snapshot::CAR_STRIDE;
        assert_eq!((u(snapshot::FEGAME_TYPE), u(snapshot::LOCAL_CAR), u(snapshot::NUMPLAYERRACECARS), u(snapshot::NUMCARS)), (3, 1, 2, 3));
        assert_eq!((u(snapshot::COMM_MODE), u(snapshot::COMMSPOSITIONAL)), (4, 0));
        assert_eq!(u(snapshot::TIME), 1);
        // the host at race index 0: `.FILENAME` +0x228 ("9339"), `.COLOUR` +0x10, `.TINYCAR` +0x518
        assert_eq!(&f[slot(0) + 0x228..slot(0) + 0x22C], b"9399");
        assert_eq!((u(slot(0) + 0x10), u(slot(0) + 0x518)), (0xFF11_2233, 1));
        // the recorded machine's own car at index 1: its record, then the driver's name
        assert_eq!(&f[slot(1) + 0x228..slot(1) + 0x22C], b"4321");
        assert_eq!(u(slot(1) + 0x328), 2);
        assert_eq!(&f[slot(1) + 0x17C..slot(1) + 0x180], b"ANNA");
        // the traffic car after the two humans: `.TYPE` 0xB, `.CLASS` 4, `.FILENAME`, `.COLOUR`
        assert_eq!((u(slot(2)), u(slot(2) + 8), u(slot(2) + 0x10)), (0xB, 4, 0xFF8C_8C90));
        assert_eq!(&f[slot(2) + 0x228..slot(2) + 0x22C], b"sbus");
        // nothing lands in the slots past the traffic
        assert!(f[slot(3)..slot(9)].iter().all(|b| *b == 0));
    }
}
