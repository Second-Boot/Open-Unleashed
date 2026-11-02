//! The internet lobby's messages and lists — the client's and the server's shared protocol side (`RE/notes/lobby.md`).
//!
//! The lobby link is an ordinary `NfsNet` session (kind 1) to a host that speaks the room protocol of [`crate::room`]; only a few
//! messages carry lobby meaning: `0xB4` / `0xB5` (a game appears / goes), `0xB8` / `0xB9` (a server appears / goes), `0xB6` (join a
//! game), `0xBA` (back to the lobby) and `0xC6` as the login result. This module builds and reads them and keeps the two lists a
//! client shows (`net+0x258` servers, `net+0x25C` games, §4 and §6.2). Pure protocol: no socket, no game types.

use crate::room::{self, MAX_PLAYERS, OMIT};
use std::net::Ipv4Addr;

/// The message types the lobby adds to the room protocol.
pub mod kind {
    /// `0xB5` — a game is gone (0x48BB10). Same 0x50-byte shape as `0xB4`; only the type and the id are read.
    pub const GAME_GONE: u8 = 0xB5;
    /// `0xB6` — a lobby member asks to join a game (`B6 id24`, 0x48E3A7).
    pub const JOIN_GAME: u8 = 0xB6;
    /// `0xB8` — a server entry (`B8 id24 count:u32be text\0`, 0x48C870 / 0x490BB0).
    pub const SERVER: u8 = 0xB8;
    /// `0xB9` — a server entry is gone (shares the code of `0xB5`).
    pub const SERVER_GONE: u8 = 0xB9;
    /// `0xC7` — **PORT-ONLY** (D56): the port's build identity, sent once after `0xC6 0xE` on an internet lobby link. Outside the
    /// original's tables on both sides (`lobby.md` §14.1): the reference server's switch (0x49127B) takes `type - 0xB1 > 0x14` to its
    /// default 0x491C3C, the client's (0x48F32F) `type - 0xB0 > 0x16` to 0x48FBD1, and the CONMAN passes anything up to `0xC8` on as a game
    /// message (0x55CEA0 cut) - so an original ignores it.
    pub const CLIENT_BUILD: u8 = 0xC7;
    /// `0xBA` — back to the lobby (0x48D97B / 0x48E778: `0x48C870(0xBA, OMIT, OMIT, 0)`, one byte).
    pub const BACK: u8 = 0xBA;
}

/// The literal the dormant server code compares the identity's second field with (0x48A700, 0x5D06FC), and the client puts there
/// (0x48AA10).
pub const PROTOCOL_VERSION: &str = "3.5";

/// The account field's limits (0x50B810): the account box 32 characters, the password box 7. The field itself is 52 bytes at
/// `rec+0x14` (marker `c`, account, `~`, password and the NUL).
pub const ACCOUNT_MAX: usize = 32;
pub const PASSWORD_MAX: usize = 7;
pub const ACCOUNT_FIELD: usize = 52;

/// The account field of a signed-on client, `c<account>~<password>` (0x48AA10: `param_5` then `"~" + param_8` when the password is
/// not empty; SIGNON 0x5109B0 puts the marker `c` before the account's text). One byte per character, cut to the field.
pub fn account_field(account: &str, password: &str) -> Vec<u8> {
    let b = |s: &str, max: usize| s.chars().take(max).map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect::<Vec<u8>>();
    let mut v = vec![b'c'];
    v.extend(b(account, ACCOUNT_MAX));
    let pw = b(password, PASSWORD_MAX);
    if !pw.is_empty() {
        v.push(b'~');
        v.extend(pw);
    }
    v.truncate(ACCOUNT_FIELD - 1);
    v
}

/// What a server reads back out of an account field: `(marker, account, password)`. `c` is a client, `s` a server registration
/// (§10). A field without a `~` has no password; a field that does not start with a marker has an empty one.
pub fn parse_account_field(field: &[u8]) -> (u8, String, String) {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    let f = &field[..end];
    let Some((&marker, rest)) = f.split_first() else { return (0, String::new(), String::new()) };
    let s: String = rest.iter().map(|&b| char::from(b)).collect();
    match s.split_once('~') {
        Some((a, p)) => (marker, a.to_string(), p.to_string()),
        None => (marker, s, String::new()),
    }
}

/// A signed-on client's identity, `persona~3.5~<version>` (0x48AA10; the third field is `net+0x2C`, the install's version).
pub fn identity(persona: &str, version: &str) -> Vec<u8> {
    crate::comm::identity::encode(persona, PROTOCOL_VERSION, version)
}

/// The client's `0xFF` CONMAN hello (0x55CED0: `0xFF 00 <string>\0 <identity>\0`) for a signed-on link: the first string is the account
/// field, as `"Player"` is for a LAN joiner. **INFERRED** (`lobby.md` §2.2, G3) — a server reads the credentials from the `'j'` record
/// and uses the hello only as the fallback.
pub fn hello(account_field: &[u8], identity: &[u8]) -> Vec<u8> {
    let mut m = vec![0xFF, 0x00];
    m.extend(account_field.iter().take_while(|&&c| c != 0));
    m.push(0);
    m.extend(identity.iter().take_while(|&&c| c != 0));
    m.push(0);
    m
}

/// `0xB8` — a server entry for a newcomer (0x490BB0 reads it, builder 0x48C870): `games << 16 | players`.
pub fn server_row(id: u32, games: u16, players: u16, text: &str) -> Vec<u8> {
    room::encode(kind::SERVER, Some((id & 0xFF_FFFF) as i32), Some(u32::from(games) << 16 | u32::from(players)), Some(text.as_bytes()))
}

/// `0xB9 id` — the entry is gone.
pub fn server_gone(id: u32) -> Vec<u8> {
    room::encode(kind::SERVER_GONE, Some((id & 0xFF_FFFF) as i32), None, None)
}

/// `0xB6 id` — JOIN (0x48E3A7).
pub fn join_request(game: u32) -> Vec<u8> {
    room::encode(kind::JOIN_GAME, Some((game & 0xFF_FFFF) as i32), None, None)
}

/// `0xBA` — back to the lobby.
pub fn back() -> Vec<u8> {
    room::encode(kind::BACK, None, None, None)
}

/// `0xB5` — a game is gone: the 0x50-byte shape of `0xB4` with the type changed (only type and id are read, G9).
pub fn game_gone(id: u32) -> Vec<u8> {
    let mut m = room::description((id & 0xFF_FFFF) as i32, &[0; 32], [0; 4], [0; 2], 0);
    m[0] = kind::GAME_GONE;
    m
}

/// A game's descriptor as `0xB4` carries it (`lobby.md` §6.2): the name field `Name~version`, `Comms.Positional`, the laps, the
/// direction string id, the track name id and the player limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDesc {
    pub name: String,
    pub version: String,
    pub positional: u8,
    pub laps: u8,
    pub direction: u16,
    pub track: u16,
    pub limit: u32,
}

impl GameDesc {
    /// The `0xB4` for game `id` with `players` in it (`id` 0 is CREATE's request, 0x48DEFF). The name field is
    /// [`crate::comm::desc::name_field`]'s (an empty name becomes `Game`).
    pub fn message(&self, id: u32, players: u32) -> Vec<u8> {
        let name = crate::comm::desc::name_field(&self.name, &self.version);
        room::description((id & 0xFF_FFFF) as i32, &name, [0, self.positional, self.laps, 0], [self.direction, self.track], players)
    }
}

/// One game of the games list (`net+0x25C`, entry 0xB4 bytes; fields as 0x48C3D0 / 0x48C490 and 0x4BA030 read them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// `entry+0xB0`, 24 bits unsigned
    pub id: u32,
    pub desc: GameDesc,
    /// `entry+0x48`, the number of players (stored little-endian at message `+0x4C`)
    pub players: u32,
}

impl Game {
    /// Read a `0xB4` (0x50 bytes): the id, the descriptor at `+0x18` and the count at `+0x4C`. `sscanf(name, "%[^~]~%s")` splits the name
    /// and the version (0x48C3D0).
    pub fn parse(m: &[u8]) -> Option<Self> {
        if m.first() != Some(&crate::msg::kind::DESCRIPTION) || m.len() < 0x50 {
            return None;
        }
        let id = u32::from(m[1]) << 16 | u32::from(m[2]) << 8 | u32::from(m[3]);
        let (name, version) = crate::comm::desc::split_name(&m[0x18..0x38]);
        let le16 = |at: usize| u16::from_le_bytes([m[at], m[at + 1]]);
        let le32 = |at: usize| u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]]);
        Some(Self {
            id,
            desc: GameDesc { name, version, positional: m[0x38 + 1], laps: m[0x38 + 2], direction: le16(0x3C), track: le16(0x3E), limit: le32(0x48) },
            players: le32(0x4C),
        })
    }

    /// The JOIN pre-checks of the client (0x50B6A0): the version against ours (0x4B9E80, notice 822) and the player count against 8
    /// (0x4B9DA0, notice 286).
    pub fn joinable(&self, own_version: &str) -> Result<(), JoinRefusal> {
        if self.desc.version != own_version {
            return Err(JoinRefusal::WrongVersion);
        }
        if self.players as usize >= MAX_PLAYERS {
            return Err(JoinRefusal::Full);
        }
        Ok(())
    }
}

/// Why the client does not even send a `0xB6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRefusal {
    WrongVersion,
    Full,
}

/// One server of the server list (`net+0x258`, entry from 0x48C360).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// `entry+0xB0`, 24 bits unsigned
    pub id: u32,
    /// `entry+0x3C >> 16`
    pub games: u16,
    /// `entry+0x48`
    pub players: u16,
    /// `entry+0x4C`: `a.b.c.d.port.Display Name` (63 characters at most) — never changed after the entry is made
    pub text: String,
    /// `entry+0xB4`: the ICMP round trip in ms; `None` is the initial −1 — the row stays hidden (0x4B9700 / 0x4B9640 / 0x4B9780)
    pub ping: Option<u32>,
}

impl Server {
    /// The row's name, 0x4B9780: the text after the fifth `.` (`strchr` ×5), else the whole string.
    pub fn display_name(&self) -> &str {
        let mut rest = self.text.as_str();
        for _ in 0..5 {
            match rest.split_once('.') {
                Some((_, r)) => rest = r,
                None => return &self.text,
            }
        }
        rest
    }

    /// Where ENTER connects: the `%d.%d.%d.%d.%d` reading of 0x48AC90 over the entry's text — four numbers make the address, a fifth is
    /// the port (the default is [`crate::PORT`]). `None` when the text does not start with a literal IPv4 (such a server is never listed,
    /// §4).
    pub fn target(&self) -> Option<(Ipv4Addr, u16)> {
        let mut it = self.text.splitn(6, '.');
        let mut n = [0i64; 5];
        let mut got = 0;
        for slot in n.iter_mut() {
            let Some(p) = it.next() else { break };
            let digits: String = p.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                break;
            }
            *slot = digits.parse().ok()?;
            got += 1;
            // a name after the number ends the reading (`%d` stops at the first non-digit; the next literal `.` would not match)
            if digits.len() != p.trim_start().len() {
                break;
            }
        }
        if got < 4 {
            return None;
        }
        let port = if got >= 5 { n[4] as u16 } else { crate::PORT };
        Some((Ipv4Addr::new(n[0] as u8, n[1] as u8, n[2] as u8, n[3] as u8), port))
    }

    /// 0x48C6E0: the ping accessor `sscanf`s `%d.%d.%d.%d` and posts the echo only with exactly that many numbers; the address to echo.
    pub fn echo_target(&self) -> Option<Ipv4Addr> {
        self.target().map(|t| t.0)
    }
}

/// What a message did to the lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changed {
    Nothing,
    Servers,
    Games,
}

/// The two lists a lobby client shows, and what feeds them (`lobby.md` §4, §6.2).
#[derive(Debug, Clone, Default)]
pub struct Lists {
    pub servers: Vec<Server>,
    pub games: Vec<Game>,
}

impl Lists {
    /// One incoming message: `0xB8` adds a server or updates the two counts of the one with that id (the text never changes),
    /// `0xB9` removes it (0x490BB0); `0xB4` adds or updates a game, `0xB5` removes it; every `0xB2` empties the games list (0x48F799).
    pub fn apply(&mut self, m: &[u8]) -> Changed {
        let id24 = |m: &[u8]| m.get(1..4).map(|b| u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]));
        match m.first().copied() {
            Some(kind::SERVER) => {
                let (Some(id), Some(w)) = (id24(m), m.get(4..8)) else { return Changed::Nothing };
                let count = u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
                let (games, players) = ((count >> 16) as u16, count as u16);
                if let Some(s) = self.servers.iter_mut().find(|s| s.id == id) {
                    s.games = games;
                    s.players = players;
                } else {
                    let t = &m[8..];
                    let t = &t[..t.iter().position(|&c| c == 0).unwrap_or(t.len())];
                    let text: String = t.iter().take(63).map(|&b| char::from(b)).collect();
                    self.servers.push(Server { id, games, players, text, ping: None });
                }
                Changed::Servers
            }
            Some(kind::SERVER_GONE) => {
                let Some(id) = id24(m) else { return Changed::Nothing };
                self.servers.retain(|s| s.id != id);
                Changed::Servers
            }
            Some(b) if b == crate::msg::kind::DESCRIPTION => {
                let Some(g) = Game::parse(m) else { return Changed::Nothing };
                match self.games.iter_mut().find(|x| x.id == g.id) {
                    Some(x) => *x = g,
                    None => self.games.push(g),
                }
                Changed::Games
            }
            Some(kind::GAME_GONE) => {
                let Some(id) = id24(m) else { return Changed::Nothing };
                self.games.retain(|g| g.id != id);
                Changed::Games
            }
            Some(0xB2) => {
                let had = !self.games.is_empty();
                self.games.clear();
                if had { Changed::Games } else { Changed::Nothing }
            }
            _ => Changed::Nothing,
        }
    }

    /// The servers a row is drawn for: those whose ping is known (≥ 0), in order (0x4B9700 / 0x4B9640).
    pub fn visible_servers(&self) -> impl Iterator<Item = &Server> {
        self.servers.iter().filter(|s| s.ping.is_some())
    }
}

/// The revision of the `0xC7` body (the head's 24-bit value).
pub const CLIENT_BUILD_REVISION: i32 = 1;
/// The longest build text the port sends (the encoder cuts at [`room::MESSAGE_MAX`] with its 4 head bytes and the NUL).
pub const CLIENT_BUILD_MAX: usize = room::MESSAGE_MAX - 5;

/// `0xC7 revision24 text\0` - **PORT-ONLY** (D56): who the client is. `text` is `<name> <version> <commit> sim=<hash>`: the
/// `sim=` word is the physics version ([`ClientBuild::sim`]), the rest is for humans.
pub fn client_build(text: &str) -> Vec<u8> {
    let t: Vec<u8> = text.bytes().filter(|&b| (0x20..0x7F).contains(&b)).take(CLIENT_BUILD_MAX).collect();
    room::encode(kind::CLIENT_BUILD, Some(CLIENT_BUILD_REVISION), None, Some(&t))
}

/// A `0xC7` read by a server: `Err` for a malformed one (a hygiene flag, never a disconnect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBuild {
    pub revision: i32,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientBuildError {
    NotOne,
    Short,
    BadRevision,
    Empty,
    NotPrintable,
    TooLong,
    NoTerminator,
}

pub fn parse_client_build(m: &[u8]) -> Result<ClientBuild, ClientBuildError> {
    if m.first() != Some(&kind::CLIENT_BUILD) {
        return Err(ClientBuildError::NotOne);
    }
    if m.len() < 5 {
        return Err(ClientBuildError::Short);
    }
    let revision = crate::msg::sign_extend_24(u32::from(m[1]) << 16 | u32::from(m[2]) << 8 | u32::from(m[3]));
    if revision != CLIENT_BUILD_REVISION {
        return Err(ClientBuildError::BadRevision);
    }
    let body = &m[4..];
    let Some(end) = body.iter().position(|&c| c == 0) else { return Err(ClientBuildError::NoTerminator) };
    if end == 0 {
        return Err(ClientBuildError::Empty);
    }
    if end > CLIENT_BUILD_MAX {
        return Err(ClientBuildError::TooLong);
    }
    if !body[..end].iter().all(|b| (0x20..0x7F).contains(b)) {
        return Err(ClientBuildError::NotPrintable);
    }
    Ok(ClientBuild { revision, text: body[..end].iter().map(|&b| char::from(b)).collect() })
}

impl ClientBuild {
    /// The physics version: the `sim=<hash>` word of the text (`None` when absent or empty).
    pub fn sim(&self) -> Option<&str> {
        self.text.split(' ').find_map(|w| w.strip_prefix("sim=")).filter(|f| !f.is_empty())
    }
}

/// `OMIT` re-exported for callers that build the lobby's own shapes.
pub const OMITTED: u32 = OMIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_field_is_c_account_tilde_password() {
        assert_eq!(account_field("racer", "pw"), b"cracer~pw");
        assert_eq!(account_field("racer", ""), b"cracer");
        // the boxes' limits: 32 and 7 characters
        let long = account_field(&"a".repeat(40), "12345678901");
        assert_eq!(long.len(), 1 + 32 + 1 + 7);
        assert_eq!(parse_account_field(b"cracer~pw\0junk"), (b'c', "racer".into(), "pw".into()));
        assert_eq!(parse_account_field(b"cracer"), (b'c', "racer".into(), String::new()));
        assert_eq!(parse_account_field(b""), (0, String::new(), String::new()));
        assert_eq!(parse_account_field(b"sname"), (b's', "name".into(), String::new()));
    }

    #[test]
    fn the_identity_and_the_hello_carry_the_persona_and_the_account() {
        assert_eq!(identity("TEST", "3.5"), b"TEST~3.5~3.5\0");
        let h = hello(b"cann~pw", b"TEST~3.5~3.5\0");
        assert_eq!(h, b"\xFF\x00cann~pw\0TEST~3.5~3.5\0");
    }

    #[test]
    fn a_server_row_round_trips_and_shows_the_name_after_the_fifth_dot() {
        let m = server_row(7, 2, 5, "127.0.0.1.9442.Stub Lobby");
        assert_eq!(m[..4], [0xB8, 0, 0, 7]);
        assert_eq!(m[4..8], [0, 2, 0, 5]);
        let mut l = Lists::default();
        assert_eq!(l.apply(&m), Changed::Servers);
        let s = &l.servers[0];
        assert_eq!((s.id, s.games, s.players), (7, 2, 5));
        assert_eq!(s.display_name(), "Stub Lobby");
        assert_eq!(s.target(), Some((Ipv4Addr::new(127, 0, 0, 1), 9442)));
        assert_eq!(s.echo_target(), Some(Ipv4Addr::new(127, 0, 0, 1)));
        // the counts change, the text never does
        l.apply(&server_row(7, 3, 6, "other text"));
        assert_eq!((l.servers[0].games, l.servers[0].players, l.servers[0].text.as_str()), (3, 6, "127.0.0.1.9442.Stub Lobby"));
        // no ping yet: hidden
        assert_eq!(l.visible_servers().count(), 0);
        l.servers[0].ping = Some(1);
        assert_eq!(l.visible_servers().count(), 1);
        l.apply(&server_gone(7));
        assert!(l.servers.is_empty());
    }

    #[test]
    fn a_name_without_dots_and_a_host_name_are_not_addresses() {
        let s = |t: &str| Server { id: 1, games: 0, players: 0, text: t.into(), ping: None };
        assert_eq!(s("Plain name").display_name(), "Plain name");
        assert_eq!(s("Plain name").target(), None);
        assert_eq!(s("lobby.example.com.9442.X").target(), None);
        assert_eq!(s("10.0.0.2.Name").target(), Some((Ipv4Addr::new(10, 0, 0, 2), crate::PORT)));
    }

    #[test]
    fn a_game_record_round_trips_and_the_joins_are_checked_like_the_client() {
        let d = GameDesc { name: "Ann's race".into(), version: "3.5".into(), positional: 1, laps: 3, direction: 411, track: 7, limit: 8 };
        let m = d.message(0, 0);
        assert_eq!(m.len(), 0x50);
        let mut l = Lists::default();
        let full = d.message(0x123456, 8);
        assert_eq!(l.apply(&full), Changed::Games);
        let g = &l.games[0];
        assert_eq!(g.id, 0x123456);
        assert_eq!(g.desc.name, "Ann's race");
        assert_eq!(g.desc.version, "3.5");
        assert_eq!((g.desc.positional, g.desc.laps, g.desc.direction, g.desc.track), (1, 3, 411, 7));
        assert_eq!(g.players, 8);
        assert_eq!(g.joinable("3.5"), Err(JoinRefusal::Full));
        assert_eq!(g.joinable("3.4"), Err(JoinRefusal::WrongVersion));
        l.apply(&d.message(0x123456, 2));
        assert_eq!(l.games.len(), 1);
        assert_eq!(l.games[0].joinable("3.5"), Ok(()));
        // a 0xB2 empties the games list; a 0xB5 removes one
        l.apply(&d.message(9, 1));
        assert_eq!(l.apply(&game_gone(9)), Changed::Games);
        assert_eq!(l.games.len(), 1);
        assert_eq!(l.apply(&room::your_id(1, b"Game")), Changed::Games);
        assert!(l.games.is_empty());
    }

    #[test]
    fn the_client_build_message_round_trips_and_malformed_ones_are_named() {
        let m = client_build("OpenUnleashed 0.1.5 3f2a9c1d4e5b-dirty.00ab12cd sim=0123456789abcdef");
        assert_eq!(m[..4], [0xC7, 0, 0, 1]);
        assert_eq!(*m.last().unwrap(), 0);
        let b = parse_client_build(&m).unwrap();
        assert_eq!(b.text, "OpenUnleashed 0.1.5 3f2a9c1d4e5b-dirty.00ab12cd sim=0123456789abcdef");
        assert_eq!(b.sim(), Some("0123456789abcdef"));
        assert_eq!(ClientBuild { revision: 1, text: "x sim=".into() }.sim(), None);
        // an overlong text is cut by the sender to the message size
        let long = client_build(&"x".repeat(500));
        assert!(long.len() <= room::MESSAGE_MAX);
        assert!(parse_client_build(&long).is_ok());
        assert_eq!(parse_client_build(&[0xC7, 0, 0]), Err(ClientBuildError::Short));
        assert_eq!(parse_client_build(&[0xC7, 0, 0, 9, b'a', 0]), Err(ClientBuildError::BadRevision));
        assert_eq!(parse_client_build(&[0xC7, 0, 0, 1, 0]), Err(ClientBuildError::Empty));
        assert_eq!(parse_client_build(&[0xC7, 0, 0, 1, b'a']), Err(ClientBuildError::NoTerminator));
        assert_eq!(parse_client_build(&[0xC7, 0, 0, 1, 1, 0]), Err(ClientBuildError::NotPrintable));
        let mut over = vec![0xC7, 0, 0, 1];
        over.extend(std::iter::repeat_n(b'a', CLIENT_BUILD_MAX + 3));
        over.push(0);
        assert_eq!(parse_client_build(&over), Err(ClientBuildError::TooLong));
        assert_eq!(parse_client_build(&back()), Err(ClientBuildError::NotOne));
    }

    #[test]
    fn the_small_requests_have_the_originals_shapes() {
        assert_eq!(join_request(0x0102_03), vec![0xB6, 1, 2, 3]);
        assert_eq!(back(), vec![0xBA]);
        assert_eq!(game_gone(5).len(), 0x50);
        assert_eq!(game_gone(5)[..4], [0xB5, 0, 0, 5]);
    }
}
