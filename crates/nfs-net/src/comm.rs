//! The EA Comm control messages — the layer that owns the datagram's tag byte
//! (`RE/notes/network.md` §7.4 … §7.7).
//!
//! 0x588A80 dispatches on the tag after subtracting `'!'`, over 0x57 cases, so every message type
//! is a printable character. Thirteen of them are handled and the rest are dropped. The bodies are
//! **big-endian** — they are read with 0x56E290 and written with 0x56E2C0, never with the
//! little-endian pair 0x56E2A8 / 0x56E301.

use crate::wire::Addr;
use thiserror::Error;

/// The tag characters 0x589228's index table actually routes somewhere.
pub mod tag {
    /// 0x589067.
    pub const STAR: u8 = b'*';
    /// 0x5891C5 — handed straight to the connection's callback at `conn+0x18`. `0x5894C0` sends
    /// under it, broadcast and with no header of its own, but **nothing in the original calls
    /// `0x5894C0`** (H36, refuted) — the game's messages ride a data layer (§7.11). This port
    /// still uses the tag: it is a real one the dispatcher handles, and it carries the messages
    /// between two copies of the port.
    pub const USER: u8 = b'<';
    /// 0x5890ED — keep-alive; resets the peer's countdown to `[0x5E1A08]` = 100.
    pub const ALIVE: u8 = b'=';
    /// 0x588D5C — "that slot is not in use", the negative answer to [`QUERY`].
    pub const NO_SUCH: u8 = b'?';
    /// 0x589152 — leave; clears the peer's bit in the mask at `conn[0x1C]+0x50`.
    pub const BYE_K: u8 = b'K';
    /// 0x589152 — the same handler as [`BYE_K`].
    pub const BYE_B: u8 = b'b';
    /// 0x588FA4.
    pub const A: u8 = b'a';
    /// 0x588B45 — the session description (§7.6).
    pub const DESCRIBE: u8 = b'd';
    /// 0x588CA2 — "tell me about slot n", answered with [`REPLY`] or [`NO_SUCH`].
    pub const QUERY: u8 = b'g';
    /// 0x588E11 — join; allocates a player slot and copies the caller's address record.
    pub const JOIN: u8 = b'j';
    /// 0x588D5C — the answer to [`QUERY`] (§7.5).
    pub const REPLY: u8 = b'p';
    /// 0x589029.
    pub const Q: u8 = b'q';
    /// 0x588B06.
    pub const R: u8 = b'r';
    /// 0x588AB8 — "who has this session?", answered with [`DESCRIBE`].
    pub const W: u8 = b'w';
    /// The answer to [`Q`]: an empty datagram. It has no case of its own in 0x588A80 — 0x5889D0
    /// only reports it back to its caller (`cmp bl, 0x21`).
    pub const QUIT_ACK: u8 = b'!';
}

/// The lowest and highest characters the jump table covers — outside this the message is dropped
/// before the switch (`cmp eax, 0x56 ; ja`).
pub const FIRST_TAG: u8 = b'!';
pub const LAST_TAG: u8 = b'w';

/// The keep-alive countdown a `'='` (and a successful `'a'`) restores, from `[0x5E1A08]`.
pub const ALIVE_TICKS: u32 = 100;

/// The longer countdown a `'*'` (and a successful `'a'`) restores, from `[0x5E1A04]`.
pub const PING_TICKS: u32 = 450;

/// Does 0x588A80 have a case for this tag, or does it fall through to 0x5891E2?
pub fn is_handled(t: u8) -> bool {
    HANDLED.contains(&t)
}

/// The characters 0x588A80 has a case for: fourteen characters, thirteen handlers (`'K'` and `'b'`
/// share 0x589152, `'?'` and `'p'` share 0x588D5C).
pub const HANDLED: [u8; 14] = [
    tag::STAR,
    tag::USER,
    tag::ALIVE,
    tag::NO_SUCH,
    tag::BYE_K,
    tag::BYE_B,
    tag::A,
    tag::DESCRIBE,
    tag::QUERY,
    tag::JOIN,
    tag::REPLY,
    tag::Q,
    tag::R,
    tag::W,
];

/// `0x56E290(p, n)` — `n` bytes at `p`, most significant first.
pub fn read_be(bytes: &[u8], at: usize, n: usize) -> Option<u32> {
    let s = bytes.get(at..at + n)?;
    Some(s.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32))
}

/// `0x56E2C0(dst, value, n)` — the same order, and the original returns `dst + n` so calls chain.
pub fn write_be(out: &mut Vec<u8>, value: u32, n: usize) {
    let full = value.to_be_bytes();
    out.extend_from_slice(&full[4 - n..]);
}

/// `0x56E2A8(p, n)` — the little-endian reader, which the control messages never use.
pub fn read_le(bytes: &[u8], at: usize, n: usize) -> Option<u32> {
    let s = bytes.get(at..at + n)?;
    Some(s.iter().rev().fold(0u32, |acc, b| (acc << 8) | *b as u32))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommError {
    #[error("payload is {have} bytes, {want} wanted")]
    Short { have: usize, want: usize },
    #[error("session id {got:#010x} is not this session's {want:#010x}")]
    WrongSession { got: u32, want: u32 },
}

/// The head every slot-addressed control message carries (§7.5).
///
/// `'g'` sends it, `'p'` and `'?'` echo all three fields back, and `'p'`'s handler drops the
/// datagram outright when `session` does not match its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// `+0x00`, big-endian — the **request number**, not a session id. `0x588130` takes it from the
    /// global counter at `[0x5E1CE8]` (which starts at 1), post-increments, writes it here with
    /// `0x56E2C0` and keeps it in the request context at `conn+0x20`; every reply handler then
    /// checks the four bytes against that context and drops the datagram when they differ
    /// (0x588B7C, 0x588DAB, 0x588FD5). A `'w'` carries no request, so the `'d'` it provokes echoes
    /// **0** (0x588AF0 passes 0 to 0x5892A0).
    pub request: u32,
    /// `+0x04`, the sender's 20-byte transport address record.
    pub from: Addr,
    /// `+0x18`, big-endian; `-1` asks about the session itself rather than a player.
    pub index: i32,
}

impl Header {
    pub const SIZE: usize = 0x1C;
    /// The index that means "the session, not a player" — `cmp eax, -1` at 0x588CE7.
    pub const SESSION_SLOT: i32 = -1;

    pub fn encode(&self, out: &mut Vec<u8>) {
        write_be(out, self.request, 4);
        out.extend_from_slice(&self.from.0);
        write_be(out, self.index as u32, 4);
    }

    pub fn decode(p: &[u8]) -> Result<Self, CommError> {
        if p.len() < Self::SIZE {
            return Err(CommError::Short { have: p.len(), want: Self::SIZE });
        }
        let mut from = [0u8; 20];
        from.copy_from_slice(&p[4..24]);
        Ok(Self {
            request: read_be(p, 0, 4).unwrap(),
            from: Addr(from),
            index: read_be(p, 0x18, 4).unwrap() as i32,
        })
    }
}

/// The record `'p'` carries after the header: 0x17 dwords copied straight out of the connection
/// (0x588CF8 / 0x588D23), so the port keeps it opaque rather than inventing a meaning for it.
pub const RECORD_SIZE: usize = 0x5C;

/// The whole `'p'` reply — `0x78` bytes (0x588CD2).
pub const REPLY_SIZE: usize = Header::SIZE + RECORD_SIZE;

/// Build the answer to a `'g'`: `'p'` with the record when the slot is live, `'?'` with the bare
/// header when it is not (0x588D27 drops the length to 0x1C and the tag to `'?'`).
pub fn answer_query(head: Header, record: Option<&[u8; RECORD_SIZE]>) -> (u8, Vec<u8>) {
    let mut out = Vec::with_capacity(REPLY_SIZE);
    head.encode(&mut out);
    match record {
        Some(r) => {
            out.extend_from_slice(r);
            (tag::REPLY, out)
        }
        None => (tag::NO_SUCH, out),
    }
}

/// The session description a `'d'` carries (§7.6 reads it, §7.10 builds it).
///
/// 0x5892A0 sends **0xAC** bytes but only writes the fields below; the gaps carry whatever was on
/// its stack, so this port sends zeroes there — an **APPROXIMATION** of those bytes, and the only
/// one in this module. The handler 0x588B45 never reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Describe {
    /// `+0x00` — the request number the `'r'` carried, or **0** when a `'w'` provoked it.
    pub request: u32,
    /// `+0x04`, the session's own address record (`conn+0x24`).
    pub from: Addr,
    /// `+0x18`, `conn+0x94`.
    pub conn_dword: u32,
    /// `+0x1C` and `+0x1E`, `conn+0x98` / `conn+0x9A`.
    pub conn_words: [u16; 2],
    /// `+0x20` and `+0x24`, `conn+0x9C` / `conn+0xA0` — stored with plain `mov`s, so these two are
    /// **little-endian** on the wire while everything around them is big-endian.
    pub conn_raw: [u32; 2],
    /// `+0x28`, 32 bytes — the name, which the handler copies into `rec+0x1C`.
    pub name: [u8; 32],
    /// `+0x48` … `+0x4B`, `session+0x74` … `+0x77`.
    pub bytes: [u8; 4],
    /// `+0x4C` and `+0x4E`, `session+0x78` / `+0x7A`.
    pub words: [u16; 2],
    /// `+0x50` and `+0x54`, `session+0x7C` / `+0x80`.
    pub dwords: [u32; 2],
    /// `+0x58`, `session+0x84` — how many players the session takes.
    pub player_limit: u32,
    /// `+0x6C`, `session+0x50` — which slots are occupied.
    pub slot_mask: u32,
}

/// The game's own session descriptor as it rides in a `'d'` (`network.md` §11.5): the 0x34 bytes
/// 0x509B00 builds and the comm layer keeps at `session+0x54…0x87`, which the `'d'` puts at
/// `+0x28…+0x5B`, so that `desc + k` is `'d'` payload `+0x28 + k`:
///
/// | desc | `'d'` | what (0x509B00) |
/// |---|---|---|
/// | `+0x00` | `+0x28` | `Comms.GameName`'s text (≤ 0x20), to which 0x48DD97 appends `~` and the version string `NfsNet+0x2C` (0x48DDC2…0x48DDF8) |
/// | `+0x21` | `+0x49` | `Comms.Positional`'s index (0x509C1D) |
/// | `+0x22` | `+0x4A` | `Track1.Laps`' index when the track is a circuit, else 0 (0x509BE9 / 0x509BEE) |
/// | `+0x24` | `+0x4C` | `Track1.Direction`'s slot 27 = `0x5B963C[index]`, the direction's string id (0x509BAE, 0x4BB790) |
/// | `+0x26` | `+0x4E` | `Track1.Track`'s slot 28, the track record's name id = `TRACKLOCALEID0` (0x509B8E) |
/// | `+0x30` | `+0x58` | the player limit, 8 (0x509B49) |
///
/// The games list's row draw (0x4BA030) reads the track from `+0x26`, the mode from `+0x21` and
/// the version from what follows the `~` (0x48C3D0: `sscanf("%[^~]~%s")`).
pub mod desc {
    /// `desc+0x21` — `Comms.Positional`'s index.
    pub const POSITIONAL: usize = 1;
    /// `desc+0x22` — `Track1.Laps`' index on a circuit.
    pub const LAPS: usize = 2;
    /// `desc+0x24` — `Track1.Direction`'s string id.
    pub const DIRECTION: usize = 0;
    /// `desc+0x26` — the track's name id.
    pub const TRACK: usize = 1;
    /// `'d'` carries the descriptor's first 0x20 bytes as the name.
    pub const NAME_LEN: usize = 32;

    /// 0x48DD97…0x48DDF8: `name` + `~` + `version`, cut to the descriptor's 0x20 bytes (the last
    /// stays a NUL). The name is what 0x509B00 copied (`strncpy` 0x20), so it is cut first. **An empty name becomes
    /// `"Game"`** (0x48DD67 `jne` over 0x48DD69…0x48DD8B, which copies the string at 0x5D07A4) — the original's host that typed
    /// nothing announces `Game~3.5` (captured).
    pub fn name_field(name: &str, version: &str) -> [u8; NAME_LEN] {
        let name = if name.is_empty() { "Game" } else { name };
        let mut out = [0u8; NAME_LEN];
        // one byte per character: the game's strings are single-byte text (the port keeps them
        // as chars below U+0100), and the wire carries those bytes
        let mut s: Vec<u8> = name.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).take(NAME_LEN - 1).collect();
        s.push(b'~');
        s.extend(version.bytes());
        for (o, b) in out.iter_mut().zip(s.iter().take(NAME_LEN - 1)) {
            *o = *b;
        }
        out
    }

    /// 0x48C3D0's `sscanf(desc, "%[^~]~%s", name, version)`: everything before the first `~`, then
    /// the first word after it. No `~` leaves the version unwritten — empty here.
    pub fn split_name(field: &[u8]) -> (String, String) {
        let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
        let s: String = field[..end].iter().map(|&b| char::from(b)).collect();
        match s.split_once('~') {
            Some((n, v)) => (n.to_string(), v.split_whitespace().next().unwrap_or("").to_string()),
            None => (s, String::new()),
        }
    }
}

impl Describe {
    /// What 0x58941B passes to the transport.
    pub const SIZE: usize = 0xAC;
    /// The last field the handler reads is `+0x6C`, so this much has to arrive.
    pub const READ: usize = 0x70;

    pub fn decode(p: &[u8]) -> Result<Self, CommError> {
        if p.len() < Self::READ {
            return Err(CommError::Short { have: p.len(), want: Self::READ });
        }
        let mut from = [0u8; 20];
        from.copy_from_slice(&p[4..24]);
        let mut name = [0u8; 32];
        name.copy_from_slice(&p[0x28..0x48]);
        Ok(Self {
            request: read_be(p, 0, 4).unwrap(),
            from: Addr(from),
            conn_dword: read_be(p, 0x18, 4).unwrap(),
            conn_words: [read_be(p, 0x1c, 2).unwrap() as u16, read_be(p, 0x1e, 2).unwrap() as u16],
            conn_raw: [read_le(p, 0x20, 4).unwrap(), read_le(p, 0x24, 4).unwrap()],
            name,
            bytes: [p[0x48], p[0x49], p[0x4a], p[0x4b]],
            words: [read_be(p, 0x4c, 2).unwrap() as u16, read_be(p, 0x4e, 2).unwrap() as u16],
            dwords: [read_be(p, 0x50, 4).unwrap(), read_be(p, 0x54, 4).unwrap()],
            player_limit: read_be(p, 0x58, 4).unwrap(),
            slot_mask: read_be(p, 0x6c, 4).unwrap(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; Self::SIZE];
        out[0x00..0x04].copy_from_slice(&self.request.to_be_bytes());
        out[0x04..0x18].copy_from_slice(&self.from.0);
        out[0x18..0x1c].copy_from_slice(&self.conn_dword.to_be_bytes());
        out[0x1c..0x1e].copy_from_slice(&self.conn_words[0].to_be_bytes());
        out[0x1e..0x20].copy_from_slice(&self.conn_words[1].to_be_bytes());
        out[0x20..0x24].copy_from_slice(&self.conn_raw[0].to_le_bytes());
        out[0x24..0x28].copy_from_slice(&self.conn_raw[1].to_le_bytes());
        out[0x28..0x48].copy_from_slice(&self.name);
        out[0x48..0x4c].copy_from_slice(&self.bytes);
        out[0x4c..0x4e].copy_from_slice(&self.words[0].to_be_bytes());
        out[0x4e..0x50].copy_from_slice(&self.words[1].to_be_bytes());
        out[0x50..0x54].copy_from_slice(&self.dwords[0].to_be_bytes());
        out[0x54..0x58].copy_from_slice(&self.dwords[1].to_be_bytes());
        out[0x58..0x5c].copy_from_slice(&self.player_limit.to_be_bytes());
        out[0x6c..0x70].copy_from_slice(&self.slot_mask.to_be_bytes());
        out
    }
}

/// `'j'` — asking a host for a slot (§7.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// `+0x00` — the request number (see [`Header::request`]).
    pub request: u32,
    /// `+0x04` — the address the joiner **typed**, i.e. the host's (0x588030's argument 2); the host
    /// echoes it in its `'a'` and the `'a'` handler copies it to `conn+0x24`.
    pub from: Addr,
    /// `+0x18`, 16 bytes; the host refuses the join when this equals its own `session+0x3C`. The
    /// original's connect path leaves it zero.
    pub name: [u8; 16],
    /// `+0x28` onwards — the player record 0x589440 copies into the slot. Not decoded, so it
    /// travels as it arrived.
    pub record: Vec<u8>,
}

/// The application's own GUID, `0x5B4980` (`{5CD46B81-ECE4-11D3-9B6A-00902724AB1E}`), which 0x48AC27
/// hands the CONMAN constructor. Its text form is the session key (`eagame_key`) and its fields
/// ride in a `'d'` at `+0x18…+0x27`.
pub const APP_GUID: [u8; 16] = [0x81, 0x6B, 0xD4, 0x5C, 0xE4, 0xEC, 0xD3, 0x11, 0x9B, 0x6A, 0x00, 0x90, 0x27, 0x24, 0xAB, 0x1E];

/// **0x587F90**: `sprintf(out, "EAGAME:%08x.%04x%04x.%02x%02x%02x%02x.%02x%02x%02x%02x", u32@0, u16@4,
/// u16@6, b@8, b@9, b@10, b@11, b@12, b@13, b@14, b@15)` (the format string is 0x5C15F8; callers 0x587EE6,
/// 0x58836A, 0x588466). `'w'` compares the first 0x20 bytes of this text (0x588AD9) and the browse
/// (0x560620) sends `strlen + 1` bytes of it.
pub fn eagame_key(g: &[u8; 16]) -> String {
    format!(
        "EAGAME:{:08x}.{:04x}{:04x}.{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}",
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
        u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]),
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

/// What the `'w'` probe carries: the key text and its NUL (`strlen + 1`, 0x560620 / 0x56062C…0x560630).
pub fn probe_payload(g: &[u8; 16]) -> Vec<u8> {
    let mut v = eagame_key(g).into_bytes();
    v.push(0);
    v
}

/// The 32 bytes a host compares a probe against (`session+0x0C`, 0x588AD9): the key text cut to 0x20.
pub fn key32(g: &[u8; 16]) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (o, b) in k.iter_mut().zip(eagame_key(g).bytes()) {
        *o = b;
    }
    k
}

/// What the `'j'` builder 0x588030 puts in the 0x88-byte payload: `+0x00` the request (0x588130),
/// `+0x04` the **host's** address record (its argument 2, the typed address, also stored at `conn+0x24`),
/// `+0x18` 16 bytes (its argument 3, a name — null from the connect path, so the bytes are zero), `+0x28`
/// the session's own 0x5C-byte record (`session+0x38`, 0x17 dwords) and `+0x84` four bytes the builder
/// never writes. Offsets inside the record (`RE/notes/REFERENCE-2026-10-02-SRVE.md` §2, measured):
pub mod record {
    /// `rec+0x00` — the sender's own sockaddr (20 bytes). The host overwrites it with the datagram's
    /// source address before it keeps the record (0x588E11: 5 dwords from `param_2`).
    pub const ADDR: usize = 0;
    /// `rec+0x14` — `"Player"`, a 16-byte field (payload `+0x3C`), the same for every profile.
    pub const PLAYER: usize = 0x14;
    /// `rec+0x48` — `<profile>~<version>~<version>`, 20 bytes incl. the NUL (payload `+0x70`); 0x48A700
    /// reads it as `[[this]+0x48]` with `"%[^~]~%[^~]~%s"`.
    pub const IDENTITY: usize = 0x48;
    pub const IDENTITY_LEN: usize = 0x14;
    /// the 0x17 dwords 0x589440 keeps
    pub const SIZE: usize = 0x5C;
    /// the field's text, constant in the capture of both profiles
    pub const PLAYER_TEXT: &[u8] = b"Player";
}

impl Join {
    /// Everything before the record.
    pub const HEAD: usize = 0x28;
    /// `push 0x88` at 0x588107: the payload's length.
    pub const SIZE: usize = 0x88;

    /// The `'j'` an original joiner sends (0x588030 + 0x588130), field by field. `to` is the host's
    /// address as typed, `own` the joiner's own socket record, `identity` the `name~version~game` text.
    /// **APPROXIMATION:** the four bytes at `+0x84` are stack leftovers in the original (`e0 0a 37 00` in
    /// both captured sessions); a host never reads them (0x589440 copies 0x5C bytes), so they are zero.
    pub fn original(request: u32, to: Addr, own: Addr, identity: &[u8]) -> Self {
        let mut rec = vec![0u8; record::SIZE + 4];
        rec[..20].copy_from_slice(&own.0);
        rec[record::PLAYER..record::PLAYER + record::PLAYER_TEXT.len()].copy_from_slice(record::PLAYER_TEXT);
        // the field is 20 bytes with its NUL
        let t = identity.iter().take_while(|&&c| c != 0).take(record::IDENTITY_LEN - 1);
        for (o, b) in rec[record::IDENTITY..record::IDENTITY + record::IDENTITY_LEN].iter_mut().zip(t) {
            *o = *b;
        }
        Self { request, from: to, name: [0; 16], record: rec }
    }

    /// The identity text of a record (`rec+0x48`, up to its NUL).
    pub fn identity(&self) -> &[u8] {
        let f = self.record.get(record::IDENTITY..).unwrap_or(&[]);
        let f = &f[..f.len().min(record::IDENTITY_LEN)];
        &f[..f.iter().position(|&c| c == 0).unwrap_or(f.len())]
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::HEAD + self.record.len());
        write_be(&mut out, self.request, 4);
        out.extend_from_slice(&self.from.0);
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&self.record);
        out
    }

    pub fn decode(p: &[u8]) -> Result<Self, CommError> {
        if p.len() < Self::HEAD {
            return Err(CommError::Short { have: p.len(), want: Self::HEAD });
        }
        let mut from = [0u8; 20];
        from.copy_from_slice(&p[4..24]);
        let mut name = [0u8; 16];
        name.copy_from_slice(&p[0x18..0x28]);
        Ok(Self { request: read_be(p, 0, 4).unwrap(), from: Addr(from), name, record: p[0x28..].to_vec() })
    }
}

/// The status word of an `'a'`, at `+0x18` (§7.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinStatus {
    /// 0x588F08 — in, at this slot.
    Joined(u32),
    /// 0x588E92 — that name is already the host's.
    NameTaken,
    /// 0x588F3D — `popcount(mask) >= limit - 1`.
    Full,
    /// 0x588EE6 — the session's own callback said no.
    Refused,
    /// A status the original never writes. Kept rather than guessed at.
    Other(i32),
}

impl JoinStatus {
    fn code(self) -> i32 {
        match self {
            Self::Joined(_) => 1,
            Self::NameTaken => -1,
            Self::Full => -2,
            Self::Refused => -3,
            Self::Other(c) => c,
        }
    }
}

/// `'a'` — the host's answer to a `'j'`: always **0x20** bytes (0x588F89).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinReply {
    /// echoed from the `'j'`
    pub request: u32,
    pub from: Addr,
    pub status: JoinStatus,
}

impl JoinReply {
    pub const SIZE: usize = 0x20;

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::SIZE);
        write_be(&mut out, self.request, 4);
        out.extend_from_slice(&self.from.0);
        write_be(&mut out, self.status.code() as u32, 4);
        // the slot index is only written on the success path; the original leaves the four bytes
        // alone otherwise, and the handler never reads them then
        write_be(&mut out, if let JoinStatus::Joined(i) = self.status { i } else { 0 }, 4);
        out
    }

    pub fn decode(p: &[u8]) -> Result<Self, CommError> {
        if p.len() < Self::SIZE {
            return Err(CommError::Short { have: p.len(), want: Self::SIZE });
        }
        let mut from = [0u8; 20];
        from.copy_from_slice(&p[4..24]);
        let code = read_be(p, 0x18, 4).unwrap() as i32;
        let slot = read_be(p, 0x1c, 4).unwrap();
        Ok(Self {
            request: read_be(p, 0, 4).unwrap(),
            from: Addr(from),
            status: match code {
                1 => JoinStatus::Joined(slot),
                -1 => JoinStatus::NameTaken,
                -2 => JoinStatus::Full,
                -3 => JoinStatus::Refused,
                c => JoinStatus::Other(c),
            },
        })
    }
}

/// A peer's identity, the string 0x48A700 parses: **`name~version~game`** — `sscanf(s,
/// "%[^~]~%[^~]~%s")` (0x5D0618) into the peer record's `+0x24` (the name), `+0x64` (the version)
/// and `+0x84` (the game); the version is then compared with `"3.5"` (0x5D06FC). The game field is
/// the empty string 0x5E8E50 in every identity the game builds (0x48AA10's callers 0x48E070,
/// 0x48E296, 0x48E60F); only the lobby's `0xB6` room request compares it with a room's `+0x40`
/// (0x49151E…0x491569, 0x4915C6…0x491610) — `network.md` §11.28. **The `'j'` of a real original carries the
/// version there** (`TEST~3.5~3.5`, `REFERENCE-2026-10-02-SRVE.md`), at `rec+0x48` of the player record.
pub mod identity {
    /// What a peer sends: `name~version~game`.
    pub fn encode(name: &str, version: &str, game: &str) -> Vec<u8> {
        // one byte per character, like every string of the game's own protocol
        let b = |s: &str| s.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect::<Vec<u8>>();
        let mut v = b(name);
        v.push(b'~');
        v.extend(b(version));
        v.push(b'~');
        v.extend(b(game));
        v.push(0);
        v
    }

    /// 0x48A700's `sscanf`: `%[^~]` (at least one character), `~`, `%[^~]`, `~`, `%s` (a word).
    /// Fields the string does not reach stay empty, as the record's buffers do.
    pub fn parse(bytes: &[u8]) -> (String, String, String) {
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        let s: String = bytes[..end].iter().map(|&b| char::from(b)).collect();
        let mut it = s.splitn(3, '~');
        let name = it.next().unwrap_or("").to_string();
        let version = it.next().unwrap_or("").to_string();
        let game = it.next().and_then(|g| g.split_whitespace().next()).unwrap_or("").to_string();
        (name, version, game)
    }
}

/// Which slot 0x589440 hands out: the first clear bit of `mask` below `limit - 1`, or `None`.
pub fn free_slot(mask: u32, limit: u32) -> Option<u32> {
    (0..limit.saturating_sub(1)).find(|i| mask & (1 << i) == 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_peers_identity_is_name_tilde_version_tilde_game() {
        let b = identity::encode("RACERB", "3.5", "DRIVER's");
        assert_eq!(b, b"RACERB~3.5~DRIVER's\0".to_vec());
        assert_eq!(identity::parse(&b), ("RACERB".to_string(), "3.5".to_string(), "DRIVER's".to_string()));
        // `%s` stops at white space; a string that runs out leaves the rest empty
        assert_eq!(identity::parse(b"A~3.4~my race\0junk"), ("A".to_string(), "3.4".to_string(), "my".to_string()));
        assert_eq!(identity::parse(b"A\0"), ("A".to_string(), String::new(), String::new()));
        assert_eq!(identity::parse(b""), (String::new(), String::new(), String::new()));
    }

    #[test]
    fn the_session_name_is_name_tilde_version_and_reads_back_the_way_0x48c3d0_scans_it() {
        // 0x48DD67…0x48DD8B: an empty name becomes "Game" (the original host that typed nothing: `Game~3.5`, captured)
        assert_eq!(&desc::name_field("", "3.5")[..9], b"Game~3.5\0");
        let f = desc::name_field("DRIVER's race", "3.5");
        assert_eq!(&f[..17], b"DRIVER's race~3.5");
        assert!(f[17..].iter().all(|&c| c == 0));
        assert_eq!(desc::split_name(&f), ("DRIVER's race".to_string(), "3.5".to_string()));
        // no `~`: the version is not written
        assert_eq!(desc::split_name(b"plain\0\0"), ("plain".to_string(), String::new()));
        // a long name is cut to the descriptor's 0x20 bytes and keeps its NUL
        let long = desc::name_field(&"N".repeat(40), "3.5");
        assert_eq!(long[31], 0);
        assert_eq!(desc::split_name(&long).0.len(), 31);
    }

    use super::*;

    #[test]
    fn the_dispatch_table_covers_exactly_thirteen_tags() {
        let live: Vec<u8> = (FIRST_TAG..=LAST_TAG).filter(|t| is_handled(*t)).collect();
        assert_eq!(live, b"*<=?Kabdgjpqrw");
        // fourteen characters, thirteen handlers: 'K' and 'b' share 0x589152, '?' and 'p' share
        // 0x588D5C.
        assert_eq!(live.len(), 14);
        assert!(!is_handled(b'z'));
        assert!(!is_handled(b'\0'));
    }

    #[test]
    fn the_readers_agree_with_0x56e290() {
        let b = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(read_be(&b, 0, 4), Some(0x1234_5678));
        assert_eq!(read_be(&b, 0, 2), Some(0x1234));
        assert_eq!(read_be(&b, 1, 1), Some(0x34));
        assert_eq!(read_be(&b, 0, 3), Some(0x0012_3456));
        assert_eq!(read_le(&b, 0, 4), Some(0x7856_3412));
        assert_eq!(read_be(&b, 3, 2), None);
    }

    #[test]
    fn the_writer_chains_the_way_0x56e2c0_does() {
        let mut out = Vec::new();
        write_be(&mut out, 0x1234_5678, 4);
        write_be(&mut out, 0xABCD, 2);
        write_be(&mut out, 0xEE, 1);
        assert_eq!(out, vec![0x12, 0x34, 0x56, 0x78, 0xAB, 0xCD, 0xEE]);
    }

    #[test]
    fn a_query_reply_round_trips() {
        let head = Header {
            request: 0xDEAD_BEEF,
            from: Addr::ip([10, 0, 0, 3], crate::PORT),
            index: 2,
        };
        let record = [0x5Au8; RECORD_SIZE];
        let (t, p) = answer_query(head, Some(&record));
        assert_eq!(t, tag::REPLY);
        assert_eq!(p.len(), REPLY_SIZE);
        assert_eq!(p.len(), 0x78);
        assert_eq!(Header::decode(&p).unwrap(), head);
        assert_eq!(&p[Header::SIZE..], &record[..]);
    }

    #[test]
    fn an_empty_slot_answers_with_the_bare_header() {
        let head = Header { request: 1, from: Addr::default(), index: 7 };
        let (t, p) = answer_query(head, None);
        assert_eq!(t, tag::NO_SUCH);
        assert_eq!(p.len(), 0x1C);
    }

    #[test]
    fn the_session_slot_is_minus_one() {
        let head = Header { request: 1, from: Addr::default(), index: Header::SESSION_SLOT };
        let mut out = Vec::new();
        head.encode(&mut out);
        assert_eq!(&out[0x18..0x1c], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(Header::decode(&out).unwrap().index, -1);
    }

    #[test]
    fn a_description_round_trips() {
        let d = Describe {
            request: 0x0102_0304,
            from: Addr::ip([10, 1, 2, 3], crate::PORT),
            conn_dword: 0x1234_5678,
            conn_words: [0x1111, 0x2222],
            conn_raw: [0xAABB_CCDD, 0x0011_2233],
            name: *b"Porsche Unleashed              \0",
            bytes: [1, 2, 3, 4],
            words: [0x3333, 0x4444],
            dwords: [0xAAAA_AAAA, 0xBBBB_BBBB],
            player_limit: 8,
            slot_mask: 0b0000_0101,
        };
        let p = d.encode();
        assert_eq!(p.len(), 0xAC);
        // the two raw fields go out little-endian while their neighbours do not
        assert_eq!(&p[0x18..0x1c], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(&p[0x20..0x24], &[0xDD, 0xCC, 0xBB, 0xAA]);
        assert_eq!(Describe::decode(&p).unwrap(), d);
        assert_eq!(Describe::decode(&p[..0x6f]), Err(CommError::Short { have: 0x6f, want: 0x70 }));
    }

    #[test]
    fn a_join_and_its_answer_round_trip() {
        let j = Join {
            request: 0xCAFE_BABE,
            from: Addr::ip([10, 0, 0, 9], crate::PORT),
            name: *b"racer\0\0\0\0\0\0\0\0\0\0\0",
            record: vec![7; 12],
        };
        let p = j.encode();
        assert_eq!(p.len(), 0x28 + 12);
        assert_eq!(Join::decode(&p).unwrap(), j);

        let a = JoinReply { request: j.request, from: j.from, status: JoinStatus::Joined(3) };
        let q = a.encode();
        assert_eq!(q.len(), 0x20);
        assert_eq!(&q[0x18..0x20], &[0, 0, 0, 1, 0, 0, 0, 3]);
        assert_eq!(JoinReply::decode(&q).unwrap(), a);

        for (st, code) in [(JoinStatus::NameTaken, -1i32), (JoinStatus::Full, -2), (JoinStatus::Refused, -3)] {
            let r = JoinReply { request: 1, from: Addr::default(), status: st };
            let b = r.encode();
            assert_eq!(read_be(&b, 0x18, 4).unwrap() as i32, code);
            assert_eq!(JoinReply::decode(&b).unwrap().status, st);
        }
    }

    #[test]
    fn the_slot_allocator_walks_the_mask_below_the_limit() {
        // 0x589440: bits 0 .. limit-2, first clear one wins
        assert_eq!(free_slot(0b0000_0001, 8), Some(1));
        assert_eq!(free_slot(0b0010_1011, 8), Some(2));
        // a full session: the limit is 8 but only seven bits are ever handed out
        assert_eq!(free_slot(0b0111_1111, 8), None);
        assert_eq!(free_slot(0, 1), None);
    }
}
