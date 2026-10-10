//! The game room above the session — who is in it, what they say, and the files they hand each
//! other (`RE/notes/network.md` §10).
//!
//! Three of the `0xB0 … 0xC6` messages carry it, all read from the room object the host side keeps
//! (0x48BB50 and the server dispatch 0x49120C) and the client's own dispatch 0x48F0A0:
//!
//! | type | what | built by | read by |
//! |---|---|---|---|
//! | `0xB0` | a player is in the room: `[id:24] name\0` | 0x48BC4B, 0x48BCD8 | 0x48F3D2 |
//! | `0xB1` | a player has gone: `[id:24]` | 0x492501, 0x492562 | 0x48F46F |
//! | `0xBB` | a chat line — two shapes, see [`ChatRequest`] and [`ChatLine`] | 0x48D9B0 / 0x491324 | 0x4912D4 / 0x48F38B |
//! | `0xBF`, `0xC0` | one chunk of a player's file, see [`Chunk`] | 0x48CCC0 | 0x48F6AD |
//!
//! Everything goes through one encoder, **0x48C870**, whose optional fields are what make the two
//! chat shapes differ; [`encode`] is that function.

use std::collections::VecDeque;

use crate::msg::kind;

/// `0x48BB59  cmp [room+0xAC], 8` — a room takes eight players.
pub const MAX_PLAYERS: usize = 8;
/// `0x48C847  push 0x3F` — the name a player record keeps, before the NUL 0x48C85A writes at +0x3F.
pub const NAME_MAX: usize = 0x3F;
/// `0x48C8F0  cmp ecx, 0xC7` — 0x48C870 stops copying a string once the message reaches 199 bytes,
/// then terminates it.
pub const MESSAGE_MAX: usize = 0xC7;
/// 0x48C88C / 0x48C8BA: a value of `0x80000000` means "leave this field out".
pub const OMIT: u32 = 0x8000_0000;
/// `0x4C333F  or edi, -1` — a chat line for everyone, which is what the input box sends unless a
/// player row is selected.
pub const TO_ALL: i32 = -1;
/// A chat line nobody shows: 0x48F3B9 drops `to == -2`. The only sender is the `CTRL-ALT!` report
/// at 0x48EEF4 (§10.2).
pub const TO_NOBODY: i32 = -2;
/// What the room's 32 Hz loop (0x48EE90) needs of the link under it.
pub trait ChunkLink {
    /// 0x48AE30, once per turn: `true` when the send queue took the turn.
    fn pump(&mut self) -> bool;
    /// 0x48C870 with the chunk.
    fn send(&mut self, msg: &[u8]);
}

/// `0x48CD43  mov [esp+0x10], 0xE8` — the largest piece of a file one chunk carries.
pub const CHUNK: usize = 0xE8;
/// The chunk header: tag, player, type, word (0x48CD36…0x48CD4B, 0x48CD81 `add edi, 8`).
pub const CHUNK_HEAD: usize = 8;
/// `0x48F019  add edx, 0x3E8` — what one chunk costs against a clock that runs at
/// `GetTickCount() << 5` (0x48EFBD), so one chunk every 31.25 ms, 32 a second.
pub const CHUNK_COST: u32 = 1000;
/// File type 0, the player's picture: 0x427A60 reads `pic16.fsh` from the HUD folder and 0x427A9B
/// posts it with `push 0`; `HLeader`'s draw asks for type 0 at 0x428761.
pub const PICTURE: u16 = 0;
/// File type 1: 0x49C733 copies 0x1C4 bytes of it into `car+0x760`. **Read, not ported** — the
/// port has no counterpart of that block.
pub const CAR_SETUP: u16 = 1;

/// **0x48C870** `(type, value, word, text)` — the one encoder every control message goes through.
///
/// ```text
/// +0            the type
/// value != OMIT  three bytes, big-endian: (v >> 16), (v >> 8), v      0x48C899…0x48C8AB
/// word  != OMIT  four bytes, big-endian                                0x48C8C2…0x48C8DC
/// text  != null  its bytes up to the NUL or until the message is 0xC7 long, then a NUL
/// ```
///
/// So a missing value moves everything after it forward, which is how a chat line from a player
/// and the host's copy of it come to have different shapes.
pub fn encode(kind: u8, value: Option<i32>, word: Option<u32>, text: Option<&[u8]>) -> Vec<u8> {
    let mut m = vec![kind];
    if let Some(v) = value {
        m.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    if let Some(w) = word {
        m.extend_from_slice(&w.to_be_bytes());
    }
    if let Some(t) = text {
        for &c in t.iter().take_while(|&&c| c != 0) {
            if m.len() >= MESSAGE_MAX {
                break;
            }
            m.push(c);
        }
        m.push(0);
    }
    m
}

/// The 24-bit head value, sign-extended the way 0x48F32F…0x48F338 does.
fn head(m: &[u8]) -> Option<i32> {
    let b = m.get(1..4)?;
    Some(crate::msg::sign_extend_24(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32))
}

/// The C string starting at `at`, without its NUL; everything to the end if there is none.
fn c_str(m: &[u8], at: usize) -> Vec<u8> {
    let s = m.get(at..).unwrap_or(&[]);
    s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec()
}

/// `0xBB` from a player to the host — **0x48D9B0**: `0x48C870(0xBB, to, OMIT, text)`, sent on the
/// player's link to the host (`[net+0xC]`), so the addressee sits where the sender's id normally
/// does and there is no word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    pub to: i32,
    pub text: Vec<u8>,
}

impl ChatRequest {
    pub fn encode(&self) -> Vec<u8> {
        encode(kind::CHAT, Some(self.to), None, Some(&self.text))
    }

    /// The host's read, 0x491250…0x4912F0: `to` from the head, the text from `+4`.
    pub fn parse(m: &[u8]) -> Option<Self> {
        (m.first() == Some(&kind::CHAT)).then_some(())?;
        Some(Self { to: head(m)?, text: c_str(m, 4) })
    }
}

/// `0xBB` from the host to the players — **0x491324**: `0x48C870(0xBB, from, to, text)`, with the
/// sender's id `peer+0xAC` in the head and the addressee as the word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub from: i32,
    pub to: i32,
    pub text: Vec<u8>,
}

impl ChatLine {
    pub fn encode(&self) -> Vec<u8> {
        encode(kind::CHAT, Some(self.from), Some(self.to as u32), Some(&self.text))
    }

    /// The player's read, 0x48F38B: the word at `+4` is the addressee, the text starts at `+8`,
    /// and `to == -2` is dropped (0x48F3B9) before the line reaches the chat box's callback
    /// `[net+0x20]`, which is called `(text, from, to)`.
    pub fn parse(m: &[u8]) -> Option<Self> {
        (m.first() == Some(&kind::CHAT)).then_some(())?;
        let w = m.get(4..8)?;
        let to = i32::from_be_bytes([w[0], w[1], w[2], w[3]]);
        (to != TO_NOBODY).then_some(())?;
        Some(Self { from: head(m)?, to, text: c_str(m, 8) })
    }
}

/// `0xB0` — **0x48BC4B / 0x48BCD8**: `0x48C870(0xB0, id, OMIT, name)`.
pub fn player_in(id: i32, name: &[u8]) -> Vec<u8> {
    encode(kind::JOIN, Some(id), None, Some(name))
}

/// `0xB4` — the room's description, 0x50 bytes (**captured** twice per newcomer, REFERENCE-2026-10-02-SRVX): the head's value, then
/// the 0x4C-byte record the games list is made of — 0x14 zeros, the 0x34 bytes at `room+0x74` (the name field `Game~3.5` padded to 32,
/// the four descriptor bytes, the two descriptor words **little-endian** (raw memory, not the `'d'`'s network order), 8 zeros, **8** =
/// the room's player limit), and last `+0x48` = the number of players in the room (`room+0xAC`). `value` is `room+0x70`, the room's
/// session number (0x490AF0, the first room 1). Two senders: **0x48BD10 / 0x48BD80** in the admission 0x48BB50 (to the
/// newcomer alone, the count before it) and **0x48BAA0**, which **0x490B70** calls for every player record of the manager's list
/// `mgr+0x164` — every connected player, the host's own included — after each change of `room+0xAC`: the admission (0x48BCEE, count
/// plus 1), a player out of the room 0x48BDA0 (0x48BEC8, count − 1; the disconnect 0x492770 takes the record off `+0x164` first) and
/// the room's creation (0x491467 / 0x491A81).
pub fn description(value: i32, name: &[u8; 32], bytes: [u8; 4], words: [u16; 2], count: u32) -> Vec<u8> {
    let mut m = vec![kind::DESCRIPTION];
    m.extend_from_slice(&[(value >> 16) as u8, (value >> 8) as u8, value as u8]);
    m.extend_from_slice(&[0u8; 0x14]);
    m.extend_from_slice(name);
    m.extend_from_slice(&bytes);
    m.extend_from_slice(&words[0].to_le_bytes());
    m.extend_from_slice(&words[1].to_le_bytes());
    m.extend_from_slice(&[0u8; 8]);
    m.extend_from_slice(&(MAX_PLAYERS as u32).to_le_bytes());
    m.extend_from_slice(&count.to_le_bytes());
    m
}

/// `0xB2` — 0x48BC1B: `0x48C870(0xB2, id, OMIT, the room's name)`, "your id is" — captured `b2 000001 "Game\0"` (the name without the
/// `~version` of the name field).
pub fn your_id(id: i32, room_name: &[u8]) -> Vec<u8> {
    encode(kind::YOUR_ID, Some(id), None, Some(room_name))
}

/// The lobby's ping exchange, `0xC3` (`network.md` §11.11).
///
/// **0x48D9F0** sends `0x48C870(0xC3, player id, net+0x148, null)` on the link to the host: the
/// head's value is the id of the player whose ping is wanted, the word is *our own* last measured
/// round trip (`net+0x148`, ms). The host's case 195 (**0x491661…0x4916CE**) stores that word on the
/// sender's peer record (`peer+0xB0`, initially −1: 0x48A570) and answers the sender with
/// `0xC3(asked id, that player's peer+0xB0)`; the asker's handler (**0x48F527…0x48F574**) finds
/// the player with that id and calls **0x490600**: `net+0x148 = GetTickCount() − player+0x44`,
/// `player+0x54 = word`, `player+0x58 = 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    /// the player whose ping is asked / answered
    pub id: i32,
    /// request: the sender's own last round trip; answer: the asked player's, or −1
    pub word: i32,
}

impl Ping {
    pub fn encode(&self) -> Vec<u8> {
        encode(kind::PING, Some(self.id), Some(self.word as u32), None)
    }

    pub fn parse(m: &[u8]) -> Option<Self> {
        (m.first() == Some(&kind::PING)).then_some(())?;
        let w = m.get(4..8)?;
        Some(Self { id: head(m)?, word: i32::from_be_bytes([w[0], w[1], w[2], w[3]]) })
    }
}

/// `0xB1` — **0x492501**: `0x48C870(0xB1, id, OMIT, null)`, the head alone.
/// `0xB3` from the host to every link — **0x48BF10**: `0x48C870(0xB3, 0, clock)`, the head's value
/// 0 and the word the host's game clock (`0x48A9A0()`).
pub fn race_start(clock: u32) -> Vec<u8> {
    encode(kind::START, Some(0), Some(clock), None)
}

pub fn player_out(id: i32) -> Vec<u8> {
    encode(kind::LEAVE, Some(id), None, None)
}

/// `0xB3` **request**, player to room — **0x48D370**: `0x48C870(0xB3, OMIT, OMIT, null)`, the type
/// byte alone, on the link to the host (`[net+0xC]`). PLAYGAME's 0x50CE90 sends it on every machine
/// (0x50CF73); the room's answer is [`Room::start_request`].
pub fn start_request() -> Vec<u8> {
    encode(kind::START, None, None, None)
}

/// `0xC5` **request**, player to room — **0x48D3A0**: `0x48C870(0xC5, OMIT, OMIT, null)`. The race
/// frame's set-up 0x4152E0 sends it (0x4153AE) once its barrier is passed and `net+0xC5` is clear.
pub fn go_request() -> Vec<u8> {
    encode(kind::GO, None, None, None)
}

/// `0xC5` from the room to every link — **0x48BFB0**: `0x48C870(0xC5, 0, clock, null)`, the first
/// request's answer (the room's `+0xCC4` latch keeps it to one).
pub fn go(clock: u32) -> Vec<u8> {
    encode(kind::GO, Some(0), Some(clock), None)
}

/// `0xB7` from a player to the room — **0x48DA60** `(a, b)`: `0x48C870(0xB7, a, b, null, 0)`, so the
/// 24-bit head is `a` and the word `b`. A finished race sends `(time, best lap)` (0x4168A9:
/// `[0x606AC4] − 0x1C0` and the smallest of the car's lap times), a player leaving the game `(−1,
/// −1)` (0x48DAD0) and one returning to the front end `(−2, −1)` (0x48DA60's callers pass −2, −1).
pub fn race_time_request(a: i32, b: i32) -> Vec<u8> {
    encode(kind::RACE_TIME, Some(a), Some(b as u32), None)
}

/// `0xB7` from the room to every link — **0x48C220** (0x48C245): `0x48C870(0xB7, sender's id,
/// a, null, 0)`: the head is the sender's player id, the word what the sender put in the head.
pub fn race_time(id: i32, a: i32) -> Vec<u8> {
    encode(kind::RACE_TIME, Some(id), Some(a as u32), None)
}

/// The values `[net+0x24]` takes and 0x509550 switches on (`network.md` §11.10), with the site that
/// writes each: `0xC6` carries the state the host wants us in (0x48F352 `mov [ebp+0x24], edi`),
/// the link's own events write 4 (0x48F0C4, 0x48FBB6) and the connect calls 3 (0x48E24A,
/// 0x48E4B4, 0x48E68E); creating a session writes 2 (0x48DF69).
pub mod comm_state {
    /// 0x48DF69: the session this machine hosts
    pub const HOSTING: u32 = 2;
    /// 0x48E24A: a connect is under way — 0x509550 keeps `IDS_MPCONNECTING` (817) up
    pub const CONNECTING: u32 = 3;
    /// 0x48F0C4 / 0x48FBB6: the link dropped — 830
    pub const LOST: u32 = 4;
    /// 831 `IDS_COULDNOTCONNECT`
    pub const COULD_NOT_CONNECT: u32 = 6;
    /// 819
    pub const BAD_LOGIN: u32 = 7;
    /// 0x509550's input dialog 821 `IDS_PERSONATAKEN`
    pub const NAME_TAKEN: u32 = 8;
    /// `0xC6 (9)` from 0x491746 "Ejected by host" — 818 `IDS_MPEJECTED`
    pub const EJECTED: u32 = 9;
    /// `0xC6 (0xA)` from 0x491578 / 0x49161D — 822 `IDS_WRONGVERSION`
    pub const WRONG_VERSION: u32 = 10;
    /// `0xC6 (0xB)` from 0x49159E / 0x491642 — 820 `IDS_GAMEFULL`
    pub const FULL: u32 = 11;
    /// 830 again
    pub const LOST_2: u32 = 12;
    /// `0xC6 (0xE)` (0x491AAA, 0x49157A path, 0x491C1B): admitted — `[net+0xBE] = 1` (0x48F357)
    pub const CONNECTED: u32 = 14;
}

/// `0xC6` — **0x48C870(0xC6, state, OMIT, null)**: what the host tells a connection about itself.
pub fn state(v: u32) -> Vec<u8> {
    encode(kind::EJECT, Some(v as i32), None, None)
}

/// One piece of a player's file: `0xBF` for the first, `0xC0` for the rest (0x48CCF3…0x48CD07).
///
/// Unlike every other message this header is **not** big-endian — 0x48CD36…0x48CD4B store it
/// straight from registers and the reader 0x48F6AD takes it straight back:
///
/// ```text
/// +0  u8   0xBF / 0xC0
/// +1  u8   the sender's id                   (the low byte of net+0xDC)
/// +2  u16  the file type, little-endian       (0x48F713 movsx edx, word [ebx+2])
/// +4  u32  0xBF: the whole size, 0xC0: the offset — little-endian
/// +8       at most 0xE8 bytes of the file
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub first: bool,
    pub from: u8,
    pub kind: u16,
    /// the size on the first chunk, the offset on the others
    pub word: u32,
    pub bytes: Vec<u8>,
}

impl Chunk {
    pub fn encode(&self) -> Vec<u8> {
        let mut m = vec![if self.first { kind::DATA_FIRST } else { kind::DATA_MORE }, self.from];
        m.extend_from_slice(&self.kind.to_le_bytes());
        m.extend_from_slice(&self.word.to_le_bytes());
        m.extend_from_slice(&self.bytes);
        m
    }

    pub fn parse(m: &[u8]) -> Option<Self> {
        let first = match *m.first()? {
            kind::DATA_FIRST => true,
            kind::DATA_MORE => false,
            _ => return None,
        };
        let h = m.get(..CHUNK_HEAD)?;
        Some(Self {
            first,
            from: h[1],
            kind: u16::from_le_bytes([h[2], h[3]]),
            word: u32::from_le_bytes([h[4], h[5], h[6], h[7]]),
            bytes: m[CHUNK_HEAD..].to_vec(),
        })
    }
}

/// A `CDataPacket` (0x490420, 16 bytes): `+0` the type, `+4` the buffer, `+8` its size, `+C` how
/// much of it has arrived — or, on the sending side, how much has gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub kind: u16,
    pub total: usize,
    pub bytes: Vec<u8>,
    /// `+0xC`
    pub done: usize,
}

impl Packet {
    /// `HLeader` 0x428772 and 0x4A51F5: only `+0xC >= +8` counts.
    pub fn complete(&self) -> bool {
        self.done >= self.total
    }
}

/// A player record, 0x60 bytes (0x48F3D2): the name at `+0`, the id at `+0x40`, its car at `+0x4C`
/// (`-1` until a car is handed out, 0x48F412) and its files on the list at `+0x5C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub id: i32,
    pub name: Vec<u8>,
    pub files: Vec<Packet>,
    /// `+0x50` — the player's finish time in ticks, −1 until its `0xB7` came (0x48F5D1; 0x48C840
    /// starts it at −1). `0x48D770` answers it.
    pub time: i32,
    /// `+0x48` — written with `+0x50` by the same `0xB7` (0x48F5D4), and by the paused `0xBE`
    /// variant (0x48F699) which the port neither sends nor hears; −1 until then. 0x48D770 only
    /// compares it (`network.md` §11.15).
    pub progress: i32,
}

/// Where the host sends something: every link (`0x48C870` with a null destination, 0x48C921) or
/// one player's (0x48C980's answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    All,
    Player(i32),
}

/// What a player makes of an incoming room message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    Joined(i32),
    Left(i32),
    Chat(ChatLine),
    /// a player's file has come in whole
    File { from: i32, kind: u16 },
    /// `0xB3` — the host is starting the race; the word is its game clock (0x48F82F)
    Start { clock: u32 },
    /// `0xBC` — option records for the option table (0x48F998 → 0x4B4B80)
    Options(Vec<Vec<u32>>),
    /// `0xC6` — the host's word on our connection, which 0x48F352 stores as the comm state
    /// `[net+0x24]` (see [`comm_state`])
    State(i32),
    /// `0xB7` — a player's finish time: `+0x50 = +0x48 = value` on the player with that id
    /// (0x48F5D1 / 0x48F5D4); −1 ("leaving game") lands the same way
    RaceTime { id: i32, value: i32 },
    /// `0xB7` with the value −2 — the player is back in the front end: its bit leaves `net+0xE5`
    /// (0x48F5B9…0x48F5CF)
    Returned(i32),
    /// `0xC5` (0x48F8D2): with `net+0xC4` set it is the room's go — `+0xC4 = 0`, `+0x15C` = the
    /// word; with it clear the player's bit joins `net+0xE5` (0x48F911…0x48F91F). The caller
    /// keeps `+0xC4`, so it decides which
    Go { id: i32, clock: u32 },
}

/// The room as one player sees it — `net+0x260` (the players), `net+0x264` (our own files still
/// going out) and `net+0x1C` (the send clock) — plus, for the host, the routing its room object
/// does on everyone's behalf.
#[derive(Debug, Clone, Default)]
pub struct Room {
    /// `net+0xDC`, our id; −1 until we have one
    pub me: i32,
    /// `net+0x260`, in the order the `0xB0`s came
    pub players: Vec<Player>,
    /// `net+0x264`
    outgoing: VecDeque<Packet>,
    /// `net+0x1C`, in `GetTickCount() << 5` units; `None` until the first tick
    clock: Option<u32>,
    /// **The host's room object, 0x48BF10 / 0x48BFB0 / 0x48C220** — `+0xCC2`: the players who are in
    /// the lobby group (bit = id). All eight at the room's construction (0x48BA39), a bit is set at a
    /// player's admission (0x48BB9A…0x48BBAC) and again by its `0xB7 (−2)` (0x48C24F…0x48C25D),
    /// cleared by its `0xB3` request (0x48BF21…0x48BF2B). The `0xBC` relay 0x48C050 hands a record
    /// only to the players whose bit equals the sender's.
    lobby: u8,
    /// `+0xCC3` — a race is running (0x48BF51 sets it, 0x48BA40 and 0x48C273 clear it)
    running: bool,
    /// `+0xCC4` — the room's `0xC5` has gone out for this race (0x48BFEF sets it, 0x48BF58 clears)
    go_sent: bool,
    /// **The players' message lists, `player+0xC8`** (0x48A2F0 files into the sender's, 0x48A390 replays one): the `0xBC`
    /// messages a player sent while it was in the lobby group, **one per key** — a new one drops every older node whose
    /// dword at `+4` (the first record's id) is the same, then is appended (0x48A316…0x48A332), so the list is the player's
    /// lobby state in the order of its last writes. It is **not** drained (the send queue is the other list, `+0xC4`).
    filed: std::collections::BTreeMap<i32, Vec<Vec<u8>>>,
}

impl Room {
    pub fn new(me: i32) -> Self {
        Self { me, lobby: 0xFF, ..Self::default() }
    }

    pub fn player(&self, id: i32) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    /// **0x48F0A0's room cases**, for a player. `0xBB` here is the host's shape ([`ChatLine`]).
    pub fn hear(&mut self, m: &[u8]) -> Option<Heard> {
        match *m.first()? {
            // 0x48F3D2: a new 0x60-byte record, appended; nothing checks for a duplicate
            kind::JOIN => {
                let id = head(m)?;
                let mut name = c_str(m, 4);
                name.truncate(NAME_MAX);
                self.players.push(Player { id, name, files: Vec::new(), time: -1, progress: -1 });
                Some(Heard::Joined(id))
            }
            // 0x48F46F: every record with that id goes
            kind::LEAVE => {
                let id = head(m)?;
                self.players.retain(|p| p.id != id);
                self.filed.remove(&id);
                Some(Heard::Left(id))
            }
            kind::CHAT => ChatLine::parse(m).map(Heard::Chat),
            // 0x48F82F: the word at `+4` is read byte by byte (0x48F8A2…0x48F8C2) and handed to
            // 0x48A9B0; the rest of the handler resets the room's ready state, which the port
            // keeps no copy of
            kind::START => {
                let w = m.get(4..8)?;
                Some(Heard::Start { clock: u32::from_be_bytes([w[0], w[1], w[2], w[3]]) })
            }
            kind::OPTIONS => Some(Heard::Options(crate::options::decode(m))),
            // 0x48F579: the player whose `+0x40` is the head; the word at `+4` is `−2` — its bit
            // leaves `net+0xE5` — or a time, which goes to `+0x50` and `+0x48` (0x48F5D1 / 0x48F5D4)
            kind::RACE_TIME => {
                let id = head(m)?;
                let w = m.get(4..8)?;
                let value = i32::from_be_bytes([w[0], w[1], w[2], w[3]]);
                if value == -2 {
                    return Some(Heard::Returned(id));
                }
                if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
                    p.time = value;
                    p.progress = value;
                }
                Some(Heard::RaceTime { id, value })
            }
            // 0x48F8D2: the word at `+4` is the clock (read before the `+0xC4` test only in the
            // branch that stores it)
            kind::GO => {
                let id = head(m)?;
                let clock = m.get(4..8).map_or(0, |w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                Some(Heard::Go { id, clock })
            }
            // 0x48F34F: the value is the state
            kind::EJECT => Some(Heard::State(head(m)?)),
            kind::DATA_FIRST | kind::DATA_MORE => {
                let c = Chunk::parse(m)?;
                // 0x48F6AD: the sender is looked up by the byte, `player+0x40 == id`
                let p = self.players.iter_mut().find(|p| p.id == c.from as i32)?;
                // 0x490420 makes the packet only when there is none of that type yet — a second
                // `0xBF` for a type already there changes nothing
                if c.first && !p.files.iter().any(|f| f.kind == c.kind) {
                    let total = c.word as usize;
                    p.files.push(Packet { kind: c.kind, total, bytes: vec![0; total], done: 0 });
                }
                let f = p.files.iter_mut().find(|f| f.kind == c.kind)?;
                if f.complete() {
                    return None;
                }
                // 0x48F744…0x48F75F: the piece goes at `+0xC`, whatever offset a `0xC0` names.
                // The original copies `len − 8` bytes without looking at the size; the port stops
                // at the buffer's end rather than write past it.
                let at = f.done;
                let n = c.bytes.len().min(f.total - at);
                f.bytes[at..at + n].copy_from_slice(&c.bytes[..n]);
                f.done = at + c.bytes.len();
                let kind = f.kind;
                f.complete().then_some(Heard::File { from: c.from as i32, kind })
            }
            _ => None,
        }
    }

    /// **0x48CB50** `(type, data, size)`: keep the file on our own record, whole, and queue it to go
    /// out — once per type (the walk of `net+0x264` at 0x48CC16).
    pub fn post(&mut self, kind: u16, data: &[u8]) {
        let me = self.me;
        if let Some(p) = self.players.iter_mut().find(|p| p.id == me)
            && !p.files.iter().any(|f| f.kind == kind)
        {
            p.files.push(Packet { kind, total: data.len(), bytes: data.to_vec(), done: data.len() });
        }
        if !self.outgoing.iter().any(|f| f.kind == kind) {
            self.outgoing.push_back(Packet { kind, total: data.len(), bytes: data.to_vec(), done: 0 });
        }
    }

    /// Whether a file is still going out.
    pub fn sending(&self) -> bool {
        !self.outgoing.is_empty()
    }

    /// **0x48EFB5…0x48F02E** with **0x48CCC0** inside it: the clock is `now_ms << 5`, and while it
    /// is ahead of `net+0x1C` one turn runs and `net+0x1C` moves on by 1000. The first call only sets the
    /// clock, as 0x48E0EE does when the session opens.
    ///
    /// A turn is the queue's pump 0x48AE30 (`link.pump()`); only when it answers 0 — the send queue is
    /// empty, or not enabled yet — does one chunk of the file at the head of `net+0x264` go, through the
    /// message builder (`link.send`, which registers it in that same queue). A host has no such queue
    /// (`Room::tick`'s link is never busy).
    pub fn tick_with(&mut self, now_ms: u32, link: &mut dyn ChunkLink) {
        let now = now_ms.wrapping_shl(5);
        let Some(clock) = self.clock.as_mut() else {
            self.clock = Some(now);
            return;
        };
        while (now.wrapping_sub(*clock) as i32) > 0 {
            let busy = link.pump();
            if !busy
                && let Some(f) = self.outgoing.front_mut()
                && self.me >= 0
            {
                let first = f.done == 0;
                let n = (f.total - f.done).min(CHUNK);
                let msg = Chunk {
                    first,
                    from: self.me as u8,
                    kind: f.kind,
                    word: if first { f.total as u32 } else { f.done as u32 },
                    bytes: f.bytes[f.done..f.done + n].to_vec(),
                }
                .encode();
                f.done += n;
                if f.done >= f.total {
                    self.outgoing.pop_front();
                }
                link.send(&msg);
            }
            *clock = clock.wrapping_add(CHUNK_COST);
        }
    }

    /// [`Room::tick_with`] over a link that is never busy: the chunks that go out.
    pub fn tick(&mut self, now_ms: u32) -> Vec<Vec<u8>> {
        struct Free(Vec<Vec<u8>>);
        impl ChunkLink for Free {
            fn pump(&mut self) -> bool {
                false
            }
            fn send(&mut self, msg: &[u8]) {
                self.0.push(msg.to_vec());
            }
        }
        let mut link = Free(Vec::new());
        self.tick_with(now_ms, &mut link);
        link.0
    }

    /// **The host's room, 0x48BB50** — a player comes in. The newcomer is told of everyone already
    /// there, one `0xB0` each (0x48BC2C…0x48BC8F) — and, right after it, **`0xC5 (id)`** (0x48BC71…
    /// 0x48BC80: `0x48C870(0xC5, id, OMIT, null, newcomer)`) for each player whose bit is **clear**
    /// in `+0xCC2` (0x48BC53…0x48BC6A `test al, dl ; jne`) — a player already in the race, which the
    /// newcomer's `+0xC4 == 0` handler (0x48F911…0x48F91F) puts into `net+0xE5`; then everyone, the
    /// newcomer included, is told of the newcomer (0x48BCC1…0x48BCD8). Returns nothing once the room
    /// holds eight.
    ///
    /// Not carried from the same function: the `0xB2` that hands the newcomer its id and the game's
    /// name (this port's id comes with the session's `'a'`).
    pub fn admit(&self, id: i32, name: &[u8]) -> Vec<(To, Vec<u8>)> {
        if self.players.len() >= MAX_PLAYERS {
            return Vec::new();
        }
        let mut out: Vec<(To, Vec<u8>)> = Vec::new();
        for p in &self.players {
            out.push((To::Player(id), player_in(p.id, &p.name)));
            if !self.in_lobby_group(p.id) {
                out.push((To::Player(id), encode(kind::GO, Some(p.id), None, None)));
            }
        }
        out.push((To::All, player_in(id, name)));
        // 0x491AAA / 0x491C1B: once the room has the player, `0xC6 (0xE)` tells its link it is in
        out.push((To::Player(id), state(comm_state::CONNECTED)));
        out
    }

    /// **0x4912D4…0x4913AC** — the host passes a player's line on. The copy goes to the addressee's
    /// link, or to every link when 0x48C980 finds none: it answers null for any id at or below −1
    /// **and for an id nobody has**, so a line to a player who is not there reaches everyone. Then,
    /// when the line was for one player and that player is not the sender, the sender gets the
    /// same copy (0x491329…0x491350) — which is how a whisper shows on both screens.
    ///
    /// The host checks two flags on the sender's record first, `+0xBA` (in) and `+0xBD` (version
    /// mismatch, see §"The peer record"); a port peer is always in and never mismatched.
    pub fn route_chat(&self, from: i32, to: i32, text: &[u8]) -> Vec<(To, Vec<u8>)> {
        let line = ChatLine { from, to, text: text.to_vec() }.encode();
        let dest = if to > TO_ALL && self.player(to).is_some() { To::Player(to) } else { To::All };
        let mut out = vec![(dest, line.clone())];
        if to != TO_ALL && to != from {
            out.push((To::Player(from), line));
        }
        out
    }
}

impl Room {
    /// **The room's `0xB3` case, 0x4917C0 → 0x48BF10** — a player asks for the race. Its bit leaves
    /// the lobby group (0x48BF21…0x48BF2B); when no race is running yet (0x48BF38 `+0xCC3`, `jne
    /// 0x48BFA4`) the room starts one — `+0xCC3 = 1` (0x48BF51), `+0xCC4 = 0` (0x48BF58) — and tells
    /// every link `0xB3 (0, clock)` (0x48BF96…0x48BF9E); a request during a running race only moves
    /// the bit. (0x48BF10's other
    /// stores — the input-exchange buffers at `+0xB0`, `+0xCB0`, `+0xCB4`, `+0xCC1` — belong to the
    /// input-exchange mode; 0x4917E8…0x49184C, which registers the race with a session list and a
    /// reference count, has no port counterpart.)
    pub fn start_request(&mut self, from: i32, clock: u32) -> Vec<(To, Vec<u8>)> {
        if (0..8).contains(&from) {
            self.lobby &= !(1u8 << from);
        }
        if self.running {
            return Vec::new();
        }
        self.running = true;
        self.go_sent = false;
        vec![(To::All, race_start(clock))]
    }

    /// **The room's `0xC5` case, 0x49185B → 0x48BFB0**: the first request of a race is answered with
    /// `0xC5 (0, clock)` to every link (0x48BFE1…0x48BFEA) and latched in `+0xCC4` (0x48BFEF, tested
    /// at 0x48BFB3); later ones change nothing.
    pub fn go_request(&mut self, clock: u32) -> Vec<(To, Vec<u8>)> {
        if self.go_sent {
            return Vec::new();
        }
        self.go_sent = true;
        vec![(To::All, go(clock))]
    }

    /// **The room's `0xB7` case, 0x4918BD → 0x4929B0 → 0x48C220**: the sender's head is a finish
    /// time, −1 ("Leaving game") or −2 ("Returning to frontend"); every link is sent `0xB7 (sender,
    /// head)` (0x48C245). For −2 the sender's bit joins the lobby group again (0x48C24F…0x48C25D)
    /// and when every player of the room is in it (`+0xCC0 & +0xCC2 == +0xCC0`, `+0xCC0` being the
    /// room's players) the race is over: `+0xCC3 = 0` (0x48C26B…0x48C273).
    pub fn race_time_request(&mut self, from: i32, head: i32) -> Vec<(To, Vec<u8>)> {
        if head == -2 && (0..8).contains(&from) {
            self.lobby |= 1u8 << from;
            let room = self.players.iter().fold(0u8, |m, p| m | 1u8.checked_shl(p.id as u32).unwrap_or(0));
            if room & self.lobby == room {
                self.running = false;
            }
        }
        let mut out = vec![(To::All, race_time(from, head))];
        if head == -2 {
            // 0x48C27A…0x48C2A0: after the bit joined the lobby group every player's list is replayed to the sender
            out.extend(self.replay_all().into_iter().map(|m| (To::Player(from), m)));
        }
        out
    }

    /// Whether a race is running in the room (`+0xCC3`).
    pub fn race_running(&self) -> bool {
        self.running
    }

    /// **The room's relay of `0xBC`, 0x48C050** (the server dispatch's case, 0x49187F): the sender's
    /// lobby-group bit (`+0xCC2`, 0x48C062…0x48C079) is compared with every room player's
    /// (0x48C097…0x48C0A4), and only those with **the same** bit are sent the record. The original's
    /// list includes the sender, which hears its own record back; the port has applied it already, so
    /// the sender is left out. (When the sender is in the lobby group the original also files the
    /// record on its player, 0x48A2F0, which the port has no use for: 0x48BB50 replays nothing of the
    /// sort to a newcomer, `network.md` §11.9.)
    pub fn relay_options(&self, from: i32, m: &[u8]) -> Vec<(To, Vec<u8>)> {
        let group = self.in_lobby_group(from);
        self.players
            .iter()
            .filter(|p| p.id != from && self.in_lobby_group(p.id) == group)
            .map(|p| (To::Player(p.id), m.to_vec()))
            .collect()
    }

    /// **0x48A2F0, called by 0x48C050 when the sender is in the lobby group** (`+0xCC2` bit set, 0x48C0C9): the message goes
    /// on the sender's own list, after every node whose dword at `+4` equals the message's (0x48A316…0x48A332). A sender
    /// already in the race files nothing. The host's own sends come through here too (its loopback link, id 0).
    pub fn file_options(&mut self, from: i32, m: &[u8]) {
        if !self.in_lobby_group(from) || m.len() < 8 {
            return;
        }
        let list = self.filed.entry(from).or_default();
        list.retain(|n| n.get(4..8) != m.get(4..8));
        list.push(m.to_vec());
    }

    /// **0x48A390 for every player already in the room** (0x48BBEE…0x48BBFC, the newcomer's admission): each player's list in
    /// the order of the room's list, host first, every node as it was sent.
    pub fn replay_all(&self) -> Vec<Vec<u8>> {
        self.players.iter().flat_map(|p| self.filed.get(&p.id).into_iter().flatten().cloned()).collect()
    }

    /// Whether `id` is in the lobby group (`+0xCC2`).
    pub fn in_lobby_group(&self, id: i32) -> bool {
        (0..8).contains(&id) && self.lobby & (1u8 << id) != 0
    }
}

/// `sprintf` for the chat's three formats (`"%s: %s"`, FE 580, FE 581): each `%s` takes the next
/// argument, `%%` is a percent sign. The texts themselves never carry a `%` — the input box refuses
/// it (0x4CCDA2).
pub fn format(f: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut chars = f.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('s') => out.push_str(it.next().copied().unwrap_or("")),
                Some('%') => out.push('%'),
                Some(o) => {
                    out.push('%');
                    out.push(o);
                }
                None => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x48BF10 sends `0xB3` with the head's value 0 and the host's clock as the word; 0x48F82F
    /// reads that word back byte by byte.
    #[test]
    fn the_race_start_carries_the_hosts_clock() {
        let m = race_start(0x0102_0304);
        assert_eq!(m, vec![0xB3, 0, 0, 0, 1, 2, 3, 4]);
        let mut r = Room::new(1);
        assert_eq!(r.hear(&m), Some(Heard::Start { clock: 0x0102_0304 }));
        assert_eq!(r.hear(&m[..6]), None, "a short one is not a start");
    }

    #[test]
    fn the_two_chat_shapes_differ_by_the_omitted_word() {
        // 0x48D9B0: `push 0x80000000` for the word, the addressee in the head
        let req = ChatRequest { to: TO_ALL, text: b"hi".to_vec() }.encode();
        assert_eq!(req, vec![0xBB, 0xFF, 0xFF, 0xFF, b'h', b'i', 0]);
        assert_eq!(ChatRequest::parse(&req), Some(ChatRequest { to: -1, text: b"hi".to_vec() }));
        // 0x491324: the sender in the head, the addressee as a big-endian word
        let line = ChatLine { from: 2, to: 5, text: b"yo".to_vec() }.encode();
        assert_eq!(line, vec![0xBB, 0, 0, 2, 0, 0, 0, 5, b'y', b'o', 0]);
        assert_eq!(ChatLine::parse(&line), Some(ChatLine { from: 2, to: 5, text: b"yo".to_vec() }));
        // 0x48F3B9: -2 never reaches the box
        let hidden = ChatLine { from: 1, to: TO_NOBODY, text: b"CTRL-ALT!".to_vec() }.encode();
        assert_eq!(ChatLine::parse(&hidden), None);
    }

    #[test]
    fn a_message_stops_at_199_bytes_and_is_terminated() {
        let long = vec![b'a'; 400];
        let m = encode(kind::CHAT, Some(-1), None, Some(&long));
        assert_eq!(m.len(), MESSAGE_MAX + 1);
        assert_eq!(*m.last().unwrap(), 0);
        // and a missing value moves the word forward
        assert_eq!(encode(0xB3, None, Some(0x0102_0304), None), vec![0xB3, 1, 2, 3, 4]);
    }

    #[test]
    fn the_host_routes_a_whisper_to_both_ends_and_an_unknown_one_to_everyone() {
        let mut r = Room::new(0);
        for m in [player_in(0, b"host"), player_in(1, b"anna"), player_in(2, b"ben")] {
            r.hear(&m);
        }
        let to_all = r.route_chat(1, TO_ALL, b"x");
        assert_eq!(to_all.len(), 1);
        assert_eq!(to_all[0].0, To::All);
        let whisper = r.route_chat(1, 2, b"x");
        assert_eq!(whisper.iter().map(|(t, _)| *t).collect::<Vec<_>>(), vec![To::Player(2), To::Player(1)]);
        // to oneself: only the one copy
        assert_eq!(r.route_chat(1, 1, b"x").len(), 1);
        // 0x48C980 finds no link for id 7 and answers null: everyone, and the sender again
        let lost = r.route_chat(1, 7, b"x");
        assert_eq!(lost.iter().map(|(t, _)| *t).collect::<Vec<_>>(), vec![To::All, To::Player(1)]);
    }

    #[test]
    fn a_newcomer_hears_of_everyone_first_and_then_everyone_of_it() {
        let mut r = Room::new(0);
        r.hear(&player_in(0, b"host"));
        r.hear(&player_in(1, b"anna"));
        let out = r.admit(2, b"ben");
        assert_eq!(out[0], (To::Player(2), player_in(0, b"host")));
        assert_eq!(out[1], (To::Player(2), player_in(1, b"anna")));
        assert_eq!(out[2], (To::All, player_in(2, b"ben")));
        // and then the room tells the newcomer's link it is in (0x491AAA)
        assert_eq!(out[3], (To::Player(2), state(comm_state::CONNECTED)));
        assert_eq!(r.hear(&out[3].1), Some(Heard::State(14)));
        assert_eq!(r.hear(&player_out(1)), Some(Heard::Left(1)));
        assert_eq!(r.players.iter().map(|p| p.id).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn a_players_message_list_is_the_lobby_state_one_node_per_key() {
        // 0x48A2F0 (from 0x48C050): the sender's list drops every node with the same dword at +4, then appends; 0x48A390 replays the
        // lists host first (0x48BBEE…0x48BBFC); a sender that left the lobby group (0xB3, bit cleared) files nothing
        let rec = |k: u16, v: u8| vec![0xBC, 0, 0, 0, k as u8, (k >> 8) as u8, 0, 0, v, 0, 0, 0];
        let mut r = Room::new(0);
        r.hear(&player_in(0, b"host"));
        r.hear(&player_in(1, b"anna"));
        for m in [rec(0x14, 1), rec(0xD, 0), rec(0xF, 1), rec(0xC, 7), rec(0xD, 2)] {
            r.file_options(0, &m);
        }
        r.file_options(1, &rec(0x255, 0));
        r.file_options(1, &rec(0x255, 1));
        // the second 0xD replaced the first and sits last; anna's list holds her last READY only
        assert_eq!(r.replay_all(), vec![rec(0x14, 1), rec(0xF, 1), rec(0xC, 7), rec(0xD, 2), rec(0x255, 1)]);
        r.start_request(1, 100);
        r.file_options(1, &rec(0x256, 1));
        assert_eq!(r.replay_all().len(), 5, "anna is in the race: nothing more is filed");
        // 0xB7 (−2): her bit returns and every list goes to her (0x48C27A…0x48C2A0)
        let out = r.race_time_request(1, -2);
        assert_eq!(out.len(), 1 + 5);
        assert!(out[1..].iter().all(|(t, _)| *t == To::Player(1)));
        // a player that leaves takes its list
        r.hear(&player_out(1));
        assert_eq!(r.replay_all().len(), 4);
    }

    #[test]
    fn a_lobby_record_reaches_only_the_senders_group() {
        // 0x48C050: `sender bit == player bit` — anna has asked for the race and left the lobby group
        let mut r = Room::new(0);
        for (id, n) in [(0, &b"host"[..]), (1, b"anna"), (2, b"ben")] {
            r.hear(&player_in(id, n));
        }
        let m = vec![0xBC, 0, 0, 1];
        let all: Vec<i32> = r.relay_options(2, &m).iter().map(|(t, _)| if let To::Player(i) = t { *i } else { -1 }).collect();
        assert_eq!(all, vec![0, 1], "everybody is in the lobby group: the others hear it");
        r.start_request(1, 100);
        let lobby: Vec<i32> = r.relay_options(2, &m).iter().map(|(t, _)| if let To::Player(i) = t { *i } else { -1 }).collect();
        assert_eq!(lobby, vec![0], "anna is in the race now: a lobby record does not reach her");
        let racing: Vec<i32> = r.relay_options(1, &m).iter().map(|(t, _)| if let To::Player(i) = t { *i } else { -1 }).collect();
        assert!(racing.is_empty(), "and hers reaches nobody in the lobby");
    }

    #[test]
    fn a_newcomer_is_told_which_players_are_already_in_the_race() {
        // 0x48BC53…0x48BC80: after each `0xB0`, `0xC5 (id)` for a player whose `+0xCC2` bit is clear
        let mut r = Room::new(0);
        r.hear(&player_in(0, b"host"));
        r.hear(&player_in(1, b"anna"));
        r.start_request(1, 500); // anna asks for the race: her bit leaves the lobby group
        let out = r.admit(2, b"ben");
        assert_eq!(out[0], (To::Player(2), player_in(0, b"host")));
        assert_eq!(out[1], (To::Player(2), player_in(1, b"anna")));
        assert_eq!(out[2], (To::Player(2), encode(kind::GO, Some(1), None, None)));
        assert_eq!(out[3], (To::All, player_in(2, b"ben")));
        // the newcomer's `+0xC4 == 0` handler puts her bit into `net+0xE5` (0x48F91F)
        assert_eq!(r.hear(&out[2].1), Some(Heard::Go { id: 1, clock: 0 }));
    }

    #[test]
    fn a_file_goes_out_in_chunks_of_232_at_32_a_second_and_comes_back_whole() {
        let file: Vec<u8> = (0..6960u32).map(|i| (i * 7) as u8).collect();
        let mut a = Room::new(3);
        a.hear(&player_in(3, b"me"));
        a.post(PICTURE, &file);
        // our own copy is whole at once (0x48CC0E adds the size to +0xC)
        assert!(a.player(3).unwrap().files[0].complete());
        let mut b = Room::new(0);
        b.hear(&player_in(3, b"me"));
        assert!(a.tick(1000).is_empty(), "the first tick only sets the clock");
        // one second is 32 000 units, 32 turns of the loop — enough for all 30 chunks
        let chunks = a.tick(2000);
        assert_eq!(chunks.len(), 6960usize.div_ceil(CHUNK));
        let head = Chunk::parse(&chunks[0]).unwrap();
        assert!(head.first);
        assert_eq!((head.from, head.kind, head.word, head.bytes.len()), (3, PICTURE, 6960, CHUNK));
        let second = Chunk::parse(&chunks[1]).unwrap();
        assert!(!second.first);
        assert_eq!(second.word, CHUNK as u32);
        let mut heard = None;
        for c in &chunks {
            heard = b.hear(c).or(heard);
        }
        assert_eq!(heard, Some(Heard::File { from: 3, kind: PICTURE }));
        assert_eq!(b.player(3).unwrap().files[0].bytes, file);
        assert!(!a.sending());
    }

    #[test]
    fn the_chat_formats_take_their_arguments_in_order() {
        assert_eq!(format("%s: (whisper to %s) %s", &["ANNA", "BEN", "hi"]), "ANNA: (whisper to BEN) hi");
        assert_eq!(format("%s: %s", &["A", "b"]), "A: b");
    }
}
