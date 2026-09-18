//! Network play, wired into the race loop (`RE/notes/network.md` §5, §7).
//!
//! The original's `NfsNet_tick` (0x48EDF0) runs straight from the world tick and broadcasts the
//! 40-byte `0xBE` car state when `(gGameTick & 7) == 7`, i.e. at 8 Hz against the 64 Hz clock. The
//! receiving side drops a packet carrying its own id (0x48F5E8) and otherwise hands the block to
//! the player it names. That is exactly what this does; the session underneath is
//! [`nfs_net::session`].
//!
//! Between the received states the remote car is **interpolated** — the original's own receiver has
//! not been read that far, so the smoothing here is an **APPROXIMATION**. Nothing else is: the
//! packet, its gate, the quantisation and the id rule are the original's.
//!
//! What carries the packet is **not** the original's path. The original hands a game message to its
//! connection's data layer (§7.11) — `u16be length` over TCP, or a six- or four-byte frame over
//! UDP — and the `'<'` tag this port sends under is dead code there (H36, refuted). Two copies of
//! the port understand each other; the original will not.

use glam::{Mat3, Quat, Vec3};
use nfs_net::session::{Event, Net, Role, name16, name32};
use nfs_net::socket::CommSocket;
use nfs_net::state::{CarState, TICK_MASK};
use nfs_physics::vehicle::Vehicle;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::time::{Duration, Instant};

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

/// One remote player's last known state and where it is being drawn.
pub struct Remote {
    pub seat: usize,
    pub state: CarState,
    /// the pose the seat had when the state arrived, so the gap can be smoothed
    pub from: (Vec3, Quat),
    pub since: u64,
}

pub struct NetPlay {
    pub net: Net,
    /// Our own player id: the host is 0, a client is its slot + 1.
    pub id: u8,
    /// Which opponent seat each remote id drives.
    pub remotes: HashMap<u8, Remote>,
    /// Seats reserved for the network, in the order they are handed out.
    pub seats: Vec<usize>,
    /// `Comms.Positional`: with it off the `0xBE` broadcast is not sent at all.
    pub positional: bool,
    next_seat: usize,
    /// When `0x587CE0` was last run. Its two counters — 100 between pings, 450 before a peer is
    /// dropped — are in **polls**, and the original registers it as a periodic task (0x587CC0 →
    /// 0x580DC0) whose period has not been read. So the port runs it on the wall clock at the
    /// game's own 64 Hz, which makes the ping about 1.6 s and the timeout about 7 s. Driving it
    /// from the simulated tick instead would make the timeout depend on how fast the simulation
    /// happens to run, which a headless run makes very obvious. **The rate is a choice; the two
    /// counters are the original's.**
    last_poll: Instant,
}

/// One turn of 0x587CE0 per 1/64 s of real time.
const POLL_PERIOD: Duration = Duration::from_micros(15_625);

impl NetPlay {
    /// `host`, `host:PORT`, `join:HOST[:PORT]`. Anything else is an error rather than a guess.
    pub fn start(spec: &str, who: &str) -> anyhow::Result<Self> {
        let (mode, rest) = spec.split_once(':').unwrap_or((spec, ""));
        let net = match mode {
            "host" => {
                let port: u16 = if rest.is_empty() {
                    nfs_net::PORT
                } else {
                    rest.parse()?
                };
                let sock = CommSocket::bind(port)?;
                let mut n = Net::host(sock, name32(&format!("{who}'s race")), KEY, 8);
                n.my_name = name16(who);
                tracing::info!(addr = %n.local_addr()?, "hosting");
                n
            }
            "join" => {
                let target = if rest.contains(':') {
                    rest.to_string()
                } else {
                    format!("{rest}:{}", nfs_net::PORT)
                };
                let host: SocketAddr = target
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("cannot resolve {target}"))?;
                let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::UNSPECIFIED,
                    0,
                )))?;
                let mut n = Net::client(sock, KEY, name16(who));
                n.probe_at(host)?;
                n.join(host, who.as_bytes())?;
                tracing::info!(%host, addr = %n.local_addr()?, "joining");
                n
            }
            other => anyhow::bail!(
                "--net wants `host`, `host:PORT` or `join:HOST[:PORT]`, not {other:?}"
            ),
        };
        let id = if net.role == Role::Host { 0 } else { u8::MAX }; // a client's id arrives with its 'a'
        Ok(Self {
            net,
            id,
            remotes: HashMap::new(),
            seats: Vec::new(),
            positional: true,
            next_seat: 0,
            last_poll: Instant::now(),
        })
    }

    pub fn is_host(&self) -> bool {
        self.net.role == Role::Host
    }

    /// The opponent seats the network owns. Called once the race has built its opponents.
    pub fn reserve(&mut self, seats: Vec<usize>) {
        self.seats = seats;
        self.next_seat = 0;
        self.remotes.clear();
    }

    /// Drain the socket. Returns the events worth logging; the car states are kept internally.
    pub fn poll(&mut self, tick: u64, seat_pose: impl Fn(usize) -> (Vec3, Quat)) -> Vec<String> {
        let mut log = Vec::new();
        for e in self.net.poll() {
            match e {
                Event::Joined { slot } => {
                    self.id = slot as u8 + 1;
                    log.push(format!("joined at slot {slot}, player id {}", self.id));
                }
                Event::Refused { status } => log.push(format!("the host refused: {status:?}")),
                Event::PeerJoined { slot, from } => {
                    log.push(format!("slot {slot} taken by {from}"))
                }
                Event::PeerLeft { slot } => log.push(format!("slot {slot} left")),
                Event::Quit { from } => log.push(format!("{from} quit")),
                Event::Found { from, describe } => {
                    let n = String::from_utf8_lossy(&describe.name)
                        .trim_end_matches('\0')
                        .to_string();
                    log.push(format!("found \"{n}\" at {from}"));
                }
                Event::Data { bytes, .. } => {
                    // 0x48F5E8: our own id comes back on a relayed packet, and it is dropped
                    let Some((who, st)) = CarState::from_packet(&bytes, self.id) else {
                        continue;
                    };
                    let seat = match self.remotes.get(&who) {
                        Some(r) => r.seat,
                        None => {
                            let Some(s) = self.seats.get(self.next_seat).copied() else {
                                continue;
                            };
                            self.next_seat += 1;
                            log.push(format!("player {who} is driving seat {s}"));
                            s
                        }
                    };
                    let from = seat_pose(seat);
                    // the packet carries the *sender's* tick (+0x04), so this line can be matched
                    // against the sender's own log without the two clocks having to agree
                    tracing::debug!(target: "netplay", "rx player={who} sender_tick={} pos={:?} steer={} brake={}", st.tick, st.position, st.steer, st.brake);
                    self.remotes.insert(
                        who,
                        Remote {
                            seat,
                            state: st,
                            from,
                            since: tick,
                        },
                    );
                }
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
                    self.remotes
                        .retain(|_, r| Some(r.seat) != self.seats.get(slot as usize).copied());
                }
                other => log.push(format!("{other:?}")),
            }
        }
        log
    }

    /// Put the last received state on a seat's car. `alpha` is how far to blend from where the car
    /// was when the packet arrived — the **APPROXIMATION** the module note mentions, since packets
    /// come at 8 Hz and the frame runs at 64.
    pub fn drive(
        &self,
        seat: usize,
        v: &mut Vehicle,
        specs: &nfs_physics::specs::CarSpecs,
        tick: u64,
    ) -> bool {
        let Some(r) = self.remotes.values().find(|r| r.seat == seat) else {
            return false;
        };
        let q = nfs_net::state::unpack_orientation(r.state.orientation);
        let target_q = Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize();
        let target_p = Vec3::from(r.state.position);
        let vel = Vec3::from(r.state.velocity);
        // eight ticks is one packet interval (TICK_MASK + 1); past that the car simply holds still
        let a = ((tick.saturating_sub(r.since) as f32) / (TICK_MASK as f32 + 1.0)).clamp(0.0, 1.0);
        let pos = r.from.0.lerp(
            target_p + vel * (a - 1.0) * (TICK_MASK as f32 + 1.0) / 64.0,
            a,
        );
        v.position = pos;
        v.velocity = vel;
        v.orientation = Mat3::from_quat(r.from.1.slerp(target_q, a)).transpose();
        v.height_marker = if r.state.airborne { 1.0 } else { 0.0 };
        // the wire carries the sender's steering and brake, so the remote car's wheels point the
        // right way and its brake reads true rather than being frozen at whatever it last was
        v.steer = r.state.steer as i32;
        v.steer_target = v.steer;
        v.brake = r.state.brake as f32 / 252.0;
        v.steer_wheels_now(specs);
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
            spin_x: v.angular_velocity.x,
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

    /// `Comms.PlayerList` once a session is open: the host lists itself and every peer that has a
    /// slot, a client lists the host and the slots the `'a'` it got told it about.
    pub fn players(&self) -> Vec<String> {
        let mut v = vec![if self.is_host() {
            "HOST (YOU)".to_string()
        } else {
            "HOST".to_string()
        }];
        for (i, p) in self.net.peers.iter().enumerate() {
            let Some(p) = p else { continue };
            let name = String::from_utf8_lossy(&p.name);
            let name = name.trim_end_matches('\0').trim();
            let label = if name.is_empty() {
                format!("PLAYER {}", i + 1)
            } else {
                name.to_uppercase()
            };
            v.push(match self.net.slot == Some(i as u32) {
                true => format!("{label} (YOU)"),
                false => label,
            });
        }
        if !self.is_host() && self.net.peers.iter().all(Option::is_none) {
            if let Some(slot) = self.net.slot {
                v.push(format!("PLAYER {} (YOU)", slot + 1));
            }
        }
        v
    }

    /// `Comms.Data(GAMENAME)` — the session's own name.
    pub fn game_name(&self) -> String {
        String::from_utf8_lossy(&self.net.name)
            .trim_end_matches('\0')
            .trim()
            .to_uppercase()
    }

    /// `Comms.Data(MYIPADDRESS)`.
    pub fn my_address(&self) -> String {
        self.net
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default()
    }

    /// `EJECTPLAYER` — message **0xC6**, which 0x491748 sends under "Ejected by host" (§5.4). The
    /// body the original puts behind the head has not been read, so only the head goes out.
    pub fn eject(&mut self, player: u8) {
        let mut m = Vec::new();
        nfs_net::msg::Message::write_head(nfs_net::msg::kind::EJECT, (player as i32) << 16, &mut m);
        if let Err(e) = self.net.send_game(&m) {
            tracing::warn!(error = %e, "eject not sent");
        }
    }

    /// `'K'` — tell the others we are going (0x589152 clears our bit on the host).
    #[allow(dead_code)]
    pub fn leave(&mut self) {
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
/// What the *handlers* behind those callback lists do has not been read, so this fills each
/// accessor from the protocol of §7 and nothing else: a session found by a `'w'` probe appears in
/// the lists, `CREATE` hosts, `JOIN`/`CONNECT` joins. The modem accessors stay empty — this port
/// has no modem, and an empty list is what a machine without one shows.
pub struct Lobby {
    net: Net,
    /// the sessions a `'w'` probe turned up, newest first
    pub found: Vec<(SocketAddr, nfs_net::comm::Describe)>,
    /// `Comms.IP` — the address being typed
    pub ip: String,
    /// which row of the list is selected
    pub sel: usize,
    /// `Comms.Positional` / `Comms.Data(POSITIONALUPDATES)` — whether the `0xBE` car-state
    /// broadcast of §5.2 is on. The host's screen has it as a slider and the joiner's as a watch,
    /// so it is the host's setting; the port keeps it here and [`NetPlay::positional`] carries it
    /// into the race.
    pub positional: bool,
    last_probe: Instant,
    who: String,
}

impl Lobby {
    /// Open a browsing socket. Port 0, so a second copy on the same machine can browse too.
    pub fn open(who: &str) -> anyhow::Result<Self> {
        let sock =
            CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
        Ok(Self {
            net: Net::client(sock, KEY, name16(who)),
            found: Vec::new(),
            ip: String::new(),
            sel: 0,
            positional: true,
            last_probe: Instant::now() - Duration::from_secs(9),
            who: who.to_string(),
        })
    }

    /// Shout a `'w'` once a second and collect the `'d'` answers. `0x560920`'s IPX announce has no
    /// IP twin (§6), so a port that wants to be found over IP has to probe for itself.
    pub fn poll(&mut self) {
        if self.last_probe.elapsed() >= Duration::from_secs(1) {
            // the subnet, as `0x5894C0`'s null address does — and the loopback as well, which the
            // original never needs because two copies of it never share a machine
            let _ = self.net.probe(nfs_net::PORT);
            let _ = self.net.probe_at(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::LOCALHOST,
                nfs_net::PORT,
            )));
            if let Ok(a) = self.ip.trim().parse::<SocketAddr>() {
                let _ = self.net.probe_at(a);
            } else if let Ok(ip) = self.ip.trim().parse::<Ipv4Addr>() {
                let _ = self
                    .net
                    .probe_at(SocketAddr::V4(SocketAddrV4::new(ip, nfs_net::PORT)));
            }
            self.last_probe = Instant::now();
        }
        for e in self.net.poll() {
            if let Event::Found { from, describe } = e {
                if !self.found.iter().any(|(a, _)| *a == from) {
                    let n = String::from_utf8_lossy(&describe.name)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string();
                    tracing::info!(target: "netplay", %from, name = %n, slots = describe.player_limit, mask = describe.slot_mask, "session found");
                }
                self.found.retain(|(a, _)| *a != from);
                self.found.insert(0, (from, *describe));
                self.found.truncate(16);
            }
        }
        self.net.tick();
    }

    /// `Comms.ServerList` / `Comms.GamesList`: one row per session, as `'d'` describes it.
    pub fn games(&self) -> Vec<String> {
        self.found
            .iter()
            .map(|(a, d)| {
                let name = String::from_utf8_lossy(&d.name)
                    .trim_end_matches('\0')
                    .trim()
                    .to_uppercase();
                let taken = d.slot_mask.count_ones() + 1;
                format!("{name}   {taken}/{}   {a}", d.player_limit.max(1))
            })
            .collect()
    }

    /// `Comms.IPList`: the addresses that answered, plus whatever is being typed.
    pub fn addresses(&self) -> Vec<String> {
        let mut v: Vec<String> = self.found.iter().map(|(a, _)| a.to_string()).collect();
        let typed = self.ip.trim();
        if !typed.is_empty() && !v.iter().any(|x| x == typed) {
            v.insert(0, typed.to_string());
        }
        v
    }

    /// `Comms.LobbyPlayerList`: who is in the session under the cursor, as far as `'d'` says —
    /// which is a count and a mask, not names, so the rows are the slots.
    pub fn players(&self) -> Vec<String> {
        let Some((_, d)) = self.found.get(self.sel) else {
            return Vec::new();
        };
        let mut v = vec!["HOST".to_string()];
        for i in 0..d.player_limit.saturating_sub(1) {
            if d.slot_mask & (1 << i) != 0 {
                v.push(format!("PLAYER {}", i + 1));
            }
        }
        v
    }

    /// The address `CONNECT` or `JOIN` would use: what is typed, else the selected row.
    pub fn target(&self) -> Option<SocketAddr> {
        let typed = self.ip.trim();
        if !typed.is_empty() {
            if let Ok(a) = typed.parse::<SocketAddr>() {
                return Some(a);
            }
            if let Ok(ip) = typed.parse::<Ipv4Addr>() {
                return Some(SocketAddr::V4(SocketAddrV4::new(ip, nfs_net::PORT)));
            }
        }
        self.found.get(self.sel).map(|(a, _)| *a)
    }

    /// `CREATE` — become the host on the well-known port.
    pub fn create(&self) -> anyhow::Result<NetPlay> {
        NetPlay::start(&format!("host:{}", nfs_net::PORT), &self.who)
    }

    /// `Comms.Data(GAMENAME)` — the session under the cursor.
    pub fn game_name(&self) -> String {
        match self.found.get(self.sel) {
            Some((_, d)) => String::from_utf8_lossy(&d.name)
                .trim_end_matches('\0')
                .trim()
                .to_uppercase(),
            None => format!("{}'S RACE", self.who.to_uppercase()),
        }
    }

    /// `Comms.Data(MYIPADDRESS)` — what `[desc+0x44]` copies out of `sock+0x08` (0x5878F0).
    pub fn my_address(&self) -> String {
        match self.net.local_addr() {
            Ok(a) => a.to_string(),
            Err(_) => String::new(),
        }
    }

    /// `JOIN` / `CONNECT` — join what [`Self::target`] names.
    pub fn connect(&self) -> anyhow::Result<NetPlay> {
        let a = self
            .target()
            .ok_or_else(|| anyhow::anyhow!("no address to connect to"))?;
        NetPlay::start(&format!("join:{a}"), &self.who)
    }
}
