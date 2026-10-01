//! The **input-exchange** race — `Comms.Positional` = 0, FEStrings 281 (`RE/notes/network.md` §11.17).
//!
//! Nothing about a car crosses the wire. Every machine sends what its controller says, the host's
//! room collects the eight players' controllers into one 24-byte **frame** sixteen times a second and
//! sends every frame, in order, to every player; each machine puts the frame's inputs into a per-car
//! ring and runs the same deterministic simulation on them, one game tick at a time, only as far as
//! the frames it has received allow. All of it is transcribed from the disassembly; the addresses are
//! on each item.
//!
//! | part | original | here |
//! |---|---|---|
//! | the sample → 3 bytes, 16 Hz | `NfsNet` input sender 0x48EB20 (tail of 0x413800 → 0x48CA00) | [`Sender`] |
//! | the room's 24-byte frames, 16 Hz | room tick 0x48B6B0, 0x48C2B0 (a player's 3 bytes), 0x48BF10 / 0x48BFB0 (start) | [`Server`] |
//! | a frame → two ring entries per car | receiver 0x48F0DC…0x48F30F, ring 0x415E00 / 0x415E80 / 0x415EF0 | [`Client::receive`], [`Ring`] |
//! | which tick may run | frame driver 0x4152E0 (0x415455…0x415495) | [`Client::allowed_ticks`] |
//! | the 128 Hz timer that counts ticks against the host's clock | 0x4A45D0, 0x48D840, 0x48A9B0 / 0x48A9D0 | [`Timer`], [`Clock`] |

use std::collections::VecDeque;

/// The ring holds 128 entries per car — the index is masked with `0x7F` (0x415E8F, 0x415EFB).
pub const RING: usize = 128;
/// Eight players, `0x200` bytes of ring each (`0x415E92` `shl esi, 7`, ×4 bytes).
pub const SEATS: usize = 8;
/// One controller sample: the four bytes `0x5E99DC…` / `0x608B14 + 4·p` — `[buttons | command << 3,
/// steer (i8), throttle, brake]` (`replay.md` §2, `nfs_game::replay::Packet`).
pub type Sample = [u8; 4];
/// One player's block of a frame: the sender's three bytes as [`Sender::sample`] packed them.
pub type Block = [u8; 3];
/// A frame: eight blocks (`0x48C2B0`: `room + 0xB0 + 24·frame + 3·player`).
pub const FRAME: usize = 24;

/// What a player who is in the room but not in the race gets in its block (0x48B89A…0x48B8A7:
/// `8, 0, 0x1F`) — command 1 (`8 >> 3`), no steer, no throttle, brake `0x1F << 2`. Command 1 is what
/// 0x416310 (0x4163B0) reads as "this racer has left" (`car+0xD8B == 1`).
pub const ABSENT: Block = [8, 0, 0x1F];
/// The command code that means "left the race" (see [`ABSENT`]).
pub const CMD_LEFT: u8 = 1;
/// The command code the reset request 0x40F7EB reads (`car+0xD8B == 5`, `physics.md` §7).
pub const CMD_RESET: u8 = 5;

/// The server's frame clock: one frame every `1000 / 16` ms — the room tick's `now << 4` against a
/// counter that steps by `0x3E8` (0x48B7CD, 0x48B8C4).
pub const FRAMES_PER_SECOND: u32 = 16;

// ---------------------------------------------------------------------------------------------
// the ring — 0x415E00 / 0x415E80 / 0x415EF0
// ---------------------------------------------------------------------------------------------

/// The per-car input ring `0x606AF0 + 0x200·p`, its "filled" flags `0x607AF0`, the write counters
/// `0x608AF4` and the latch `0x608B14`.
#[derive(Debug, Clone)]
pub struct Ring {
    data: Box<[[Sample; RING]; SEATS]>,
    full: Box<[[bool; RING]; SEATS]>,
    /// `0x608AF4[p]`
    write: [u32; SEATS],
    /// `0x608AF0`, the number of frames latched
    frames: u32,
    /// `0x608B14 + 4·p` — what the last [`Ring::latch`] took
    pub latched: [Sample; SEATS],
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

impl Ring {
    pub fn new() -> Self {
        let mut r = Self {
            data: Box::new([[[0; 4]; RING]; SEATS]),
            full: Box::new([[false; RING]; SEATS]),
            write: [0; SEATS],
            frames: 0,
            latched: [[0; 4]; SEATS],
        };
        r.reset();
        r
    }

    /// **0x415E00** — the race frame's first act (0x41537B): every entry cleared, entry 0 of every
    /// car marked full (`setl bl` on `ecx < 1`, 0x415E15), the write counters at 1, the frame count
    /// at 0. So **the first frame the game runs on is a zero input for everybody** and the network's
    /// first frame lands in entry 1.
    pub fn reset(&mut self) {
        for p in 0..SEATS {
            for c in 0..RING {
                self.data[p][c] = [0; 4];
                self.full[p][c] = c < 1;
            }
            self.write[p] = 1;
        }
        self.frames = 0;
        self.latched = [[0; 4]; SEATS];
    }

    /// **0x415E80(p, sample)** — store at `write[p] & 0x7F` when that entry is free (`[0x607AF0 + …] == 0`),
    /// mark it full and step the counter; answers `false` when the entry was still full, i.e. the
    /// reader is 128 entries behind. (The split-screen arm's `0x5CC818` bump — `[0x657404] == 1` —
    /// is not this mode's.)
    pub fn push(&mut self, p: usize, s: Sample) -> bool {
        let i = (self.write[p] as usize) & (RING - 1);
        if self.full[p][i] {
            return false;
        }
        self.data[p][i] = s;
        self.full[p][i] = true;
        self.write[p] = self.write[p].wrapping_add(1);
        true
    }

    /// **0x415EF0(frame)** — at every even game tick `t` the frame driver latches entry `t / 2`
    /// (0x415BD4…): the first `players` cars' samples go to `0x608B14 + 4·p`, their "full" flags are
    /// cleared (0x415F37…0x415F4A) and the frame count steps.
    pub fn latch(&mut self, frame: u32, players: usize) {
        let i = (frame as usize) & (RING - 1);
        for p in 0..players.min(SEATS) {
            self.latched[p] = self.data[p][i];
        }
        for p in 0..players.min(SEATS) {
            self.full[p][i] = false;
        }
        self.frames = self.frames.wrapping_add(1);
    }

    /// How many entries car `p` has in the ring that nobody has taken.
    pub fn pending(&self, p: usize) -> usize {
        self.full[p].iter().filter(|f| **f).count()
    }
}

// ---------------------------------------------------------------------------------------------
// the sender — 0x48EB20
// ---------------------------------------------------------------------------------------------

/// The player's side of the exchange: `net+0xEC`, `+0x14C…0x14F`, the command list `+0x150`, the
/// counter `+0xF4` and the round-trip bookkeeping `+0xFD`, `+0x105`, `+0x108`, `+0x128`.
#[derive(Debug, Clone, Default)]
pub struct Sender {
    /// `net+0xEC` — 0 when the next sample starts a pair, non-zero after a packet went out (the `0xB3`
    /// handler sets it to 3, 0x48F83D)
    count: u32,
    /// `net+0x14C…0x14F`: steer, throttle, brake, buttons (`& 7`) of the first sample of the pair
    avg: [u8; 4],
    /// `net+0x150` — the command codes (`sample[0] >> 3`) waiting to go out, one per packet
    commands: VecDeque<u8>,
    /// `net+0xF4` — samples taken
    pub taken: u32,
    /// `net+0xFD + k` — the frame bit of player `k` (see [`Client::receive`])
    pub bits: [bool; SEATS],
    /// `net+0x105`
    pub armed: bool,
    /// `net+0x108 + 4k` — `GetTickCount()` at the toggle (own) or the last change (others')
    pub stamp: [u32; SEATS],
    /// `net+0x128 + 4k` — the round trip `0x490140`'s non-positional arm reads
    pub latency: [u32; SEATS],
}

impl Sender {
    /// The reset the `0xB3` handler does (0x48F82F…0x48F896): `+0xEC = 3`, the bits, the stamps and
    /// the latencies cleared, `+0x105 = 1`.
    pub fn on_start(&mut self) {
        self.count = 3;
        self.taken = 0;
        self.bits = [false; SEATS];
        self.stamp = [0; SEATS];
        self.latency = [0; SEATS];
        self.armed = true;
    }

    /// **0x48EB20** — one 32 Hz sample of the local controller `s` (`[0x5E99DC…]`), for player `me`
    /// at `now` (`GetTickCount`). Every command code in the sample is queued (0x48EB60…0x48EB8D).
    /// The first sample of a pair is stored (0x48ED73…0x48EDB7: steer, throttle, brake and `buttons
    /// & 7`, `+0xEC = 0`); the second is **averaged** with it (steer as a signed byte rounded toward
    /// zero, the others as `(a + b) >> 1`, 0x48EBA6…0x48EBFC), the two samples' buttons ORed, and
    /// packed into three bytes (0x48EC5E…0x48ECE9):
    ///
    /// ```text
    /// b0   buttons | (the queue's first command) << 3     — bit 2 forced set (below)
    /// b1   (steer & 0xFE) << 1  +  (throttle >> 5) & 3
    /// b2   (throttle & 0xFC) << 3  +  (brake >> 2) & 0x1F
    /// ```
    ///
    /// so the wire keeps steer to six bits, throttle and brake to five (bits 6…2). Bit 2 of `b0` is
    /// `(this + 0xFD) != 0` (0x48ED38 `lea` / `neg` / `sbb` / `and 4`): the address of the frame-bit
    /// array, not one of its bytes, so it is **always** set — the original's own slip, which is why
    /// the round trip below is measured once.
    ///
    /// Answers the three bytes when a packet is due (every second call — 16 Hz), else `None`.
    pub fn sample(&mut self, s: Sample, me: usize, now: u32) -> Option<Block> {
        // 0x48EB5D…0x48EB92: a non-zero command code is queued whichever arm follows
        let cmd = s[0] >> 3;
        if cmd != 0 {
            self.commands.push_back(cmd);
        }
        if self.count != 0 {
            // 0x48ED73: the store arm
            self.avg = [s[1], s[2], s[3], s[0] & 7];
            self.count = 0;
            self.taken = self.taken.wrapping_add(1);
            return None;
        }
        // 0x48EBA6: signed steer, `(a + b) / 2` toward zero (`cdq; sub eax, edx; sar eax, 1`)
        let sum = i32::from(self.avg[0] as i8) + i32::from(s[1] as i8);
        self.avg[0] = ((sum - (sum >> 31)) >> 1) as u8;
        self.avg[1] = ((u32::from(self.avg[1]) + u32::from(s[2])) >> 1) as u8;
        self.avg[2] = ((u32::from(self.avg[2]) + u32::from(s[3])) >> 1) as u8;
        self.avg[3] |= s[0] & 7;
        let mut b = [self.avg[3], 0, 0];
        if let Some(c) = self.commands.pop_front() {
            b[0] |= c << 3;
        }
        // 0x48EC5E…0x48ECE9
        b[1] = ((self.avg[0] & 0xFE) << 1).wrapping_add((self.avg[1] >> 5) & 3);
        b[2] = ((self.avg[1] & 0xFC) << 3).wrapping_add((self.avg[2] >> 2) & 0x1F);
        self.count = 3;
        // 0x48ECF9…0x48ED2E: the frame bit and the stamp
        if self.armed {
            self.bits[me] = !self.bits[me];
            self.stamp[me] = now;
            self.armed = false;
        }
        b[0] = (b[0] & 0xFB) | 4;
        self.taken = self.taken.wrapping_add(1);
        Some(b)
    }
}

/// **0x48F1F8…0x48F250** — a block back into the sample the ring takes. `second` is the frame's
/// second entry (0x48F249 keeps `& 7`, the first `& 0xFB`): the command bits ride the first only.
///
/// ```text
/// [0] b0 & 0xFB   (second: b0 & 7)
/// [1] (b1 as i8 >> 1) & 0xFE                    — steer
/// [2] ((b1 & 3) << 5) + ((b2 >> 3) & 0x1C)      — throttle
/// [3] (b2 & 0x1F) << 2                          — brake
/// ```
pub fn decode(b: Block, second: bool) -> Sample {
    [
        if second { b[0] & 7 } else { b[0] & 0xFB },
        (((b[1] as i8) >> 1) as u8) & 0xFE,
        ((b[1] & 3) << 5).wrapping_add((b[2] >> 3) & 0x1C),
        (b[2] & 0x1F) << 2,
    ]
}

// ---------------------------------------------------------------------------------------------
// the host's clock — 0x48A9B0 / 0x48A9D0 / 0x48AA00
// ---------------------------------------------------------------------------------------------

/// The clock object at `net+0x18`: `[0]` is an offset added to `GetTickCount()` (0x48AA00).
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    pub offset: i32,
}

impl Clock {
    /// **0x48AA00** — the synchronised time.
    pub fn now(&self, local: u32) -> u32 {
        local.wrapping_add(self.offset as u32)
    }

    /// **0x48A9B0(target)** — `offset = target − GetTickCount()`; the `0xB3` handler sets the clock
    /// to the host's `now` (0x48F8C4…0x48F8C8).
    pub fn set(&mut self, target: u32, local: u32) {
        self.offset = target.wrapping_sub(local) as i32;
    }

    /// **0x48A9D0(target)** — the clock only ever jumps forward: when `target − offset − now` is
    /// positive the offset is reset to `target − now`; when it is negative the offset steps down by
    /// **one millisecond**; zero leaves it.
    pub fn adjust(&mut self, target: u32, local: u32) {
        let d = target.wrapping_sub(self.offset as u32).wrapping_sub(local) as i32;
        if d > 0 {
            self.offset = target.wrapping_sub(local) as i32;
        } else if d < 0 {
            self.offset = self.offset.wrapping_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the 128 Hz timer — 0x4A45D0
// ---------------------------------------------------------------------------------------------

/// The state of the multimedia timer callback (network arm, `[0x6573E8] == 3`): the tick counters
/// `0x6559E8` (128 Hz), `0x6559EC` (64 Hz), `0x6559F0` (32 Hz), `0x6559F8` (64 Hz, cleared at the
/// race frame's start, 0x415302), the model `0x655A00` and its four-callback cool-down `0x6559F4`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timer {
    /// `0x6559E8`
    pub c128: u32,
    /// `0x6559EC`
    pub c64: u32,
    /// `0x6559F0`
    pub c32: u32,
    /// `0x6559F8` — the game's tick clock: the frame driver runs ticks only below it
    pub wall: u32,
    /// `0x655A00`, in units of `1000` per 128 Hz iteration
    model: i32,
    /// `0x6559F4`
    cool: u32,
}

impl Timer {
    /// **0x4A45D0**, one callback, with `ms` = 0x48D840's answer (the synchronised time less the start
    /// time `net+0x15C`, in milliseconds). The model is compared against `ms << 7`: a callback that
    /// finds it **ahead** runs no iteration (and waits four callbacks), one that finds it **more than
    /// 1000 behind** runs two (and waits four), any other runs one — so the 128 Hz iterations
    /// follow the host's clock, not this machine's. Each iteration counts 128 Hz, and on even ones 64
    /// Hz (`wall`), and every second of those a 32 Hz **input sample is due** (`0x412E70` /
    /// `0x413800`, 0x4A46CC…0x4A46D1). Answers the number of samples due.
    pub fn callback(&mut self, ms: i32) -> u32 {
        let d = (ms.wrapping_shl(7)).wrapping_sub(self.model);
        let mut iterations = 1i32;
        if self.cool != 0 {
            self.cool -= 1;
        } else if d < 0 {
            iterations = 0;
            self.cool = 4;
        } else if d > 0x3E8 {
            iterations = 2;
            self.cool = 4;
        }
        self.model = self.model.wrapping_add(iterations * 1000);
        let mut samples = 0;
        for _ in 0..iterations {
            self.c128 = self.c128.wrapping_add(1);
            if self.c128 & 1 == 0 {
                self.c64 = self.c64.wrapping_add(1);
                self.wall = self.wall.wrapping_add(1);
                if self.c64 & 1 == 0 {
                    self.c32 = self.c32.wrapping_add(1);
                    samples += 1;
                }
            }
        }
        samples
    }
}

// ---------------------------------------------------------------------------------------------
// the player's exchange state and the receiver — 0x48F0DC…0x48F30F
// ---------------------------------------------------------------------------------------------

/// What [`Client::receive`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// dropped by the gate (paused, `+0xBC` set) or too short to be a frame
    Dropped,
    /// taken; `overflow` when a ring entry was still full (0x415E80 answered 0)
    Taken { overflow: bool },
}

/// `NfsNet`'s exchange fields.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub ring: Ring,
    pub sender: Sender,
    pub clock: Clock,
    pub timer: Timer,
    /// `net+0xF0` — frame entries received (two per frame, 0x48F2E3)
    pub received: u32,
    /// `[0x5CC818]` — the input-frame counter (0x48F2F4: the same step)
    pub input_frames: u32,
    /// `net+0xC4` — waiting for the host's start message (set by `0xB3`, cleared by `0xC5`)
    pub waiting: bool,
    /// `net+0xBC` — no more frames or samples (set on an overflow, cleared by `0xB3`)
    pub stopped: bool,
    /// `net+0xC5` — a failure was reported (an overflow, 0x48F29B) or the barrier gave up
    pub failed: bool,
    /// `net+0x15C` — the host's clock at the start (`0xC5`'s word)
    pub start_ms: u32,
    /// `net+0x154` — the host's clock in the last heartbeat (`0xC4`), 0 before one
    pub beat_host: u32,
    /// `net+0x158` — `GetTickCount()` when it arrived
    pub beat_local: u32,
}

impl Client {
    pub fn new() -> Self {
        Self::default()
    }

    /// **0x48F82F**, the `0xB3` handler, as far as the exchange is concerned: the counters back to
    /// zero, the sender's reset, waiting for the start, and the clock set to the host's (0x48F8C8).
    /// The timer's own counters restart with the race frame (0x415302 clears `0x6559F8`).
    pub fn on_start(&mut self, host_now: u32, local: u32) {
        self.received = 0;
        self.stopped = false;
        self.failed = false;
        self.waiting = true;
        self.beat_host = 0;
        self.sender.on_start();
        self.clock.set(host_now, local);
    }

    /// **The race frame's first steps** (0x415376…0x415380): the ring is cleared, the tick counters
    /// start again (0x415302, 0x415E55: `[0x5CC818] = 0`).
    pub fn enter_race(&mut self) {
        self.ring.reset();
        self.input_frames = 0;
        self.timer = Timer::default();
    }

    /// **0x48F8D2**, `0xC5` with a word while [`Client::waiting`]: the host's clock at the start.
    /// Answers whether it was taken (it is not when nobody was waiting — that is the lobby's `0xC5`).
    pub fn on_go(&mut self, host_now: u32) -> bool {
        if !self.waiting {
            return false;
        }
        self.waiting = false;
        self.start_ms = host_now;
        true
    }

    /// **0x48F92A**, `0xC4`: the host's clock and ours when it arrived.
    pub fn on_beat(&mut self, host_now: u32, local: u32) {
        self.beat_host = host_now;
        self.beat_local = local;
    }

    /// **0x48EF2C…0x48EF85**, once per 128 Hz callback: pull the clock toward the last heartbeat's
    /// time carried forward by the local time since it came.
    pub fn update_clock(&mut self, local: u32) {
        if self.beat_host == 0 {
            return;
        }
        let target = self.beat_host.wrapping_sub(self.beat_local).wrapping_add(local);
        self.clock.adjust(target, local);
    }

    /// **0x48D840** — milliseconds since the start on the synchronised clock; 0 while waiting.
    pub fn since_start(&self, local: u32) -> i32 {
        if self.waiting {
            return 0;
        }
        self.clock.now(local).wrapping_sub(self.start_ms) as i32
    }

    /// **0x48F0DC…0x48F30F** — a frame from the host (a datagram whose first byte is below `0xB0`,
    /// eight blocks of three bytes). `me` is our own player id (`net+0xDC`), `car(k)` the race index
    /// (`player+0x4C`) of the player with id `k`, if it is on `net+0x260` and in the race, and `now`
    /// `GetTickCount()`.
    ///
    /// First the round-trip pass (0x48F11B…0x48F192, see [`Sender::bits`]); then, **twice** (0x48F309
    /// `cmp eax, 2`), every player's block is decoded ([`decode`]; the second time without the
    /// command bits) and pushed into that car's ring (0x48F256 → 0x415E80), and both counters step.
    /// A ring that is still full (the reader is 128 entries behind) ends the exchange for this
    /// machine: `+0xC5 = 1`, `+0xBC = 1`, `+0xC4 = 0` (0x48F29B…0x48F2C0) — and the game then reports
    /// it (0x48DA60(−2, −1), `0xB7`).
    pub fn receive(&mut self, pkt: &[u8], me: usize, car: impl Fn(usize) -> Option<usize>, now: u32) -> Received {
        if self.stopped || pkt.len() < FRAME {
            return Received::Dropped;
        }
        let s = &mut self.sender;
        for k in 0..SEATS {
            let cl = pkt[3 * k] & 4 == 4;
            let al = s.bits[k];
            if k == me {
                if cl == al && !s.armed {
                    s.latency[k] = now.wrapping_sub(s.stamp[k]);
                    s.armed = true;
                }
            } else if cl == al {
                s.bits[k] = !al;
                if (s.stamp[k] as i32) > 0 {
                    s.latency[k] = now.wrapping_sub(s.stamp[k]);
                }
                s.stamp[k] = now;
            }
        }
        let mut overflow = false;
        for second in [false, true] {
            for k in 0..SEATS {
                let Some(p) = car(k).filter(|p| *p < SEATS) else { continue };
                let b = [pkt[3 * k], pkt[3 * k + 1], pkt[3 * k + 2]];
                if !self.ring.push(p, decode(b, second)) && !self.failed {
                    self.failed = true;
                    self.stopped = true;
                    self.waiting = false;
                    overflow = true;
                }
            }
            self.received = self.received.wrapping_add(1);
            self.input_frames = self.input_frames.wrapping_add(1);
        }
        Received::Taken { overflow }
    }

    /// **The frame driver's target** (0x415455…0x415495, `[0x657408] == 0`, connected): the tick
    /// counter's `min(0x6559F8, 2·F0 + 1) − 1` where `F0` is [`Client::received`]; the loop runs
    /// ticks while `ticks_done <= target` (0x415BD4). Answers how many ticks in all may have run,
    /// i.e. `target + 1`; `None` is "none yet".
    pub fn allowed_ticks(&self) -> u32 {
        self.timer.wall.min(2 * self.received + 1)
    }
}

// ---------------------------------------------------------------------------------------------
// the host's room — 0x48B6B0
// ---------------------------------------------------------------------------------------------

/// What the room tick wants sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// `0xC4` to everyone, unreliable (0x48B71E…0x48B75E): `[0xC4, 0, 0, 0, now BE]`
    Beat { now: u32 },
    /// one frame to player `slot`, reliable (0x48B930…0x48B961)
    Frame { slot: usize, bytes: [u8; FRAME] },
    /// a player 128 frames behind: `0xC6 (4)` to it, and it is dropped (0x48B872…0x48B893)
    Drop { slot: usize },
}

/// The room object's exchange fields: the frame ring `+0xB0`, the head `+0xCB0`, the clocks
/// `+0xCB4 / +0xCB8 / +0xCBC` and the masks `+0xCC0…0xCC4`, plus each player's `+0xA8`.
#[derive(Debug, Clone)]
pub struct Server {
    frames: Box<[[u8; FRAME]; RING]>,
    /// `+0xCB0` — the index of the frame being filled
    pub head: u32,
    /// `+0xCB4` — the next heartbeat, in ms
    next_beat: i32,
    /// `+0xCB8` — the frame clock, `now << 4` units, one frame per `0x3E8`
    clock16: i32,
    /// `+0xCBC` — the send clock, `now << 5` units, one catch-up entry per player per `0x3E8`
    clock32: i32,
    /// `+0xCC0` — the players in the room (bit = slot)
    pub joined: u8,
    /// `+0xCC1` — the players whose first sample has come (0x48C2ED)
    got: u8,
    /// `+0xCC2` — the players **not** in the race (set on admission, cleared by their `0xB3`)
    pub not_ready: u8,
    /// `+0xCC3` — a race has started
    pub started: bool,
    /// `+0xCC4` — the exchange runs (`0xC5` came)
    pub running: bool,
    /// `+0xA8` of each player: the next frame to send it
    next: [u32; SEATS],
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Self {
            frames: Box::new([[0; FRAME]; RING]),
            head: 0,
            next_beat: 0,
            clock16: 0,
            clock32: 0,
            joined: 0,
            got: 0,
            not_ready: 0xFF,
            started: false,
            running: false,
            next: [0; SEATS],
        }
    }

    /// **0x48BB66** — a player comes in: its bit in `+0xCC0` and in `+0xCC2` (not in the race yet).
    pub fn join(&mut self, slot: usize) {
        self.joined |= 1 << slot;
        self.not_ready |= 1 << slot;
    }

    /// **0x48BDDA…0x48BDFF** — a player goes: its bit leaves `+0xCC0`.
    pub fn leave(&mut self, slot: usize) {
        self.joined &= !(1 << slot);
        self.not_ready &= !(1 << slot);
    }

    /// **0x48BF10** — a player's `0xB3`: it is in the race (its `+0xCC2` bit cleared, its next frame
    /// 0). The first one starts the room: the frame ring zeroed (`0xC00` bytes), the head, `+0xCC1`
    /// and `+0xCC4` cleared, the heartbeat clock set, and **`0xB3` (0, now)** goes to everybody.
    /// Answers `Some(now)` when that message is due.
    pub fn ready(&mut self, slot: usize, now: u32) -> Option<u32> {
        self.not_ready &= !(1 << slot);
        self.next[slot] = 0;
        if self.started {
            return None;
        }
        self.started = true;
        self.running = false;
        for f in self.frames.iter_mut() {
            *f = [0; FRAME];
        }
        self.head = 0;
        self.got = 0;
        self.next_beat = now as i32;
        Some(now)
    }

    /// **0x48BFB0** — a player's `0xC5` (0x48D3A0, at the race frame's start): the first one starts the
    /// exchange — `+0xCB8 = now << 4`, `+0xCBC = now << 5` — and `0xC5 (0, now)` goes to everybody.
    /// Answers `Some(now)` when that message is due.
    pub fn go(&mut self, now: u32) -> Option<u32> {
        if self.running {
            return None;
        }
        self.clock16 = (now as i32).wrapping_shl(4);
        self.clock32 = (now as i32).wrapping_shl(5);
        self.running = true;
        Some(now)
    }

    /// **0x48C2B0(sample, slot)** — a player's three bytes go into the block of the frame being
    /// filled, and the player's bit into `+0xCC1`.
    pub fn input(&mut self, slot: usize, block: Block) {
        let f = &mut self.frames[(self.head as usize) & (RING - 1)];
        f[3 * slot..3 * slot + 3].copy_from_slice(&block);
        self.got |= 1 << slot;
    }

    /// **0x48C220(player, −2, −1)** — a player leaves the race (its `0xB7`, or the room's own call
    /// when it goes, 0x48BDCE): it is not in the race again; and when nobody is left in it the room
    /// is no longer started (0x48C273), so the next race starts from the top.
    pub fn out_of_race(&mut self, slot: usize) {
        self.not_ready |= 1 << slot;
        if self.joined & self.not_ready == self.joined {
            self.started = false;
            self.running = false;
        }
    }

    /// **0x48B6B0**, one call (the original runs it from the 128 Hz timer while a room is started;
    /// each call catches up on whatever is due, so the rate it is called at changes nothing but when
    /// a frame is sent). `players` are the slots on the room's list, in list order; `now` is the
    /// host's `GetTickCount()`.
    ///
    /// 1. every second a `0xC4` heartbeat with `now` (0x48B6DD…0x48B782; one per second **owed**, all
    ///    carrying the same `now`);
    /// 2. once every player in the race (`joined & !not_ready`) has sent a first sample (`+0xCC1`,
    ///    0x48B7A5…0x48B7BF) and the exchange runs: a new frame for every 1/16 s owed
    ///    (`now << 4` against `+0xCB8`, 0x48B7C5…0x48B8DC): the head frame is copied to the next
    ///    entry, every player's command bits are cleared in it (`& 7`), a player not in the race gets
    ///    [`ABSENT`], and a player in the race whose next frame has fallen 128 behind (`next ^ head`
    ///    is a multiple of 128, 0x48B86D) is dropped with `0xC6 (4)`;
    /// 3. for every 1/32 s owed (`now << 5` against `+0xCBC`, 0x48B8E0…0x48B99C): each player in the
    ///    race that is behind gets **one** more frame — `next` < head — and its counter steps
    ///    **only when the link took the message**: 0x48B959 calls 0x55C270 and `test eax, eax ;
    ///    je 0x48B980` (0x48B961) leaves the whole list loop on a 0 — the counter stays, and no
    ///    later player is served that 1/32 s (the clock still advances, 0x48B980…0x48B99C).
    ///    `can_send(slot)` answers what 0x595C40 would (a packet free on that link, 0x595C58) and is
    ///    asked once per frame; the host's own player (no link) always takes it.
    pub fn tick(&mut self, now: u32, players: &[usize], can_send: &mut dyn FnMut(usize) -> bool) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.started || self.joined == 0 {
            return out;
        }
        // 0x48B6DD…0x48B797
        let ms = now as i32;
        if ms.wrapping_sub(self.next_beat) > 0 {
            loop {
                out.push(Out::Beat { now });
                self.next_beat = self.next_beat.wrapping_add(1000);
                if ms.wrapping_sub(self.next_beat) <= 0 {
                    break;
                }
            }
        }
        if !self.running {
            return out;
        }
        // 0x48B7A5…0x48B7BF
        let active = self.joined & !self.not_ready;
        if self.got & active != active {
            return out;
        }
        // 0x48B7C5…0x48B8DC
        let t16 = ms.wrapping_shl(4);
        let mut dropped: Vec<usize> = Vec::new();
        while t16.wrapping_sub(self.clock16) > 0 {
            let cur = (self.head as usize) & (RING - 1);
            self.head = self.head.wrapping_add(1);
            let nxt = (self.head as usize) & (RING - 1);
            self.frames[nxt] = self.frames[cur];
            for &k in players {
                if dropped.contains(&k) {
                    continue;
                }
                let f = &mut self.frames[nxt];
                f[3 * k] &= 7;
                if self.not_ready & (1 << k) != 0 {
                    f[3 * k..3 * k + 3].copy_from_slice(&ABSENT);
                } else if (self.next[k] ^ self.head) & 0x7F == 0 {
                    out.push(Out::Drop { slot: k });
                    dropped.push(k);
                }
            }
            self.clock16 = self.clock16.wrapping_add(1000);
        }
        // 0x48B8E0…0x48B99C
        let t32 = ms.wrapping_shl(5);
        while t32.wrapping_sub(self.clock32) > 0 {
            for &k in players {
                if dropped.contains(&k) || self.not_ready & (1 << k) != 0 {
                    continue;
                }
                if self.next[k] != self.head {
                    if !can_send(k) {
                        break; // 0x48B963
                    }
                    out.push(Out::Frame { slot: k, bytes: self.frames[(self.next[k] as usize) & (RING - 1)] });
                    self.next[k] = self.next[k].wrapping_add(1);
                }
            }
            self.clock32 = self.clock32.wrapping_add(1000);
        }
        out
    }

    /// The block of the frame at `index` for `slot` (a test's and a log's view).
    pub fn block(&self, index: u32, slot: usize) -> Block {
        let f = &self.frames[(index as usize) & (RING - 1)];
        [f[3 * slot], f[3 * slot + 1], f[3 * slot + 2]]
    }
}

// ---------------------------------------------------------------------------------------------
// the messages
// ---------------------------------------------------------------------------------------------

/// `0xC4`, the heartbeat: `0x48C870` builds `[0xC4, 0, 0, 0]` and the word — the host's clock — at
/// `+4` (0x48B71E…0x48B73A).
pub fn beat(now: u32) -> Vec<u8> {
    crate::room::encode(0xC4, Some(0), Some(now), None)
}

/// `0xC5` from a player to the host (0x48D3A0): the type byte alone (`value` and `word` omitted).
pub fn go_request() -> Vec<u8> {
    crate::room::encode(0xC5, None, None, None)
}

/// `0xC5` from the host to everybody (0x48BFDC…0x48BFEA): `(0, now)` — the exchange starts.
pub fn go(now: u32) -> Vec<u8> {
    crate::room::encode(0xC5, Some(0), Some(now), None)
}

/// `0xB3` from a player to the host (0x48D370): the request that puts it in the race.
pub fn ready_request() -> Vec<u8> {
    crate::room::encode(0xB3, None, None, None)
}

/// `0xB7 (id, word)` — a player's report (0x48C220 to everyone, 0x48DA60 from the player); the word
/// `−2` is "left the race".
pub fn report(id: i32, word: i32) -> Vec<u8> {
    crate::room::encode(0xB7, Some(id), Some(word as u32), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_the_link_refuses_stays_and_ends_the_players_loop() {
        // 0x48B963: `je 0x48B980` — the counter does not step and no later player is served
        let mut s = Server::new();
        s.join(0);
        s.join(1);
        s.join(2);
        s.ready(0, 1000);
        s.ready(1, 1000);
        s.ready(2, 1000);
        s.go(1000);
        for k in 0..3 {
            s.input(k, [0, 0, 0]);
        }
        let players = [0usize, 1, 2];
        // a second later: 16 frames are due to be made and 32 entries to be sent
        let out = s.tick(1070, &players, &mut |k| k != 1);
        // player 0 was served, player 1 refused: player 2 was not looked at in that pass
        let frames: Vec<usize> = out.iter().filter_map(|o| if let Out::Frame { slot, .. } = o { Some(*slot) } else { None }).collect();
        assert!(frames.contains(&0) && !frames.contains(&1) && !frames.contains(&2), "{frames:?}");
        // and the refused one's counter did not move
        let out = s.tick(1140, &players, &mut |_| true);
        assert!(out.iter().any(|o| matches!(o, Out::Frame { slot: 1, .. })));
    }

    #[test]
    fn a_pair_of_samples_is_stored_then_averaged_and_packed_into_three_bytes() {
        let mut s = Sender::default();
        s.on_start();
        // `+0xEC` is 3 after a `0xB3`: the first sample is stored, no packet
        assert_eq!(s.sample([1, 40u8, 100, 20], 0, 0), None);
        // the second is averaged with it: steer (40 + 60) / 2 = 50, throttle (100 + 120) / 2 = 110,
        // brake (20 + 30) / 2 = 25; the buttons of both, bit 2 forced on
        let b = s.sample([2 | (2 << 3), 60u8, 120, 30], 0, 5).unwrap();
        assert_eq!(b[0], 0x10 | 3 | 4);
        assert_eq!(b[1], ((50u8 & 0xFE) << 1) + ((110 >> 5) & 3));
        assert_eq!(b[2], ((110u8 & 0xFC) << 3).wrapping_add((25 >> 2) & 0x1F));
        // and the next pair starts again with a store
        assert_eq!(s.sample([0, 0, 0, 0], 0, 9), None);
    }

    #[test]
    fn a_negative_steer_average_rounds_toward_zero() {
        let mut s = Sender::default();
        s.on_start();
        s.sample([0, (-3i8) as u8, 0, 0], 0, 0);
        let b = s.sample([0, (-4i8) as u8, 0, 0], 0, 0).unwrap();
        // (−3 + −4) = −7 → −3 (toward zero), stored as 0xFD; `& 0xFE` = 0xFC; `<< 1` = 0xF8
        assert_eq!(b[1] & 0xFC, 0xF8);
        let d = decode(b, false);
        // the receiver's `sar 1` puts it back: 0xFC as a signed byte is −4
        assert_eq!(d[1] as i8, -4);
    }

    #[test]
    fn the_wire_keeps_six_bits_of_steer_and_five_of_throttle_and_brake() {
        let mut s = Sender::default();
        s.on_start();
        s.sample([0, 63, 0x7E, 0x7E], 0, 0);
        let b = s.sample([0, 63, 0x7E, 0x7E], 0, 0).unwrap();
        let d = decode(b, false);
        assert_eq!(d[1] as i8, 62);
        assert_eq!(d[2], 0x7C);
        assert_eq!(d[3], 0x7C);
        // a full left lock (−63) comes back as −64: `& 0xFE` on 0xC1 is 0xC0
        let mut s = Sender::default();
        s.on_start();
        s.sample([0, (-63i8) as u8, 0, 0], 0, 0);
        let d = decode(s.sample([0, (-63i8) as u8, 0, 0], 0, 0).unwrap(), false);
        assert_eq!(d[1] as i8, -64);
    }

    #[test]
    fn the_second_entry_of_a_frame_keeps_no_command_and_the_first_no_bit_2() {
        let b = [4 | 1 | (5 << 3), 0, 0];
        assert_eq!(decode(b, false)[0], 1 | (5 << 3));
        assert_eq!(decode(b, true)[0], 1 | 4);
    }

    #[test]
    fn a_command_goes_out_in_the_next_packet_and_only_one_per_packet() {
        let mut s = Sender::default();
        s.on_start();
        // two commands in one pair: the packet carries the first, the second waits
        assert_eq!(s.sample([2 << 3, 0, 0, 0], 0, 0), None);
        let a = s.sample([3 << 3, 0, 0, 0], 0, 0).unwrap();
        assert_eq!(a[0] >> 3, 2);
        assert_eq!(s.sample([0, 0, 0, 0], 0, 0), None);
        let b = s.sample([0, 0, 0, 0], 0, 0).unwrap();
        assert_eq!(b[0] >> 3, 3);
    }

    #[test]
    fn the_ring_starts_with_one_zero_entry_and_refuses_a_write_over_an_unread_one() {
        let mut r = Ring::new();
        // entry 0 is full, the write counter is at 1
        assert_eq!(r.pending(0), 1);
        for i in 1..RING {
            assert!(r.push(0, [i as u8, 0, 0, 0]), "entry {i}");
        }
        // the ring is full: entry 0 is still there
        assert!(!r.push(0, [9, 9, 9, 9]));
        r.latch(0, 1);
        assert_eq!(r.latched[0], [0, 0, 0, 0]);
        assert!(r.push(0, [9, 9, 9, 9]));
        r.latch(1, 1);
        assert_eq!(r.latched[0], [1, 0, 0, 0]);
    }

    fn frame_of(blocks: &[(usize, Block)]) -> [u8; FRAME] {
        let mut f = [0u8; FRAME];
        for (k, b) in blocks {
            f[3 * k..3 * k + 3].copy_from_slice(b);
        }
        f
    }

    #[test]
    fn a_frame_gives_every_car_two_ring_entries_and_counts_two() {
        let mut c = Client::new();
        c.enter_race();
        c.on_start(1000, 1000);
        let f = frame_of(&[(0, [4 | (2 << 3), 0x10, 0x20]), (1, [4, 0, 0])]);
        let got = c.receive(&f, 0, |k| (k < 2).then_some(k), 0);
        assert_eq!(got, Received::Taken { overflow: false });
        assert_eq!((c.received, c.input_frames), (2, 2));
        // entry 0 was the preloaded zero; the two decoded entries follow
        c.ring.latch(0, 2);
        c.ring.latch(1, 2);
        assert_eq!(c.ring.latched[0][0], 2 << 3);
        c.ring.latch(2, 2);
        assert_eq!(c.ring.latched[0][0], 4, "the second entry has no command and keeps bit 2");
    }

    #[test]
    fn the_first_frame_measures_the_round_trip_once() {
        let mut c = Client::new();
        c.enter_race();
        c.on_start(0, 0);
        // our first packet: the sender toggles its bit and stamps
        c.sender.sample([0; 4], 1, 100);
        c.sender.sample([0; 4], 1, 100);
        assert!(c.sender.bits[1]);
        assert_eq!(c.sender.stamp[1], 100);
        // the frame carries bit 2 in our block: it matches, the latency is now − stamp
        let f = frame_of(&[(1, [4, 0, 0])]);
        c.receive(&f, 1, Some, 180);
        assert_eq!(c.sender.latency[1], 80);
        assert!(c.sender.armed);
        // a later frame changes nothing: the bit we sent (always 1) no longer equals the toggled one
        c.sender.sample([0; 4], 1, 300);
        c.sender.sample([0; 4], 1, 300);
        assert!(!c.sender.bits[1]);
        c.receive(&f, 1, Some, 400);
        assert_eq!(c.sender.latency[1], 80);
    }

    #[test]
    fn a_full_ring_stops_the_exchange_for_that_machine() {
        let mut c = Client::new();
        c.enter_race();
        c.on_start(0, 0);
        let f = frame_of(&[(0, [4, 0, 0])]);
        // 64 frames fill the 127 free entries (2 each); the 64th overflows on its second entry
        let mut overflowed = false;
        for _ in 0..64 {
            if c.receive(&f, 0, |k| (k == 0).then_some(0), 0) == (Received::Taken { overflow: true }) {
                overflowed = true;
            }
        }
        assert!(overflowed);
        assert!(c.stopped && c.failed);
        assert_eq!(c.receive(&f, 0, |k| (k == 0).then_some(0), 0), Received::Dropped);
    }

    #[test]
    fn the_tick_target_is_the_smaller_of_the_wall_clock_and_the_frames_received() {
        let mut c = Client::new();
        c.enter_race();
        c.timer.wall = 100;
        assert_eq!(c.allowed_ticks(), 1, "no frame yet: only the first tick");
        c.received = 10;
        assert_eq!(c.allowed_ticks(), 21);
        c.received = 200;
        assert_eq!(c.allowed_ticks(), 100);
    }

    #[test]
    fn the_timer_runs_one_iteration_a_callback_and_counts_off_the_hosts_clock() {
        let mut t = Timer::default();
        // a machine exactly on time: one iteration per callback
        let mut samples = 0;
        for cb in 0..128i32 {
            // callback `cb` runs at cb × 7.8125 ms: `ms << 7` is 1000 × cb + 1000 ahead of the model
            let ms = (cb * 78125 + 39062) / 10000;
            samples += t.callback(ms);
        }
        assert_eq!(t.c128, 128);
        assert_eq!(t.wall, 64);
        assert_eq!(samples, 32, "a 32 Hz sample every 4th iteration");
        // a machine that is behind by more than one iteration runs two and then waits four callbacks
        let mut t = Timer::default();
        t.callback(1000);
        assert_eq!(t.c128, 2);
        t.callback(1000);
        assert_eq!(t.c128, 3, "the cool-down: one iteration whatever the clock says");
        // one that is ahead of the host's clock runs none
        let mut t = Timer::default();
        t.callback(0);
        assert_eq!(t.c128, 1);
        t.callback(0);
        assert_eq!(t.c128, 1);
    }

    #[test]
    fn the_synchronised_clock_only_moves_forward_or_one_millisecond_back() {
        let mut c = Clock::default();
        c.set(5000, 1000);
        assert_eq!(c.now(1000), 5000);
        // the host is 30 ms ahead of us now: jump
        c.adjust(5030 + 10, 1010);
        assert_eq!(c.now(1010), 5040);
        // the host is behind us: one millisecond off the offset
        let before = c.offset;
        c.adjust(5000, 1010);
        assert_eq!(c.offset, before - 1);
    }

    #[test]
    fn the_server_makes_a_frame_every_sixteenth_of_a_second_after_everyone_has_sent_once() {
        let mut s = Server::new();
        for k in 0..2 {
            s.join(k);
        }
        assert_eq!(s.ready(0, 1000), Some(1000));
        assert_eq!(s.ready(1, 1000), None);
        assert_eq!(s.go(1000), Some(1000));
        assert_eq!(s.go(1010), None);
        let players = [0usize, 1];
        // nobody has sent: no frames, but the heartbeat is due
        let out = s.tick(1005, &players, &mut |_| true);
        assert!(out.iter().all(|o| !matches!(o, Out::Frame { .. })));
        s.input(0, [4, 1, 2]);
        assert!(s.tick(1006, &players, &mut |_| true).is_empty(), "one of two has sent");
        s.input(1, [4 | 8, 3, 4]);
        // 1000 ms + 62.5 ms: the first frame is made and, at 1/32 s, sent
        let out = s.tick(1070, &players, &mut |_| true);
        let frames: Vec<_> = out.iter().filter(|o| matches!(o, Out::Frame { .. })).collect();
        assert_eq!(frames.len(), 2 * 2, "two frames were owed (the clock started at 1000), one per player per 1/32 s owed");
        // frame 0 is what was received into the head: the block of player 1 has its command bit
        match frames[0] {
            Out::Frame { slot, bytes } => {
                assert_eq!(*slot, 0);
                assert_eq!(&bytes[0..6], &[4, 1, 2, 4 | 8, 3, 4]);
            }
            _ => unreachable!(),
        }
        // the command bits are cleared in the copy: frame 1 carries `4 | 8 & 7 = 4` for player 1
        assert_eq!(s.block(1, 1)[0], 4);
    }

    #[test]
    fn a_player_not_in_the_race_gets_the_left_block_and_one_128_frames_behind_is_dropped() {
        let mut s = Server::new();
        for k in 0..2 {
            s.join(k);
        }
        s.ready(0, 0);
        s.go(0);
        s.input(0, [4, 0, 0]);
        // player 1 never sent and is not ready: the frame is made, its block is the absent one
        let out = s.tick(70, &[0, 1], &mut |_| true);
        assert!(out.iter().any(|o| matches!(o, Out::Frame { slot: 0, .. })));
        assert_eq!(s.block(1, 1), ABSENT);
        // now make player 1 ready and never let it read: at 128 frames it is dropped
        let mut s = Server::new();
        for k in 0..2 {
            s.join(k);
        }
        s.ready(0, 0);
        s.ready(1, 0);
        s.go(0);
        s.input(0, [4, 0, 0]);
        s.input(1, [4, 0, 0]);
        // a huge jump in time: 129 frames owed at once, so player 1's next (0) is 128 behind at the end
        let mut dropped = false;
        let out = s.tick(129 * 63, &[0, 1], &mut |_| true);
        for o in &out {
            if matches!(o, Out::Drop { .. }) {
                dropped = true;
            }
        }
        assert!(dropped, "{out:?}");
    }

    #[test]
    fn the_room_starts_again_when_everybody_has_left_the_race() {
        let mut s = Server::new();
        s.join(0);
        s.join(1);
        s.ready(0, 5);
        s.ready(1, 5);
        assert!(s.started);
        s.out_of_race(0);
        assert!(s.started);
        s.out_of_race(1);
        assert!(!s.started && !s.running);
        assert_eq!(s.ready(0, 9), Some(9));
    }
}
