//! The front end's side of network play: the browser and the address lists.

use super::*;


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
    pub(super) seen: HashMap<SocketAddr, Instant>,
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
    /// whether the list is written through to [`comms_ip_list`] (off in the tests)
    pub(super) persist: bool,
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
pub(super) const SESSION_TTL: Duration = Duration::from_secs(4);

/// `Comms.IPList`, the accessor's contents: the address strings (newest first) and the selected
/// row. It lives in the profile's option block (`FECharVectAccessor`, ctor 0x50B8BE, Save 0x4B8D70 /
/// Load 0x4B8E00 write `u32 n`, `n` strings, `u32 selection`, `savegame.md` §6.1) — the process
/// holds it the way the original's accessor map `[0x65B3A8]` does, [`comms_ip_list`] /
/// [`set_comms_ip_list`] are the profile's save and load, and [`Lobby`] is the peer-to-peer screen's
/// view of it.
static COMMS_IPLIST: std::sync::Mutex<(Vec<String>, u32)> = std::sync::Mutex::new((Vec::new(), 0));

/// The accessor as the profile save writes it.
pub fn comms_ip_list() -> (Vec<String>, u32) {
    COMMS_IPLIST.lock().map(|g| g.clone()).unwrap_or_default()
}

/// The accessor as the profile load (0x4B8E00) sets it.
pub fn set_comms_ip_list(items: Vec<String>, selection: u32) {
    if let Ok(mut g) = COMMS_IPLIST.lock() {
        *g = (items, selection);
    }
}

/// `Comms.GameName`'s capacity - `0x4BC8B0("Comms.GameName", 0, 0xF, 0)` (0x50B953…0x50B961).
pub const GAME_NAME_MAX: usize = 15;

/// The name dialog's field capacity: 0x509E40 builds it with `0x5140B0(0x259, 0, 0x19, 0, 0)`.
pub const GAME_NAME_DIALOG_MAX: usize = 0x19;

/// Where `Comms.GameName`'s text is kept between runs. **Glue**: the original keeps it in the
/// profile's accessor block (`savegame.md` §6); the port keeps a text file like the IP list's.
pub fn game_name_path() -> std::path::PathBuf {
    nfs_game::save::save_dir().join("comms_gamename.txt")
}

impl Lobby {
    /// Open a browsing socket. Port 0, so a second copy on the same machine can browse too.
    pub fn open(who: &str) -> anyhow::Result<Self> {
        let sock =
            CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
        Ok(Self {
            net: with_probe(Net::client(sock, key(), name16(who))),
            found: Vec::new(),
            seen: HashMap::new(),
            ping: HashMap::new(),
            ip: String::new(),
            history: comms_ip_list().0,
            ip_sel: comms_ip_list().1 as usize,
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
        self.keep();
    }

    /// Writes the list and its selection through to the accessor (the profile save reads it there);
    /// off in the tests.
    fn keep(&self) {
        if self.persist {
            set_comms_ip_list(self.history.clone(), self.ip_sel as u32);
        }
    }

    /// `Comms.IPList`'s selection moved to `row` (a drop-down pick or a step): the box mirrors the
    /// list's current text while it is not focused (0x4C2310: when the list's text differs from
    /// the string's, the string is cleared, set to the list's and updated). Answers that text.
    pub fn pick_ip(&mut self, row: usize) -> Option<String> {
        let t = self.history.get(row)?.clone();
        self.ip_sel = row;
        self.ip = t.clone();
        self.keep();
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
    pub(super) fn note_found(&mut self, from: SocketAddr, describe: Box<nfs_net::comm::Describe>, now: Instant) {
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
    /// answers `None`. A bare address takes the well-known port 9442 ([`nfs_net::PORT`]); see
    /// [`parse_typed_address`] for what the string may say.
    pub fn connect_target(&self) -> Option<SocketAddr> {
        let typed = self.history.get(self.ip_sel)?.trim();
        // **TEST GLUE** (not in the original): `host:port`, so a test session on another port can be reached
        typed.parse::<SocketAddr>().ok().or_else(|| parse_typed_address(typed))
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
