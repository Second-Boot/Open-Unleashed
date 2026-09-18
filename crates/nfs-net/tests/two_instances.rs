//! Two copies of the port, talking to each other over a real UDP socket.
//!
//! Everything these tests exercise is the original's protocol as `RE/notes/network.md` §7 records
//! it — the one-byte tag, the big-endian control bodies, the slot mask and the countdowns. The one
//! step that is a hypothesis (H36) is which tag the game's own `0xB0 … 0xC6` messages ride: `'<'`,
//! because `0x5894C0` is the only user-data sender in the module and adds no header. Between two
//! copies of the port that is self-consistent; the original has not been on the other end yet.

use nfs_net::comm::{tag, JoinStatus};
use nfs_net::msg::{kind, Message};
use nfs_net::session::{name16, name32, Event, Net};
use nfs_net::socket::CommSocket;
use nfs_net::state::CarState;
use nfs_net::wire::Datagram;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// The application key a `'w'` probe carries; both ends have to agree on it (§7.13).
const KEY: [u8; 32] = [0xA5; 32];

fn loopback() -> CommSocket {
    CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap()
}

fn host(limit: u32) -> Net {
    let mut n = Net::host(loopback(), name32("Zone Industrial"), KEY, limit);
    n.my_name = name16("host");
    n
}

fn client(name: &str) -> Net {
    Net::client(loopback(), KEY, name16(name))
}

/// Poll both sides until `want` is satisfied, or give up. Real sockets, so this waits — and the
/// two sides answer each other a round apart, so the events are accumulated rather than examined
/// one poll at a time.
fn pump<T>(a: &mut Net, b: &mut Net, mut want: impl FnMut(&[Event], &[Event]) -> Option<T>) -> T {
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    for _ in 0..400 {
        ea.extend(a.poll());
        eb.extend(b.poll());
        if let Some(t) = want(&ea, &eb) {
            return t;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the two sides never got where they were supposed to:\n  a: {ea:?}\n  b: {eb:?}");
}

fn find(events: &[Event], f: impl Fn(&Event) -> bool) -> Option<&Event> {
    events.iter().find(|e| f(e))
}

#[test]
fn a_client_discovers_the_host_joins_it_and_the_car_state_flows_both_ways() {
    let mut h = host(8);
    let mut c = client("racer");
    let host_at = h.local_addr().unwrap();

    // 'w' with the session key -> 'd'
    c.probe_at(host_at).unwrap();
    let d = pump(&mut h, &mut c, |_, ec| {
        find(ec, |e| matches!(e, Event::Found { .. })).cloned()
    });
    let Event::Found { describe, .. } = d else { unreachable!() };
    assert_eq!(&describe.name[..15], b"Zone Industrial");
    assert_eq!(describe.player_limit, 8);
    assert_eq!(describe.slot_mask, 0);

    // 'j' -> 'a'
    c.join(host_at, b"a car record").unwrap();
    let (slot, peer_slot) = pump(&mut h, &mut c, |eh, ec| {
        let j = find(ec, |e| matches!(e, Event::Joined { .. }))?;
        let p = find(eh, |e| matches!(e, Event::PeerJoined { .. }))?;
        match (j, p) {
            (Event::Joined { slot }, Event::PeerJoined { slot: s2, .. }) => Some((*slot, *s2)),
            _ => None,
        }
    });
    assert_eq!((slot, peer_slot), (0, 0));
    assert_eq!(h.mask, 0b1);
    assert_eq!(c.slot, Some(0));
    assert_eq!(h.peers[0].as_ref().unwrap().name, name16("racer"));

    // the 0xBE car state, client -> host
    let sent = CarState {
        steer: -3,
        airborne: true,
        brake: 9,
        buttons: 1,
        tick: 4096,
        spin_x: 0.125,
        orientation: [100, -200, 300, 16384],
        velocity: [12.5, -0.25, 40.0],
        position: [1234.5, 6.25, -987.75],
    };
    c.send_game(&sent.packet(7)).unwrap();
    let got = pump(&mut h, &mut c, |eh, _| {
        let Event::Data { bytes, slot, .. } = find(eh, |e| matches!(e, Event::Data { .. }))? else { return None };
        assert_eq!(*slot, Some(0));
        Some(bytes.clone())
    });
    assert_eq!(got.len(), nfs_net::state::PACKET_SIZE);
    let m = Message::parse(&got).unwrap();
    assert_eq!(m.kind, kind::CAR_STATE);
    // the sender's id is byte 1, so it lands in the top eight bits of the head's 24-bit value
    assert_eq!(m.value, 7 << 16);
    let (from_id, back) = CarState::from_packet(&got, 0).unwrap();
    assert_eq!(from_id, 7);
    assert_eq!(back.position, sent.position);
    assert_eq!(back.orientation, sent.orientation);
    assert_eq!(back.tick, sent.tick);
    assert_eq!(back.airborne, sent.airborne);
    // and a packet carrying our own id is dropped, the way 0x48F5E8 drops it
    assert!(CarState::from_packet(&got, 7).is_none());

    // and host -> client
    h.send_game(&sent.packet(0)).unwrap();
    let got = pump(&mut h, &mut c, |_, ec| {
        let Event::Data { bytes, .. } = find(ec, |e| matches!(e, Event::Data { .. }))? else { return None };
        Some(bytes.clone())
    });
    assert_eq!(CarState::from_packet(&got, 7).unwrap().1.position, sent.position);
}

#[test]
fn the_host_fills_its_slots_and_then_says_it_is_full() {
    // a limit of 8 hands out seven slots — 0x589440 stops at limit - 1
    let mut h = host(4);
    let host_at = h.local_addr().unwrap();
    let mut joined = Vec::new();
    let mut clients: Vec<Net> = (0..4).map(|i| client(&format!("p{i}"))).collect();

    for (i, c) in clients.iter_mut().enumerate() {
        c.join(host_at, &[]).unwrap();
        let st = pump(&mut h, c, |_, ec| {
            ec.iter().find_map(|e| match e {
                Event::Joined { slot } => Some(JoinStatus::Joined(*slot)),
                Event::Refused { status } => Some(*status),
                _ => None,
            })
        });
        joined.push((i, st));
    }
    assert_eq!(joined[0].1, JoinStatus::Joined(0));
    assert_eq!(joined[1].1, JoinStatus::Joined(1));
    assert_eq!(joined[2].1, JoinStatus::Joined(2));
    assert_eq!(joined[3].1, JoinStatus::Full);
    assert_eq!(h.mask, 0b111);
}

#[test]
fn a_name_the_host_already_uses_is_refused() {
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("host");
    c.join(host_at, &[]).unwrap();
    let st = pump(&mut h, &mut c, |_, ec| {
        ec.iter().find_map(|e| match e {
            Event::Refused { status } => Some(*status),
            Event::Joined { slot } => Some(JoinStatus::Joined(*slot)),
            _ => None,
        })
    });
    assert_eq!(st, JoinStatus::NameTaken);
    assert_eq!(h.mask, 0);
}

#[test]
fn a_leave_frees_the_slot_and_so_does_a_countdown_that_runs_out() {
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("racer");
    c.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());
    assert_eq!(h.mask, 1);

    c.leave().unwrap();
    pump(&mut h, &mut c, |eh, _| find(eh, |e| matches!(e, Event::PeerLeft { slot: 0 })).cloned());
    assert_eq!(h.mask, 0);

    // and again, this time letting the 450-poll countdown expire with nobody talking
    let mut c2 = client("other");
    c2.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c2, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());
    let mut left = None;
    for _ in 0..600 {
        for e in h.tick() {
            if let Event::PeerLeft { slot } = e {
                left = Some(slot);
            }
        }
        // drain whatever the host's pings provoke, but never let the peer answer
        h.poll();
        if left.is_some() {
            break;
        }
    }
    assert_eq!(left, Some(0));
    assert_eq!(h.mask, 0);
}

#[test]
fn a_peer_that_keeps_talking_is_never_dropped() {
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("racer");
    c.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());

    for i in 0..600 {
        if i % 50 == 0 {
            // a bare '=' is the keep-alive, and 0x588890 refreshes the record on any datagram
            c.sock.send_to(host_at, tag::ALIVE, &[]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        h.poll();
        c.poll();
        assert!(h.tick().is_empty(), "the peer was dropped at poll {i}");
    }
    assert_eq!(h.mask, 1);
}

#[test]
fn the_slot_query_answers_p_when_the_slot_is_live_and_question_mark_when_it_is_not() {
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("racer");

    // nobody in slot 0 yet
    c.query_slot(host_at, 0).unwrap();
    let raw = raw_reply(&mut h, &mut c, host_at);
    assert_eq!(raw.tag, tag::NO_SUCH);
    assert_eq!(raw.payload.len(), 0x1C);

    c.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());

    c.query_slot(host_at, 0).unwrap();
    let raw = raw_reply(&mut h, &mut c, host_at);
    assert_eq!(raw.tag, tag::REPLY);
    assert_eq!(raw.payload.len(), 0x78);
    // the record's first twenty bytes are the peer's address, which is what 0x587DA9 pings
    let rec = &raw.payload[0x1C..];
    assert_eq!(&rec[..2], &[2, 0]);
    assert_eq!(&rec[4..8], &[127, 0, 0, 1]);
}

#[test]
fn an_in_race_datagram_is_the_original_shape_on_the_wire() {
    // the host sends to whatever joined it, so a bare socket joins and then reads the raw bytes
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("racer");
    c.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());

    let st = CarState { tick: 64, position: [1.0, 2.0, 3.0], ..CarState::default() };
    h.send_game(&st.packet(0)).unwrap();

    // read it off the client's socket without the session interpreting it
    let d = loop {
        if let Some((_, d)) = c.sock.recv().unwrap() {
            if d.tag == nfs_net::data::TAG && !d.payload.is_empty() {
                break d;
            }
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };

    // 0x3D, then six bytes of frame, then the 40-byte 0xBE packet
    assert_eq!(d.tag, b'=');
    assert_eq!(1 + d.payload.len(), 47);
    let f = nfs_net::data::Frame::decode(&d.payload).unwrap();
    assert_eq!(f.pieces.len(), 1);
    assert_eq!(f.flags, 0);
    // the ack is what this side next expects from the peer; nothing has arrived yet, so it is the
    // peer's own starting sequence
    assert_eq!(f.ack, 1);
    assert_eq!(f.pieces[0].body.len(), nfs_net::state::PACKET_SIZE);
    assert_eq!(f.pieces[0].body[0], kind::CAR_STATE);
    // the sequence starts at 1, the way 0x5952EF starts conn+0x5A
    assert_eq!(f.seq, 1);
    assert_eq!(CarState::from_packet(&f.pieces[0].body, 9).unwrap().1.position, [1.0, 2.0, 3.0]);
}

/// Read the host's answer straight off a bare socket, so the test sees the bytes rather than an
/// interpretation of them.
fn raw_reply(h: &mut Net, c: &mut Net, host_at: SocketAddr) -> Datagram {
    let mut probe = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap();
    let mut p = Vec::new();
    nfs_net::comm::Header { request: 1, from: probe.local_record().unwrap(), index: 0 }.encode(&mut p);
    probe.send_to(host_at, tag::QUERY, &p).unwrap();
    for _ in 0..400 {
        h.poll();
        c.poll();
        if let Some((_, d)) = probe.recv().unwrap() {
            return d;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("no answer to 'g'");
}
