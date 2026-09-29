//! Network play, wired into the race loop (`RE/notes/network.md` §5, §7).
//!
//! The original's `NfsNet_tick` (0x48EDF0) runs straight from the world tick and broadcasts the
//! 40-byte `0xBE` car state when `(gGameTick & 7) == 7`, i.e. at 8 Hz against the 64 Hz clock. The
//! receiving side drops a packet carrying its own id (0x48F5E8) and otherwise hands the block to
//! the player it names. That is exactly what this does; the session underneath is
//! [`nfs_net::session`].
//!
//! Between the received states the remote car is **dead-reckoned** exactly as the original's receiver
//! does it (`network.md` §8.4 / §11.18, ported by NM): the store 0x4A0DF0 with its lag book
//! ([`Remote::store`]), the 64 Hz apply 0x4A0A40 / 0x4A04A0 ([`NetPlay::apply`],
//! `nfs_physics::vehicle::netcar`). The packet, its gate, the quantisation and the id rule are the
//! original's too.
//!
//! What carries the packet is **not** the original's path. The original hands a game message to its
//! connection's data layer (§7.11) — `u16be length` over TCP, or a six- or four-byte frame over
//! UDP — and the `'<'` tag this port sends under is dead code there (H36, refuted). Two copies of
//! the port understand each other; the original will not.

use glam::Vec3;
use nfs_game::layout::loading::LoadingRow;
use nfs_net::msg::kind;
use nfs_net::room::{self, ChatLine, ChatRequest, Heard, Room, To};
use nfs_net::session::{Event, Net, Role, name16};
use nfs_renderer::{Renderer, TextureData, TextureId};
use nfs_net::socket::CommSocket;
use nfs_net::state::{CarState, TICK_MASK};
use nfs_physics::vehicle::Vehicle;
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::time::{Duration, Instant};

/// `NfsNet+0x2C`, the version string the session name carries after its `~` (0x48DDC2…0x48DDF8).
/// 0x48DBE0 reads the install's `version.txt` (`push 0x5D0790`, at most 5 bytes: 0x48DC77…0x48DC7C)
/// into it. **APPROXIMATION** for a missing / empty file: the executable's own `"3.5"` (0x5D06FC,
/// the literal 0x48A700 compares a peer's version with) — the original would keep what the
/// configuration read before it (0x4B7240(7), not traced).
pub fn game_version() -> String {
    nfs_assets::paths::GamePaths::locate()
        .ok()
        .and_then(|p| std::fs::read(p.root.join("version.txt")).ok())
        .map(|b| b.into_iter().take(5).take_while(|&c| c != 0).map(|c| c as char).collect::<String>())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "3.5".to_string())
}

/// The address record a host's `'d'` carries at `+0x04` — its machine's own address as 0x583899
/// builds `MYIPADDRESS` (`gethostname` → `gethostbyname` → the first `AF_INET` address), with the
/// session's port. **Platform glue:** the original's record is the socket's own (0.0.0.0 while
/// bound to every interface); a browser here merges the answers that reach it over several of the
/// host's interfaces on this record, so it has to name the machine. `None` when no address is known.
fn advertised_record() -> Option<nfs_net::wire::Addr> {
    let ip: Ipv4Addr = nfs_net::socket::my_ip_string().parse().ok()?;
    (!ip.is_unspecified() && !ip.is_loopback()).then(|| nfs_net::wire::Addr::ip(ip.octets(), port()))
}

/// The application's own identity, from **0x5B4980** — the 16-byte GUID
/// `{5CD46B81-ECE4-11D3-9B6A-00902724AB1E}` that 0x48AC27 hands the CONMAN constructor and
/// `0x55C370` stores at `conman+0xC0`. It is what the `EAGAME:%08x.%04x%04x.%02x…` format of §1
/// prints: `EAGAME:5cd46b81.ece411d3.9b6a0090.2724ab1e`, and `0x5B4990` carries `0x24E2` after it.
///
/// A `'w'` probe compares **32** bytes against `session+0x0C`, and how the original lays the GUID
/// into those 32 is **not traced** — so the port zero-pads it, which at least means two copies of
/// the port agree on the original's identity rather than on one I made up.
const KEY: [u8; 32] = [
    0x81, 0x6B, 0xD4, 0x5C, 0xE4, 0xEC, 0xD3, 0x11, 0x9B, 0x6A, 0x00, 0x90, 0x27, 0x24, 0xAB, 0x1E,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// **0x55FF60** — the next key of the keyboard ring, which the blocking waits test against ESC (0x1B):
/// PLAYGAME's wait (0x50CFD7) and the barrier's four (0x48CE6F, 0x48CFA7, 0x48D0C1, 0x48D248). The
/// original's loops pump its own window's messages; the port's do not run the event loop, so this is
/// **platform glue**: on macOS the OS key state of Escape (`CGEventSourceKeyState`) — not in a scripted
/// `NFS_NO_FOCUS` run, which has no user at the keyboard — and, for the tests, **TEST GLUE**
/// `NFS_NET_ESC_MS=<ms>` (an ESC that arrives that long after the wait began). Elsewhere: never.
#[derive(Debug, Clone, Copy)]
pub struct KeyRing {
    since: Instant,
}

impl KeyRing {
    pub fn begin() -> Self {
        Self { since: Instant::now() }
    }

    /// Whether the next key is ESC.
    pub fn esc(&self) -> bool {
        if let Some(ms) = std::env::var("NFS_NET_ESC_MS").ok().and_then(|v| v.parse::<u64>().ok())
            && self.since.elapsed() >= Duration::from_millis(ms)
        {
            return true;
        }
        os_escape_down()
    }
}

#[cfg(target_os = "macos")]
fn os_escape_down() -> bool {
    unsafe extern "C" {
        #[link_name = "CGEventSourceKeyState"]
        fn key_state(state_id: i32, key: u16) -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {}
    if crate::NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    // kCGEventSourceStateCombinedSessionState = 0, kVK_Escape = 0x35
    unsafe { key_state(0, 0x35) }
}

#[cfg(not(target_os = "macos"))]
fn os_escape_down() -> bool {
    false
}

/// **TEST GLUE**: `NFS_NET_LAG_MS`, the artificial delay of every car-state packet.
fn lag_glue() -> Option<u64> {
    std::env::var("NFS_NET_LAG_MS").ok().and_then(|v| v.parse().ok()).filter(|ms| *ms > 0)
}

/// One remote player's car as the receiver keeps it — the block `car+0x38…+0x5B` and the lag book
/// `car+0x5C…+0x70`, `+0xDA8`, `+0xDAC` that 0x4A0DF0 maintains (`network.md` §8.4).
#[derive(Debug, Clone)]
pub struct Remote {
    pub seat: usize,
    /// `+0x38…+0x5B`; `state.tick` is `+0x3C` (0 until a block has come)
    pub state: CarState,
    /// `+0x5C`: the smoothed lag, `(7·old + lag) / 8` (0x4A0EB1…0x4A0ED1)
    pub lag_avg: i32,
    /// `+0x60`: consecutive fresh blocks (lag below 0x20)
    pub fresh: i32,
    /// `+0x64`: consecutive late blocks (lag above 0x40)
    pub late: i32,
    /// `+0x68`: the last block's lag, `|now − tick|`, at least 1
    pub lag: i32,
    /// `+0x70`: the car is stale — the apply leaves it alone (`+0xDA8 = 1`)
    pub stale: bool,
    /// `+0xDAC`: a block has come (set by the first, 0x4A0E9F)
    pub got: bool,
    /// `+0xDA8`: the last apply found the car stale
    pub held: bool,
    /// 0x4A03D0 is due: the store found the lag past its limits with `+0x70` clear; the seat's car takes
    /// the block's velocity and matrix at the next [`NetPlay::store_snaps`]
    snap_due: bool,
}

impl Remote {
    pub fn new(seat: usize) -> Self {
        Self { seat, state: CarState::default(), lag_avg: 0, fresh: 0, late: 0, lag: 0, stale: false, got: false, held: false, snap_due: false }
    }

    /// **0x4A0DF0(car, block)** — the store. A block whose tick is not newer than `+0x3C` is dropped
    /// (0x4A0E04 `jl`, signed); otherwise it becomes the block, the lag `|now − tick|` (at least 1, 0x4A0E4B…)
    /// goes to `+0x68` and into the smoothed `+0x5C = (7·old + lag) / 8`, and:
    ///
    /// * a lag above 0x40 counts `+0x64` up and, once that passes 4 — or at once above 0x80 — the car is
    ///   stale (`+0x70 = 1`), the snap 0x4A03D0 first if it was not; a lag of 0x40 or less clears `+0x64`;
    /// * a lag below 0x20 counts `+0x60` up, and past 8 the car is fresh again (`+0x70 = 0`); otherwise `+0x60 = 0`;
    /// * the first block (`+0x3C` was 0) sets `+0xDAC`.
    ///
    /// Answers whether the block was taken. `now` is `gGameTick` `[0x606AC4]`.
    pub fn store(&mut self, blk: CarState, now: i32) -> bool {
        let old = self.state.tick as i32;
        if old >= blk.tick as i32 {
            return false;
        }
        self.state = blk;
        let age = now.wrapping_sub(blk.tick as i32);
        self.lag = age.abs().max(1);
        // `(x + (x >> 31 & 7)) >> 3` is the division truncating toward zero
        self.lag_avg = (self.lag_avg * 7 + self.lag) / 8;
        if self.lag < 0x41 {
            self.late = 0;
        } else {
            self.late += 1;
            if self.late > 4 || self.lag > 0x80 {
                if !self.stale {
                    self.snap_due = true;
                }
                self.stale = true;
            }
        }
        if self.lag < 0x20 {
            self.fresh += 1;
            if self.fresh > 8 {
                self.stale = false;
            }
        } else {
            self.fresh = 0;
        }
        if old == 0 {
            self.got = true;
        }
        true
    }

    /// The block as the apply 0x4A0A40 reads it (`+0x39`, `+0x3C`, `+0x40`, `+0x42`, `+0x4A`, `+0x50`).
    pub fn block(&self) -> nfs_physics::vehicle::NetBlock {
        let s = &self.state;
        nfs_physics::vehicle::NetBlock {
            tick: s.tick as i32,
            airborne: s.airborne,
            yaw_rate: s.yaw_rate,
            matrix: nfs_net::state::matrix_of(s.orientation),
            velocity: Vec3::from(s.velocity),
            position: Vec3::from(s.position),
        }
    }
}

pub struct NetPlay {
    pub net: Net,
    /// Our own player id: the host is 0, a client is its slot + 1.
    pub id: u8,
    /// Which opponent seat each remote id drives.
    pub remotes: HashMap<u8, Remote>,
    /// The seats a remote player has driven at least once this race. A player who then leaves keeps
    /// its car where it stopped: 0x49B9C8 no longer applies packets to it (0x490140 above 2) and the
    /// network never hands it to a controller, so the seat stays the network's.
    driven: Vec<usize>,
    /// Seats reserved for the network, in the order they are handed out.
    pub seats: Vec<usize>,
    /// `Comms.Positional`: with it off the `0xBE` broadcast is not sent at all.
    pub positional: bool,
    /// The game room over the session — who is in it, their chat and their files
    /// (`nfs_net::room`, `network.md` §10). The host also plays the room object's part.
    pub room: Room,
    /// The lines the chat box's callback has taken (`[net+0x20]` = 0x4C2CE0), as they came.
    chat: Vec<ChatLine>,
    /// Whether that callback is set: a chat display box opens it (0x4C2A09) and closes it
    /// (0x4C2A80), and a line that arrives with it clear is dropped (0x48F3B1).
    chat_open: bool,
    /// `Comms.PlayerList`'s selection — `+0x1D` clear and `+0x18` the row (0x4BC630, 0x4BC4B0).
    /// The accessor starts with none (0x4BC235 sets `+0x1D`); the chat input box sends to the
    /// selected row's player when there is one (0x4C3338…0x4C3362).
    pub player_sel: Option<usize>,
    /// Set whenever the room or the chat changed, so the front end knows to rebind its lists.
    dirty: bool,
    /// `GetTickCount`'s stand-in for the room's send clock (0x48EFB5).
    started: Instant,
    /// `HLeader`'s pictures, uploaded once per player (0x423DE0 uploads each frame; the bytes
    /// never change once whole, so once is the same picture).
    pictures: HashMap<i32, Option<(TextureId, u32, u32)>>,
    next_seat: usize,
    /// TEST GLUE `NFS_NET_LAG_MS`: the car-state packets held back, with when they are heard
    lagged: VecDeque<(Instant, Vec<u8>)>,
    /// A `0xB3` came in (0x48F82F): the host is starting the race. Taken by [`NetPlay::take_start`].
    start_due: bool,
    /// see [`JoinState`]
    join: JoinState,
    /// A client that has probed the host and not yet sent its `'j'`: the host's address, our name and
    /// when the probe last went out (it is repeated once a second, like the connect command's
    /// attempts, 0x55C7D2).
    join_wait: Option<(SocketAddr, String, Instant)>,
    /// The option table's plain band as `0x4B4B80` fills it from `0xBC` (ids below 0x10E: `*ptr =
    /// value`, 0x4B4BCF). The port keeps the ids something reads; the other records are named in the
    /// log. `network.md` §11.6.
    pub options: HashMap<u32, u32>,
    /// `(id, value)` the option table took since [`NetPlay::take_options`] last looked.
    options_in: Vec<(u32, u32)>,
    /// Host: the last value sent for each option id (see [`NetPlay::sync_option`]).
    sent: HashMap<u32, u32>,
    /// The race settings every `0xBC` record has written so far (0x4B4B80 into the option table),
    /// typed — what a player's launch reads (`network.md` §11.7).
    pub launch: nfs_net::options::Received,
    /// `net+0xE6` / `+0xE7`: the launch barrier's two bytes, toggled by `0xBD` (0x48F784).
    barrier: u16,
    /// When `0x587CE0` was last run. Its two counters — 100 between pings, 450 before a peer is
    /// dropped — are in **polls**, and the original registers it as a periodic task (0x587CC0 →
    /// 0x580DC0) whose period has not been read. So the port runs it on the wall clock at the
    /// game's own 64 Hz, which makes the ping about 1.6 s and the timeout about 7 s. Driving it
    /// from the simulated tick instead would make the timeout depend on how fast the simulation
    /// happens to run, which a headless run makes very obvious. **The rate is a choice; the two
    /// counters are the original's.**
    last_poll: Instant,
    /// The per-player slot table the lobby fills over `0xBC` (`nfs_net::slots`, `network.md` §11.9).
    pub slots: nfs_net::slots::Slots,
    /// The state the host last put us in with `0xC6` (0x48F352 `[net+0x24] = value`); `None` before.
    state_in: Option<u32>,
    /// The link dropped after we were in (0x48F0C4 / 0x48FBB6 write state 4).
    lost: bool,
    /// `net+0xE5`: the players (bit = id) the race has started for — every player present when the
    /// host's `0xB3` arrives (0x48F82F `[+0xE5] = [+0xE4]`), cleared by a `0xB1` (0x48F51B).
    pub in_race: u8,
    /// The lobby's ping bookkeeping, per room player: `player+0x44` / `+0x54` / `+0x58`
    /// (0x4905A0, 0x490600; created by 0x48F3D2 with 0 / −1 / 0). `network.md` §11.11.
    pings: HashMap<i32, PlayerPing>,
    /// `net+0x148` — the last measured round trip, which every request carries.
    net148: u32,
    /// The host's `peer+0xB0` per player id: what each player last reported (−1 until it has).
    reported: HashMap<i32, i32>,
    /// `player+0x4C`, the race index 0x4B4DA0 handed each player of the room (0x4B4EBC, 0x4B4ED2),
    /// as `(player id, index)`; empty before a launch. The record of a player who has left the room
    /// goes with it (0x48F46F), the numbers of the others stay.
    launch_index: Vec<(i32, u32)>,
    /// The **input-exchange** mode's player side (`Comms.Positional` off, FEStrings 281): the sender
    /// 0x48EB20, the ring, the receiver 0x48F0DC, the clock and the tick counters — and `net+0x128 +
    /// 4·i`, the round trip 0x490140's non-positional arm reads (`network.md` §11.13, §11.17).
    pub exch: nfs_net::exchange::Client,
    /// The host's room object's half of the same (0x48B6B0 …): the frames it builds and sends.
    server: nfs_net::exchange::Server,
    /// Whether this race runs by input exchange (set by [`NetPlay::exchange_race_frame`]).
    exchange: bool,
    /// The 128 Hz timer callback's emulation: when it started counting (`net+0xC4` cleared) and how
    /// many callbacks have run since (0x4A45D0; **glue**: the original's is a thread).
    timer_anchor: Option<Instant>,
    timer_done: u64,
    /// `car+0xD8B` of each race index: the command code of the sample the last latch took
    /// (0x4992F6: `n = [0x5E9664] >> 3`).
    commands: [u8; 8],
    /// `net+0xC4` — "the room's go is still to come": set by the room's `0xB3` (0x48F82F, 0x48F861),
    /// cleared by the first `0xC5` after it (0x48F8F5). PLAYGAME's wait (0x50CFA9) and the race
    /// frame's (0x4152E0, `while (… +0xC4) 0x4B0FA0`) both test it.
    go_wait: bool,
    /// `net+0x15C` — the room's clock word of that first `0xC5` (0x48F906)
    go_clock: Option<u32>,
    /// `net+0xC5` — "already reported back": cleared by `0xB3` (0x48F85A), set by the abort arms
    /// that send `0xB7 (−2, −1)` (0x50D028, 0x4B6D67 …)
    aborted: bool,
    /// `net+0xBD` — a `0xB7` has gone out since the race started (0x48DA60 sets it, `0xB3` clears it
    /// at 0x48F868, 0x48DAD0 tests it)
    sent_time: bool,
    /// **Test glue**: a fixed `GetTickCount()` for the unit tests.
    tick_override: Option<u32>,
}

/// **0x490140**'s answer: how a racer's connection looks. `HPosition`'s row badge takes its colour
/// from 0x5B2D90 by this number (0x42B4DE), the race-progress pass 0x416310 retires a racer whose
/// number is above 2, and the remote-car apply 0x49B9C8 runs 0x4A0A40 only for a number below 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkQuality {
    /// 0: green 0xCC00FF00
    Good = 0,
    /// 1: yellow 0xCCFFFF00
    Fair = 1,
    /// 2: red 0xCCFF0000
    Poor = 2,
    /// 3: no connection, or the race index is not on `net+0x260`: black 0xCC000000
    NotListed = 3,
    /// 4: on the list but its bit is not in `net+0xE5` (not racing / left): white 0xCCFFFFFF
    NotRacing = 4,
}

impl LinkQuality {
    /// The colour table 0x5B2D90, as `0xAARRGGBB`.
    pub const COLOURS: [u32; 5] = [0xCC00_FF00, 0xCCFF_FF00, 0xCCFF_0000, 0xCC00_0000, 0xCCFF_FFFF];

    pub fn colour(self) -> u32 {
        Self::COLOURS[self as usize]
    }

    /// 0x416310 (`2 < result`) retires the racer; 0x49B9C8 (`result < 3`) drives it.
    pub fn gone(self) -> bool {
        self as u8 > 2
    }
}

/// One room player's ping state as 0x4905A0 keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlayerPing {
    /// `+0x44`: `GetTickCount()` when the last request went out
    asked: u32,
    /// `+0x54`: what the last answer said, −1 before any
    ping: i32,
    /// `+0x58`: a request is out
    pending: bool,
}

impl Default for PlayerPing {
    fn default() -> Self {
        Self { asked: 0, ping: -1, pending: false }
    }
}

/// `GetTickCount()`'s stand-in: milliseconds since the machine's start, a `u32` the original
/// compares as a signed `int` (0x4905A0: `1000 < now − +0x44`). **Glue**: the counter starts an
/// hour "before" the process, not at the machine's boot, so it is a large positive number (which
/// the first, link-event value of `net+0x148` relies on) that stays below 2³¹.
fn tick_ms() -> u32 {
    tick_ms_at(Instant::now())
}

/// [`tick_ms`] at a given instant (the timer emulation runs its callbacks at their own times).
fn tick_ms_at(t: Instant) -> u32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    3_600_000 + t.saturating_duration_since(*START.get_or_init(Instant::now)).as_millis() as u32
}

/// Where a session stands as far as **joining** goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinState {
    /// hosting: nothing to join
    Hosting,
    /// the `'j'` is out and no `'a'` has come back (since when)
    Pending(Instant),
    /// an `'a'` with status 1 (0x588FA4)
    Joined,
    /// an `'a'` with anything else
    Refused(nfs_net::comm::JoinStatus),
}

/// How long a connect may take before it gives up. The original's connect command carries the
/// deadline `[0x5DEB48] << 4` (0x48AC90: `0x55D370(conman, addr, [0x5DEB48] * 16, 0)`), the command
/// loop re-tries every `[0x5DEB48] * 5` (0x55C7D2, 0x55C86D) and each attempt's handshake waits 5000 ms
/// (0x55CA15: `0x1388`) — `[0x5DEB48]` is the timer module's ticks per second (written by 0x564F3D…,
/// zeroed by 0x565336), so the deadline is 16 s. **APPROXIMATION:** the unit is inferred from those
/// two constants, the timer's own value was not read. When it runs out the conman reports event 5
/// (0x55C9CF), and the game's callback makes it state **4** (0x48FBB6, `net+0xDC = −1`) — not 6.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(16);

/// The well-known port. **TEST GLUE** (not in the original): `NFS_NET_PORT` moves it, so two
/// test sessions (or two two-instance runs) on one machine do not fight over 1106.
fn port() -> u16 {
    std::env::var("NFS_NET_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(nfs_net::PORT)
}

/// How long the launch barrier waits at most (**glue**: the original waits for ever while the
/// session lives).
pub const BARRIER_LIMIT: Duration = Duration::from_secs(120);

/// Where a barrier's four phases stand ([`NetPlay::barrier_begin`]).
pub struct BarrierRun {
    phase: usize,
    sent: bool,
    end: Instant,
    keys: KeyRing,
}

/// One turn of 0x587CE0 per 1/64 s of real time.
const POLL_PERIOD: Duration = Duration::from_micros(15_625);

impl NetPlay {
    /// `host`, `host:PORT`, `join:HOST[:PORT]`. Anything else is an error rather than a guess.
    pub fn start(spec: &str, who: &str) -> anyhow::Result<Self> {
        Self::start_named(spec, who, &format!("{who}'s race"))
    }

    /// [`NetPlay::start`] with the session's name — `Comms.GameName`'s text, which 0x509B00 puts
    /// into the descriptor (0x509B41) whatever it is, empty included.
    pub fn start_named(spec: &str, who: &str, game_name: &str) -> anyhow::Result<Self> {
        let (mode, rest) = spec.split_once(':').unwrap_or((spec, ""));
        let mut join_target = None;
        let net = match mode {
            "host" => {
                let port: u16 = if rest.is_empty() {
                    port()
                } else {
                    rest.parse()?
                };
                let sock = CommSocket::bind(port)?;
                // 0x48DD97…0x48DDF8: the session's name is `<Comms.GameName>~<version>`
                let name = nfs_net::comm::desc::name_field(game_name, &game_version());
                // **TEST GLUE** (not in the original): `NFS_NET_LIMIT=n` sets the player limit
                // (`session+0x84`, 8 in the game: 0x509B49), so a "game full" refusal (`-2`, 0x588F3D)
                // can be reached with two instances instead of eight
                let limit = std::env::var("NFS_NET_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
                let mut n = Net::host(sock, name, KEY, limit);
                n.advertise = advertised_record();
                n.my_name = name16(who);
                // 0x588EBF…0x588ED3 → the room's checks (0x48A700, 0x49151E…): the version and the game
                n.identity_gate = Some((game_name.to_string(), game_version()));
                tracing::info!(addr = %n.local_addr()?, "hosting");
                n
            }
            "join" => {
                let target = if rest.contains(':') {
                    rest.to_string()
                } else {
                    format!("{rest}:{}", port())
                };
                let host: SocketAddr = target
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("cannot resolve {target}"))?;
                let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::UNSPECIFIED,
                    0,
                )))?;
                let n = Net::client(sock, KEY, name16(who));
                // the original's connect command probes the address first (0x55C82x: `0x5803E0`, the
                // enumerate) and only then runs the handshake that carries the identity string
                // (0x55CED0, `name~version~game` with the game from the answer): the `'j'` follows the
                // host's `'d'` (see `NetPlay::poll`)
                n.probe_at(host)?;
                join_target = Some(host);
                tracing::info!(%host, addr = %n.local_addr()?, "joining");
                n
            }
            // `accountlogin`'s SIGNON (0x5109B0 → 0x48E1A0(net, 1, 0, "porsche-entry.earacing.com")): a connect to the
            // EA lobby service, which no longer exists — the session is a client that never hears an answer, so the
            // connect's 16 s deadline ends it in state 4 (830)
            "signon" => {
                let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
                Net::client(sock, KEY, name16(who))
            }
            other => anyhow::bail!(
                "--net wants `host`, `host:PORT` or `join:HOST[:PORT]`, not {other:?}"
            ),
        };
        let id = if net.role == Role::Host { 0 } else { u8::MAX }; // a client's id arrives with its 'a'
        let mut n = Self {
            net,
            id,
            remotes: HashMap::new(),
            driven: Vec::new(),
            seats: Vec::new(),
            positional: true,
            room: Room::new(if id == u8::MAX { -1 } else { id as i32 }),
            chat: Vec::new(),
            chat_open: false,
            player_sel: None,
            dirty: true,
            started: Instant::now(),
            pictures: HashMap::new(),
            next_seat: 0,
            lagged: VecDeque::new(),
            start_due: false,
            join: if mode == "host" { JoinState::Hosting } else { JoinState::Pending(Instant::now()) },
            join_wait: join_target.map(|h| (h, who.to_string(), Instant::now())),
            options: HashMap::new(),
            options_in: Vec::new(),
            sent: HashMap::new(),
            launch: Default::default(),
            barrier: 0,
            last_poll: Instant::now(),
            slots: Default::default(),
            state_in: None,
            lost: false,
            in_race: 0,
            pings: HashMap::new(),
            net148: tick_ms(),
            reported: HashMap::new(),
            launch_index: Vec::new(),
            exch: Default::default(),
            server: Default::default(),
            exchange: false,
            timer_anchor: None,
            timer_done: 0,
            commands: [0; 8],
            go_wait: false,
            go_clock: None,
            aborted: false,
            sent_time: false,
            tick_override: None,
        };
        // the host's own player is the room's first, id 0: in the original the host's game
        // joins the room object like any other (0x48BB50 hands out the lowest clear bit of
        // `room+0xCC0`, and the room is empty)
        if n.is_host() {
            let out = n.room.admit(0, &who.as_bytes()[..who.len().min(room::NAME_MAX)]);
            n.dispatch(out);
        }
        Ok(n)
    }

    /// Deliver what the room object sends — the host's half (0x48C870 on the room's links). Every
    /// link means every peer and the host's own player; one player's link is that peer's address,
    /// or the host itself for id 0.
    fn dispatch(&mut self, out: Vec<(To, Vec<u8>)>) {
        for (to, m) in out {
            match to {
                To::All => {
                    if let Err(e) = self.net.send_game(&m) {
                        tracing::debug!(error = %e, "room message not sent");
                    }
                    self.hear(&m);
                }
                To::Player(id) if id == self.id as i32 => self.hear(&m),
                To::Player(id) => {
                    if let Some(a) = u32::try_from(id - 1).ok().and_then(|s| self.net.peer_addr(s))
                        && let Err(e) = self.net.send_game_to(&[a], &m)
                    {
                        tracing::debug!(error = %e, "room message not sent");
                    }
                }
            }
        }
    }

    /// One room message for our own player (0x48F0A0's cases).
    fn hear(&mut self, m: &[u8]) {
        // 0x48F784 (`0xBD`): `[net+0xE6] ^= 1 << value` (a word: bits 8…15 are `+0xE7`)
        if m.first() == Some(&0xBD) {
            if let Ok(h) = nfs_net::msg::Message::parse(m) {
                self.barrier ^= 1u16.wrapping_shl(h.value as u32);
            }
            return;
        }
        // 0x48F527…0x48F574 (`0xC3`): the answer to a ping request
        if let Some(p) = room::Ping::parse(m) {
            self.ping_answer(p, self.now());
            return;
        }
        // 0x48F92A (`0xC4`): the host's clock, and ours when it came (0 without a connection) — the input
        // exchange's heartbeat (network.md §11.17)
        if m.first() == Some(&0xC4) {
            if let Ok(h) = nfs_net::msg::Message::parse(m)
                && let Ok(w) = h.body_u32()
            {
                let local = if self.established() { self.now() } else { 0 };
                self.exch.on_beat(w, local);
            }
            return;
        }
        match self.room.hear(m) {
            // the chat box's callback 0x4C2CE0 queues `(text, from, to)`; with no box open the
            // callback slot is clear and the line goes nowhere (0x48F3B1)
            Some(Heard::Chat(line)) if self.chat_open => {
                self.chat.push(line);
                // 0x4C2711: the box keeps a hundred lines, dropping the oldest
                let over = self.chat.len().saturating_sub(100);
                self.chat.drain(..over);
                self.dirty = true;
            }
            Some(Heard::Chat(_)) | None => {}
            // the host's own room object sends this and does not act on it (0x48BF10 sets the
            // started byte itself); a player takes it as the race starting
            Some(Heard::Start { clock }) => {
                // 0x48F837: `[net+0xE5] = [net+0xE4]` — everyone present is now in the race
                self.in_race = self.room.players.iter().fold(0u8, |m, p| m | 1u8.checked_shl(p.id as u32).unwrap_or(0));
                // 0x48F82F on every machine, the host's own included (its room sends to its own link
                // too): `+0xC5 = 0` (0x48F85A), `+0xC4 = 1` (0x48F861), `+0xBD = 0` (0x48F868), the
                // barrier's word `+0xE6 = 0` (0x48F896)
                self.aborted = false;
                self.go_wait = true;
                self.go_clock = None;
                self.sent_time = false;
                self.barrier = 0;
                // the exchange's counters, the sender's reset and the clock set to the room's word (0x48F82F…0x48F8C8)
                self.exch.on_start(clock, self.now());
                // (0x48F87F…0x48F88B also clears the input-exchange timers `[+0x108 + 4i]` /
                // `[+0x128 + 4i]`, which the port has no counterpart of; the players' `+0x48` /
                // `+0x50` are reset by the launch, 0x48D3D0 — [`NetPlay::launch_begin`])
                if !self.is_host() {
                    tracing::info!(target: "netplay", clock, "the host started the race (0xB3)");
                    self.start_due = true;
                }
            }
            // 0x48F579: `+0x50` / `+0x48` of the player; the finish time HPosition shows (0x42B37A)
            Some(Heard::RaceTime { id, value }) => {
                tracing::info!(target: "netplay", player = id, value, "0xB7: a player's finish time");
                self.dirty = true;
            }
            // 0x48F5B9…0x48F5CF: the player's bit leaves `net+0xE5`
            Some(Heard::Returned(id)) => {
                if (0..8).contains(&id) {
                    self.in_race &= !(1u8 << id);
                }
                tracing::info!(target: "netplay", player = id, "0xB7 (-2): a player is back in the front end");
                self.dirty = true;
            }
            // 0x48F8D2: the first `0xC5` after `0xB3` is the room's go; a later one puts the sender's
            // bit into `net+0xE5`
            Some(Heard::Go { id, clock }) => {
                if self.go_wait {
                    self.go_wait = false;
                    self.go_clock = Some(clock);
                    // 0x48F906: the word is the room's clock at the start — `net+0x15C`, what the exchange's
                    // tick clock counts from
                    self.exch.on_go(clock);
                    tracing::info!(target: "netplay", clock, "the room's go (0xC5)");
                } else if (0..8).contains(&id) {
                    self.in_race |= 1u8 << id;
                }
            }
            // 0x48F998: every record goes to 0x4B4B80 (the option table at 0x5D1E40); the plain
            // band (id < 0x10E) is `*ptr = value`. The port keeps those, and says what else came.
            Some(Heard::Options(recs)) => {
                for r in recs {
                    // the launch's per-slot records for a slot past the players are the host's
                    // traffic cars (0x4B4F25…0x4B4F75: slot `8 + k` rebased to `n + k`); before the
                    // launch (`net+0xE5 == 0`) a slot is a lobby player's
                    if self.in_race != 0 && self.launch.take_traffic(&r, self.room.players.len() as u32) {
                        tracing::info!(target: "netplay", record = ?r, "traffic car record (0xBC)");
                        continue;
                    }
                    // …and a slot below it is a player's **race index** (0x4B4F25…0x4B4F66): the
                    // car the owner races, its name, set-up and options (`network.md` §11.12)
                    if self.in_race != 0 && self.launch.take_car(&r, self.room.players.len() as u32) {
                        tracing::info!(target: "netplay", record = ?r, "player car record (0xBC, race index)");
                        continue;
                    }
                    if self.slots.apply(&r) {
                        tracing::info!(target: "netplay", record = ?r, "player slot record (0xBC)");
                        continue;
                    }
                    // the typed table the launch reads (every band, as 0x4B4B80 writes them)
                    let read = self.launch.take(&r);
                    match r.as_slice() {
                        [id, v] if *id < 0x10E => {
                            tracing::info!(target: "netplay", id = format_args!("{id:#x}"), value = *v, "option record (0xBC)");
                            self.options.insert(*id, *v);
                            self.options_in.push((*id, *v));
                        }
                        other => tracing::info!(target: "netplay", record = ?other, read, "option record (0xBC), string / array / per-slot band"),
                    }
                }
                self.dirty = true;
            }
            // 0x48F34F: the value is the comm state; 0xE is "in" (`+0xBE = 1`), anything else ends
            // the connection (`+0xBF = 1`, teardown 0x48E790)
            Some(Heard::State(v)) => {
                if !self.is_host() {
                    tracing::info!(target: "netplay", state = v, "the host set the comm state (0xC6)");
                    self.state_in = Some(v as u32);
                    self.dirty = true;
                }
            }
            // 0x48F46F / 0x48F4FD…0x48F51B: the player's bits leave `+0xE4` and `+0xE5`
            Some(Heard::Left(id)) => {
                self.pings.remove(&id);
                self.reported.remove(&id);
                if (0..8).contains(&id) {
                    self.in_race &= !(1u8 << id);
                    self.slots.clear(id as usize);
                }
                self.dirty = true;
            }
            Some(h) => {
                // 0x48F3F9…0x48F403: a new player record starts with no ping and no request out
                if let Heard::Joined(id) = h {
                    self.pings.insert(id, PlayerPing::default());
                }
                tracing::info!(target: "netplay", "room: {h:?}");
                self.dirty = true;
            }
        }
    }

    /// **0x4B4DA0's walk** (0x4B4EF5…0x4B4FC7): the records this machine's walk sends are appended
    /// with 0x48D490 (the buffer is sent first when a record would take it past 0xF0) and the rest
    /// goes out with 0x48D440 — several records to a message, not one each as the selection path
    /// (0x48D550) sends them. On the host the list holds the broadcast bands, its own slot's records
    /// and the traffic cars'; on a player only its own slot's ([`Self::own_launch_records`]) — every
    /// machine runs this walk. What a machine sends it holds already (the host's own table is the
    /// source; a player's own records are applied by `own_launch_records`), so nothing is heard back.
    pub fn send_launch(&mut self, stream: &[Vec<u32>]) {
        let mut out = nfs_net::options::Outgoing::new();
        let mut msgs = Vec::new();
        for r in stream {
            msgs.extend(out.push(r));
            tracing::info!(target: "netplay", record = ?r, "launch record sent (0xBC)");
        }
        msgs.extend(out.flush());
        for m in msgs {
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "launch record not sent");
            }
        }
    }

    /// **The own-slot arm of 0x4B4DA0's walk** (0x4B4F25…0x4B4F66): the records the launch chain left
    /// for this player's slot (`OwnCar::launch_list`, at the player id `[net+0xDC]`) go out with the
    /// slot rewritten to this machine's **race index** — `player+0x4C`, its place in the room's list
    /// among the players `net+0xE5` marks ([`Self::race_indices`]). Returns them for the caller to
    /// send, and applies them to this machine's own table at that index — where the original hears
    /// them back from the host's relay (0x48C050 sends to the sender too; this port's relay leaves
    /// the sender out, `network.md` §8.3).
    pub fn own_launch_records(&mut self, own: &nfs_net::launchcar::OwnCar) -> Vec<Vec<u32>> {
        let me = self.my_player_id();
        let index = self.race_indices().into_iter().find(|(id, _)| *id == me).map_or(0, |(_, i)| i);
        let players = self.room.players.len() as u32;
        let recs: Vec<Vec<u32>> = own
            .launch_list(me.max(0) as u32)
            .into_iter()
            .map(|mut r| {
                r[1] = index;
                r
            })
            .collect();
        for r in &recs {
            self.launch.take_car(r, players);
        }
        recs
    }

    /// The car record every **other** player of the race holds at its race index, in the room's
    /// order — the seat order ([`Self::remote_players`]): `(player id, race index, name, record)`.
    pub fn remote_cars(&self) -> Vec<(i32, u32, String, nfs_net::launchcar::CarSlot)> {
        let me = self.my_player_id();
        self.race_indices()
            .into_iter()
            .filter(|(id, _)| *id != me)
            .map(|(id, idx)| {
                let name = self.room.players.iter().find(|p| p.id == id).map(|p| p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect()).unwrap_or_default();
                (id, idx, name, self.launch.cars.get(&idx).cloned().unwrap_or_default())
            })
            .collect()
    }

    /// The race indices 0x4B4DA0 hands out (0x4B4EBC, 0x4B4ED2): the players `net+0xE5` marks, in the
    /// room's list order, numbered from 0 (`player+0x4C`); `(player id, index)`. With no start
    /// heard yet (`+0xE5 == 0`) every player of the room counts.
    pub fn race_indices(&self) -> Vec<(i32, u32)> {
        let all = self.in_race == 0;
        self.room
            .players
            .iter()
            .filter(|p| all || self.in_race.checked_shr(p.id as u32).is_some_and(|b| b & 1 != 0))
            .enumerate()
            .map(|(i, p)| (p.id, i as u32))
            .collect()
    }

    /// **0x48D6A0** — the barrier every machine runs at the end of 0x4B4DA0: `0xBD(id)`, wait for
    /// every player's bit in `[net+0xE6]`, `0xBD(id + 8)`, wait for `[net+0xE7]`, then the same
    /// two again to bring both bytes back to zero. The players are `[net+0xE5]`'s, read afresh
    /// at every look (a player who leaves, `0xB1`, drops out of it, 0x48F51B). Because a player's
    /// `0xBD` follows its `0xBC`s on the same link, a machine that has passed the first wait has
    /// applied the host's whole stream. Answers `false` when it left without them.
    ///
    /// The waits redraw the loading screen through `0x4A4A70(−1)` in the original, which is a
    /// **no-op here**: that function returns at once unless `[0x655A28]` is set, and only its step
    /// 0 sets it — called at 0x4B6CCA, *after* 0x4B4DA0 (0x4B6CC4) has returned, and cleared again
    /// by step 10 (0x4B68CB). So the barrier shows nothing new on either side.
    ///
    /// **APPROXIMATION** (direction: the original waits for ever while the session lives, leaving
    /// only for "Abort" — the session gone, 0x55FF60 == 0x1B — through 0x48DA60(−2, −1)): the port
    /// leaves when the link to the host is lost, and — **glue**, so a headless run cannot hang —
    /// after [`BARRIER_LIMIT`].
    pub fn launch_barrier(&mut self) -> bool {
        let Some(mut run) = self.barrier_begin() else { return true };
        loop {
            if let Some(passed) = self.barrier_poll(&mut run) {
                if passed {
                    tracing::info!(target: "netplay", "launch barrier passed (0xBD ×4)");
                }
                return passed;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The barrier as a loop a caller can draw inside — 0x48CDF0's wait loops call the loading
    /// screen's 0x4A4A70(−1) every turn (0x48CE27, 0x48CF5F, 0x48D079, 0x48D200), which redraws the
    /// other players' rows while the screen is up. `None` with no player id (nothing to wait for).
    pub fn barrier_begin(&self) -> Option<BarrierRun> {
        // **TEST GLUE**: `NFS_NET_STALL_MS=<ms>` — a joiner that does not answer for that long before its
        // first barrier (so the host's wait can be shown to end on ESC)
        static STALLED: std::sync::Once = std::sync::Once::new();
        if !self.is_host()
            && let Some(ms) = std::env::var("NFS_NET_STALL_MS").ok().and_then(|v| v.parse::<u64>().ok())
        {
            STALLED.call_once(|| std::thread::sleep(Duration::from_millis(ms)));
        }
        (self.room.me >= 0).then(|| BarrierRun { phase: 0, sent: false, end: Instant::now() + BARRIER_LIMIT, keys: KeyRing::begin() })
    }

    /// One look of the barrier's wait: `Some(true)` when it has passed, `Some(false)` when it left
    /// without the other players (the link lost, or [`BARRIER_LIMIT`]), `None` while it waits.
    pub fn barrier_poll(&mut self, run: &mut BarrierRun) -> Option<bool> {
        // (value to send, wait for `bits` of the byte to equal `want`)
        const PHASES: [(i32, bool, bool); 4] = [(0, false, true), (8, true, true), (0, false, false), (8, true, false)];
        let (add, high, want_all) = *PHASES.get(run.phase)?;
        if !run.sent {
            let out = vec![(To::All, room::encode(0xBD, Some(self.room.me + add), None, None))];
            self.dispatch(out);
            run.sent = true;
        }
        let mask = u16::from(self.in_race);
        let bits = if high { self.barrier >> 8 } else { self.barrier & 0xFF } & mask;
        if bits == if want_all { mask } else { 0 } {
            run.phase += 1;
            run.sent = false;
            return (run.phase == PHASES.len()).then_some(true);
        }
        // 0x48CE6F / 0x48CFA7 / 0x48D0C1 / 0x48D248: `0x55FF60() == 0x1B` — like a dead link, the abort arm
        if run.keys.esc() {
            tracing::info!(target: "netplay", "launch barrier: ESC (0x48CE6F…) — the wait is abandoned");
            return Some(false);
        }
        if self.lost || Instant::now() > run.end {
            tracing::warn!(target: "netplay", lost = self.lost, link = %self.net.report(), "launch barrier: left without the other players");
            return Some(false);
        }
        None
    }

    /// The option records a `0xBC` brought since the last call, in order.
    pub fn take_options(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.options_in)
    }

    /// **0x48D550** on the host: the stream goes to every player, one `0xBC` per record (the
    /// port's records are all two dwords, `(id, value)`). The host's own option table is written by
    /// the caller — the original's launch applies the whole stream locally first (0x4B4DFD).
    pub fn send_options(&mut self, stream: &[(u32, u32)]) {
        if !self.is_host() {
            return;
        }
        let recs: Vec<Vec<u32>> = stream.iter().map(|&(id, v)| vec![id, v]).collect();
        for m in nfs_net::options::messages_each(&recs) {
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "option record not sent");
            }
        }
        for &(id, v) in stream {
            self.sent.insert(id, v);
            tracing::info!(target: "netplay", id = format_args!("{id:#x}"), value = v, "option record sent (0xBC)");
        }
    }

    /// **0x4BC240 on a mode-2 accessor** (`Track1.Track` / `.Direction` / `.Laps` are built with
    /// mode 2 — 0x4FA4F0 pushes 2 into 0x4BC1F0, 0x4BB7A0 / 0x4BB680 take it as their 4th argument
    /// — and 0x4B7F70 lets a mode-2 accessor send when this machine hosts the session): its slot 1
    /// encodes `(id, value)` and 0x48D550 sends it. The original sends on every selection call;
    /// the port calls this each front-end tick with the accessor's current value and sends when it
    /// differs from what was last sent — the same records at the moments a user changes them,
    /// minus the repeats of an unchanged value. The first value seen for an id is the accessor's
    /// own starting state (they are set up before any session exists, so nothing is sent for it).
    /// Answers whether a record went out.
    pub fn sync_option(&mut self, id: u32, value: u32) -> bool {
        if !self.is_host() {
            return false;
        }
        match self.sent.get(&id) {
            Some(v) if *v == value => return false,
            None => {
                self.sent.insert(id, value);
                return false;
            }
            Some(_) => {}
        }
        self.send_options(&[(id, value)]);
        true
    }

    /// The game's descriptor as the `'d'` carries it (`nfs_net::comm::desc`), which 0x509B00 puts
    /// together for 0x48DCB0 when the session is created.
    pub fn set_descriptor(&mut self, positional: u8, laps: u8, direction: u16, track: u16) {
        self.net.desc_bytes = [0, positional, laps, 0];
        self.net.desc_words = [direction, track];
    }

    /// The room's word for "now": `0x48A9A0()`, `GetTickCount()` plus an offset a host keeps at zero,
    /// so the milliseconds since this session opened — **APPROXIMATION** of the origin of the tick
    /// counter only; a player uses the word for nothing here (the original sets its own clock offset
    /// from `0xB3`'s, 0x48A9B0, which the port has no counterpart of).
    fn room_clock(&self) -> u32 {
        // the same scale as the exchange room's heartbeat and frame clocks ([`NetPlay::now`]), which the
        // players' synchronised clock is set from (0x48F8C8) and the go's word is read against
        self.now()
    }

    /// **The host's room, one player's request** (`0xB3` 0x4917C0, `0xC5` 0x49185B, `0xB7` 0x4918BD): the
    /// room object answers every link ([`nfs_net::room::Room::start_request`], `go_request`,
    /// `race_time_request`). A request is a one-byte `0xB3` / `0xC5` and an 8-byte `0xB7` (head =
    /// the sender's time or −1 / −2, word = its best lap). 0x4929B0 logs the `0xB7` as
    /// `"Finish %02d:%02d.%02d, Best Lap %02d:%02d.%02d"`, `"Leaving game"` or `"Returning to
    /// frontend"` (0x492CF0).
    fn serve_race_request(&mut self, from: i32, m: &[u8]) {
        let clock = self.room_clock();
        let out = match m.first().copied() {
            Some(kind::START) => {
                tracing::info!(target: "netplay", player = from, "room: Start Race (0xB3 request)");
                let out = self.room.start_request(from, clock);
                // 0x48BF10's other half (network.md §11.17): a race the room has just started has fresh
                // exchange buffers (the ring zeroed, the head and `+0xCC1` cleared, `+0xCC4` clear —
                // 0x48BF51…0x48BF84), and the player's frame pointer starts at 0
                if !out.is_empty() {
                    self.server.started = false;
                    self.server.running = false;
                }
                if let Ok(slot) = usize::try_from(from) {
                    let _ = self.server.ready(slot, clock);
                }
                out
            }
            Some(kind::GO) => {
                let out = self.room.go_request(clock);
                // 0x48BFB0's other half: the frame and send clocks start at the go
                if !out.is_empty() {
                    let _ = self.server.go(clock);
                }
                out
            }
            Some(kind::RACE_TIME) => {
                let head = m.get(1..4).map_or(0, |b| nfs_net::msg::sign_extend_24(u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2])));
                let best = m.get(4..8).map_or(0, |w| i32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                match head {
                    -2 => {
                        tracing::info!(target: "netplay", player = from, "room: Returning to frontend (0xB7)");
                        // 0x48C220's exchange half: the player is out of the race again; the last one out
                        // ends the started room
                        if let Ok(slot) = usize::try_from(from) {
                            self.server.out_of_race(slot);
                        }
                    }
                    -1 => tracing::info!(target: "netplay", player = from, "room: Leaving game (0xB7)"),
                    t => tracing::info!(
                        target: "netplay",
                        player = from,
                        "room: Finish {:02}:{:02}.{:02}, Best Lap {:02}:{:02}.{:02}",
                        t / 0xF00,
                        (t / 64) % 60,
                        (t & 63) * 100 / 64,
                        best / 0xF00,
                        (best / 64) % 60,
                        (best & 63) * 100 / 64
                    ),
                }
                self.room.race_time_request(from, head)
            }
            _ => return,
        };
        self.dispatch(out);
    }

    /// One request of ours to the room: the host's own room object is a player's link away in the
    /// original (loopback) and here a call; a player sends it to the host.
    fn request(&mut self, m: Vec<u8>) {
        if self.is_host() {
            let me = self.id as i32;
            self.serve_race_request(me, &m);
        } else if let Err(e) = self.net.send_game(&m) {
            tracing::warn!(error = %e, "room request not sent");
        }
    }

    /// **0x48D370** — PLAYGAME's `0xB3` request (0x50CF73), on every machine: the room takes this
    /// player out of the lobby group and, when no race is running, starts one for everybody.
    pub fn request_start(&mut self) {
        self.request(room::start_request());
    }

    /// **0x48D3A0** — the race frame's `0xC5` request (0x4153AE), sent while `net+0xC5` is clear and
    /// the connection is up.
    pub fn request_go(&mut self) {
        if !self.aborted && self.established() {
            self.request(room::go_request());
        }
    }

    /// **0x48DA60**`(a, b)` — `0xB7` to the room with the head `a` and the word `b`, and `+0xBD = 1`;
    /// needs the session and its link (0x48DA67…0x48DA78).
    pub fn send_race_time(&mut self, a: i32, b: i32) {
        if !self.link_up() {
            return;
        }
        self.sent_time = true;
        self.request(room::race_time_request(a, b));
    }

    /// The abort arm PLAYGAME's wait, the barrier and the session loop share (0x50D028…0x50D04D,
    /// 0x48CF0E…0x48CF1D, 0x4B6D67…): with the connection up and `net+0xC5` clear, `+0xC5 = 1`,
    /// `+0xBC = 1`, `+0xC4 = 0`, and `0xB7 (−2, −1)` — "returning to the front end".
    pub fn abort_race(&mut self) {
        if self.established() && !self.aborted {
            self.aborted = true;
            self.go_wait = false;
            self.send_race_time(-2, -1);
        }
    }

    /// **0x48DAD0** — the session loop's normal exit (0x4B6F5B, after the race is shut down): a player
    /// that has sent no `0xB7` since the race started says `(−1, −1)` ("leaving game" — its time
    /// stays −1), then `(−2, −1)` for everyone — its bit leaves `net+0xE5` on every machine.
    pub fn leave_race(&mut self) {
        if !self.established() {
            return;
        }
        // (test glue's view of the data layer: the reliable layer's counters, `network.md` §11.18)
        tracing::info!(target: "netplay", link = %self.net.report(), "data layer at the race's end");
        if !self.sent_time {
            self.send_race_time(-1, -1);
        }
        self.send_race_time(-2, -1);
    }

    /// **PLAYGAME's wait, 0x50CF78…0x50CFF1**: pump the session until the room's `0xB3` has set `+0xC4`
    /// with the connection up (`+0xBF == 0`, `+0xBE`, `+0xDC ≥ 0`, 0x50CFA9), and give up — `false` —
    /// when the link is gone (0x50CF86 `net+8`, 0x50CFB7…0x50CFCF). **APPROXIMATION** (direction: the
    /// original also leaves when 0x55FF60 answers ESC, 0x50CFD7 `cmp eax, 0x1B`, and otherwise waits
    /// for ever): the port has no key pump inside this call, so it leaves after
    /// [`BARRIER_LIMIT`] instead — glue, so a headless run cannot hang.
    pub fn wait_released(&mut self) -> bool {
        let end = Instant::now() + BARRIER_LIMIT;
        let keys = KeyRing::begin();
        loop {
            if !self.established() || self.lost {
                return false;
            }
            if self.go_wait {
                return true;
            }
            // 0x50CFD7 `0x55FF60() == 0x1B`: ESC leaves the wait through the `(−2, −1)` abort arm
            if keys.esc() {
                tracing::info!(target: "netplay", "PLAYGAME: ESC (0x50CFD7) — the wait is abandoned");
                return false;
            }
            if Instant::now() > end {
                tracing::warn!(target: "netplay", "PLAYGAME: no 0xB3 came");
                return false;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// **The race frame's wait, 0x4152E0** (`while (… +0xC4 != 0) 0x4B0FA0`): pump until the room's first
    /// `0xC5` has cleared `+0xC4`. The loop's condition is the connection up and `+0xC4` set, so it
    /// also ends when the link goes. [`BARRIER_LIMIT`] is the glue as above.
    pub fn wait_go(&mut self) {
        let end = Instant::now() + BARRIER_LIMIT;
        while self.go_wait && self.established() && !self.lost {
            if Instant::now() > end {
                tracing::warn!(target: "netplay", "race start: no 0xC5 came");
                break;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// **0x48D3D0** — the launch's first step (0x4B4E28): the barrier, then every player's `+0x50 =
    /// +0x48 = +0x4C = −1` — the finish times of an earlier race, and the race indices.
    pub fn launch_begin(&mut self) -> bool {
        let passed = self.launch_barrier();
        for p in &mut self.room.players {
            p.time = -1;
            p.progress = -1;
        }
        self.launch_index.clear();
        // 0x4B4DB6 `memset(0x6573E8, 0, 0x3E24)` clears `[0x658608 + slot·0x580]` with the rest of the
        // option block (the port refills the other tables from the stream and keeps them as they are)
        self.launch.progress.clear();
        passed
    }

    /// **0x48D770(race index)** — the finish time of the player whose `+0x4C` is `index` (`+0x50`), −1
    /// when there is none, the connection is down, or (with `COMMSPOSITIONAL` set) another player
    /// that has not finished reports a smaller `+0x48` than the time (0x48D7A5…0x48D7E5).
    pub fn race_time(&self, index: u32) -> i32 {
        if !self.link_up() {
            return -1;
        }
        let table = self.index_table();
        let (mut mine, mut floor) = (-1, -1);
        for p in &self.room.players {
            let idx = table.iter().find(|(id, _)| *id == p.id).map(|(_, i)| *i);
            if idx == Some(index) {
                mine = p.time;
            } else if p.time == -1 && (floor == -1 || p.progress < floor) {
                floor = p.progress;
            }
        }
        if !self.positional || floor == -1 || mine < floor {
            return mine;
        }
        -1
    }

    /// **0x48D7F0(race index)** — the finish place: 1 + the players whose `+0x50` is above −1 and
    /// below this one's time; −1 while 0x48D770 has no time for it.
    pub fn race_rank(&self, index: u32) -> i32 {
        let t = self.race_time(index);
        if t < 0 {
            return -1;
        }
        1 + self.room.players.iter().filter(|p| p.time > -1 && p.time < t).count() as i32
    }

    /// Whether the host has announced the race since the last call.
    pub fn take_start(&mut self) -> bool {
        std::mem::take(&mut self.start_due)
    }

    /// Where a joiner joined (or is joining).
    pub fn host_addr(&self) -> Option<SocketAddr> {
        self.net.host
    }

    /// How many players the room holds (`net+0x260`'s length).
    /// `[net+0xDC]`, this machine's player id (-1 before it has one).
    pub fn my_player_id(&self) -> i32 {
        self.room.me
    }

    /// The `.STARTINGPOS` records the host drew, kept as if heard (the host's own table takes them).
    pub fn note_grid(&mut self, recs: &[Vec<u32>]) {
        for r in recs {
            self.launch.take(r);
        }
    }

    pub fn room_len(&self) -> usize {
        self.room.players.len()
    }

    pub fn is_host(&self) -> bool {
        self.net.role == Role::Host
    }

    /// The opponent seats the network owns. Called once the race has built its opponents.
    pub fn reserve(&mut self, seats: Vec<usize>) {
        self.seats = seats;
        self.next_seat = 0;
        self.remotes.clear();
        self.driven.clear();
        self.exchange = false;
    }

    /// The seats the network owns this tick: every one a remote player has driven, present or gone.
    /// In an input-exchange race that is every seat with a player behind it (the launch's numbering,
    /// `player+0x4C`): each is a car the same simulation runs on the frames' inputs, from the first tick.
    pub fn owned_seats(&self) -> Vec<usize> {
        if self.exchanging() {
            return (0..self.seats.len()).filter(|k| self.seat_race_index(*k).is_some()).collect();
        }
        let mut v = self.driven.clone();
        v.extend(self.remotes.values().map(|r| r.seat).filter(|s| !self.driven.contains(s)));
        v
    }

    /// Drain the socket. Returns the events worth logging; the car states are kept internally.
    pub fn poll(&mut self, tick: u64) -> Vec<String> {
        let mut log = Vec::new();
        // 0x48FB9F…0x48FBA5 (link event 1): `net+0x148 = GetTickCount()` while the connection is not
        // yet complete (`+0xBE == 0`), so the first request carries a large number
        if !self.established() {
            self.net148 = tick_ms();
        }
        // the connect command's attempts repeat (0x55C7D2: every `[0x5DEB48] * 5`); a probe lost on the
        // way is sent again once a second
        if let Some((host, _, last)) = self.join_wait.as_mut()
            && last.elapsed() >= Duration::from_secs(1)
        {
            let _ = self.net.probe_at(*host);
            *last = Instant::now();
        }
        for e in self.net.poll() {
            match e {
                Event::Joined { slot } => {
                    self.join = JoinState::Joined;
                    self.id = slot as u8 + 1;
                    self.room.me = self.id as i32;
                    log.push(format!("joined at slot {slot}, player id {}", self.id));
                }
                Event::Refused { status } => {
                    self.join = JoinState::Refused(status);
                    log.push(format!("the host refused: {status:?}"))
                }
                Event::PeerJoined { slot, from } => {
                    log.push(format!("slot {slot} taken by {from}"));
                    // 0x48BB50: the room tells the newcomer who is there, and everyone of it
                    let name = self.net.peers.get(slot as usize).and_then(|p| p.as_ref()).map(|p| p.name).unwrap_or_default();
                    let name = name.split(|&c| c == 0).next().unwrap_or_default().to_vec();
                    let out = self.room.admit(slot as i32 + 1, &name);
                    self.dispatch(out);
                    // 0x48BB66: the newcomer's bit in the exchange room's `+0xCC0` and `+0xCC2`
                    self.server.join(slot as usize + 1);
                    // A newcomer hears **no** earlier record: 0x48BB50 / 0x4924D0 send the `0xB0`s and the
                    // room description, no state (`network.md` §11.9). What makes every machine know
                    // every car is the launch's per-slot resend with race indices (`send_launch`,
                    // §11.12) — the port used to hand the host's table to a newcomer here.
                }
                Event::PeerLeft { slot } => {
                    self.slots.clear(slot as usize + 1);
                    self.in_race &= !(1u8 << (slot + 1));
                    // 0x48BDA0: a player in the race is reported out (0x48C220), then its bit leaves `+0xCC0`
                    self.server.out_of_race(slot as usize + 1);
                    self.server.leave(slot as usize + 1);
                    log.push(format!("slot {slot} left"));
                    // 0x48BE05: the room tells everyone left that the player has gone
                    self.dispatch(vec![(To::All, room::player_out(slot as i32 + 1))]);
                }
                Event::Quit { from } => log.push(format!("{from} quit")),
                Event::Found { from, describe } => {
                    let n = String::from_utf8_lossy(&describe.name)
                        .trim_end_matches('\0')
                        .to_string();
                    log.push(format!("found \"{n}\" at {from}"));
                    // the host has answered the probe: the handshake follows, carrying the identity
                    // string `name~version~game` (0x55CED0; the game is the answer's name, the
                    // version this install's own — 0x48DDC2's `NfsNet+0x2C`)
                    if let Some((host, who, _)) = self.join_wait.take() {
                        let game = nfs_net::comm::desc::split_name(&describe.name).0;
                        let id = nfs_net::comm::identity::encode(&who, &game_version(), &game);
                        log.push(format!("join sent, identity {:?}", String::from_utf8_lossy(&id[..id.len() - 1])));
                        if let Err(e) = self.net.join(host, &id) {
                            log.push(format!("join not sent: {e}"));
                        }
                    }
                }
                // a player's chat line reaches the host as a request, and the room object
                // passes it on rewritten (0x4912D4…0x4913AC)
                Event::Data { slot: Some(s), bytes, .. }
                    if self.is_host() && bytes.first() == Some(&kind::CHAT) =>
                {
                    if let Some(r) = ChatRequest::parse(&bytes) {
                        let out = self.room.route_chat(s as i32 + 1, r.to, &r.text);
                        self.dispatch(out);
                    }
                }
                // a player's `0xBC` record: the room's relay 0x48C050 sends it to the players in the
                // sender's lobby group (`+0xCC2`) and no others — the host's own copy included
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.first() == Some(&kind::OPTIONS) => {
                    let out = self.room.relay_options(s as i32 + 1, &bytes);
                    self.dispatch(out);
                }
                // the room's `0xB3` / `0xC5` / `0xB7` cases (0x49120C's jump table, 0x4917C0 / 0x49185B /
                // 0x4918BD): a player's request, and the room's answer to every link
                Event::Data { slot: Some(s), bytes, .. }
                    if self.is_host() && matches!(bytes.first(), Some(&kind::START) | Some(&kind::GO) | Some(&kind::RACE_TIME)) =>
                {
                    self.serve_race_request(s as i32 + 1, &bytes);
                }
                // a player's ping request reaches the host's room alone (0x491661)
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.first() == Some(&kind::PING) => {
                    if let Some(p) = room::Ping::parse(&bytes) {
                        self.ping_request(s as i32 + 1, p);
                    }
                }
                // **the input exchange** (network.md §11.17). The server dispatch 0x49120C hands a
                // message below `0xB0` to the player record's 0x48A2C0 → the room's 0x48C2B0: the
                // three bytes go into the block of the frame being filled
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.len() >= 3 && bytes[0] < 0xB0 => {
                    self.server.input(s as usize + 1, [bytes[0], bytes[1], bytes[2]]);
                }
                // and a frame from the host reaches a player (0x48F0DC)
                Event::Data { bytes, .. } if !self.is_host() && bytes.len() >= nfs_net::exchange::FRAME && bytes[0] < 0xB0 => {
                    self.exch_receive(&bytes);
                }
                Event::Data { bytes, .. } if bytes.first() != Some(&kind::CAR_STATE) => self.hear(&bytes),
                Event::Data { bytes, .. } => {
                    // **TEST GLUE** (not in the original): `NFS_NET_LAG_MS=<ms>` holds every car-state
                    // packet that long before the receiver hears it, to show the dead reckoning across
                    // a slow link (`network.md` 8.4)
                    match lag_glue() {
                        Some(ms) => self.lagged.push_back((Instant::now() + Duration::from_millis(ms), bytes)),
                        None => self.car_state_in(&bytes, tick, &mut log),
                    }
                }
            }
        }
        while self.lagged.front().is_some_and(|(due, _)| *due <= Instant::now()) {
            if let Some((_, bytes)) = self.lagged.pop_front() {
                self.car_state_in(&bytes, tick, &mut log);
            }
        }
        // 0x587CE0 can drop a peer whose countdown ran out, and that has to be reported too
        let mut due = self.last_poll.elapsed();
        let mut events = Vec::new();
        // at most a second's worth, so a stall does not drop everyone at once
        let mut budget = 64;
        while due >= POLL_PERIOD && budget > 0 {
            events.extend(self.net.tick());
            self.last_poll += POLL_PERIOD;
            due -= POLL_PERIOD;
            budget -= 1;
        }
        if budget == 0 {
            self.last_poll = Instant::now();
        }
        for e in events {
            match e {
                Event::PeerLeft { slot } => {
                    log.push(format!("slot {slot} timed out"));
                    self.server.out_of_race(slot as usize + 1);
                    self.server.leave(slot as usize + 1);
                    self.remotes
                        .retain(|_, r| Some(r.seat) != self.seats.get(slot as usize).copied());
                    self.dispatch(vec![(To::All, room::player_out(slot as i32 + 1))]);
                }
                other => log.push(format!("{other:?}")),
            }
        }
        // 0x48F0C4 / 0x48FBB6: the link to the host went away after we were in
        if !self.is_host() && self.join == JoinState::Joined && self.net.host_silent() {
            self.lost = true;
            self.dirty = true;
        }
        // the room's files go out a chunk at a time on their own clock (0x48EFB5…0x48F02E), on
        // the broadcast channel `[net+8]` — which is `send_game` here, relayed by the host
        let ms = self.started.elapsed().as_millis() as u32;
        for m in self.room.tick(ms) {
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "file chunk not sent");
            }
        }
        // the exchange's room tick 0x48B6B0 (the original runs it from the 128 Hz timer)
        if self.is_host() {
            log.extend(self.server_tick());
        }
        log
    }

    /// **0x48F5E8** for one `0xBE` datagram: our own id comes back on a relayed packet and is dropped;
    /// the sender's car is found (or given the next reserved seat) and 0x4A0DF0 stores the block.
    fn car_state_in(&mut self, bytes: &[u8], tick: u64, log: &mut Vec<String>) {
        // 0x48F5E8: our own id comes back on a relayed packet, and it is dropped
        let Some((who, st)) = CarState::from_packet(bytes, self.id) else {
            return;
        };
        let seat = match self.remotes.get(&who) {
            Some(r) => r.seat,
            None => {
                // the seat is the player's index among the others (`remote_players`);
                // a player the room has not announced takes the next free one
                let idx = self.remote_players().iter().position(|p| p.0 == who as i32);
                let Some(s) = idx.and_then(|k| self.seats.get(k)).or_else(|| self.seats.get(self.next_seat)).copied() else {
                    return;
                };
                self.next_seat += 1;
                log.push(format!("player {who} is driving seat {s}"));
                s
            }
        };
        // the packet carries the *sender's* tick (+0x04), so this line can be matched
        // against the sender's own log without the two clocks having to agree
        tracing::debug!(target: "netplay", "rx player={who} sender_tick={} pos={:?} steer={} brake={}", st.tick, st.position, st.steer, st.brake);
        if !self.driven.contains(&seat) {
            self.driven.push(seat);
        }
        // 0x48F671 → 0x4A0DF0: the store, with `gGameTick` as the receiver's clock
        self.remotes.entry(who).or_insert_with(|| Remote::new(seat)).store(st, tick as i32);
    }

    /// **0x4905A0** for the room's player `id` (`ecx` = its record): the ping to draw, `player+0x54`
    /// (−1 = none yet). While the connection is up (`net+8`, `+0xBF == 0`, `+0xBE != 0`) and no
    /// request is out (`+0x58 == 0`), a request goes out when more than a second has passed since
    /// the last (`1000 < GetTickCount() − +0x44`): `+0x58 = 1`, `+0x44 = now`, then 0x48D9F0 sends
    /// `0xC3(id, net+0x148)` to the host. The lobby's row draws (0x4BC04D, 0x4BADF9) call it every
    /// frame for each row that has a player.
    pub fn ping(&mut self, id: i32) -> i32 {
        self.ping_at(id, self.now())
    }

    fn now(&self) -> u32 {
        self.tick_override.unwrap_or_else(tick_ms)
    }

    fn ping_at(&mut self, id: i32, now: u32) -> i32 {
        let up = self.established();
        let Some(p) = self.pings.get_mut(&id) else { return -1 };
        if up && !p.pending && (now.wrapping_sub(p.asked) as i32) > 1000 {
            p.pending = true;
            p.asked = now;
            let req = room::Ping { id, word: self.net148 as i32 };
            if self.is_host() {
                // the host's own game is a player of its own room (0x48BB50): the request is
                // answered in process
                self.ping_request(0, req);
            } else if let Err(e) = self.net.send_game(&req.encode()) {
                tracing::debug!(error = %e, "ping request not sent");
            }
        }
        self.pings.get(&id).map_or(-1, |p| p.ping)
    }

    /// **The host's case 195, 0x491661…0x4916CE**: the sender's word is kept (`peer+0xB0`); then
    /// for the room's player with the asked id the sender is answered `0xC3(id, that player's
    /// peer+0xB0)` (0x4916B7…0x4916BC).
    fn ping_request(&mut self, from: i32, p: room::Ping) {
        self.reported.insert(from, p.word);
        if self.room.player(p.id).is_some() {
            let word = self.reported.get(&p.id).copied().unwrap_or(-1);
            self.dispatch(vec![(To::Player(from), room::Ping { id: p.id, word }.encode())]);
        }
    }

    /// **0x48F527…0x48F574 → 0x490600**: the player with the asked id (`+0x40`) takes the answer:
    /// `net+0x148 = now − player+0x44` (0x48DA30 → `[net+0x148]`, needing the connection up),
    /// `player+0x54 = word`, `player+0x58 = 0`.
    fn ping_answer(&mut self, p: room::Ping, now: u32) {
        let up = self.established();
        if let Some(e) = self.pings.get_mut(&p.id) {
            if up {
                self.net148 = now.wrapping_sub(e.asked);
            }
            let changed = e.ping != p.word;
            e.ping = p.word;
            e.pending = false;
            self.dirty = true;
            if changed {
                tracing::info!(target: "netplay", player = p.id, ping = p.word, own_round_trip = self.net148, "ping answer (0xC3)");
            }
        }
    }

    /// The lobby row draw's calls of 0x4905A0, one per room player that has a row: what a frame of
    /// `Comms.PlayerList` (0x4BBEB0) does. The rows are drawn only with the connection up.
    pub fn ping_step(&mut self) {
        let ids: Vec<i32> = self.room.players.iter().map(|p| p.id).collect();
        for id in ids {
            self.ping(id);
        }
    }

    /// The ping the lobby draws for player `id`: `min(player+0x54, 9999)` when it is above −1
    /// (0x4BC04D…0x4BC06A, signed compares), `None` otherwise.
    pub fn ping_shown(&self, id: i32) -> Option<u32> {
        let v = self.pings.get(&id)?.ping;
        (v > -1).then_some(v.min(9999) as u32)
    }

    /// **0x4B4DA0's numbering** (`player+0x4C`, and our own index into `[0x65741C]`): kept for the race,
    /// where 0x490140 and the seats read it. Players who are not in the race have none (−1).
    pub fn note_race_indices(&mut self, index: &[(i32, u32)]) {
        self.launch_index = index.to_vec();
    }

    /// The numbering the race reads. **Glue:** a `--net` CLI run (test scaffolding) never runs the
    /// launch 0x4B4DA0, so with none noted the room's list order stands in for it.
    fn index_table(&self) -> Vec<(i32, u32)> {
        if self.launch_index.is_empty() { self.race_indices() } else { self.launch_index.clone() }
    }

    /// `[0x65741C]` (`LOCAL_CAR`): our own race index.
    pub fn local_race_index(&self) -> Option<u32> {
        self.index_table().iter().find(|(id, _)| *id == self.id as i32).map(|(_, i)| *i)
    }

    /// The race index of the `k`-th **other** player in room order — the opponent seat `k` of the
    /// port (`remote_players`, 0x4B4DA0 numbering the list in order).
    pub fn seat_race_index(&self, k: usize) -> Option<u32> {
        self.index_table().iter().filter(|(id, _)| *id != self.id as i32).nth(k).map(|(_, i)| *i)
    }

    /// `net+8 != 0 && +0xBF == 0`: the session exists and its link has not dropped (0x48F0C4 sets
    /// `+0xBF` for the events 9 / 10 and for any `0xC6` other than 0xE, 0x48F34F).
    pub fn link_up(&self) -> bool {
        self.is_host() || (self.join == JoinState::Joined && !self.lost && self.state_in.is_none_or(|s| s == nfs_net::room::comm_state::CONNECTED))
    }

    /// **0x490140(race index)**, read to the instruction (`network.md` §11.13):
    ///
    /// * 3 when the session is gone (`net+8`, `+0xBF`), the list `net+0x260` is empty or holds no
    ///   player whose `+0x4C` is `race_index` (0x49016B…0x490179, 0x490187);
    /// * 4 when 0x490390 says the player is not racing: needs the connection up (`+0xBE`, `+0xDC ≥ 0`)
    ///   and its bit in `net+0xE5` (0x490199);
    /// * with `COMMSPOSITIONAL` (`[0x657408]`) set (0x4901A0): the local car's ping through 0x4905A0
    ///   × 0.001 (`[0x5B49A8]`), any other car's packet age `(game tick − car+0x3C) ×` 1/64
    ///   (`[0x5B2828]`), against `[0x5B2750]` = 1.0 (→ 2) and `[0x5B49A0]` = 0.3 (→ 1, else 0);
    /// * without it (0x490243): `net+0x128 + 4·index` × 0.001, against `[0x5B2778]` = 0.5 (→ 2) and
    ///   `[0x5B4998]` = 0.2 (→ 1, else 0) — 0 when the connection is not fully up.
    ///
    /// `game_tick` is `[0x606AC4]`. Like 0x4905A0 (which it calls for the local car) it may send a
    /// ping request.
    pub fn link_quality(&mut self, race_index: u32, game_tick: u32) -> LinkQuality {
        if !self.link_up() {
            return LinkQuality::NotListed;
        }
        // the player of `net+0x260` whose `+0x4C` is the index
        let Some(id) = self
            .index_table()
            .iter()
            .find(|(id, i)| *i == race_index && self.room.player(*id).is_some())
            .map(|(id, _)| *id)
        else {
            return LinkQuality::NotListed;
        };
        // 0x490390: connection up (`+0xBE`, `+0xDC >= 0`) and the player's bit of `net+0xE5`
        // (glue: with no launch ever run there is no `net+0xE5` to test)
        let racing = self.launch_index.is_empty() && self.in_race == 0 || (0..8).contains(&id) && self.in_race & (1u8 << id) != 0;
        if !(self.established() && racing) {
            return LinkQuality::NotRacing;
        }
        let (bad, fair) = if self.positional {
            let secs = if Some(race_index) == self.local_race_index() {
                f64::from(self.ping(id)) * 0.001
            } else {
                let last = self.remotes.get(&(id as u8)).map_or(0, |r| r.state.tick);
                f64::from(game_tick.wrapping_sub(last) as i32) * 0.015625
            };
            (secs > 1.0, secs > 0.3)
        } else {
            let ms = if self.established() { self.exch.sender.latency.get(race_index as usize).copied().unwrap_or(0) as i32 } else { 0 };
            let secs = f64::from(ms) * 0.001;
            (secs > 0.5, secs > 0.2)
        };
        match (bad, fair) {
            (true, _) => LinkQuality::Poor,
            (false, true) => LinkQuality::Fair,
            _ => LinkQuality::Good,
        }
    }

    /// **0x416310's network test** (FEGAME_TYPE 3, a racer still in state 0; 0x41637D…0x4163F5): with
    /// `COMMSPOSITIONAL` set the racer goes to state 3 when 0x490140 answers above 2; without it when
    /// the connection is not fully up (`net+8`, `+0xBF`, `+0xBE`, `+0xDC ≥ 0`, 0x4163B9…0x4163E1) —
    /// or `car+0xD8B == 1`, a flag the input-exchange mode would set and this port never does.
    pub fn retired(&mut self, race_index: u32, game_tick: u32) -> bool {
        if self.positional {
            self.link_quality(race_index, game_tick).gone()
        } else {
            // 0x4163B0: `car+0xD8B == 1` — the command code of the racer's last sample, which the
            // room writes (`ABSENT`) for a player that is not in the race — or no full connection
            !self.established() || self.commands.get(race_index as usize) == Some(&nfs_net::exchange::CMD_LEFT)
        }
    }

    // ------------------------------------------------------------------------------------------
    // the input exchange (`Comms.Positional` off) — network.md §11.17
    // ------------------------------------------------------------------------------------------

    /// Whether the race in play runs by input exchange (`[0x657408] == 0`, entered through
    /// [`NetPlay::exchange_race_frame`]).
    pub fn exchanging(&self) -> bool {
        self.exchange && !self.positional
    }

    /// **0x48B6B0**, for as long as the room is started (see [`nfs_net::exchange::Server::tick`]):
    /// the `0xC4` heartbeat to everyone, each player's next frame (reliably in the original), and the
    /// `0xC6 (4)` that drops a player 128 frames behind.
    fn server_tick(&mut self) -> Vec<String> {
        use nfs_net::exchange::Out;
        let mut log = Vec::new();
        if !self.server.started {
            return log;
        }
        let players: Vec<usize> = self.room.players.iter().filter_map(|p| usize::try_from(p.id).ok()).filter(|i| *i < 8).collect();
        let now = self.now();
        // 0x48B959…0x48B963: a frame the link does not take (no packet free, 0x595C58) stays and ends
        // the list loop; `spent` is what this call has already promised each link
        let mut spent = [0usize; 8];
        let addrs: Vec<Option<std::net::SocketAddr>> = (0..8).map(|k| if k == 0 { None } else { self.net.peer_addr(k as u32 - 1) }).collect();
        let net = &self.net;
        let mut can = |k: usize| {
            let ok = match addrs.get(k).copied().flatten() {
                Some(a) => net.reliable_room(a) > spent[k],
                None => true,
            };
            if ok {
                spent[k] += 1;
            }
            ok
        };
        let outs = self.server.tick(now, &players, &mut can);
        for o in outs {
            match o {
                Out::Beat { now } => self.dispatch(vec![(To::All, nfs_net::exchange::beat(now))]),
                Out::Frame { slot: 0, bytes } => self.exch_receive(&bytes),
                Out::Frame { slot, bytes } => {
                    if let Some(a) = self.net.peer_addr(slot as u32 - 1)
                        && let Err(e) = self.net.send_game_to(&[a], &bytes)
                    {
                        tracing::debug!(error = %e, "frame not sent");
                    }
                }
                Out::Drop { slot } => {
                    // 0x48B872…0x48B893: `0xC6 (4)` to it and its slot freed (0x4925A0)
                    log.push(format!("player {slot} is 128 frames behind: dropped (0xC6 4)"));
                    self.dispatch(vec![(To::Player(slot as i32), room::state(4))]);
                    self.net.remove_peer(slot as u32 - 1);
                    self.slots.clear(slot);
                    self.in_race &= !(1u8 << slot);
                    self.server.out_of_race(slot);
                    self.server.leave(slot);
                    self.dispatch(vec![(To::All, room::player_out(slot as i32))]);
                }
            }
        }
        log
    }

    /// **0x48F0DC…0x48F30F** — a frame from the room: [`nfs_net::exchange::Client::receive`] with the
    /// launch's numbering (`player+0x4C`) of the players on the list. A full ring is the original's
    /// error path (0x48F29B…0x48F2C0): `+0xC5`, `+0xBC` set, `+0xC4` cleared, and `0x48DA60(−2, −1)` —
    /// a `0xB7` to the room, which counts this machine out of the race.
    fn exch_receive(&mut self, pkt: &[u8]) {
        if self.id == u8::MAX || !self.established() {
            return;
        }
        let idx = self.index_table();
        let room = &self.room;
        let car = |k: usize| idx.iter().find(|(id, _)| *id == k as i32 && room.player(*id).is_some()).map(|(_, i)| *i as usize);
        let now = self.now();
        let me = self.id as usize;
        let taken = self.exch.receive(pkt, me, car, now);
        if let nfs_net::exchange::Received::Taken { overflow: true } = taken {
            tracing::warn!(target: "netplay", "the input ring is full: this machine is 128 entries behind the frames (0x48F29B) — it leaves the race");
            // 0x48F29B…0x48F2C0: `+0xC5 = 1`, `+0xBC = 1`, `+0xC4 = 0` and 0x48DA60(−2, −1) — the abort arm
            self.abort_race();
            self.lost = true;
            self.dirty = true;
        }
    }

    /// **The race frame's ring** (0x4152E0, `[0x6573E8] == 3`): after its barriers (0x415376) the ring is
    /// cleared (0x415E00, 0x41537B) and the timer's counters restart, **before** the `0xC5` request
    /// (0x4153AE) whose answer starts the room's frames; with `Comms.Positional` off the race is an
    /// input-exchange one from here (`network.md` §11.17). Called by `App::net_race_go`.
    pub fn exchange_race_frame(&mut self) {
        if !self.established() || self.in_race == 0 || self.id == u8::MAX {
            return;
        }
        self.exch.enter_race();
        self.timer_anchor = None;
        self.timer_done = 0;
        self.commands = [0; 8];
        self.exchange = !self.positional;
        // The original's first tick, `t = 0`, latches entry 0 — the zero sample 0x415E00 preloaded —
        // and frees it (0x415EF0). The port's ticks count from 1 (its `simulate` increments first), so
        // that latch is made here: without it entry 0 would stay full and the 128th write would find it so.
        if self.exchange {
            let n = self.index_table().len();
            self.exch.ring.latch(0, n);
        }
    }

    /// **0x413800 → 0x48CA00 → 0x48EB20**: one 32 Hz sample of this machine's controller — the sender
    /// packs every second one into three bytes (16 Hz) and they go to the host's room, unreliably
    /// (0x55D4D0's `0x80000000`); the host's own goes into its room directly (its link is to itself).
    pub fn exchange_sample(&mut self, s: nfs_net::exchange::Sample) {
        // 0x48EB28…0x48EB4A: an id, `[0x657408] == 0`, not stopped
        if !self.exchanging() || self.exch.stopped || self.id == u8::MAX || !self.established() {
            return;
        }
        let me = self.id as usize;
        let now = self.now();
        if let Some(b) = self.exch.sender.sample(s, me, now) {
            if self.is_host() {
                self.server.input(0, b);
            } else if let Err(e) = self.net.send_game(&b) {
                tracing::debug!(error = %e, "input packet not sent");
            }
        }
    }

    /// One turn of what runs beside the game loop in the original: the events, the room's tick and the
    /// 128 Hz timer's callbacks ([`NetPlay::exchange_callbacks`]). Answers how many 32 Hz samples are due.
    pub fn exchange_pump(&mut self, paused: bool) -> u32 {
        let _ = self.poll_lobby();
        if !self.exchanging() {
            return 0;
        }
        self.exchange_callbacks(Instant::now(), paused)
    }

    /// **0x4A45D0** — the timer callback, emulated (**glue:** the original's is a thread every
    /// 7.8125 ms), with **0x48EE90**'s clock pull beside it: while `net+0xC4` is set nothing counts
    /// (0x4A462C); while the game is paused (`[0x5CC81C]`) the callback returns (0x4A45F1); otherwise
    /// [`nfs_net::exchange::Timer::callback`] runs with 0x48D840's milliseconds since the start.
    /// Answers the number of 32 Hz samples now due (0x4A46CC: `0x412E70` then `0x413800`).
    pub fn exchange_callbacks(&mut self, now: Instant, paused: bool) -> u32 {
        if self.exch.waiting {
            return 0;
        }
        let anchor = *self.timer_anchor.get_or_insert(now);
        let period = Duration::from_nanos(7_812_500);
        let due = now.saturating_duration_since(anchor).as_nanos() / period.as_nanos();
        let mut samples = 0;
        while u128::from(self.timer_done) < due {
            self.timer_done += 1;
            if paused {
                continue;
            }
            let local = tick_ms_at(anchor + period * (self.timer_done as u32));
            self.exch.update_clock(local);
            let ms = self.exch.since_start(local);
            samples += self.exch.timer.callback(ms);
        }
        samples
    }

    /// **0x415EF0(frame)** — at an even game tick `t` the frame driver latches ring entry `t / 2` for
    /// every player of the race (`[0x658084]`), and each car's command code `car+0xD8B` is the sample's
    /// `[0x5E9664] >> 3` (0x4992F6).
    pub fn exchange_latch(&mut self, frame: u32) {
        let n = self.index_table().len();
        self.exch.ring.latch(frame, n);
        for p in 0..8 {
            self.commands[p] = self.exch.ring.latched[p][0] >> 3;
        }
    }

    /// The sample of the race index `p` the last latch took (`0x608B14 + 4·p`).
    pub fn latched(&self, p: usize) -> nfs_net::exchange::Sample {
        self.exch.ring.latched.get(p).copied().unwrap_or([0; 4])
    }

    /// **The frame driver's gate** (0x415455…0x415495, 0x415BD4): a tick may run while the ticks done
    /// stay at or below `min(0x6559F8, 2·F0 + 1) − 1`; without a connection (0x4154A4) the target is
    /// the input-frame counter's, `2·[0x5CC818]`. Answers how many ticks in all may have run.
    pub fn exchange_ticks_allowed(&self) -> u64 {
        if self.established() && !self.exch.stopped {
            u64::from(self.exch.allowed_ticks())
        } else {
            2 * u64::from(self.exch.input_frames) + 1
        }
    }

    /// The race is over for the exchange: nothing more is sampled or gated. (What the room is told is
    /// 0x48DAD0's `0xB7`s, [`NetPlay::leave_race`], sent by the front end's return.)
    pub fn exchange_leave(&mut self) {
        self.exchange = false;
    }

    /// The front end's poll: the same drain as the race's, with no seats to drive. Answers whether
    /// the room or the chat changed, i.e. whether the lists want binding again.
    pub fn poll_lobby(&mut self) -> bool {
        for line in self.poll(0) {
            tracing::info!(target: "netplay", "{line}");
        }
        std::mem::take(&mut self.dirty)
    }

    /// **0x4A03D0 for the seats whose store asked for it** (`snap_due`): the block's velocity and matrix
    /// go straight into the car (`Vehicle::net_store_snap`). The original runs it inside the receiver
    /// (0x4A0E70), on the network thread, before the next 64 Hz tick reads the car; the port runs it
    /// right after the drain, before the tick's physics.
    pub fn store_snaps(&mut self, mut car: impl FnMut(usize, &nfs_physics::vehicle::NetBlock)) {
        for r in self.remotes.values_mut() {
            if std::mem::take(&mut r.snap_due) {
                car(r.seat, &r.block());
            }
        }
    }

    /// **0x4991E0's remote arm** (0x499285…0x4992CD), which a car with `car+0xDA8 == 0` and `car+0xDAC` set
    /// takes: the controller is the last block — the steering `car+0xD90 = (i8) +0x38`, the brake
    /// `car+0xD7D = +0x3A`, `car+0xD85` the low bit of `+0x3B` (the handbrake bit of the packed buttons).
    /// The throttle `car+0xD7C` is not written from the wire and the gear is forced neutral by the apply,
    /// so it is 0 here. `None` before the first block or while the car is stale (`+0x70`: the AI drives).
    pub fn remote_controls(&self, seat: usize) -> Option<nfs_physics::vehicle::Controls> {
        let r = self.remotes.values().find(|r| r.seat == seat).filter(|r| r.got && !r.held && !r.stale)?;
        Some(nfs_physics::vehicle::Controls {
            throttle: 0,
            brake: r.state.brake,
            steer: r.state.steer as i32,
            handbrake: r.state.buttons & 1 != 0,
            command: 0,
        })
    }

    /// **0x49B9AB…0x49B9EC** — the gate in `Car_integrateOrient` 0x49B8E0, then the apply 0x4A0A40
    /// (`Vehicle::net_dead_reckon`): `[0x657408]` (a positional race — the caller), `car+0xDAC` (a block
    /// came) and `car+0x70` clear (else `+0xDA8 = 1` and nothing moves). The class test 0x490140 ≤ 2 is the
    /// caller's (`NetPlay::link_quality`). `centre` is the seat's slice centre (the retrack's walk).
    /// Answers whether the seat is a remote car with a block.
    pub fn apply(
        &mut self,
        seat: usize,
        v: &mut Vehicle,
        specs: &nfs_physics::specs::CarSpecs,
        grid: &nfs_physics::collision::TriangleGrid,
        centre: Option<Vec3>,
        tick: u64,
    ) -> bool {
        let Some(r) = self.remotes.values_mut().find(|r| r.seat == seat) else {
            return false;
        };
        if !r.got {
            return false;
        }
        if r.stale {
            r.held = true;
        } else {
            r.held = false;
            let blk = r.block();
            if v.net_dead_reckon(&blk, tick as i32, grid, specs, centre) {
                // 0x4A0DC3: `|age| > 0x100` — the next ticks leave the car alone until the lag recovers
                r.stale = true;
            }
        }
        true
    }

    /// The send gate of 0x48EE20 — once every eight ticks, the 40-byte `0xBE` packet.
    pub fn send(&mut self, tick: u64, v: &Vehicle) {
        if tick & TICK_MASK != TICK_MASK || self.id == u8::MAX || !self.positional {
            return;
        }
        let m = v.orientation.transpose();
        let st = CarState {
            // `car+0xD94` is the ramped steering the port keeps in `Vehicle::steer`, and
            // `car+0xD80` the effective brake, which 0x49E11C reads as a 0..252 byte
            steer: v.steer.clamp(-126, 126) as i8,
            airborne: v.height_marker >= 0.5,
            brake: (v.brake * 252.0).clamp(0.0, 252.0) as u8,
            // the packed controller buttons of [0x5E9664]; bits 0 and 1 are UNKNOWN and the
            // command nibble has no equivalent here, so nothing is invented
            buttons: 0,
            tick: tick as u32,
            // `car+0x38C`, the second component of the angular velocity — the yaw rate
            yaw_rate: v.angular_velocity.y,
            orientation: nfs_net::state::pack_orientation([
                m.x_axis.to_array(),
                m.y_axis.to_array(),
                m.z_axis.to_array(),
            ]),
            velocity: v.velocity.to_array(),
            position: v.position.to_array(),
        };
        tracing::debug!(target: "netplay", "tx tick={tick} pos={:?} steer={} brake={}", st.position, st.steer, st.brake);
        if let Err(e) = self.net.send_game(&st.packet(self.id)) {
            tracing::debug!(error = %e, "car state not sent");
        }
    }

    /// `Comms.PlayerList` — **0x4BBBD0**, the accessor's refill: one row per player on
    /// `net+0x260` in the order the room announced them, the name as it came (`player+0`) and the
    /// id `player+0x40` kept with the row (`+0xC`, which 0x4BBB40 and 0x4BBB60 read), then rows of
    /// FE 287 "empty" with id −1 until there are eight (0x4BBD07…0x4BBDA9). *(Until 2026-09-25 this
    /// answered "HOST (YOU)" and "PLAYER n", upper-cased — not the original's.)*
    ///
    /// Not carried: the row draw 0x4BBEB0, which numbers the rows and marks a player whose ready
    /// bit (`net+0xE5`, 0x490390) is set — `READY` has no port.
    pub fn players(&self, empty: &str) -> Vec<(String, i32)> {
        let mut v: Vec<(String, i32)> = self
            .room
            .players
            .iter()
            .map(|p| (p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect(), p.id))
            .collect();
        while v.len() < room::MAX_PLAYERS {
            v.push((empty.to_string(), -1));
        }
        v
    }

    /// Step `Comms.PlayerList`'s selection. With none yet the step lands on row 0.
    pub fn step_player(&mut self, delta: i32) {
        let n = room::MAX_PLAYERS.max(self.room.players.len()) as i32;
        let cur = self.player_sel.map_or(0, |s| s as i32 + delta);
        self.player_sel = Some(cur.rem_euclid(n) as usize);
    }

    /// Open or close the chat box's callback — the box's slots 21 and 22 (0x4C2940, 0x4C2A80).
    /// Opening drains what the box held (0x4C2A10…0x4C2A6A), so a box always opens empty.
    pub fn chat_box(&mut self, open: bool) {
        if open && !self.chat_open {
            self.chat.clear();
        }
        self.chat_open = open;
    }

    /// The chat input box's Return — **0x4C3310**: the addressee is the selected player row's id
    /// when `Comms.PlayerList` has a selection (0x4BC630, 0x4BBB40), everyone otherwise, and the
    /// line goes out through **0x48D9B0**, on the link to the host. The host's own line is the
    /// room object's to pass on straight away. Nothing is sent before the session has given us an
    /// id (0x48D9B0's gate).
    pub fn say(&mut self, text: &str, empty: &str) {
        if self.id == u8::MAX {
            return;
        }
        let to = self
            .player_sel
            .and_then(|r| self.players(empty).get(r).map(|p| p.1))
            .unwrap_or(room::TO_ALL);
        // the box took only 0x20…0xFE (0x4CCD95…0x4CCD9D), one byte each
        let text: Vec<u8> = text.chars().map(|c| c as u32).filter(|&c| c < 0x100).map(|c| c as u8).collect();
        if self.is_host() {
            let out = self.room.route_chat(0, to, &text);
            self.dispatch(out);
        } else if let Err(e) = self.net.send_game(&ChatRequest { to, text }.encode()) {
            tracing::warn!(error = %e, "chat line not sent");
        }
    }

    /// The chat box's rows, formatted the way **0x4C24F0** formats each line: `"%s: %s"`
    /// (0x5D7E24) for a line to everyone, FE 580 `IDS_WHISPER` for one to us, FE 581
    /// `IDS_WHISPERTO` otherwise. The names come from `Comms.PlayerList` (the box's
    /// `data.accessor`, `[this+0x154]`): 0x4BBB60 finds the row with that id and answers **row 0**
    /// when there is none. "Us" is `net+0xDC`, or −2 with no session (0x4C2559…0x4C257B).
    ///
    /// The original formats a line once, when its box's tick 0x4C2DB0 drains it; this formats the
    /// kept lines each time the list is bound, so a name that changes in between would read the
    /// new way — which no shipped path does.
    pub fn chat_rows(&self, empty: &str, whisper: &str, whisper_to: &str) -> Vec<String> {
        let rows = self.players(empty);
        let name = |id: i32| rows.iter().find(|r| r.1 == id).or(rows.first()).map_or(String::new(), |r| r.0.clone());
        let me = if self.id == u8::MAX { -2 } else { self.id as i32 };
        self.chat
            .iter()
            .map(|l| {
                let text: String = l.text.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                if l.to == room::TO_ALL {
                    room::format("%s: %s", &[&name(l.from), &text])
                } else if l.to == me {
                    room::format(whisper, &[&name(l.from), &text])
                } else {
                    room::format(whisper_to, &[&name(l.from), &name(l.to), &text])
                }
            })
            .collect()
    }

    /// **0x427A60**, from the race load 0x4B67B0 (which zeroes the game clock at 0x4B67F3): read
    /// `pic16.fsh` from the HUD folder and post it as file type 0 (0x48CB50), so every other
    /// player's `HLeader` can show it.
    pub fn post_picture(&mut self, bytes: &[u8]) {
        self.room.post(room::PICTURE, bytes);
    }

    /// **`HLeader`'s** picture (draw 0x4286F0): the car in first place is `seat` (`None` for our
    /// own), its player is the one whose car it is (0x48D720 matches `player+0x4C` against
    /// `car+0x524`; here a seat is a remote id's through [`Remote::seat`] — **APPROXIMATION**, as
    /// what writes `player+0x4C` has not been read), and its picture is file type 0 once whole
    /// (0x4903F0, 0x428772). The upload takes the file's first image, the record `[file+0x14]`
    /// points at (0x42878E) — an FSH's first directory entry.
    pub fn leader_picture(&mut self, r: &mut Renderer, seat: Option<usize>) -> Option<TextureId> {
        let id = match seat {
            None => self.id as i32,
            Some(s) => *self.remotes.iter().find(|(_, x)| x.seat == s)?.0 as i32,
        };
        self.picture_of(r, id).map(|(t, _, _)| t)
    }

    /// The picture of player `id` once its file is whole, uploaded once: texture, width, height
    /// (0x4903F0 finds the file of type 0, 0x428772 / 0x4A51F5 test `+0xC >= +8`).
    pub fn picture_of(&mut self, r: &mut Renderer, id: i32) -> Option<(TextureId, u32, u32)> {
        if let Some(t) = self.pictures.get(&id) {
            return *t;
        }
        let f = self.room.player(id)?.files.iter().find(|f| f.kind == room::PICTURE && f.complete())?;
        let tex = nfs_formats::fsh::Fsh::parse(&f.bytes)
            .ok()
            .and_then(|fsh| fsh.decode(&f.bytes, 0).ok())
            .map(|img| (r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)), img.width as u32, img.height as u32));
        self.pictures.insert(id, tex);
        tex
    }

    /// **0x4A4A70(step)'s send, 0x4A4AD4…0x4A4B4F**: with the connection up (`net+8`, `+0xBF == 0`,
    /// `+0xBE`, `+0xDC ≥ 0`), the screen up (`[0x655A28]`, the caller's) and `step > 0`, the record
    /// `(0x256, [0x65741C], step)` goes out through 0x48D550 — every player's bar. The sender hears
    /// it back from the host's relay in the original; here — **glue**, as the other 0x48D550
    /// senders — it is applied to our own table at once.
    pub fn send_loading_step(&mut self, step: i32) {
        if step <= 0 || !self.established() {
            return;
        }
        let rec = vec![nfs_net::options::id::LOADING, self.local_race_index().unwrap_or(0), step as u32];
        for m in nfs_net::options::messages_each(std::slice::from_ref(&rec)) {
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "loading step not sent");
            }
        }
        self.launch.take(&rec);
        tracing::info!(target: "netplay", record = ?rec, "loading step sent (0xBC 0x256)");
    }

    /// **0x4A4A70's per-player block, 0x4A5149…0x4A5353**, while the screen is up and the
    /// connection is: for each of the race's `NUMPLAYERRACECARS` slots — the player whose `+0x4C` is
    /// the slot (0x48D720) that is racing (0x490390: its bit of `net+0xE5`) with a whole picture
    /// gets its picture at `(74·grid + 32, 364)` and the two bars `(x, 436, 6)` by the progress its
    /// `0x256` reported; a slot without a racing player a 54 × 64 black box (0x4A52FC); a player
    /// whose picture has not come whole nothing (0x4A5208 `je 0x4A5327`). `grid` is the slot's
    /// `.STARTINGPOS` (`[0x658200 + slot·0x580]`).
    pub fn loading_rows(&mut self, r: &mut Renderer) -> Vec<LoadingRow> {
        if !self.established() {
            return Vec::new();
        }
        let table = self.index_table();
        let mut out = Vec::new();
        for slot in 0..table.len() as u32 {
            let x = 74.0 * self.launch.grid.get(&slot).copied().unwrap_or(0) as f32 + 32.0;
            let who = table.iter().find(|(_, i)| *i == slot).map(|(id, _)| *id).filter(|id| self.room.player(*id).is_some());
            let racing = who.is_some_and(|id| (0..8).contains(&id) && self.in_race & (1u8 << id) != 0);
            match who.filter(|_| racing) {
                None => out.push(LoadingRow::Absent { x }),
                Some(id) => {
                    if let Some((picture, w, h)) = self.picture_of(r, id) {
                        out.push(LoadingRow::Player { x, picture, w: w as f32, h: h as f32, progress: self.launch.progress.get(&slot).copied().unwrap_or(0) });
                    }
                }
            }
        }
        out
    }

    /// `Comms.Data(GAMENAME)` — the session's own name.    /// `Comms.Data(GAMENAME)` — the session's own name.
    pub fn game_name(&self) -> String {
        nfs_net::comm::desc::split_name(&self.net.name).0.trim().to_string()
    }

    /// `Comms.Data(MYIPADDRESS)` — the machine's address as the room shows it (0x50BF09), which
    /// is [`nfs_net::socket::my_ip_string`].
    pub fn my_address(&self) -> String {
        nfs_net::socket::my_ip_string()
    }

    /// **`[net+0x24]`** as 0x509550 reads it (0x48CA40), `nfs_net::room::comm_state`; the writers are
    /// `network.md` §11.10's table. A host is `HOSTING` (0x48DF69). A joiner is `CONNECTING` (0x48E4B4,
    /// 0x48E68E) until the host's `0xC6 (0xE)` arrives; `0xC6` with another value is that state (9
    /// ejected by the host 0x491746, 10 the room's version / game gate 0x490AB0 / 0x49157A, 11 full
    /// 0x49159E, 12 the host closing the room 0x48C035).
    ///
    /// The comm layer's own events give **4** (0x48F0C4, 0x48FBB6): a connect that runs out
    /// ([`JOIN_TIMEOUT`], conman event 5) and a host that goes silent after we were in (events 9 / 10).
    /// **APPROXIMATION** of the two joins' refusals, which the original never had in this form (its
    /// version / game / full checks are the room's `0xC6` after a link that is up): a refused `'a'`
    /// maps to 11 (`Full`, −2), 10 (`Refused`, −3: the identity gate of `nfs_net::session`) and — with
    /// no original writer on a LAN, state 8 is the lobby service's `0xC6` value — 8 (`NameTaken`, −1,
    /// the comm layer's own name test 0x588E7E, whose game-level consequence was not traced). States 6
    /// and 7 have no writer in the game's code at all (no `0xC6` the room server sends carries them), so
    /// nothing here produces them.
    pub fn comm_state(&self) -> u32 {
        use nfs_net::comm::JoinStatus;
        use nfs_net::room::comm_state as cs;
        if self.is_host() {
            return cs::HOSTING;
        }
        if self.lost {
            return cs::LOST;
        }
        match self.join {
            JoinState::Hosting => cs::HOSTING,
            JoinState::Pending(t) if t.elapsed() > JOIN_TIMEOUT => cs::LOST,
            JoinState::Pending(_) => cs::CONNECTING,
            JoinState::Joined => self.state_in.unwrap_or(cs::CONNECTING),
            JoinState::Refused(JoinStatus::NameTaken) => cs::NAME_TAKEN,
            JoinState::Refused(JoinStatus::Full) => cs::FULL,
            JoinState::Refused(_) => cs::WRONG_VERSION,
        }
    }

    /// `net+0xBE` with an id (0x4EDCD0 ≥ 0): the connection is established — every test the lobby
    /// screens' ticks make first.
    pub fn established(&self) -> bool {
        self.is_host() || (self.join == JoinState::Joined && self.state_in == Some(nfs_net::room::comm_state::CONNECTED) && !self.lost)
    }

    /// The other players in the room's order — their race index among the others is their seat
    /// (0x4B4DA0 numbers `player+0x4C` in list order over the players `net+0xE5` marks, and the
    /// opponents are all but our own): `(id, name, slot record)`.
    pub fn remote_players(&self) -> Vec<(i32, String, nfs_net::slots::PlayerSlot)> {
        self.room
            .players
            .iter()
            .filter(|p| p.id != self.id as i32)
            .map(|p| {
                let name = p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                let slot = usize::try_from(p.id).ok().and_then(|i| self.slots.slot.get(i)).cloned().unwrap_or_default();
                (p.id, name, slot)
            })
            .collect()
    }

    /// Our own slot in the table: the player id (0x4EDCD0).
    pub fn my_slot(&self) -> Option<usize> {
        (self.id != u8::MAX).then_some(self.id as usize).filter(|i| *i < nfs_net::slots::SLOTS)
    }

    /// **0x48D550 with our own slot records**: sent to the room, and — **glue** — applied to our own
    /// table at once, where the original hears them back from the host's relay (0x48C050 sends to
    /// every player with the sender's `+0xCC2` bit, the sender included; this port's relay leaves
    /// the sender out).
    pub fn send_slot_records(&mut self, recs: &[Vec<u32>]) {
        for m in nfs_net::options::messages_each(recs) {
            if self.is_host() {
                // the host's own record goes to its room like anyone's, and the room's relay
                // (0x48C050) sends it to the players in the host's lobby group only
                let out = self.room.relay_options(self.id as i32, &m);
                self.dispatch(out);
            } else if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "slot record not sent");
            }
        }
        for r in recs {
            self.slots.apply(r);
            tracing::info!(target: "netplay", record = ?r, "player slot record sent (0xBC)");
        }
        self.dirty = true;
    }

    /// **0x5091B0** over the table: every player in the room has `.READY` set and nobody is in the
    /// race yet (`net+0xE5 == 0`) — the create screen's PLAYGAME is live then.
    pub fn everyone_ready(&self) -> bool {
        self.established()
            && self.in_race == 0
            && self.room.players.iter().all(|p| usize::try_from(p.id).ok().and_then(|i| self.slots.slot.get(i)).is_some_and(|s| s.ready))
    }

    /// `EJECTPLAYER` — the host's request `0xB1 (id)` reaches its own room, whose server dispatch
    /// (0x4916E1…0x491765) refuses the host itself (`+0xB9`, 0x49171D) and otherwise sends the
    /// target `0xC6 (9)` "Ejected by host" (0x491746…0x49174D), frees it (0x4925A0) and tells
    /// everyone `0xB1`. Answers whether a player was ejected.
    pub fn eject(&mut self, player: i32) -> bool {
        if !self.is_host() || player <= 0 {
            return false;
        }
        let slot = (player - 1) as u32;
        let Some(a) = self.net.peer_addr(slot) else { return false };
        let _ = self.net.send_game_to(&[a], &room::state(nfs_net::room::comm_state::EJECTED));
        self.net.remove_peer(slot);
        self.slots.clear(player as usize);
        self.in_race &= !(1u8 << player);
        self.dispatch(vec![(To::All, room::player_out(player))]);
        true
    }

    /// `'K'` — tell the others we are going (0x589152 clears our bit on the host). The comms
    /// screens' back sends it (`App::fe_session_leave`, 0x48E6F0 / 0x48E9C0).
    pub fn leave(&mut self) {
        // 0x48E9C0 (0x48EA0D…0x48EA38): a host's teardown asks its room to eject its own player
        // (`0xB1` with its own id), and the room's answer to that (0x491717…0x491723 → 0x48C000) frees
        // every other player and sends each `0xC6 (0xC)` — their state 12, the notice 830 and back
        if self.is_host() {
            for a in self.net.everyone() {
                let _ = self.net.send_game_to(&[a], &room::state(nfs_net::room::comm_state::LOST_2));
            }
        }
        let _ = self.net.leave();
    }
}

/// The front end's side of network play.
///
/// The four network layouts name exactly which accessors and callbacks they want, and those names
/// are data, not guesses — `FEData/Layouts/{serverlist,udpip,PeerToPeerIP,modem}.lay`:
///
/// | layout | `callbacklist=` | accessors | callbacks |
/// |---|---|---|---|
/// | `serverlist` | `ID_COMMSSERVERSCREEN` | `Comms.ServerList` | `ENTER` |
/// | `udpip` | `ID_COMMSWEBGAMESSCREEN` | `Comms.GamesList`, `Comms.LobbyPlayerList`, `Comms.ChatInput` | `CREATE`, `JOIN` |
/// | `PeerToPeerIP` | `ID_PEERTOPEERIP` | `Comms.IP`, `Comms.IPList` | `CREATE`, `CONNECT` |
/// | `modem` | — | `Comms.ModemList`, `Comms.PhoneList`, `Comms.PhoneNumber` | `CREATE`, `DIAL` |
///
/// A session found by a `'w'` probe appears in the games list, `CREATE` hosts, `JOIN`/`CONNECT`
/// joins. `Comms.LobbyPlayerList` is **not** filled from a browse: 0x4BAD40 lists only `net+0x260`
/// of a connected NfsNet (`network.md` §11.14). The modem accessors stay empty — this port
/// has no modem, and an empty list is what a machine without one shows.
pub struct Lobby {
    net: Net,
    /// the sessions a `'w'` probe turned up, newest first
    pub found: Vec<(SocketAddr, nfs_net::comm::Describe)>,
    /// When each `found` entry last answered a probe. **APPROXIMATION** (port's choice): the
    /// original's server list is redrawn from the live peer list (0x4B9780 walks it under the
    /// NfsNet lock and skips a peer whose ping 0x48C6E0 is negative), so a host that goes away
    /// leaves it; a `'d'` carries no ping, so the port drops an entry that has not answered for
    /// [`SESSION_TTL`].
    seen: HashMap<SocketAddr, Instant>,
    /// The round trip of each listed session's last `'d'`, in milliseconds — what the server
    /// list's ping column shows (`ID_PINGPOS`, 0x4B9780 draws 0x48C6E0's value, at most 9999).
    /// Measured from the probe that asked to the answer that came.
    pub ping: HashMap<SocketAddr, u32>,
    /// `Comms.IP` — the string accessor's text as the last commit left it
    pub ip: String,
    /// `Comms.IPList` — a `FECharVectAccessor` (0x50B8BE) whose rows are the addresses committed
    /// so far, the newest at row 0 (0x4C2120 / 0x4C2220 call 0x4B8B70 with position 0 and then
    /// select row 0).
    pub history: Vec<String>,
    /// the selected row of `Comms.IPList`
    pub ip_sel: usize,
    /// which row of the games list is selected
    pub sel: usize,
    /// `Comms.Positional` / `Comms.Data(POSITIONALUPDATES)` — whether the `0xBE` car-state
    /// broadcast of §5.2 is on. The host's screen has it as a slider and the joiner's as a watch,
    /// so it is the host's setting; the port keeps it here and [`NetPlay::positional`] carries it
    /// into the race.
    pub positional: bool,
    last_probe: Instant,
    who: String,
    /// a send error has been reported already (once per screen is enough)
    warned: bool,
    /// whether [`Self::commit_ip`] writes [`history_path`] (off in the tests)
    persist: bool,
}

/// One row of the sessions list — see [`Lobby::rows`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub name: String,
    pub players: u32,
    pub limit: u32,
    pub addr: String,
    pub ping: Option<u32>,
    /// what follows the `~` in the session's name (0x48C3D0's `%s`)
    pub version: String,
    /// `desc+0x21`, `Comms.Positional`'s index — the games list's MODE column (0x4BA030 indexes
    /// the table 0x5B9674 with it)
    pub mode: u8,
    /// `desc+0x26`, the host's `TRACKLOCALEID0` — the TRACK column shows `FEStrings[id + 0x28]`
    pub track_id: u16,
}

/// What names a session across the answers of one probe round: the address record of its `'d'`
/// (+0x04), when it names an address at all.
fn merge_key(d: &nfs_net::comm::Describe) -> Option<nfs_net::wire::Addr> {
    d.from.ipv4().filter(|ip| *ip != [0; 4]).map(|_| d.from)
}

/// How long a listed session may stay silent before it leaves the list — see [`Lobby::seen`].
const SESSION_TTL: Duration = Duration::from_secs(4);

/// The file the port keeps `Comms.IPList` in. **APPROXIMATION** of where: the original writes the
/// list into the profile's option block (`savegame.md` §6: `FECharVectAccessor` saves `u32 n`, the
/// `n` strings and the selection); the port's profile save carries no such block, so the list is
/// a line-per-address file beside the saves.
/// `Comms.GameName`'s capacity - `0x4BC8B0("Comms.GameName", 0, 0xF, 0)` (0x50B953…0x50B961).
pub const GAME_NAME_MAX: usize = 15;

/// The name dialog's field capacity: 0x509E40 builds it with `0x5140B0(0x259, 0, 0x19, 0, 0)`.
pub const GAME_NAME_DIALOG_MAX: usize = 0x19;

/// Where `Comms.GameName`'s text is kept between runs. **Glue**: the original keeps it in the
/// profile's accessor block (`savegame.md` §6); the port keeps a text file like the IP list's.
pub fn game_name_path() -> std::path::PathBuf {
    nfs_game::save::save_dir().join("comms_gamename.txt")
}

pub fn history_path() -> std::path::PathBuf {
    nfs_game::save::save_dir().join("comms_iplist.txt")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path())
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

impl Lobby {
    /// Open a browsing socket. Port 0, so a second copy on the same machine can browse too.
    pub fn open(who: &str) -> anyhow::Result<Self> {
        let sock =
            CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
        Ok(Self {
            net: Net::client(sock, KEY, name16(who)),
            found: Vec::new(),
            seen: HashMap::new(),
            ping: HashMap::new(),
            ip: String::new(),
            history: load_history(),
            ip_sel: 0,
            sel: 0,
            positional: true,
            last_probe: Instant::now() - Duration::from_secs(9),
            who: who.to_string(),
            warned: false,
            persist: true,
        })
    }

    /// `NFS5.CHARVECTACCESSORINPUTBOX`'s commit (0x4C2120 on Return, 0x4C2220 on release): a
    /// non-empty string goes into the list at position 0 (0x4B8B70) — **whatever the list already
    /// holds**, the code has no duplicate test — and the list selects row 0 (slot 0x2C(0)).
    pub fn commit_ip(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.history.insert(0, text.to_string());
        self.ip_sel = 0;
        self.ip = text.to_string();
        if !self.persist {
            return;
        }
        let path = history_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, self.history.join("\n") + "\n") {
            tracing::warn!(error = %e, path = %path.display(), "cannot keep Comms.IPList");
        }
    }

    /// `Comms.IPList`'s selection moved to `row` (a drop-down pick or a step): the box mirrors the
    /// list's current text while it is not focused (0x4C2310: when the list's text differs from
    /// the string's, the string is cleared, set to the list's and updated). Answers that text.
    pub fn pick_ip(&mut self, row: usize) -> Option<String> {
        let t = self.history.get(row)?.clone();
        self.ip_sel = row;
        self.ip = t.clone();
        Some(t)
    }

    /// Shout a `'w'` once a second and collect the `'d'` answers. The IP transport's own
    /// "announce" is a stub (0x560610 `mov eax, 1`, §6), but its **browse** is not: 0x560620 sends
    /// the session key as a `'w'` with a null address, i.e. the broadcast of 0x5872C0 — that is
    /// what [`nfs_net::socket::probe_targets`] starts with. Its other entries are platform glue
    /// and are labelled there. A typed or picked address is probed directly as well.
    pub fn poll(&mut self) {
        if self.last_probe.elapsed() >= Duration::from_secs(1) {
            let sent = self.net.probe_all(port());
            // macOS refuses local-network sends (EHOSTUNREACH / EPERM) until the user has allowed
            // "Local Network" for the app: when *nothing* could be sent, say so once, in the log
            // (one refusal among several is only a route the machine has not got)
            if !self.warned
                && let Some((kind, to, Err(e))) = sent.iter().find(|(_, _, r)| r.is_err())
                && sent.iter().filter(|(k, _, _)| *k != nfs_net::socket::ProbeKind::Loopback).all(|(_, _, r)| r.is_err())
            {
                tracing::warn!(target: "netplay", ?kind, %to, error = %e, "no probe could be sent — on macOS check System Settings > Privacy & Security > Local Network");
                self.warned = true;
            }
            if let Ok(a) = self.ip.trim().parse::<SocketAddr>() {
                let _ = self.net.probe_at(a);
            } else if let Ok(ip) = self.ip.trim().parse::<Ipv4Addr>() {
                let _ = self.net.probe_at(SocketAddr::V4(SocketAddrV4::new(ip, port())));
            }
            self.last_probe = Instant::now();
        }
        let now = Instant::now();
        for e in self.net.poll() {
            match e {
                Event::Found { from, describe } => self.note_found(from, describe, now),
                _ => {}
            }
        }
        // a session that has stopped answering leaves the list; only what this poll itself heard is
        // ever dropped (an entry with no timestamp was put there by hand — the layout sweep does)
        let before = self.found.len();
        let seen = &self.seen;
        self.found.retain(|(a, _)| seen.get(a).is_none_or(|t| now.duration_since(*t) < SESSION_TTL));
        if self.found.len() != before {
            self.sel = self.sel.min(self.found.len().saturating_sub(1));
        }
        self.net.tick();
    }

    /// One `'d'` answer heard from `from`.
    fn note_found(&mut self, from: SocketAddr, describe: Box<nfs_net::comm::Describe>, now: Instant) {
        // **One row per session.** The original probes with one broadcast and hears one
        // `'d'` per host; the port's probe also goes out over every interface and the
        // loopback (`nfs_net::socket::probe_targets`, glue), so one host answers several
        // times, from a different address each. The answers name the same session by the
        // address record a `'d'` carries at `+0x04` (the host's own address,
        // [`advertised_record`]); those are one row, kept at a non-loopback address unless
        // only the loopback answered. A record that names no address (0.0.0.0) cannot say
        // which host it is, so it is only ever merged with the same source address.
        let key = merge_key(&describe);
        let at = self
            .found
            .iter()
            .position(|(a, d)| *a == from || key.is_some() && merge_key(d) == key);
        let kept = match at {
            Some(i) => {
                let old = self.found[i].0;
                let addr = if old.ip().is_loopback() && !from.ip().is_loopback() { from } else { old };
                if addr != old {
                    self.seen.remove(&old);
                    self.ping.remove(&old);
                }
                self.found[i] = (addr, *describe);
                addr
            }
            None => {
                let n = String::from_utf8_lossy(&describe.name).trim_end_matches('\0').trim().to_string();
                tracing::info!(target: "netplay", %from, name = %n, slots = describe.player_limit, mask = describe.slot_mask, "session found");
                self.found.insert(0, (from, *describe));
                self.found.truncate(16);
                from
            }
        };
        self.seen.insert(kept, now);
        if from == kept {
            self.ping.insert(kept, now.duration_since(self.last_probe).as_millis().min(9999) as u32);
        }
    }

    /// The pieces of one listed session: its name (`+0x28` of the `'d'`, trimmed), how many of the
    /// limit are taken (`popcount(mask) + 1` of `+0x58`, the host being one), the address it
    /// answered from and the measured ping.
    pub fn rows(&self) -> Vec<SessionRow> {
        self.found
            .iter()
            .map(|(a, d)| {
                // 0x48C3D0: `sscanf(desc, "%[^~]~%s", name, version)`
                let (name, version) = nfs_net::comm::desc::split_name(&d.name);
                SessionRow {
                    name,
                    players: d.slot_mask.count_ones() + 1,
                    limit: d.player_limit.max(1),
                    addr: a.to_string(),
                    ping: self.ping.get(a).copied(),
                    version,
                    mode: d.bytes[nfs_net::comm::desc::POSITIONAL],
                    track_id: d.words[nfs_net::comm::desc::TRACK],
                }
            })
            .collect()
    }

    /// `Comms.IPList`: the addresses committed so far (see [`Self::commit_ip`]).
    pub fn addresses(&self) -> Vec<String> {
        self.history.clone()
    }

    /// What `CONNECT` (0x50B150) connects to: the text of the `Comms.IPList` accessor's current
    /// row (`[this+0x284]`, slot 4). An empty one is 0x50B150's error branch — the notice
    /// `IDS_MUSTHAVEPEERTOPEERADDRESS` 816 (0x50B1D9…, through 0x513440(0x330, 0)) — so this
    /// answers `None`. A bare address takes the well-known port 1106.
    pub fn connect_target(&self) -> Option<SocketAddr> {
        let typed = self.history.get(self.ip_sel)?.trim();
        typed
            .parse::<SocketAddr>()
            .ok()
            .or_else(|| typed.parse::<Ipv4Addr>().ok().map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, port()))))
    }

    /// What `JOIN` on the games screen joins: the row of `Comms.GamesList` under the cursor.
    pub fn join_target(&self) -> Option<SocketAddr> {
        self.found.get(self.sel).map(|(a, _)| *a)
    }

    /// `CREATE` — become the host on the well-known port, the session named `game_name`
    /// (`Comms.GameName`, at most 15 characters: the accessor's capacity, 0x50B961).
    pub fn create(&self, game_name: &str) -> anyhow::Result<NetPlay> {
        let name: String = game_name.chars().take(GAME_NAME_MAX).collect();
        NetPlay::start_named(&format!("host:{}", port()), &self.who, &name)
    }

    /// `Comms.Data(GAMENAME)` — the session under the cursor.
    pub fn game_name(&self) -> String {
        match self.found.get(self.sel) {
            Some((_, d)) => nfs_net::comm::desc::split_name(&d.name).0.trim().to_string(),
            None => format!("{}'S RACE", self.who.to_string()),
        }
    }

    /// `Comms.Data(MYIPADDRESS)` — see [`NetPlay::my_address`].
    pub fn my_address(&self) -> String {
        nfs_net::socket::my_ip_string()
    }

    /// `JOIN` / `CONNECT` — join `to`.
    pub fn connect(&self, to: SocketAddr) -> anyhow::Result<NetPlay> {
        NetPlay::start(&format!("join:{to}"), &self.who)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet_lobby() -> Lobby {
        let mut l = Lobby::open("tester").unwrap();
        l.persist = false;
        l.history.clear();
        l
    }

    /// 0x4C2120 / 0x4C2220: a committed address goes to the front of `Comms.IPList` and the list
    /// selects row 0 — with no duplicate test — and CONNECT (0x50B150) reads the list's current row.
    #[test]
    fn a_committed_address_goes_to_the_front_and_connect_reads_the_current_row() {
        let mut l = quiet_lobby();
        assert_eq!(l.connect_target(), None, "an empty list is 0x50B150's 816 branch");
        l.commit_ip("192.168.0.5");
        l.commit_ip(" 10.0.0.7:1200 ");
        assert_eq!(l.addresses(), vec!["10.0.0.7:1200", "192.168.0.5"]);
        assert_eq!(l.connect_target(), Some("10.0.0.7:1200".parse().unwrap()));
        l.pick_ip(1);
        assert_eq!(l.ip, "192.168.0.5");
        assert_eq!(l.connect_target(), Some("192.168.0.5:1106".parse().unwrap()), "a bare address takes port 1106");
        l.commit_ip("192.168.0.5");
        assert_eq!(l.addresses().len(), 3, "the original adds the row again");
        l.commit_ip("   ");
        assert_eq!(l.addresses().len(), 3, "an empty string is not a row");
    }

    #[test]
    fn a_session_that_stops_answering_leaves_the_list() {
        let mut l = quiet_lobby();
        let a: SocketAddr = "10.0.0.9:1106".parse().unwrap();
        let d = nfs_net::comm::Describe {
            request: 0,
            from: nfs_net::wire::Addr::ip([10, 0, 0, 9], 1106),
            conn_dword: 0,
            conn_words: [0, 0],
            conn_raw: [0, 0],
            name: [0; 32],
            bytes: [0; 4],
            words: [0, 0],
            dwords: [0, 0],
            player_limit: 8,
            slot_mask: 0,
        };
        l.found.push((a, d));
        l.seen.insert(a, Instant::now() - SESSION_TTL - Duration::from_secs(1));
        l.poll();
        assert!(l.found.is_empty());
    }

    fn answer(ip: [u8; 4], name: &str, mask: u32) -> Box<nfs_net::comm::Describe> {
        Box::new(nfs_net::comm::Describe {
            request: 0,
            from: nfs_net::wire::Addr::ip(ip, 1106),
            conn_dword: 0,
            conn_words: [0, 0],
            conn_raw: [0, 0],
            name: nfs_net::comm::desc::name_field(name, "3.5"),
            bytes: [0, 1, 0, 0],
            words: [410, 313],
            dwords: [0, 0],
            player_limit: 8,
            slot_mask: mask,
        })
    }

    /// One host answers a probe over its loopback and its LAN address: one row, at the LAN
    /// address (a host that only the loopback reaches is still listed); two hosts with the same
    /// name but different address records stay two rows.
    #[test]
    fn one_session_answering_over_several_interfaces_is_one_row() {
        let mut l = quiet_lobby();
        let now = Instant::now();
        let lo: SocketAddr = "127.0.0.1:1106".parse().unwrap();
        let lan: SocketAddr = "192.168.0.82:1106".parse().unwrap();
        let other: SocketAddr = "192.168.0.99:1106".parse().unwrap();
        l.note_found(lo, answer([192, 168, 0, 82], "DRIVER's race", 0), now);
        assert_eq!(l.found.len(), 1);
        assert_eq!(l.found[0].0, lo, "only the loopback has answered so far");
        l.note_found(lan, answer([192, 168, 0, 82], "DRIVER's race", 1), now);
        l.note_found(lo, answer([192, 168, 0, 82], "DRIVER's race", 1), now);
        assert_eq!(l.found.len(), 1);
        assert_eq!(l.found[0].0, lan, "a non-loopback address wins");
        assert_eq!(l.found[0].1.slot_mask, 1, "the newest answer's description is kept");
        assert!(l.seen.contains_key(&lan) && !l.seen.contains_key(&lo));
        l.note_found(other, answer([192, 168, 0, 99], "DRIVER's race", 0), now);
        assert_eq!(l.rows().len(), 2);
        // a record that names no address cannot be merged with another source
        let mut a = answer([0, 0, 0, 0], "X", 0);
        a.from = nfs_net::wire::Addr::ip([0, 0, 0, 0], 1106);
        l.note_found("10.1.1.1:1106".parse().unwrap(), a.clone(), now);
        l.note_found("10.1.1.2:1106".parse().unwrap(), a, now);
        assert_eq!(l.rows().len(), 4);
    }

    /// The games list's fields come from the descriptor (0x4BA030 / 0x48C3D0): name and version
    /// around the `~`, `desc+0x21` the mode, `desc+0x26` the track's name id.
    #[test]
    fn a_row_carries_the_descriptors_fields() {
        let mut l = quiet_lobby();
        l.note_found("192.168.0.82:1106".parse().unwrap(), answer([192, 168, 0, 82], "DRIVER's race", 3), Instant::now());
        let r = &l.rows()[0];
        assert_eq!((r.name.as_str(), r.version.as_str()), ("DRIVER's race", "3.5"));
        assert_eq!((r.mode, r.track_id, r.players), (1, 313, 3));
    }

    /// A host sends `(id, value)` when a mode-2 accessor changes, but not for the value it started
    /// on; a player's option table takes what comes.
    #[test]
    fn a_host_sends_a_changed_track_option_and_a_player_takes_it() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        assert!(!host.sync_option(0xC, 300), "the starting value is not a selection");
        assert!(!host.sync_option(0xC, 300));
        assert!(host.sync_option(0xC, 313));
        assert!(!host.sync_option(0xC, 313));
        let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
        let m = nfs_net::options::message(&[0xC, 313, 0xF, 2]);
        p.hear(&m);
        assert_eq!(p.take_options(), vec![(0xC, 313), (0xF, 2)]);
        assert_eq!(p.options.get(&0xC), Some(&313));
        assert!(p.take_options().is_empty());
    }

    fn block(tick: i32) -> CarState {
        CarState { steer: 0, airborne: false, brake: 0, buttons: 0, tick: tick as u32, yaw_rate: 0.0, orientation: [0; 4], velocity: [0.0; 3], position: [0.0; 3] }
    }

    /// 0x4A0DF0: a block no newer than `+0x3C` is dropped; the lag is `|now − tick|` (at least 1) and the
    /// smoothed lag `(7·old + lag) / 8`, truncated toward zero; the first block sets `+0xDAC`.
    #[test]
    fn the_store_keeps_the_lag_book_the_original_keeps() {
        let mut r = Remote::new(0);
        assert!(r.store(block(100), 101));
        assert!(r.got && r.lag == 1 && r.lag_avg == 0, "1 → (0·7 + 1) / 8 = 0");
        assert!(!r.store(block(100), 102), "not newer: dropped");
        assert!(!r.store(block(99), 102));
        assert_eq!(r.state.tick, 100);
        assert!(r.store(block(108), 108), "age 0 counts as lag 1");
        assert_eq!(r.lag, 1);
        assert!(r.store(block(200), 160), "a block from the future: |age| = 40");
        assert_eq!(r.lag, 40);
        assert_eq!(r.lag_avg, (7 * 0 + 40) / 8);
        assert!(r.store(block(300), 390));
        assert_eq!(r.lag_avg, (7 * 5 + 90) / 8);
    }

    /// 0x4A0E9F…: a lag above 0x40 counts `+0x64` and the fifth such block — or one above 0x80 — makes the
    /// car stale with the snap 0x4A03D0 due once; nine blocks under 0x20 make it fresh again.
    #[test]
    fn a_car_goes_stale_after_five_late_blocks_and_recovers_after_nine_fresh_ones() {
        let mut r = Remote::new(0);
        r.store(block(1), 2);
        let mut t = 10;
        for k in 1..=4 {
            r.store(block(t), t + 70);
            assert!(!r.stale, "late block {k}");
            t += 8;
        }
        r.store(block(t), t + 70);
        assert!(r.stale && r.snap_due);
        r.snap_due = false;
        // a 64-tick lag is not late (`< 0x41`): the counter clears, the car stays stale
        t += 8;
        r.store(block(t), t + 64);
        assert!(r.stale && r.late == 0 && !r.snap_due);
        // one block above 0x80 goes stale at once, and snaps only if it was fresh
        let mut r = Remote::new(0);
        r.store(block(1), 2);
        r.store(block(10), 10 + 129);
        assert!(r.stale && r.snap_due);
        for k in 0..9 {
            assert!(r.stale, "{k}");
            r.store(block(200 + 8 * k), 200 + 8 * k + 5);
        }
        assert!(!r.stale, "more than 8 fresh blocks");
    }

    /// The lobby's ping (0x4905A0 / 0x490600 / the host's case 195): a player asks the host for a
    /// player's ping once a second, with its own last round trip as the word; the host keeps that
    /// word per sender and answers with the asked player's, −1 until it has reported.
    #[test]
    fn the_ping_exchange_reports_the_last_round_trip_and_hides_unknown_pings() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        let out = host.room.admit(1, b"PLAYER");
        host.dispatch(out);
        assert!(host.pings.contains_key(&0) && host.pings.contains_key(&1));
        assert_eq!(host.ping_shown(0), None, "created with −1");
        host.net148 = 123_456; // the value the link event left there: a large tick
        let t0 = 10_000_000;
        host.tick_override = Some(t0);
        host.ping_at(0, t0);
        // the first report is the large number: min(.., 9999)
        assert_eq!(host.ping_shown(0), Some(9999));
        assert_eq!(host.net148, 0, "0x490600: now − player+0x44, in process");
        // no second request inside the second, one after it (`1000 < now − +0x44`)
        host.pings.get_mut(&0).unwrap().ping = 5;
        host.tick_override = Some(t0 + 1000);
        host.ping_at(0, t0 + 1000);
        assert_eq!(host.ping_shown(0), Some(5));
        host.tick_override = Some(t0 + 1001);
        host.ping_at(0, t0 + 1001);
        assert_eq!(host.ping_shown(0), Some(0));
        // player 1 has reported nothing: −1, hidden
        host.tick_override = Some(t0 + 2000);
        host.ping_at(1, t0 + 2000);
        assert_eq!(host.ping_shown(1), None);
        // its request stores its word; the next answer about it carries it
        host.ping_request(1, room::Ping { id: 1, word: 37 });
        host.tick_override = Some(t0 + 4000);
        host.ping_at(1, t0 + 4000);
        assert_eq!(host.ping_shown(1), Some(37));
        // the wire shape: value = the asked id, word big-endian
        assert_eq!(room::Ping { id: 1, word: 37 }.encode(), vec![0xC3, 0, 0, 1, 0, 0, 0, 37]);
        assert_eq!(room::Ping::parse(&[0xC3, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF]), Some(room::Ping { id: 1, word: -1 }));
    }

    /// A player's slot records reach the table of whoever hears them (0x4B4B80's per-slot band), and
    /// 0x5091B0 needs every player in the room ready.
    #[test]
    fn slot_records_fill_the_table_and_the_create_screens_gate_needs_everyone_ready() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        assert!(host.established());
        assert!(!host.everyone_ready(), "the host has not sent its ready yet");
        host.send_slot_records(&[nfs_net::slots::ready_record(0, true)]);
        assert!(host.everyone_ready());
        // a second player comes in and is not ready
        let out = host.room.admit(1, b"PLAYER");
        host.dispatch(out);
        assert!(!host.everyone_ready());
        let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
        for r in nfs_net::slots::car_records(1, 161, b"'97 Boxster", 0xFF11_2233) {
            p.hear(&nfs_net::options::message(&r));
        }
        assert_eq!((p.slots.slot[1].car_id, p.slots.slot[1].colour), (161, 0xFF11_2233));
        assert_eq!(p.slots.slot[1].car_name, b"'97 Boxster".to_vec());
        host.hear(&nfs_net::options::message(&nfs_net::slots::ready_record(1, true)));
        assert!(host.everyone_ready());
        // the race started for everyone: PLAYGAME is off again (`net+0xE5 != 0`)
        host.hear(&room::race_start(1));
        assert!(!host.everyone_ready());
    }

    /// **0x490140**: 3 for a player who is not on the list or a link that is gone, 4 for one that is
    /// listed but not racing, and 0 / 1 / 2 by the local ping (× 0.001 against 0.3 / 1.0) or the packet
    /// age of the others (ticks × 1/64 against the same), or `net+0x128` × 0.001 against 0.2 / 0.5
    /// when the game is not positional. The bounds are strict compares (`fcom`, `test ah, 0x41`).
    #[test]
    fn the_link_classification_reads_the_ping_for_us_and_the_packet_age_for_the_others() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        let out = host.room.admit(1, b"PLAYER");
        host.dispatch(out);
        host.note_race_indices(&[(0, 0), (1, 1)]);
        host.in_race = 0b11;
        assert_eq!((host.local_race_index(), host.seat_race_index(0)), (Some(0), Some(1)));
        let st = |tick| CarState { steer: 0, airborne: false, brake: 0, buttons: 0, tick, yaw_rate: 0.0, orientation: [0; 4], velocity: [0.0; 3], position: [0.0; 3] };
        host.remotes.insert(1, Remote { state: st(1000), ..Remote::new(0) });
        // a car's packet age: 19 ticks = 0.297 s, 20 = 0.3125; 64 = 1.0 (not above it), 65 above
        assert_eq!(host.link_quality(1, 1019), LinkQuality::Good);
        assert_eq!(host.link_quality(1, 1020), LinkQuality::Fair);
        assert_eq!(host.link_quality(1, 1064), LinkQuality::Fair);
        assert_eq!(host.link_quality(1, 1065), LinkQuality::Poor);
        // a car that has sent nothing has `car+0x3C == 0`: its age is the whole game tick
        host.remotes.clear();
        assert_eq!(host.link_quality(1, 10), LinkQuality::Good);
        assert_eq!(host.link_quality(1, 100), LinkQuality::Poor);
        // our own car: the ping of the local player's record (0x4905A0), 0.3 s and 1.0 s
        host.tick_override = Some(10_000_000);
        host.pings.get_mut(&0).unwrap().pending = true;
        for (ping, want) in [(-1, LinkQuality::Good), (300, LinkQuality::Good), (301, LinkQuality::Fair), (1000, LinkQuality::Fair), (1001, LinkQuality::Poor)] {
            host.pings.get_mut(&0).unwrap().ping = ping;
            assert_eq!(host.link_quality(0, 5000), want, "ping {ping}");
        }
        // `Comms.Positional` off: `net+0x128` × 0.001 against 0.2 / 0.5
        host.positional = false;
        for (ms, want) in [(200, LinkQuality::Good), (201, LinkQuality::Fair), (500, LinkQuality::Fair), (501, LinkQuality::Poor)] {
            host.exch.sender.latency[1] = ms as u32;
            assert_eq!(host.link_quality(1, 5000), want, "{ms} ms");
        }
        host.positional = true;
        // listed, but its bit of `net+0xE5` is not set: white; gone from the room: black
        host.in_race = 0b01;
        assert_eq!(host.link_quality(1, 1), LinkQuality::NotRacing);
        assert!(LinkQuality::NotRacing.gone() && !LinkQuality::Poor.gone());
        host.hear(&room::player_out(1));
        assert_eq!(host.link_quality(1, 1), LinkQuality::NotListed);
        assert!(host.retired(1, 1), "0x416310 retires a racer above 2");
        assert!(!host.retired(0, 1), "we are still listed and racing");
        // the colour table 0x5B2D90
        assert_eq!(LinkQuality::COLOURS[LinkQuality::Poor as usize], 0xCCFF_0000);
        // a joiner whose link has dropped (`+0xBF`): 3 for everyone
        let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
        p.join = JoinState::Joined;
        p.net.role = Role::Client;
        p.state_in = Some(nfs_net::room::comm_state::CONNECTED);
        p.room.hear(&room::player_in(0, b"HOSTY"));
        p.room.hear(&room::player_in(2, b"PLAYER"));
        p.id = 2;
        p.note_race_indices(&[(0, 0), (2, 1)]);
        p.in_race = 0b101;
        assert_eq!(p.link_quality(1, 1), LinkQuality::Good);
        p.lost = true;
        assert_eq!(p.link_quality(1, 1), LinkQuality::NotListed);
    }

    /// `0xB7` and the three-way race protocol (`network.md` §11.15): a player's finish time reaches
    /// every player under the sender's id and 0x48D770 / 0x48D7F0 answer it by race index; `−1` and
    /// `−2` do not make a time; `−2` takes the sender's bit out of `net+0xE5` and, once every player
    /// is back, the room's race is over.
    #[test]
    fn a_finish_time_reaches_the_room_and_the_return_frees_it() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        for id in [1, 2] {
            let out = host.room.admit(id, format!("P{id}").as_bytes());
            host.dispatch(out);
        }
        host.note_race_indices(&[(0, 0), (1, 1), (2, 2)]);
        // PLAYGAME's `0xB3` request on the host starts the race for the room and comes back to it
        host.request_start();
        assert_eq!(host.in_race, 0b111, "0x48F837 `+0xE5 = +0xE4`");
        assert!(host.go_wait, "0x48F861 `+0xC4 = 1`");
        assert!(host.room.race_running());
        assert!(!host.room.in_lobby_group(0));
        assert!(host.room.in_lobby_group(1), "player 1 has not asked yet");
        // a joiner's `0xB3` request only moves its bit
        host.serve_race_request(1, &room::start_request());
        assert!(!host.room.in_lobby_group(1));
        // the room's go is answered once
        let first = host.room.go_request(5);
        assert_eq!(first.len(), 1);
        assert!(host.room.go_request(6).is_empty());
        host.hear(&first[0].1);
        assert!(!host.go_wait, "0x48F8F5 `+0xC4 = 0` on the first 0xC5");
        // nobody has finished: no time, no rank
        assert_eq!((host.race_time(0), host.race_rank(0)), (-1, -1));
        // player 2 finishes in 4000 ticks with a best lap of 1900, the host in 4100
        host.serve_race_request(2, &room::race_time_request(4000, 1900));
        host.serve_race_request(0, &room::race_time_request(4100, 2000));
        assert_eq!((host.race_time(2), host.race_rank(2)), (4000, 1));
        assert_eq!((host.race_time(0), host.race_rank(0)), (4100, 2));
        assert_eq!((host.race_time(1), host.race_rank(1)), (-1, -1), "player 1 has sent none");
        // `−1` (leaving the game) is a value too, and stays −1; `−2` frees the bit
        host.serve_race_request(1, &room::race_time_request(-1, -1));
        assert_eq!(host.race_time(1), -1);
        host.serve_race_request(1, &room::race_time_request(-2, -1));
        assert_eq!(host.in_race, 0b101);
        assert!(host.room.race_running(), "the host and player 2 are still out");
        host.serve_race_request(0, &room::race_time_request(-2, -1));
        host.serve_race_request(2, &room::race_time_request(-2, -1));
        assert_eq!(host.in_race, 0);
        assert!(!host.room.race_running(), "everyone is back: +0xCC3 = 0");
        // a new launch clears the times (0x48D3D0)
        host.room.players.iter_mut().for_each(|p| p.time = -1);
        host.launch_begin();
        assert_eq!(host.race_rank(2), -1);
    }

    /// The launch's own-slot walk (0x4B4F25…0x4B4F66): a machine's records leave with its **race
    /// index** as the slot, each machine keeps every player's record at that index and not in the
    /// id-indexed lobby table.
    #[test]
    fn the_launch_resend_numbers_the_players_by_race_index_and_leaves_the_lobby_table_alone() {
        use nfs_net::launchcar::{id, OwnCar};
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        let out = host.room.admit(1, b"PLAYER");
        host.dispatch(out);
        // lobby time: player 1 picked a car by id (the lobby table is indexed by player id)
        for r in nfs_net::slots::car_records(1, 161, b"'97 Boxster", 0xFF11_2233) {
            host.hear(&nfs_net::options::message(&r));
        }
        host.hear(&room::race_start(1));
        assert_eq!(host.race_indices(), vec![(0, 0), (1, 1)]);
        let mine = OwnCar { filename: b"993".to_vec(), roof_accessor: 1, specname: b"993coupe".to_vec(), ..Default::default() };
        let sent = host.own_launch_records(&mine);
        assert!(sent.iter().all(|r| r[1] == 0), "the host's race index is 0");
        assert!(nfs_net::launchcar::well_formed(&sent));
        // the other machine's walk: its records at race index 1 come in as `0xBC`
        let theirs = OwnCar { filename: b"boxster".to_vec(), roof_accessor: 0, transmission: 1, ..Default::default() };
        let their_recs: Vec<Vec<u32>> = theirs.launch_list(1).into_iter().map(|mut r| { r[1] = 1; r }).collect();
        let mut out = nfs_net::options::Outgoing::new();
        let mut msgs: Vec<Vec<u8>> = their_recs.iter().flat_map(|r| out.push(r)).collect();
        msgs.extend(out.flush());
        assert!(msgs.len() > 1, "a whole record list goes in several messages (0xF0 rule)");
        for m in msgs {
            host.hear(&m);
        }
        let cars = host.remote_cars();
        assert_eq!(cars.len(), 1);
        assert_eq!((cars[0].0, cars[0].1), (1, 1));
        assert_eq!(cars[0].3.text(id::FILENAME).unwrap(), b"boxster".to_vec());
        assert_eq!(cars[0].3.num(id::CABRIODOWN), Some(0));
        assert_eq!(host.launch.cars[&0].text(id::FILENAME).unwrap(), b"993".to_vec());
        assert_eq!(host.launch.cars[&0].num(id::CABRIODOWN), Some(1));
        // the lobby table still holds what the lobby said, at the player id
        assert_eq!(host.slots.slot[1].car_id, 161);
        assert_eq!(host.slots.slot[1].car_name, b"'97 Boxster".to_vec());
    }

    /// `[net+0x24]` as the port maps it: a joiner is 3 until the host's `0xC6 (0xE)`.
    #[test]
    fn a_joiner_is_connecting_until_the_host_says_it_is_in() {
        use nfs_net::room::comm_state as cs;
        let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
        assert_eq!(p.comm_state(), cs::HOSTING);
        p.join = JoinState::Pending(Instant::now());
        p.net.role = Role::Client;
        assert_eq!(p.comm_state(), cs::CONNECTING);
        p.join = JoinState::Joined;
        assert_eq!(p.comm_state(), cs::CONNECTING);
        p.hear(&room::state(cs::CONNECTED));
        assert_eq!(p.comm_state(), cs::CONNECTED);
        p.hear(&room::state(cs::EJECTED));
        assert_eq!(p.comm_state(), cs::EJECTED);
        p.join = JoinState::Refused(nfs_net::comm::JoinStatus::NameTaken);
        assert_eq!(p.comm_state(), cs::NAME_TAKEN);
        p.join = JoinState::Pending(Instant::now() - JOIN_TIMEOUT - Duration::from_secs(1));
        assert_eq!(p.comm_state(), cs::LOST, "a connect that runs out is the comm layer's event 5: state 4 (0x48FBB6), not 6");
    }

    /// What 0x427A60 posts and `HLeader` shows: the shipped `DashHud/pic16.fsh`, sent through the
    /// room's chunks and decoded the way [`NetPlay::leader_picture`] decodes it — the first entry,
    /// 54 × 64. Skipped without the game's files.
    #[test]
    fn the_shipped_picture_survives_the_chunks_and_decodes_at_54_by_64() {
        let root = std::env::var_os("NFS5_GAME_PATH").map(std::path::PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/NFS-Porsche-Unleashed/game_info/data"))
        });
        let Some(bytes) = root.and_then(|r| std::fs::read(r.join("GameData/DashHud/pic16.fsh")).ok()) else {
            return;
        };
        let mut a = Room::new(2);
        a.hear(&room::player_in(2, b"anna"));
        a.post(room::PICTURE, &bytes);
        let mut b = Room::new(0);
        b.hear(&room::player_in(2, b"anna"));
        a.tick(0);
        let mut t = 0;
        while a.sending() {
            t += 100;
            for m in a.tick(t) {
                b.hear(&m);
            }
        }
        let f = &b.player(2).unwrap().files[0];
        assert!(f.complete());
        let fsh = nfs_formats::fsh::Fsh::parse(&f.bytes).unwrap();
        let img = fsh.decode(&f.bytes, 0).unwrap();
        assert_eq!((img.width, img.height), (54, 64));
    }

    /// **The input exchange end to end, over the loopback**: a host and a player, `Comms.Positional`
    /// off, each sending its own controller (a distinct sample every 1/32 s, one with a command
    /// code); the room's frames come back to both, and every frame the two machines latch holds the
    /// same sample for both cars — the property the simulation's agreement rests on (`network.md`
    /// §11.17). Also: the round trip 0x490140's non-positional arm reads is measured, the command
    /// reaches the *other* machine's copy of the car, and a racer whose sample carries command 1 is
    /// "left" (`car+0xD8B == 1`, 0x4163B0).
    #[test]
    fn two_machines_latch_the_same_sample_for_both_cars() {
        let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
        // (the identity gate is `comm::identity`'s own test's business)
        host.net.identity_gate = None;
        let port = host.net.local_addr().unwrap().port();
        let mut p = NetPlay::start(&format!("join:127.0.0.1:{port}"), "PLAYER").unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until && !(p.join == JoinState::Joined && p.established() && host.room_len() == 2 && p.room_len() == 2) {
            host.poll_lobby();
            p.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            p.established() && host.room_len() == 2 && p.room_len() == 2,
            "the player is in the room: join {:?} established {} rooms {} / {}",
            p.join,
            p.established(),
            host.room_len(),
            p.room_len()
        );
        host.positional = false;
        p.positional = false;
        // PLAYGAME's `0xB3` request on every machine (0x48D370): the room starts the race and tells everyone
        host.request_start();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !p.take_start() {
            host.poll_lobby();
            p.poll_lobby();
        }
        p.request_start();
        for m in [&mut host, &mut p] {
            m.note_race_indices(&[(0, 0), (1, 1)]);
            assert!(m.exch.waiting, "0xB3 sets net+0xC4");
        }
        // the race frame's ring (after its barriers, which the launch's tests cover), then the `0xC5` round
        for m in [&mut host, &mut p] {
            m.exchange_race_frame();
            assert!(m.exchanging());
        }
        host.request_go();
        p.request_go();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && (host.exch.waiting || p.exch.waiting) {
            host.poll_lobby();
            p.poll_lobby();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!host.exch.waiting && !p.exch.waiting, "0xC5 (0, now) clears net+0xC4 on both");
        assert_eq!(host.exch.start_ms, p.exch.start_ms);
        // a second and a half of play: each side samples at the timer's 32 Hz (on a loaded machine
        // the loop runs on — up to six seconds — until the 40th sample's frames have come back)
        let end = Instant::now() + Duration::from_millis(1500);
        let long_end = Instant::now() + Duration::from_millis(6000);
        let mut n = 0u32;
        while Instant::now() < end || (Instant::now() < long_end && host.exch.received.min(p.exch.received) < 44) {
            for (who, m) in [(0u8, &mut host), (1u8, &mut p)] {
                for _ in 0..m.exchange_pump(false) {
                    n += 1;
                    // the command 5 (reset) on the player's 40th sample
                    // (on samples 40…43, not on one alone: the samples are unreliable, and on a loaded machine
                    // the one carrying the command can arrive after the room has built the frame it belongs to)
                    let cmd = if who == 1 && (40..44).contains(&m.exch.sender.taken) { 5 << 3 } else { 0 };
                    m.exchange_sample([cmd | who, 10 * (who + 1) + (n % 7) as u8, 100 + (n % 5) as u8, 20 * who]);
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(host.exch.received >= 20 && p.exch.received >= 20, "frames came: {} / {}", host.exch.received, p.exch.received);
        // every frame both have: the latched samples agree, car by car
        // (`received` counts ring entries, two to a frame)
        let entries = host.exch.received.min(p.exch.received);
        let mut saw_reset = [false; 2];
        for f in 1..=entries {
            host.exchange_latch(f);
            p.exchange_latch(f);
            for car in 0..2 {
                assert_eq!(host.latched(car), p.latched(car), "frame {f}, car {car}");
                saw_reset[car] |= host.latched(car)[0] >> 3 == 5;
            }
        }
        assert!(saw_reset[1] && !saw_reset[0], "the player's reset command is in its car's samples on both machines, once");
        // the round trip was measured once (0x48F141: the first time our own bit comes back — the
        // frame is 1/16 s old at least), and is small here; it is not measured again
        let rtt = p.exch.sender.latency[1];
        assert!((1..2000).contains(&rtt), "{rtt} ms");
        // a racer whose sample carries command 1 has left (0x4163B0)
        assert!(!host.retired(1, 0) && !p.retired(0, 0));
        host.commands[1] = nfs_net::exchange::CMD_LEFT;
        assert!(host.retired(1, 0));
    }
}
