//! The game room of `crate::room` between two sessions on real loopback sockets: a host that plays
//! the room object's part (0x48BB50, 0x4912D4) and a player that only hears (0x48F0A0).
//!
//! Two copies of the port, not the original — what this shows is that the bytes `room` builds
//! survive the session's data layer and its relay the way the reads say they should.

use nfs_net::msg::kind;
use nfs_net::room::{self, ChatRequest, Heard, Room, To};
use nfs_net::session::{name16, name32, Event, Net};
use nfs_net::socket::CommSocket;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

const KEY: [u8; 32] = [0x5A; 32];

fn loopback() -> CommSocket {
    CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap()
}

/// The host's side: a session, and a room it is also a player of (id 0).
struct Host {
    net: Net,
    room: Room,
    heard: Vec<Heard>,
}

impl Host {
    /// Deliver what the room object sends: to the peers over the session, and to the host's own
    /// player directly, since in this port the host is both.
    fn send(&mut self, out: Vec<(To, Vec<u8>)>) {
        for (to, m) in out {
            match to {
                To::All => {
                    self.net.send_game(&m).unwrap();
                    if let Some(h) = self.room.hear(&m) {
                        self.heard.push(h);
                    }
                }
                To::Player(0) => {
                    if let Some(h) = self.room.hear(&m) {
                        self.heard.push(h);
                    }
                }
                To::Player(id) => {
                    let a = self.net.peer_addr(id as u32 - 1).unwrap();
                    self.net.send_game_to(&[a], &m).unwrap();
                }
            }
        }
    }

    fn poll(&mut self) {
        for e in self.net.poll() {
            match e {
                Event::PeerJoined { slot, .. } => {
                    let name = self.net.peers[slot as usize].as_ref().unwrap().name;
                    let name = name.split(|&c| c == 0).next().unwrap().to_vec();
                    let out = self.room.admit(slot as i32 + 1, &name);
                    self.send(out);
                }
                Event::Data { slot: Some(s), bytes, .. } if bytes.first() == Some(&kind::CHAT) => {
                    let r = ChatRequest::parse(&bytes).unwrap();
                    let out = self.room.route_chat(s as i32 + 1, r.to, &r.text);
                    self.send(out);
                }
                Event::Data { bytes, .. } => {
                    if let Some(h) = self.room.hear(&bytes) {
                        self.heard.push(h);
                    }
                }
                _ => {}
            }
        }
    }
}

fn pump(h: &mut Host, c: &mut Net, room: &mut Room, heard: &mut Vec<Heard>, until: impl Fn(&Host, &[Heard]) -> bool) {
    for _ in 0..500 {
        h.poll();
        for e in c.poll() {
            if let Event::Data { bytes, .. } = e
                && let Some(x) = room.hear(&bytes)
            {
                heard.push(x);
            }
        }
        if until(h, heard) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never got there: host heard {:?}, player heard {heard:?}", h.heard);
}

#[test]
fn a_player_joins_the_room_chats_and_hands_over_its_picture() {
    let mut net = Net::host(loopback(), name32("room test"), KEY, 8);
    net.my_name = name16("host");
    let mut h = Host { net, room: Room::new(0), heard: Vec::new() };
    // the host's own player comes in first, as the room's id 0
    let me = h.room.admit(0, b"host");
    h.send(me);
    assert_eq!(h.room.players.len(), 1);

    let mut c = Net::client(loopback(), KEY, name16("anna"));
    let at = h.net.local_addr().unwrap();
    c.join(at, b"anna").unwrap();
    let mut room = Room::new(-1);
    let mut heard = Vec::new();
    // the host admits the newcomer to the room as soon as it has a slot, so the room's first
    // messages can arrive in the same poll as the `'a'` — none of that poll may be dropped
    for _ in 0..500 {
        h.poll();
        for e in c.poll() {
            match e {
                Event::Joined { slot } => room.me = slot as i32 + 1,
                Event::Data { bytes, .. } => heard.extend(room.hear(&bytes)),
                _ => {}
            }
        }
        if room.me >= 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(room.me, 1);

    // 0x48BB50: the newcomer hears of the host and then of itself; the host hears of the newcomer
    pump(&mut h, &mut c, &mut room, &mut heard, |h, _| h.room.players.len() == 2);
    pump(&mut h, &mut c, &mut room, &mut heard, |_, heard| heard.len() >= 2);
    assert_eq!(heard[..2], [Heard::Joined(0), Heard::Joined(1)]);
    assert_eq!(room.players.iter().map(|p| p.name.as_slice()).collect::<Vec<_>>(), vec![&b"host"[..], b"anna"]);

    // a line for everyone: the player's request goes to the host only, and comes back rewritten
    c.send_game(&ChatRequest { to: room::TO_ALL, text: b"hello".to_vec() }.encode()).unwrap();
    pump(&mut h, &mut c, &mut room, &mut heard, |_, heard| heard.iter().any(|x| matches!(x, Heard::Chat(_))));
    let line = heard.iter().find_map(|x| if let Heard::Chat(l) = x { Some(l.clone()) } else { None }).unwrap();
    assert_eq!((line.from, line.to, line.text.as_slice()), (1, -1, &b"hello"[..]));
    assert!(h.heard.iter().any(|x| matches!(x, Heard::Chat(l) if l.from == 1)));

    // the picture: posted, sent a chunk at a time on the room's clock, and whole at the host
    let file: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
    room.post(room::PICTURE, &file);
    assert!(room.tick(0).is_empty());
    let mut t = 0;
    while room.sending() {
        t += 40;
        for m in room.tick(t) {
            c.send_game(&m).unwrap();
        }
    }
    pump(&mut h, &mut c, &mut room, &mut heard, |h, _| h.heard.contains(&Heard::File { from: 1, kind: room::PICTURE }));
    assert_eq!(h.room.player(1).unwrap().files[0].bytes, file);
}
