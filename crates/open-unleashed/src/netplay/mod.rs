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

mod barrier;
mod cars;
mod exchange;
mod keys;
mod launch;
mod lobby;
mod ping;
mod remote;
mod players;
mod session;
#[cfg(test)]
mod tests;

pub use keys::*;
pub use lobby::*;
pub use remote::*;

/// `NfsNet+0x2C`, the version string the session name carries after its `~` (0x48DDC2…0x48DDF8).
/// 0x48DBE0 first fills it with the first 4 bytes (`strncpy`, `[+0x31] = 0`) of the registry
/// string 0x4B7240(7) reads — the value **`Client Version`** of the game's key, whose table row
/// (0x5D6F28 + 7·12) gives the default `"0.0"` (0x5D07A0) when the registry has none — then reads
/// the install's `version.txt` (`push 0x5D0790`, at most 5 bytes: 0x48DC77…0x48DC7C, taken as they
/// are, no trimming) over it when the file opens and is not empty (0x533DE0 ≠ 0).
/// CONFIRMED 2026-09-30: a missing or empty file leaves the registry's `Client Version`
/// (this install's `reg.reg`: `"0.0"`). Until this date the port answered `"3.5"`, the literal
/// 0x48A700 compares a peer's version with, which is not what the constructor keeps, and trimmed
/// the file's text.
///
/// **PORT-DEVIATION D19** (`RE/notes/PORT-DEVIATIONS.md`): with neither the file nor a registry export
/// — a player's `game/` folder, which the install instructions fill with `GameData` and `FEData` only —
/// the port announces [`SHIPPED_VERSION`], what every installed copy's `version.txt` holds, instead of the
/// table's `"0.0"`. The original cannot be in that state (its installer writes both); `"0.0"` would make a
/// host refuse every other player. The file, when present, still wins, as it does there.
pub fn game_version() -> String {
    let paths = nfs_assets::paths::GamePaths::locate().ok();
    let file = paths
        .as_ref()
        .and_then(|p| std::fs::read(p.root.join("version.txt")).ok())
        .map(|b| b.into_iter().take(5).take_while(|&c| c != 0).map(|c| c as char).collect::<String>())
        .filter(|v| !v.is_empty());
    let reg = paths.map(|p| p.registry());
    version_from(file, reg.as_ref())
}

/// 0x48DBE0's choice, from what was read: the file's text, else the registry's `Client Version` (4 chars), else
/// `"0.0"` — or, with no registry export at all, [`SHIPPED_VERSION`] (D19).
pub fn version_from(file: Option<String>, reg: Option<&nfs_formats::regfile::RegFile>) -> String {
    if let Some(v) = file {
        return v;
    }
    match reg.and_then(|r| r.value(nfs_formats::regfile::GAME_KEY, "Client Version")) {
        Some(nfs_formats::regfile::RegValue::Sz(v)) => v.chars().take(4).collect(),
        Some(_) => "0.0".to_string(),
        None if reg.is_some_and(|r| !r.is_empty()) => "0.0".to_string(),
        None => {
            tracing::info!(version = SHIPPED_VERSION, "no version.txt beside the game data and no registry export: the shipped version");
            SHIPPED_VERSION.to_string()
        }
    }
}

/// The version every installed copy of the game carries in its `version.txt` (the literal 0x48A700 compares a
/// peer's version with, 0x5D06FC), announced when a player's data folder has no such file (D19).
pub const SHIPPED_VERSION: &str = "3.5";

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
/// `{5CD46B81-ECE4-11D3-9B6A-00902724AB1E}` that 0x48AC27 hands the CONMAN constructor
/// ([`nfs_net::comm::APP_GUID`]). Its session key is the text 0x587F90 prints from it,
/// `EAGAME:5cd46b81.ece411d3.9b6a0090.2724ab1e`; a host compares the first 32 bytes of a `'w'`
/// (0x588AD9) and a browse sends the whole text with its NUL (0x560620). **CONFIRMED on the running
/// original** (`REFERENCE-2026-10-02-SRVE.md` §4): the host answers the 32-byte and the 43-byte forms and
/// ignores the zero-padded raw GUID this port sent before.
fn key() -> [u8; 32] {
    nfs_net::comm::key32(&nfs_net::comm::APP_GUID)
}

/// A session of ours: the key to match, and the browse's whole key text to send.
fn with_probe(mut n: Net) -> Net {
    n.probe_payload = nfs_net::comm::probe_payload(&nfs_net::comm::APP_GUID);
    n
}

/// What a joiner's `'j'` carries at `rec+0x48`: `<profile>~<version>~<version>` — captured with the
/// profile `TEST` as `TEST~3.5~3.5` (`REFERENCE-2026-10-02-SRVE.md` §2); the third field is the version
/// again, where the earlier reading of the callers' empty string 0x5E8E50 gave an empty game
/// (the room's own test of it belongs to the lobby's `0xB6` request only, `network.md` §11.28).
fn join_identity(who: &str) -> Vec<u8> {
    let v = game_version();
    nfs_net::comm::identity::encode(who, &v, &v)
}

/// 0x48AC90's reading of a typed address: `sscanf(s, "%d.%d.%d.%d.%d", a, b, c, d, &port)`. Four numbers or
/// more make the dotted address `a&0xff . b&0xff . c&0xff . d&0xff` (the rest of the string is not
/// read), and a **fifth number is the port** (`a.b.c.d.port`); fewer than four make the whole string a
/// host name for `gethostbyname` (0x560660). Without a fifth number the port is the default pushed at
/// 0x48E4CD, `[0x5B4990]` = 9442. `%d` skips leading white space and takes an optional sign; the literal
/// `.` of the format matches only a `.`.
pub fn parse_typed_address(s: &str) -> Option<SocketAddr> {
    let b = s.as_bytes();
    let mut at = 0;
    let mut nums: Vec<i32> = Vec::new();
    while nums.len() < 5 {
        if !nums.is_empty() {
            if b.get(at) != Some(&b'.') {
                break;
            }
            at += 1;
        }
        while b.get(at).is_some_and(|c| c.is_ascii_whitespace()) {
            at += 1;
        }
        let neg = b.get(at) == Some(&b'-');
        if neg || b.get(at) == Some(&b'+') {
            at += 1;
        }
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if start == at {
            break;
        }
        let v: i64 = s[start..at].parse().unwrap_or(i64::MAX);
        nums.push(if neg { -(v as i32) } else { v as i32 });
    }
    if nums.len() >= 4 {
        let p = nums.get(4).map_or(port(), |&p| p as u16);
        let ip = Ipv4Addr::new(nums[0] as u8, nums[1] as u8, nums[2] as u8, nums[3] as u8);
        return Some(SocketAddr::V4(SocketAddrV4::new(ip, p)));
    }
    // a name: `gethostbyname` (0x560660) — the first IPv4 answer
    (s, port()).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
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
    /// Host: slots whose `'j'` is answered but whose CONMAN hello (`0xFF`, 0x55CED0) has not come yet — the room admits a
    /// player only after the link layer has answered it (`0xFE, 1`); the original host sends **nothing** before that.
    hello_wait: std::collections::HashSet<u32>,
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
/// zeroed by 0x565336), so the deadline is 16 s. **CONFIRMED on the running original (2026-10-01 SV, Wine, gdb):**
/// `[0x5DEB48]` reads 128 (0x80) — `TIMER_FREQ`, 128 ticks a second, `timing.md` §2 — and 0x5822B0, the
/// conman's ms → ticks converter (`ticks_now + ms · [0x5DEB48] / 1000`, the 0x10624DD3 multiply), turns the
/// handshake's 5000 ms into 640 ticks the same way, so `[0x5DEB48] << 4` = 2048 ticks = 16 s. When it runs out the conman reports event 5
/// (0x55C9CF), and the game's callback makes it state **4** (0x48FBB6, `net+0xDC = −1`) — not 6.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(16);

/// The well-known port. **TEST GLUE** (not in the original): `NFS_NET_PORT` moves it, so two
/// test sessions (or two two-instance runs) on one machine do not fight over 9442.
fn port() -> u16 {
    std::env::var("NFS_NET_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(nfs_net::PORT)
}

/// How long the launch barrier, PLAYGAME's wait and the race frame's wait last at most. PORT-DEVIATION
/// D12 (PORT-DEVIATIONS.md) [the original's waits have none: 0x48CE27, 0x50CFD7, 0x4152E0] — **glue**,
/// the original waits for ever while the session lives.
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

/// `room+0x70`, the 24-bit value of the room's `0xB4` (0x48BAA0 pushes it, 0x48BBBE): the room's session number. It is the manager's counter
/// `[mgr+0x150]` (0x491CF5 sets it to −1) incremented by 0x490AF0 (`& 0x7FFFFF`) until no room (`+0x70`, lists `+0x174` / `+0x178`) and no
/// player (`+0xAC`, list `+0x164`) already has it, and stored by the room's constructor 0x48B9B0 (0x48B9D7, the argument 0x491402 / 0x491A07
/// computes). The first room of a session skips 0 because the host's own player (id 0) is in the list, so its value is **1** — the value of every
/// capture (one room per run). HIGH: the player list's identity at 0x490B0E was read, not run; a port host creates one room per session.
const ROOM_ID: i32 = 1;
