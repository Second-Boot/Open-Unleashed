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

/// `0xB1` — **0x492501**: `0x48C870(0xB1, id, OMIT, null)`, the head alone.
pub fn player_out(id: i32) -> Vec<u8> {
    encode(kind::LEAVE, Some(id), None, None)
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
}

impl Room {
    pub fn new(me: i32) -> Self {
        Self { me, ..Self::default() }
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
                self.players.push(Player { id, name, files: Vec::new() });
                Some(Heard::Joined(id))
            }
            // 0x48F46F: every record with that id goes
            kind::LEAVE => {
                let id = head(m)?;
                self.players.retain(|p| p.id != id);
                Some(Heard::Left(id))
            }
            kind::CHAT => ChatLine::parse(m).map(Heard::Chat),
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
    /// is ahead of `net+0x1C` one chunk of the file at the head of `net+0x264` goes and `net+0x1C`
    /// moves on by 1000. The first call only sets the clock, as 0x48E0EE does when the session opens.
    ///
    /// The loop also asks the link whether it is busy (0x48AE30) and skips the send when it is;
    /// this port's link never is, so that test is not carried.
    pub fn tick(&mut self, now_ms: u32) -> Vec<Vec<u8>> {
        let now = now_ms.wrapping_shl(5);
        let Some(clock) = self.clock.as_mut() else {
            self.clock = Some(now);
            return Vec::new();
        };
        let mut out = Vec::new();
        while (now.wrapping_sub(*clock) as i32) > 0 {
            if let Some(f) = self.outgoing.front_mut()
                && self.me >= 0
            {
                let first = f.done == 0;
                let n = (f.total - f.done).min(CHUNK);
                out.push(
                    Chunk {
                        first,
                        from: self.me as u8,
                        kind: f.kind,
                        word: if first { f.total as u32 } else { f.done as u32 },
                        bytes: f.bytes[f.done..f.done + n].to_vec(),
                    }
                    .encode(),
                );
                f.done += n;
                if f.done >= f.total {
                    self.outgoing.pop_front();
                }
            }
            *clock = clock.wrapping_add(CHUNK_COST);
        }
        out
    }

    /// **The host's room, 0x48BB50** — a player comes in. The newcomer is told of everyone already
    /// there, one `0xB0` each (0x48BC2C…0x48BC8F), and then everyone, the newcomer included, is
    /// told of the newcomer (0x48BCC1…0x48BCD8). Returns nothing once the room holds eight.
    ///
    /// Not carried from the same function: the `0xB2` that hands the newcomer its id and the game's
    /// name (this port's id comes with the session's `'a'`), and the `0xC5` sent for each player
    /// whose ready bit is set, since `READY` is not ported.
    pub fn admit(&self, id: i32, name: &[u8]) -> Vec<(To, Vec<u8>)> {
        if self.players.len() >= MAX_PLAYERS {
            return Vec::new();
        }
        let mut out: Vec<(To, Vec<u8>)> =
            self.players.iter().map(|p| (To::Player(id), player_in(p.id, &p.name))).collect();
        out.push((To::All, player_in(id, name)));
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
        assert_eq!(r.hear(&player_out(1)), Some(Heard::Left(1)));
        assert_eq!(r.players.iter().map(|p| p.id).collect::<Vec<_>>(), vec![0]);
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
