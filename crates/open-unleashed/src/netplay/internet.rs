//! The internet lobby link of `NetPlay` — the original's kind-1 session to a lobby address (`RE/notes/lobby.md`).
//!
//! The link is the same client session the LAN join uses, with three differences: the `'j'` record's account field is
//! `c<account>~<password>` instead of `Player` (0x48AA10, SIGNON 0x5109B0), the player's id in a room arrives in a `0xB2` (in the lobby
//! it stays below zero), and `0xB4` / `0xB5` / `0xB8` / `0xB9` feed the games and servers lists (`nfs_net::lobby`). A game's own traffic is
//! the LAN room's, relayed by the lobby server.

use super::*;
use nfs_net::lobby::{self, GameDesc, JoinRefusal, Lists, Server};
use nfs_net::icmp::Echo;
use std::sync::mpsc;

/// **PORT-DEVIATION D55** (`RE/notes/PORT-DEVIATIONS.md`): the lobby's address. The original's is the registry value `Primary Address`
/// (0x5099A0 → 0x4B7240(6)), which the Verok `nfs5.dll` replaces by the constant `porsche-entry.earacing.com` (`lobby.md` §1); the
/// port has this built-in default and the override of [`LOBBY_SETTINGS_FILE`]. `host[:port]`; the port defaults to 9442.
/// **PLACEHOLDER** — the real default is yet to be chosen; `.invalid` never resolves, so until then SIGNON ends like the
/// original's against a dead service (817, then 830 after the connect's 16 s).
pub const LOBBY_DEFAULT_ADDRESS: &str = "lobby.invalid:9442";

/// The port's own text settings file beside the save files (D55): a line `address=host[:port]` overrides [`LOBBY_DEFAULT_ADDRESS`].
pub const LOBBY_SETTINGS_FILE: &str = "lobby.txt";

/// `REGISTERUSER`'s page (0x4A5EA0 `ShellExecute("open", RegUrl)`, the registry value `RegUrl`, default `http://login.earacing.com`,
/// 0x5D1398). Kept as the original's default; the registration page of the port's own service is yet to be chosen (D55).
pub const REGISTER_URL: &str = "http://login.earacing.com";

/// The key of the settings file's address line.
const ADDRESS_KEY: &str = "address";

/// Where [`lobby_address`] reads the override.
pub fn lobby_settings_path() -> std::path::PathBuf {
    nfs_game::save::save_dir().join(LOBBY_SETTINGS_FILE)
}

/// The address a SIGNON connects to: `NFS_LOBBY_ADDRESS`, else the settings file's `address=` line, else [`LOBBY_DEFAULT_ADDRESS`].
pub fn lobby_address() -> String {
    // the order: the environment variable, then the settings file, then the built-in default
    if let Ok(v) = std::env::var(LOBBY_ADDRESS_ENV) && !v.trim().is_empty() {
        return v.trim().to_string();
    }
    address_from(std::fs::read_to_string(lobby_settings_path()).ok().as_deref())
}

/// The environment variable that overrides everything else (D55).
pub const LOBBY_ADDRESS_ENV: &str = "NFS_LOBBY_ADDRESS";

/// [`lobby_address`] from the file's text (`#` starts a comment line; the last `address=` line wins; blanks are ignored).
pub fn address_from(file: Option<&str>) -> String {
    file.and_then(|t| {
        t.lines()
            .filter_map(|l| l.trim().strip_prefix(ADDRESS_KEY)?.trim_start().strip_prefix('='))
            .map(|v| v.trim().to_string())
            .rfind(|v| !v.is_empty())
    })
    .unwrap_or_else(|| LOBBY_DEFAULT_ADDRESS.to_string())
}

/// Resolve a lobby address: `host`, `host:port`, or the original's `a.b.c.d[.port]`. `None` when it does not resolve — the connect then
/// behaves like an unanswered one (the 16 s deadline, `network.md` §11.21 item 7).
pub fn resolve_lobby(addr: &str) -> Option<SocketAddr> {
    if let Some(a) = parse_dotted(addr) {
        return Some(a);
    }
    let with_port = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", nfs_net::PORT) };
    with_port.to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
}

/// `a.b.c.d` or `a.b.c.d.port` with plain numbers (0x48AC90's `%d.%d.%d.%d.%d`).
fn parse_dotted(s: &str) -> Option<SocketAddr> {
    let p: Vec<&str> = s.trim().split('.').collect();
    if !(p.len() == 4 || p.len() == 5) || p.iter().any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n: Vec<u32> = p.iter().map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let port = n.get(4).map_or(nfs_net::PORT, |&v| v as u16);
    Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(n[0] as u8, n[1] as u8, n[2] as u8, n[3] as u8), port)))
}

/// What the server list's row draw needs of a listed server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub name: String,
    pub games: u16,
    pub players: u16,
    pub ping: u32,
}

/// The lobby link's state beyond the LAN session's: the lists, the credentials it signed on with and the ping machinery.
pub struct Internet {
    pub lists: Lists,
    /// the `'j'` record's account field, `c<account>~<password>`
    pub account: Vec<u8>,
    /// where the link goes (`None`: the name did not resolve)
    pub target: Option<SocketAddr>,
    /// `net+0xC6`: a game request (CREATE / JOIN) is pending over the link, which stays up when the answer is a refusal (`lobby.md` §7)
    pub pending_game: bool,
    /// the descriptor last sent for the game we created (what `set_descriptor` compares with)
    pub desc: Option<GameDesc>,
    /// selected rows of the two lists
    pub sel_server: usize,
    pub sel_game: usize,
    /// when the `'a'` came: the link's own round trip, the stand-in of an echo this host cannot send (D55)
    connected_after: Option<u32>,
    started: Instant,
    echoes_tx: mpsc::Sender<(u32, Echo)>,
    echoes_rx: mpsc::Receiver<(u32, Echo)>,
    /// server id -> when its echo was last posted (0x48C6E0 re-pings every `5 × 128` ticks = 5 s)
    posted: HashMap<u32, Instant>,
}

impl Internet {
    fn new(account: Vec<u8>, target: Option<SocketAddr>) -> Self {
        let (echoes_tx, echoes_rx) = mpsc::channel();
        Self {
            lists: Lists::default(),
            account,
            target,
            pending_game: false,
            desc: None,
            sel_server: 0,
            sel_game: 0,
            connected_after: None,
            started: Instant::now(),
            echoes_tx,
            echoes_rx,
            posted: HashMap::new(),
        }
    }
}

/// How often 0x48C6E0 posts a new echo for an entry: `5 × [0x5DEB48]` ticks at 128 per second.
const PING_PERIOD: Duration = Duration::from_secs(5);

impl NetPlay {
    /// **SIGNON's / ENTER's connect** — `0x48E1A0(net, 1, 0, address)`: a client session whose `'j'` carries the account field
    /// `c<account>~<password>` and the identity `<persona>~3.5~<version>` (`lobby.md` §2.1). `target` `None` is an address that did not
    /// resolve: the session never gets an answer and ends in state 4 after [`JOIN_TIMEOUT`].
    pub fn start_lobby(target: Option<SocketAddr>, persona: &str, account: &str, password: &str) -> anyhow::Result<Self> {
        let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
        let mut net = with_probe(Net::client(sock, key(), name16(persona)));
        let field = lobby::account_field(account, password);
        if let Some(t) = target {
            net.join_account(t, &field, &lobby::identity(persona, &game_version()))?;
            tracing::info!(%t, "lobby link: 'j' sent (0x48E1A0 kind 1)");
        } else {
            tracing::info!("lobby link: the address does not resolve (the connect runs out its deadline)");
        }
        let mut n = Self::from_net(net, "signon", persona, target.map(|t| (t, persona.to_string(), Instant::now())));
        n.inet = Some(Box::new(Internet::new(field, target)));
        Ok(n)
    }

    /// Whether this session is an internet lobby link.
    pub fn is_internet(&self) -> bool {
        self.inet.is_some()
    }

    /// The link is gone: comm state 4 (a connect that ran out, a silent host).
    pub fn lost(&self) -> bool {
        self.comm_state() == nfs_net::room::comm_state::LOST
    }

    pub fn internet(&self) -> Option<&Internet> {
        self.inet.as_deref()
    }

    pub fn internet_mut(&mut self) -> Option<&mut Internet> {
        self.inet.as_deref_mut()
    }

    /// Whether this machine's front end acts as the **room's host**: a LAN host, or on an internet link the first player of the room
    /// (0x48CAA0 — the creator is admitted first, and the host migrates by list order, `lobby.md` §7.4).
    pub fn has_authority(&self) -> bool {
        self.is_host() || (self.inet.is_some() && self.room.me >= 0 && self.room.players.first().is_some_and(|p| p.id == self.room.me))
    }

    /// The lobby's `0xB2 / 0xB4 / 0xB5 / 0xB8 / 0xB9` (0x48F9D2, 0x48F799): answers whether it was one.
    pub(super) fn hear_lobby(&mut self, m: &[u8]) -> bool {
        let Some(i) = self.inet.as_mut() else { return false };
        match m.first().copied() {
            Some(0xB2) => {
                // 0x48F799: `net+0xDC = id`, `net+0xE5 = 0`, the games list freed
                let id = nfs_net::msg::Message::parse(m).map_or(-1, |h| h.value);
                i.lists.apply(m);
                self.in_race = 0;
                if id >= 0 {
                    self.id = id.min(255) as u8;
                    self.room.me = id;
                } else {
                    // **APPROXIMATION** (direction: 0x48BDA0's `0xB1 (id)` "to the room" — whether the leaver itself hears it is
                    // unread, `lobby.md` §7.6): the server tells the leaver `0xB1` for every *remaining* player and `0xB2 (−2)`, so the
                    // leaver's own row of the room would stay in the list (`net+0x260`) beside the lobby members' `0xB0`s that follow;
                    // it is dropped here, as `back_to_lobby` does for the player's own `0xBA`.
                    let old = self.room.me;
                    if old >= 0 {
                        self.room.players.retain(|p| p.id != old);
                    }
                    self.id = u8::MAX;
                    self.room.me = -1;
                }
                tracing::info!(target: "netplay", id, "lobby: 0xB2 (your id in the room)");
                self.dirty = true;
                true
            }
            Some(0xB4 | 0xB5 | 0xB8 | 0xB9) => {
                i.lists.apply(m);
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    /// The server rows of the list: only entries whose ping is known (0x4B9700).
    pub fn server_rows(&self) -> Vec<(Server, ServerRow)> {
        let Some(i) = self.inet.as_ref() else { return Vec::new() };
        i.lists
            .visible_servers()
            .map(|s| (s.clone(), ServerRow { name: s.display_name().to_string(), games: s.games, players: s.players, ping: s.ping.unwrap_or(0).min(9999) }))
            .collect()
    }

    /// The games list as the FE's rows (`Comms.GamesList`, 0x4BA030): `players` is `entry+0x48`.
    pub fn game_rows(&self) -> Vec<SessionRow> {
        let Some(i) = self.inet.as_ref() else { return Vec::new() };
        i.lists
            .games
            .iter()
            .map(|g| SessionRow {
                name: g.desc.name.clone(),
                players: g.players,
                limit: g.desc.limit,
                addr: format!("{}", g.id),
                ping: None,
                version: g.desc.version.clone(),
                mode: g.desc.positional,
                track_id: g.desc.track,
            })
            .collect()
    }

    /// **CREATE** — `0xB4` with id 0 and the descriptor, `net+0xC6 = 1` (0x48DEFF, 0x48DF5A). Needs the link up and no id (a lobby member).
    pub fn create_game(&mut self, name: &str, positional: u8) -> bool {
        if !self.established() || self.room.me >= 0 {
            return false;
        }
        let desc = GameDesc { name: name.to_string(), version: game_version(), positional, laps: 0, direction: 0, track: 0, limit: nfs_net::room::MAX_PLAYERS as u32 };
        let m = desc.message(0, 0);
        let Some(i) = self.inet.as_mut() else { return false };
        i.pending_game = true;
        i.desc = Some(desc);
        if let Err(e) = self.net.send_game(&m) {
            tracing::warn!(error = %e, "CREATE not sent");
        }
        tracing::info!(target: "netplay", name, "lobby: CREATE (0xB4 id 0)");
        true
    }

    /// The pre-checks and the request of **JOIN** (0x50B6A0): the selected game's version against ours (822), its count against 8 (286), then
    /// `0xB6 id` and `net+0xC6 = 1` (0x48E3A7). `Ok(false)` with no row.
    pub fn join_game(&mut self, row: usize) -> Result<bool, JoinRefusal> {
        let Some(i) = self.inet.as_mut() else { return Ok(false) };
        let Some(g) = i.lists.games.get(row) else { return Ok(false) };
        g.joinable(&game_version())?;
        let id = g.id;
        i.pending_game = true;
        if let Err(e) = self.net.send_game(&lobby::join_request(id)) {
            tracing::warn!(error = %e, "JOIN not sent");
        }
        tracing::info!(target: "netplay", id, "lobby: JOIN (0xB6)");
        Ok(true)
    }

    /// A descriptor change of the game we created (7.2): `0xB4` id 0 again, now with an id in the room. Sent only when it differs from the
    /// last one sent.
    pub(super) fn update_description(&mut self, positional: u8, laps: u8, direction: u16, track: u16) {
        let Some(i) = self.inet.as_mut() else { return };
        let Some(old) = i.desc.clone() else { return };
        let new = GameDesc { positional, laps, direction, track, ..old.clone() };
        if new == old {
            return;
        }
        i.desc = Some(new.clone());
        if self.room.me >= 0 && let Err(e) = self.net.send_game(&new.message(0, 0)) {
            tracing::debug!(error = %e, "description not sent");
        }
    }

    /// **Back to the lobby** — `0xBA` (0x48D97B / 0x48E778) when a game is pending or joined; the link stays. The id goes back below zero
    /// at once (the server's `0xB2 (−2, "")` says the same).
    pub fn back_to_lobby(&mut self) {
        let Some(i) = self.inet.as_mut() else { return };
        if !i.pending_game && self.room.me < 0 {
            return;
        }
        i.pending_game = false;
        i.desc = None;
        if let Err(e) = self.net.send_game(&lobby::back()) {
            tracing::debug!(error = %e, "0xBA not sent");
        }
        // a player already out of the room (ejected, or the room shut under it) has the lobby's own list by now: only a player
        // still in the room drops the room's rows (the server's `0xBA` answer then brings the lobby's)
        if self.room.me >= 0 {
            self.room.players.clear();
        }
        self.id = u8::MAX;
        self.room.me = -1;
        self.in_race = 0;
        self.dirty = true;
    }

    /// 0x509550's states 9 / 10 / 11 with `net+0xC6` set: the notice shows and the state goes back to `0xE` instead of a teardown.
    pub fn restore_state(&mut self) {
        self.state_in = Some(nfs_net::room::comm_state::CONNECTED);
    }

    /// Whether a game request is pending over the link (`net+0xC6`).
    pub fn game_pending(&self) -> bool {
        self.inet.as_ref().is_some_and(|i| i.pending_game)
    }

    /// The ping machinery of the server list, run from [`NetPlay::poll`]: 0x48C6E0 posts an ICMP echo for an entry whose text starts with a
    /// literal IPv4 once `5 × 128` ticks have passed since the last (an entry that answers shows its round trip, one that does not
    /// stays hidden: `lobby.md` §4). **PORT-DEVIATION D55:** the echo is the unprivileged one of `nfs_net::icmp`; where the host cannot
    /// send it the link's own round trip stands in (APPROXIMATION: every row then shows the entry link's time).
    pub(super) fn poll_internet(&mut self) {
        let Some(i) = self.inet.as_mut() else { return };
        if i.connected_after.is_none() && self.join == JoinState::Joined {
            i.connected_after = Some((i.started.elapsed().as_millis() as u32).max(1));
        }
        while let Ok((id, echo)) = i.echoes_rx.try_recv() {
            let fallback = i.connected_after.unwrap_or(1);
            if let Some(s) = i.lists.servers.iter_mut().find(|s| s.id == id) {
                s.ping = match echo {
                    Echo::Reply(ms) => Some(ms),
                    Echo::Timeout => None,
                    Echo::Unavailable => Some(fallback),
                };
                self.dirty = true;
            }
        }
        let due: Vec<(u32, Ipv4Addr)> = i
            .lists
            .servers
            .iter()
            .filter(|s| i.posted.get(&s.id).is_none_or(|t| t.elapsed() >= PING_PERIOD))
            .filter_map(|s| s.echo_target().map(|ip| (s.id, ip)))
            .collect();
        for (id, ip) in due {
            i.posted.insert(id, Instant::now());
            let tx = i.echoes_tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send((id, nfs_net::icmp::echo(ip)));
            });
        }
    }

    /// The target the ENTER on the selected server row connects to.
    pub fn selected_server_target(&self) -> Option<SocketAddr> {
        let i = self.inet.as_ref()?;
        let rows: Vec<&Server> = i.lists.visible_servers().collect();
        let s = rows.get(i.sel_server.min(rows.len().saturating_sub(1)))?;
        s.target().map(|(ip, port)| SocketAddr::V4(SocketAddrV4::new(ip, port)))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_file_overrides_the_default_address() {
        assert_eq!(address_from(None), LOBBY_DEFAULT_ADDRESS);
        assert_eq!(address_from(Some("# comment\naddress = lobby.example.org:9500\n")), "lobby.example.org:9500");
        assert_eq!(address_from(Some("address=a\naddress=b\n")), "b");
        assert_eq!(address_from(Some("address=\nother=1\n")), LOBBY_DEFAULT_ADDRESS);
        assert_eq!(address_from(Some("addressX=1\n")), LOBBY_DEFAULT_ADDRESS);
    }

    #[test]
    fn addresses_resolve_the_way_0x48ac90_reads_them() {
        assert_eq!(resolve_lobby("127.0.0.1"), Some("127.0.0.1:9442".parse().unwrap()));
        assert_eq!(resolve_lobby("127.0.0.1.9500"), Some("127.0.0.1:9500".parse().unwrap()));
        assert_eq!(resolve_lobby("127.0.0.1:9501"), Some("127.0.0.1:9501".parse().unwrap()));
        assert_eq!(resolve_lobby("localhost:9502").map(|a| a.port()), Some(9502));
        assert_eq!(resolve_lobby("lobby.invalid:9442"), None);
    }
}
