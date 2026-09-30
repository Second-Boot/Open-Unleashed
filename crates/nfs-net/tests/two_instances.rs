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
        yaw_rate: 0.125,
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

/// The joiner's identity string `name~version~game` (0x48A700) is what the host's gate reads: another
/// version is the callback's refusal (0x588EBF…0x588ED3 → `-3`), and the slot the allocator handed
/// out is given back. The game field is not a LAN / direct join's business (the room-name test is the
/// lobby's `0xB6` request only; the game sends it empty, 0x5E8E50) — a game name with a space joins.
#[test]
fn the_identity_gate_refuses_another_version_and_lets_any_game_name_in() {
    use nfs_net::comm::identity;
    let join_with = |h: &mut Net, name: &str, id: &[u8]| {
        let mut c = client(name);
        c.join(h.local_addr().unwrap(), id).unwrap();
        pump(h, &mut c, |_, ec| {
            ec.iter().find_map(|e| match e {
                Event::Refused { status } => Some(*status),
                Event::Joined { slot } => Some(JoinStatus::Joined(*slot)),
                _ => None,
            })
        })
    };
    let mut h = host(8);
    h.identity_gate = Some("3.5".to_string());
    assert_eq!(join_with(&mut h, "old", &identity::encode("old", "3.4", "Zone")), JoinStatus::Refused, "version");
    assert_eq!(join_with(&mut h, "bare", &[]), JoinStatus::Refused, "no identity at all");
    assert_eq!(h.mask, 0, "a refused joiner keeps no slot");
    assert_eq!(join_with(&mut h, "right", &identity::encode("right", "3.5", "")), JoinStatus::Joined(0));
    // the game field is not compared: a joiner of a room named with a space (the CREATE dialog's
    // default `Игра DRIVER`) or naming anything else still gets in
    assert_eq!(join_with(&mut h, "other", &identity::encode("other", "3.5", "Elsewhere")), JoinStatus::Joined(1));
    // a full host answers -2 before it looks at the name or the identity (0x588E4F…0x588E67 first)
    let mut full = host(2);
    full.identity_gate = Some("3.5".to_string());
    assert_eq!(join_with(&mut full, "a", &identity::encode("a", "3.5", "Zone")), JoinStatus::Joined(0));
    assert_eq!(join_with(&mut full, "host", &identity::encode("host", "9.9", "Nowhere")), JoinStatus::Full);
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
    // the ack is `conn+0x5E`, the last sequence delivered in order (0x595B50); nothing has arrived
    // yet, so it is the connection's start value, 0 (0x5951A0)
    assert_eq!(f.ack, 0);
    assert_eq!(f.pieces[0].body.len(), nfs_net::state::PACKET_SIZE);
    assert_eq!(f.pieces[0].body[0], kind::CAR_STATE);
    // the `0xBE` state goes unreliably (0x55D4D0's `len | 0x80000000`): sequence 0xFFFE (0x595D7C)
    assert_eq!(f.seq, nfs_net::data::UNRELIABLE_SEQ);
    assert_eq!(CarState::from_packet(&f.pieces[0].body, 9).unwrap().1.position, [1.0, 2.0, 3.0]);

    // a reliable message (the room's `0xB1`) takes the counter, which starts at 1 (0x5951A0)
    h.send_game(&[0xB1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let d = loop {
        if let Some((_, d)) = c.sock.recv().unwrap() {
            if d.tag == nfs_net::data::TAG && d.payload.len() > 6 && d.payload[6] == 0xB1 {
                break d;
            }
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    let f = nfs_net::data::Frame::decode(&d.payload).unwrap();
    assert_eq!(f.seq, 1);
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

/// Both sides joined, loopback, a chosen share of the data frames dropped on each side's sends.
fn joined_pair(loss: u32) -> (Net, Net) {
    let mut h = host(8);
    let host_at = h.local_addr().unwrap();
    let mut c = client("racer");
    c.join(host_at, &[]).unwrap();
    pump(&mut h, &mut c, |_, ec| find(ec, |e| matches!(e, Event::Joined { .. })).cloned());
    h.sock.set_drop(loss, 7);
    c.sock.set_drop(loss, 11);
    (h, c)
}

/// Run both sides for `ms` milliseconds, gathering the game messages each hears.
fn run_for(h: &mut Net, c: &mut Net, ms: u64, mut on_tick: impl FnMut(&mut Net, &mut Net)) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let (mut got_h, mut got_c) = (Vec::new(), Vec::new());
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    // the client's send queue and the host's peer queues are pumped 32 times a second by the room's
    // loop (0x48EE90, 0x48B4F0); a test has none
    let mut next_pump = std::time::Instant::now();
    while std::time::Instant::now() < end {
        on_tick(h, c);
        if std::time::Instant::now() >= next_pump {
            h.pump_outbox();
            c.pump_outbox();
            next_pump += std::time::Duration::from_micros(31_250);
        }
        for e in h.poll() {
            if let Event::Data { bytes, .. } = e {
                got_h.push(bytes);
            }
        }
        for e in c.poll() {
            if let Event::Data { bytes, .. } = e {
                got_c.push(bytes);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    (got_h, got_c)
}

#[test]
fn a_burst_of_room_messages_beyond_the_pool_arrives_whole_and_in_order() {
    // the launch stream sends dozens of records at once (network.md §11.18: the pool holds seven)
    let (mut h, mut c) = joined_pair(0);
    for i in 0..80u8 {
        // long records (option strings run to a couple of hundred bytes): two of them never fit
        // beside a third in one datagram, which the frame's size check has to see
        let mut m = vec![0xBC, i];
        m.resize(if i % 3 == 0 { 8 } else { 170 }, 0);
        h.send_game(&m).unwrap();
    }
    let (_, got) = run_for(&mut h, &mut c, 6000, |_, _| {});
    let seen: Vec<u8> = got.iter().filter(|m| m[0] == 0xBC).map(|m| m[1]).collect();
    assert_eq!(seen, (0..80).collect::<Vec<u8>>());
}

#[test]
fn reliable_messages_both_ways_survive_a_lossy_link_in_order() {
    let (mut h, mut c) = joined_pair(10);
    let mut n = 0u8;
    let (got_h, got_c) = run_for(&mut h, &mut c, 12000, |h, c| {
        // the room's messages go down (reliable), a player's `0xB7` reports go up (reliable), the
        // input samples (unreliable) go up too and carry the acks
        if n < 100 {
            h.send_game(&[0xC5, n, 0, 0, 0, 0, 0, 0]).unwrap();
            c.send_game(&[0xB7, n, 0, 0, 0, 0, 0, 0]).unwrap();
            c.send_game(&[1, 2, 3]).unwrap();
            n += 1;
        }
    });
    let down: Vec<u8> = got_c.iter().filter(|m| m[0] == 0xC5).map(|m| m[1]).collect();
    let up: Vec<u8> = got_h.iter().filter(|m| m[0] == 0xB7).map(|m| m[1]).collect();
    assert_eq!(down, (0..100).collect::<Vec<u8>>(), "the room's messages: lost, doubled or reordered");
    assert_eq!(up, (0..100).collect::<Vec<u8>>(), "the reports: lost, doubled or reordered");
    assert!(h.sock.dropped() + c.sock.dropped() > 20, "the glue dropped nothing");
}
