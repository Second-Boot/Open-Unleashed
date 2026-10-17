use super::*;

fn quiet_lobby() -> Lobby {
    let mut l = Lobby::open("tester").unwrap();
    l.persist = false;
    l.history.clear();
    l
}

/// 0x4C2120 / 0x4C2220: a committed address goes to the front of `Comms.IPList` and the list
/// selects row 0 — with no duplicate test — and CONNECT (0x50B150) reads the list's current row.
#[test]
fn a_committed_address_goes_to_the_front_and_connect_reads_the_current_row() {
    let mut l = quiet_lobby();
    assert_eq!(l.connect_target(), None, "an empty list is 0x50B150's 816 branch");
    l.commit_ip("192.168.0.5");
    l.commit_ip(" 10.0.0.7:1200 ");
    assert_eq!(l.addresses(), vec!["10.0.0.7:1200", "192.168.0.5"]);
    assert_eq!(l.connect_target(), Some("10.0.0.7:1200".parse().unwrap()));
    l.pick_ip(1);
    assert_eq!(l.ip, "192.168.0.5");
    assert_eq!(l.connect_target(), Some("192.168.0.5:9442".parse().unwrap()), "a bare address takes port 9442");
    l.commit_ip("192.168.0.5");
    assert_eq!(l.addresses().len(), 3, "the original adds the row again");
    l.commit_ip("   ");
    assert_eq!(l.addresses().len(), 3, "an empty string is not a row");
}

/// 0x48AC90: `%d.%d.%d.%d.%d` — four numbers the address, a fifth the port, the default 9442 (0x48E4CD).
#[test]
fn a_typed_address_is_read_as_the_original_scans_it() {
    let t = |s: &str| parse_typed_address(s);
    assert_eq!(t("192.168.2.71"), Some("192.168.2.71:9442".parse().unwrap()));
    assert_eq!(t("127.0.0.1.5000"), Some("127.0.0.1:5000".parse().unwrap()));
    // each number is masked to a byte, and the string after the fourth is not read
    assert_eq!(t("300.1.2.3junk"), Some("44.1.2.3:9442".parse().unwrap()));
    assert_eq!(t(" 10. 0.0. 7"), Some("10.0.0.7:9442".parse().unwrap()));
    // fewer than four numbers: a name for gethostbyname
    assert_eq!(t("localhost").map(|a| a.port()), Some(9442));
    // (what the resolver makes of `1.2.3` is its own — inet_addr's short form, 1.2.0.3)
}

#[test]
fn a_session_that_stops_answering_leaves_the_list() {
    let mut l = quiet_lobby();
    let a: SocketAddr = "10.0.0.9:9442".parse().unwrap();
    let d = nfs_net::comm::Describe {
        request: 0,
        from: nfs_net::wire::Addr::ip([10, 0, 0, 9], 9442),
        conn_dword: 0,
        conn_words: [0, 0],
        conn_raw: [0, 0],
        name: [0; 32],
        bytes: [0; 4],
        words: [0, 0],
        dwords: [0, 0],
        player_limit: 8,
        slot_mask: 0,
    };
    l.found.push((a, d));
    l.seen.insert(a, Instant::now() - SESSION_TTL - Duration::from_secs(1));
    l.poll();
    assert!(l.found.is_empty());
}

fn answer(ip: [u8; 4], name: &str, mask: u32) -> Box<nfs_net::comm::Describe> {
    Box::new(nfs_net::comm::Describe {
        request: 0,
        from: nfs_net::wire::Addr::ip(ip, 9442),
        conn_dword: 0,
        conn_words: [0, 0],
        conn_raw: [0, 0],
        name: nfs_net::comm::desc::name_field(name, "3.5"),
        bytes: [0, 1, 0, 0],
        words: [410, 313],
        dwords: [0, 0],
        player_limit: 8,
        slot_mask: mask,
    })
}

/// One host answers a probe over its loopback and its LAN address: one row, at the LAN
/// address (a host that only the loopback reaches is still listed); two hosts with the same
/// name but different address records stay two rows.
#[test]
fn one_session_answering_over_several_interfaces_is_one_row() {
    let mut l = quiet_lobby();
    let now = Instant::now();
    let lo: SocketAddr = "127.0.0.1:9442".parse().unwrap();
    let lan: SocketAddr = "192.168.0.82:9442".parse().unwrap();
    let other: SocketAddr = "192.168.0.99:9442".parse().unwrap();
    l.note_found(lo, answer([192, 168, 0, 82], "DRIVER's race", 0), now);
    assert_eq!(l.found.len(), 1);
    assert_eq!(l.found[0].0, lo, "only the loopback has answered so far");
    l.note_found(lan, answer([192, 168, 0, 82], "DRIVER's race", 1), now);
    l.note_found(lo, answer([192, 168, 0, 82], "DRIVER's race", 1), now);
    assert_eq!(l.found.len(), 1);
    assert_eq!(l.found[0].0, lan, "a non-loopback address wins");
    assert_eq!(l.found[0].1.slot_mask, 1, "the newest answer's description is kept");
    assert!(l.seen.contains_key(&lan) && !l.seen.contains_key(&lo));
    l.note_found(other, answer([192, 168, 0, 99], "DRIVER's race", 0), now);
    assert_eq!(l.rows().len(), 2);
    // a record that names no address cannot be merged with another source
    let mut a = answer([0, 0, 0, 0], "X", 0);
    a.from = nfs_net::wire::Addr::ip([0, 0, 0, 0], 9442);
    l.note_found("10.1.1.1:9442".parse().unwrap(), a.clone(), now);
    l.note_found("10.1.1.2:9442".parse().unwrap(), a, now);
    assert_eq!(l.rows().len(), 4);
}

/// The games list's fields come from the descriptor (0x4BA030 / 0x48C3D0): name and version
/// around the `~`, `desc+0x21` the mode, `desc+0x26` the track's name id.
#[test]
fn a_row_carries_the_descriptors_fields() {
    let mut l = quiet_lobby();
    l.note_found("192.168.0.82:9442".parse().unwrap(), answer([192, 168, 0, 82], "DRIVER's race", 3), Instant::now());
    let r = &l.rows()[0];
    assert_eq!((r.name.as_str(), r.version.as_str()), ("DRIVER's race", "3.5"));
    assert_eq!((r.mode, r.track_id, r.players), (1, 313, 3));
}

/// A host sends `(id, value)` when a mode-2 accessor changes, but not for the value it started
/// on; a player's option table takes what comes.
#[test]
fn a_host_sends_a_changed_track_option_and_a_player_takes_it() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    assert!(!host.sync_option(0xC, 300), "the starting value is not a selection");
    assert!(!host.sync_option(0xC, 300));
    assert!(host.sync_option(0xC, 313));
    assert!(!host.sync_option(0xC, 313));
    let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
    let m = nfs_net::options::message(&[0xC, 313, 0xF, 2]);
    p.hear(&m);
    assert_eq!(p.take_options(), vec![(0xC, 313), (0xF, 2)]);
    assert_eq!(p.options.get(&0xC), Some(&313));
    assert!(p.take_options().is_empty());
}

fn block(tick: i32) -> CarState {
    CarState { steer: 0, airborne: false, brake: 0, buttons: 0, tick: tick as u32, yaw_rate: 0.0, orientation: [0; 4], velocity: [0.0; 3], position: [0.0; 3] }
}

/// 0x4A0DF0: a block no newer than `+0x3C` is dropped; the lag is `|now − tick|` (at least 1) and the
/// smoothed lag `(7·old + lag) / 8`, truncated toward zero; the first block sets `+0xDAC`.
#[test]
fn the_store_keeps_the_lag_book_the_original_keeps() {
    let mut r = Remote::new(0);
    assert!(r.store(block(100), 101));
    assert!(r.got && r.lag == 1 && r.lag_avg == 0, "1 → (0·7 + 1) / 8 = 0");
    assert!(!r.store(block(100), 102), "not newer: dropped");
    assert!(!r.store(block(99), 102));
    assert_eq!(r.state.tick, 100);
    assert!(r.store(block(108), 108), "age 0 counts as lag 1");
    assert_eq!(r.lag, 1);
    assert!(r.store(block(200), 160), "a block from the future: |age| = 40");
    assert_eq!(r.lag, 40);
    assert_eq!(r.lag_avg, (7 * 0 + 40) / 8);
    assert!(r.store(block(300), 390));
    assert_eq!(r.lag_avg, (7 * 5 + 90) / 8);
}

/// 0x4A0E9F…: a lag above 0x40 counts `+0x64` and the fifth such block — or one above 0x80 — makes the
/// car stale with the snap 0x4A03D0 due once; nine blocks under 0x20 make it fresh again.
#[test]
fn a_car_goes_stale_after_five_late_blocks_and_recovers_after_nine_fresh_ones() {
    let mut r = Remote::new(0);
    r.store(block(1), 2);
    let mut t = 10;
    for k in 1..=4 {
        r.store(block(t), t + 70);
        assert!(!r.stale, "late block {k}");
        t += 8;
    }
    r.store(block(t), t + 70);
    assert!(r.stale && r.snap_due);
    r.snap_due = false;
    // a 64-tick lag is not late (`< 0x41`): the counter clears, the car stays stale
    t += 8;
    r.store(block(t), t + 64);
    assert!(r.stale && r.late == 0 && !r.snap_due);
    // one block above 0x80 goes stale at once, and snaps only if it was fresh
    let mut r = Remote::new(0);
    r.store(block(1), 2);
    r.store(block(10), 10 + 129);
    assert!(r.stale && r.snap_due);
    for k in 0..9 {
        assert!(r.stale, "{k}");
        r.store(block(200 + 8 * k), 200 + 8 * k + 5);
    }
    assert!(!r.stale, "more than 8 fresh blocks");
}

/// The lobby's ping (0x4905A0 / 0x490600 / the host's case 195): a player asks the host for a
/// player's ping once a second, with its own last round trip as the word; the host keeps that
/// word per sender and answers with the asked player's, −1 until it has reported.
#[test]
fn the_ping_exchange_reports_the_last_round_trip_and_hides_unknown_pings() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    let out = host.room.admit(1, b"PLAYER");
    host.dispatch(out);
    assert!(host.pings.contains_key(&0) && host.pings.contains_key(&1));
    assert_eq!(host.ping_shown(0), None, "created with −1");
    host.net148 = 123_456; // the value the link event left there: a large tick
    let t0 = 10_000_000;
    host.tick_override = Some(t0);
    host.ping_at(0, t0);
    // the first report is the large number: min(.., 9999)
    assert_eq!(host.ping_shown(0), Some(9999));
    assert_eq!(host.net148, 0, "0x490600: now − player+0x44, in process");
    // no second request inside the second, one after it (`1000 < now − +0x44`)
    host.pings.get_mut(&0).unwrap().ping = 5;
    host.tick_override = Some(t0 + 1000);
    host.ping_at(0, t0 + 1000);
    assert_eq!(host.ping_shown(0), Some(5));
    host.tick_override = Some(t0 + 1001);
    host.ping_at(0, t0 + 1001);
    assert_eq!(host.ping_shown(0), Some(0));
    // player 1 has reported nothing: −1, hidden
    host.tick_override = Some(t0 + 2000);
    host.ping_at(1, t0 + 2000);
    assert_eq!(host.ping_shown(1), None);
    // its request stores its word; the next answer about it carries it
    host.ping_request(1, room::Ping { id: 1, word: 37 });
    host.tick_override = Some(t0 + 4000);
    host.ping_at(1, t0 + 4000);
    assert_eq!(host.ping_shown(1), Some(37));
    // the wire shape: value = the asked id, word big-endian
    assert_eq!(room::Ping { id: 1, word: 37 }.encode(), vec![0xC3, 0, 0, 1, 0, 0, 0, 37]);
    assert_eq!(room::Ping::parse(&[0xC3, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF]), Some(room::Ping { id: 1, word: -1 }));
}

/// A player's slot records reach the table of whoever hears them (0x4B4B80's per-slot band), and
/// 0x5091B0 needs every player in the room ready.
#[test]
fn slot_records_fill_the_table_and_the_create_screens_gate_needs_everyone_ready() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    assert!(host.established());
    assert!(!host.everyone_ready(), "the host has not sent its ready yet");
    host.send_slot_records(&[nfs_net::slots::ready_record(0, true)]);
    assert!(host.everyone_ready());
    // a second player comes in and is not ready
    let out = host.room.admit(1, b"PLAYER");
    host.dispatch(out);
    assert!(!host.everyone_ready());
    let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
    for r in nfs_net::slots::car_records(1, 161, b"'97 Boxster", 0xFF11_2233) {
        p.hear(&nfs_net::options::message(&r));
    }
    assert_eq!((p.slots.slot[1].car_id, p.slots.slot[1].colour), (161, 0xFF11_2233));
    assert_eq!(p.slots.slot[1].car_name, b"'97 Boxster".to_vec());
    host.hear(&nfs_net::options::message(&nfs_net::slots::ready_record(1, true)));
    assert!(host.everyone_ready());
    // the race started for everyone: PLAYGAME is off again (`net+0xE5 != 0`)
    host.hear(&room::race_start(1));
    assert!(!host.everyone_ready());
}

/// **0x490140**: 3 for a player who is not on the list or a link that is gone, 4 for one that is
/// listed but not racing, and 0 / 1 / 2 by the local ping (× 0.001 against 0.3 / 1.0) or the packet
/// age of the others (ticks × 1/64 against the same), or `net+0x128` × 0.001 against 0.2 / 0.5
/// when the game is not positional. The bounds are strict compares (`fcom`, `test ah, 0x41`).
#[test]
fn the_link_classification_reads_the_ping_for_us_and_the_packet_age_for_the_others() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    let out = host.room.admit(1, b"PLAYER");
    host.dispatch(out);
    host.note_race_indices(&[(0, 0), (1, 1)]);
    host.in_race = 0b11;
    assert_eq!((host.local_race_index(), host.seat_race_index(0)), (Some(0), Some(1)));
    let st = |tick| CarState { steer: 0, airborne: false, brake: 0, buttons: 0, tick, yaw_rate: 0.0, orientation: [0; 4], velocity: [0.0; 3], position: [0.0; 3] };
    host.remotes.insert(1, Remote { state: st(1000), ..Remote::new(0) });
    // a car's packet age: 19 ticks = 0.297 s, 20 = 0.3125; 64 = 1.0 (not above it), 65 above
    assert_eq!(host.link_quality(1, 1019), LinkQuality::Good);
    assert_eq!(host.link_quality(1, 1020), LinkQuality::Fair);
    assert_eq!(host.link_quality(1, 1064), LinkQuality::Fair);
    assert_eq!(host.link_quality(1, 1065), LinkQuality::Poor);
    // a car that has sent nothing has `car+0x3C == 0`: its age is the whole game tick
    host.remotes.clear();
    assert_eq!(host.link_quality(1, 10), LinkQuality::Good);
    assert_eq!(host.link_quality(1, 100), LinkQuality::Poor);
    // our own car: the ping of the local player's record (0x4905A0), 0.3 s and 1.0 s
    host.tick_override = Some(10_000_000);
    host.pings.get_mut(&0).unwrap().pending = true;
    for (ping, want) in [(-1, LinkQuality::Good), (300, LinkQuality::Good), (301, LinkQuality::Fair), (1000, LinkQuality::Fair), (1001, LinkQuality::Poor)] {
        host.pings.get_mut(&0).unwrap().ping = ping;
        assert_eq!(host.link_quality(0, 5000), want, "ping {ping}");
    }
    // `Comms.Positional` off: `net+0x128` × 0.001 against 0.2 / 0.5
    host.positional = false;
    for (ms, want) in [(200, LinkQuality::Good), (201, LinkQuality::Fair), (500, LinkQuality::Fair), (501, LinkQuality::Poor)] {
        host.exch.sender.latency[1] = ms as u32;
        assert_eq!(host.link_quality(1, 5000), want, "{ms} ms");
    }
    host.positional = true;
    // listed, but its bit of `net+0xE5` is not set: white; gone from the room: black
    host.in_race = 0b01;
    assert_eq!(host.link_quality(1, 1), LinkQuality::NotRacing);
    assert!(LinkQuality::NotRacing.gone() && !LinkQuality::Poor.gone());
    host.hear(&room::player_out(1));
    assert_eq!(host.link_quality(1, 1), LinkQuality::NotListed);
    assert!(host.retired(1, 1), "0x416310 retires a racer above 2");
    assert!(!host.retired(0, 1), "we are still listed and racing");
    // the colour table 0x5B2D90
    assert_eq!(LinkQuality::COLOURS[LinkQuality::Poor as usize], 0xCCFF_0000);
    // a joiner whose link has dropped (`+0xBF`): 3 for everyone
    let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
    p.join = JoinState::Joined;
    p.net.role = Role::Client;
    p.state_in = Some(nfs_net::room::comm_state::CONNECTED);
    p.room.hear(&room::player_in(0, b"HOSTY"));
    p.room.hear(&room::player_in(2, b"PLAYER"));
    p.id = 2;
    p.note_race_indices(&[(0, 0), (2, 1)]);
    p.in_race = 0b101;
    assert_eq!(p.link_quality(1, 1), LinkQuality::Good);
    p.lost = true;
    assert_eq!(p.link_quality(1, 1), LinkQuality::NotListed);
}

/// `0xB7` and the three-way race protocol (`network.md` §11.15): a player's finish time reaches
/// every player under the sender's id and 0x48D770 / 0x48D7F0 answer it by race index; `−1` and
/// `−2` do not make a time; `−2` takes the sender's bit out of `net+0xE5` and, once every player
/// is back, the room's race is over.
#[test]
fn a_finish_time_reaches_the_room_and_the_return_frees_it() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    for id in [1, 2] {
        let out = host.room.admit(id, format!("P{id}").as_bytes());
        host.dispatch(out);
    }
    host.note_race_indices(&[(0, 0), (1, 1), (2, 2)]);
    // PLAYGAME's `0xB3` request on the host starts the race for the room and comes back to it
    host.request_start();
    assert_eq!(host.in_race, 0b111, "0x48F837 `+0xE5 = +0xE4`");
    assert!(host.go_wait, "0x48F861 `+0xC4 = 1`");
    assert!(host.room.race_running());
    assert!(!host.room.in_lobby_group(0));
    assert!(host.room.in_lobby_group(1), "player 1 has not asked yet");
    // a joiner's `0xB3` request only moves its bit
    host.serve_race_request(1, &room::start_request());
    assert!(!host.room.in_lobby_group(1));
    // the room's go is answered once
    let first = host.room.go_request(5);
    assert_eq!(first.len(), 1);
    assert!(host.room.go_request(6).is_empty());
    host.hear(&first[0].1);
    assert!(!host.go_wait, "0x48F8F5 `+0xC4 = 0` on the first 0xC5");
    // nobody has finished: no time, no rank
    assert_eq!((host.race_time(0), host.race_rank(0)), (-1, -1));
    // player 2 finishes in 4000 ticks with a best lap of 1900, the host in 4100
    host.serve_race_request(2, &room::race_time_request(4000, 1900));
    host.serve_race_request(0, &room::race_time_request(4100, 2000));
    assert_eq!((host.race_time(2), host.race_rank(2)), (4000, 1));
    assert_eq!((host.race_time(0), host.race_rank(0)), (4100, 2));
    assert_eq!((host.race_time(1), host.race_rank(1)), (-1, -1), "player 1 has sent none");
    // `−1` (leaving the game) is a value too, and stays −1; `−2` frees the bit
    host.serve_race_request(1, &room::race_time_request(-1, -1));
    assert_eq!(host.race_time(1), -1);
    host.serve_race_request(1, &room::race_time_request(-2, -1));
    assert_eq!(host.in_race, 0b101);
    assert!(host.room.race_running(), "the host and player 2 are still out");
    host.serve_race_request(0, &room::race_time_request(-2, -1));
    host.serve_race_request(2, &room::race_time_request(-2, -1));
    assert_eq!(host.in_race, 0);
    assert!(!host.room.race_running(), "everyone is back: +0xCC3 = 0");
    // a new launch clears the times (0x48D3D0)
    host.room.players.iter_mut().for_each(|p| p.time = -1);
    host.launch_begin();
    assert_eq!(host.race_rank(2), -1);
}

/// The launch's own-slot walk (0x4B4F25…0x4B4F66): a machine's records leave with its **race
/// index** as the slot, each machine keeps every player's record at that index and not in the
/// id-indexed lobby table.
#[test]
fn the_launch_resend_numbers_the_players_by_race_index_and_leaves_the_lobby_table_alone() {
    use nfs_net::launchcar::{id, OwnCar};
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    let out = host.room.admit(1, b"PLAYER");
    host.dispatch(out);
    // lobby time: player 1 picked a car by id (the lobby table is indexed by player id)
    for r in nfs_net::slots::car_records(1, 161, b"'97 Boxster", 0xFF11_2233) {
        host.hear(&nfs_net::options::message(&r));
    }
    host.hear(&room::race_start(1));
    assert_eq!(host.race_indices(), vec![(0, 0), (1, 1)]);
    let mine = OwnCar { filename: b"993".to_vec(), roof_accessor: 1, specname: b"993coupe".to_vec(), ..Default::default() };
    let sent = host.own_launch_records(&mine);
    assert!(sent.iter().all(|r| r[1] == 0), "the host's race index is 0");
    assert!(nfs_net::launchcar::well_formed(&sent));
    // the other machine's walk: its records at race index 1 come in as `0xBC`
    let theirs = OwnCar { filename: b"boxster".to_vec(), roof_accessor: 0, transmission: 1, ..Default::default() };
    let their_recs: Vec<Vec<u32>> = theirs.launch_list(1).into_iter().map(|mut r| { r[1] = 1; r }).collect();
    let mut out = nfs_net::options::Outgoing::new();
    let mut msgs: Vec<Vec<u8>> = their_recs.iter().flat_map(|r| out.push(r)).collect();
    msgs.extend(out.flush());
    assert!(msgs.len() > 1, "a whole record list goes in several messages (0xF0 rule)");
    for m in msgs {
        host.hear(&m);
    }
    let cars = host.remote_cars();
    assert_eq!(cars.len(), 1);
    assert_eq!((cars[0].0, cars[0].1), (1, 1));
    assert_eq!(cars[0].3.text(id::FILENAME).unwrap(), b"boxster".to_vec());
    assert_eq!(cars[0].3.num(id::CABRIODOWN), Some(0));
    assert_eq!(host.launch.cars[&0].text(id::FILENAME).unwrap(), b"993".to_vec());
    assert_eq!(host.launch.cars[&0].num(id::CABRIODOWN), Some(1));
    // the lobby table still holds what the lobby said, at the player id
    assert_eq!(host.slots.slot[1].car_id, 161);
    assert_eq!(host.slots.slot[1].car_name, b"'97 Boxster".to_vec());
}

/// `[net+0x24]` as the port maps it: a joiner is 3 until the host's `0xC6 (0xE)`.
#[test]
fn a_joiner_is_connecting_until_the_host_says_it_is_in() {
    use nfs_net::room::comm_state as cs;
    let mut p = NetPlay::start("host:0", "PLAYER").unwrap();
    assert_eq!(p.comm_state(), cs::HOSTING);
    p.join = JoinState::Pending(Instant::now());
    p.net.role = Role::Client;
    assert_eq!(p.comm_state(), cs::CONNECTING);
    p.join = JoinState::Joined;
    assert_eq!(p.comm_state(), cs::CONNECTING);
    p.hear(&room::state(cs::CONNECTED));
    assert_eq!(p.comm_state(), cs::CONNECTED);
    p.hear(&room::state(cs::EJECTED));
    assert_eq!(p.comm_state(), cs::EJECTED);
    p.join = JoinState::Refused(nfs_net::comm::JoinStatus::NameTaken);
    assert_eq!(p.comm_state(), cs::NAME_TAKEN);
    p.join = JoinState::Pending(Instant::now() - JOIN_TIMEOUT - Duration::from_secs(1));
    assert_eq!(p.comm_state(), cs::LOST, "a connect that runs out is the comm layer's event 5: state 4 (0x48FBB6), not 6");
}

/// What 0x427A60 posts and `HLeader` shows: the shipped `DashHud/pic16.fsh`, sent through the
/// room's chunks and decoded the way [`NetPlay::leader_picture`] decodes it — the first entry,
/// 54 × 64. Skipped without the game's files.
#[test]
fn the_shipped_picture_survives_the_chunks_and_decodes_at_54_by_64() {
    let root = std::env::var_os("NFS5_GAME_PATH").map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/NFS-Porsche-Unleashed/game_info/data"))
    });
    let Some(bytes) = root.and_then(|r| std::fs::read(r.join("GameData/DashHud/pic16.fsh")).ok()) else {
        return;
    };
    let mut a = Room::new(2);
    a.hear(&room::player_in(2, b"anna"));
    a.post(room::PICTURE, &bytes);
    let mut b = Room::new(0);
    b.hear(&room::player_in(2, b"anna"));
    a.tick(0);
    let mut t = 0;
    while a.sending() {
        t += 100;
        for m in a.tick(t) {
            b.hear(&m);
        }
    }
    let f = &b.player(2).unwrap().files[0];
    assert!(f.complete());
    let fsh = nfs_formats::fsh::Fsh::parse(&f.bytes).unwrap();
    let img = fsh.decode(&f.bytes, 0).unwrap();
    assert_eq!((img.width, img.height), (54, 64));
}

/// **The input exchange end to end, over the loopback**: a host and a player, `Comms.Positional`
/// off, each sending its own controller (a distinct sample every 1/32 s, one with a command
/// code); the room's frames come back to both, and every frame the two machines latch holds the
/// same sample for both cars — the property the simulation's agreement rests on (`network.md`
/// §11.17). Also: the round trip 0x490140's non-positional arm reads is measured, the command
/// reaches the *other* machine's copy of the car, and a racer whose sample carries command 1 is
/// "left" (`car+0xD8B == 1`, 0x4163B0).
#[test]
fn two_machines_latch_the_same_sample_for_both_cars() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    // (the identity gate is `comm::identity`'s own test's business)
    host.net.identity_gate = None;
    let port = host.net.local_addr().unwrap().port();
    let mut p = NetPlay::start(&format!("join:127.0.0.1:{port}"), "PLAYER").unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !(p.join == JoinState::Joined && p.established() && host.room_len() == 2 && p.room_len() == 2) {
        host.poll_lobby();
        p.poll_lobby();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        p.established() && host.room_len() == 2 && p.room_len() == 2,
        "the player is in the room: join {:?} established {} rooms {} / {}",
        p.join,
        p.established(),
        host.room_len(),
        p.room_len()
    );
    host.positional = false;
    p.positional = false;
    // PLAYGAME's `0xB3` request on every machine (0x48D370): the room starts the race and tells everyone
    host.request_start();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !p.take_start() {
        host.poll_lobby();
        p.poll_lobby();
    }
    p.request_start();
    for m in [&mut host, &mut p] {
        m.note_race_indices(&[(0, 0), (1, 1)]);
        assert!(m.exch.waiting, "0xB3 sets net+0xC4");
    }
    // the race frame's ring (after its barriers, which the launch's tests cover), then the `0xC5` round
    for m in [&mut host, &mut p] {
        m.exchange_race_frame();
        assert!(m.exchanging());
    }
    host.request_go();
    p.request_go();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && (host.exch.waiting || p.exch.waiting) {
        host.poll_lobby();
        p.poll_lobby();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!host.exch.waiting && !p.exch.waiting, "0xC5 (0, now) clears net+0xC4 on both");
    assert_eq!(host.exch.start_ms, p.exch.start_ms);
    // a second and a half of play: each side samples at the timer's 32 Hz (on a loaded machine
    // the loop runs on — up to six seconds — until the 40th sample's frames have come back)
    let end = Instant::now() + Duration::from_millis(1500);
    let long_end = Instant::now() + Duration::from_millis(6000);
    let mut n = 0u32;
    while Instant::now() < end || (Instant::now() < long_end && host.exch.received.min(p.exch.received) < 44) {
        for (who, m) in [(0u8, &mut host), (1u8, &mut p)] {
            for _ in 0..m.exchange_pump(false) {
                n += 1;
                // the command 5 (reset) on the player's 40th sample
                // (on samples 40…43, not on one alone: the samples are unreliable, and on a loaded machine
                // the one carrying the command can arrive after the room has built the frame it belongs to)
                let cmd = if who == 1 && (40..44).contains(&m.exch.sender.taken) { 5 << 3 } else { 0 };
                m.exchange_sample([cmd | who, 10 * (who + 1) + (n % 7) as u8, 100 + (n % 5) as u8, 20 * who]);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(host.exch.received >= 20 && p.exch.received >= 20, "frames came: {} / {}", host.exch.received, p.exch.received);
    // every frame both have: the latched samples agree, car by car
    // (`received` counts ring entries, two to a frame)
    let entries = host.exch.received.min(p.exch.received);
    let mut saw_reset = [false; 2];
    for f in 1..=entries {
        host.exchange_latch(f);
        p.exchange_latch(f);
        for car in 0..2 {
            assert_eq!(host.latched(car), p.latched(car), "frame {f}, car {car}");
            saw_reset[car] |= host.latched(car)[0] >> 3 == 5;
        }
    }
    assert!(saw_reset[1] && !saw_reset[0], "the player's reset command is in its car's samples on both machines, once");
    // the round trip was measured once (0x48F141: the first time our own bit comes back — the
    // frame is 1/16 s old at least), and is small here; it is not measured again
    let rtt = p.exch.sender.latency[1];
    assert!((1..2000).contains(&rtt), "{rtt} ms");
    // a racer whose sample carries command 1 has left (0x4163B0)
    assert!(!host.retired(1, 0) && !p.retired(0, 0));
    host.commands[1] = nfs_net::exchange::CMD_LEFT;
    assert!(host.retired(1, 0));
}

/// 0x4991E0's three arms for a remote car of a positional race: the local pad before the first block
/// (`car+0xDA8 == 0`, `car+0xDAC == 0`), the block's steering / brake / handbrake / horn after it, the AI
/// arm once the 64 Hz gate has held the car (`car+0xDA8 == 1`); and every reserved seat is the network's
/// from the first tick (a seat is not owned only once a block has come).
#[test]
fn a_remote_seat_is_driven_by_the_local_pad_until_its_first_block_then_by_the_block_then_by_the_ai() {
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    host.reserve(vec![0, 1]);
    assert_eq!(host.owned_seats(), vec![0, 1], "every reserved seat, blocks or not");
    assert!(matches!(host.seat_arm(0), SeatArm::Pad));
    assert!(SeatArm::Pad.horn(true) && !SeatArm::Pad.horn(false), "the local pad's horn");
    let st = |tick, buttons| CarState { steer: -40, airborne: false, brake: 90, buttons, tick, yaw_rate: 0.0, orientation: [0; 4], velocity: [0.0; 3], position: [0.0; 3] };
    let mut r = Remote::new(0);
    r.store(st(100, 0b11), 100);
    assert!(r.got);
    host.remotes.insert(1, r);
    match host.seat_arm(0) {
        SeatArm::Block { brake, steer, handbrake, horn } => assert_eq!((brake, steer, handbrake, horn), (90, -40, true, true)),
        other => panic!("{other:?}"),
    }
    assert!(host.seat_arm(0).horn(false), "the block's bit 1");
    // the apply held the car (`car+0xDA8 = 1`): the AI arm, the horn byte cleared (0x499279)
    host.remotes.get_mut(&1).unwrap().held = true;
    host.remotes.get_mut(&1).unwrap().stale = true;
    assert!(matches!(host.seat_arm(0), SeatArm::Ai { stale: true, .. }));
    assert!(!host.seat_arm(0).horn(true));
    // the other seat has no block yet
    assert!(matches!(host.seat_arm(1), SeatArm::Pad));
}

/// two joiners whose `'j'` the host answers at once (the live bug: two originals typed the host's
/// address in the same second). The room's message of the first joiner's admission (`To::All`, the `0xB0`) must
/// not reach the second one before its own CONMAN hello answer `0xFE 1`: in the original a player object exists
/// only after the hello (0x48F0C4 event 1, 0x55CED0), so 0x48C870's walk never reaches that peer. Raised over
/// loopback with two bare `Net` clients standing in for the originals (no game, no window).
#[test]
fn the_hosts_room_broadcast_skips_a_peer_that_has_not_said_hello() {
    use nfs_net::session::Event;
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    let to = SocketAddr::from(([127, 0, 0, 1], host.net.local_addr().unwrap().port()));
    let client = |who: &str| {
        let sock = CommSocket::bind_on(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let mut n = with_probe(Net::client(sock, key(), name16(who)));
        n.join(to, &join_identity(who)).unwrap();
        n
    };
    let (mut a, mut b) = (client("AAAA"), client("BBBB"));
    let until = Instant::now() + Duration::from_secs(5);
    // the host answers both `'j'`: two slots in `hello_wait`, neither has said hello
    while host.hello_wait.len() < 2 && Instant::now() < until {
        host.poll(0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.hello_wait.len(), 2, "both joiners were taken");
    // A hears its `'a'` and sends the hello; B stays silent
    let hello = |n: &mut Net| n.send_game(&[0xFF, 0x00, b'P', 0]).unwrap();
    let mut got_a = Vec::new();
    let mut got_b = Vec::new();
    let mut a_hello = false;
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !(a_hello && got_a.len() >= 2) {
        if !a_hello && a.poll().iter().any(|e| matches!(e, Event::Joined { .. })) {
            hello(&mut a);
            a_hello = true;
        }
        host.poll(0);
        got_a.extend(a.poll().into_iter().filter(|e| matches!(e, Event::Data { .. })));
        got_b.extend(b.poll().into_iter().filter(|e| matches!(e, Event::Data { .. })));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(a_hello && !got_a.is_empty(), "A, which said hello, is told the room");
    assert!(got_b.is_empty(), "B has not said hello: the room's message must not reach it first, got {got_b:?}");
    // B's hello: the host answers `0xFE 1` first of all
    hello(&mut b);
    let until = Instant::now() + Duration::from_secs(5);
    let mut first = None;
    while first.is_none() && Instant::now() < until {
        host.poll(0);
        first = b.poll().into_iter().find_map(|e| if let Event::Data { bytes, .. } = e { Some(bytes) } else { None });
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(first.as_deref().and_then(|m| m.get(..2)), Some(&[0xFE, 1][..]), "the first data frame of B is the link message 0xFE 1");
}

/// the room's description `0xB4` after a change of the room's count goes to **every** player (0x490B70 walks the
/// manager's list `mgr+0x164`, 0x48BAA0 for each), not to the newcomer alone: an earlier joiner hears the newcomer's `0xB0` and
/// then `0xB4` with the count + 1 (the original host's capture runB: `b0 000002`, `b4 … count 3` to the first joiner), and after a
/// player leaves the room (0x48BDA0: `0xB1`, count − 1, 0x490B70) the `0xB4` with the count − 1. Loopback, two bare `Net` clients.
#[test]
fn the_rooms_description_goes_to_every_player_after_each_change_of_the_count() {
    use nfs_net::session::Event;
    let mut host = NetPlay::start("host:0", "HOSTY").unwrap();
    let to = SocketAddr::from(([127, 0, 0, 1], host.net.local_addr().unwrap().port()));
    let client = |who: &str| {
        let sock = CommSocket::bind_on(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let mut n = with_probe(Net::client(sock, key(), name16(who)));
        n.join(to, &join_identity(who)).unwrap();
        n
    };
    let hello = |n: &mut Net| n.send_game(&[0xFF, 0x00, b'P', 0]).unwrap();
    let data = |n: &mut Net| -> Vec<Vec<u8>> {
        n.poll().into_iter().filter_map(|e| if let Event::Data { bytes, .. } = e { Some(bytes) } else { None }).collect()
    };
    // One turn of a joining client (a bare `Net` has no retry of its own). Every event of the poll is looked at —
    // the one that says "joined" must not swallow the data frames that came with it — and a `'j'` that got no
    // answer is sent again once a second, as `NetPlay::poll` does for the game (`join_wait`): the host answers a
    // repeated one with the slot the sender already has (0x588F41). Under a loaded machine (a parallel
    // `cargo test --workspace`) the first datagram and its answer are not always on time inside the 5 s allowed.
    let turn = |n: &mut Net, who: &str, said: &mut bool, last_join: &mut Instant, got: &mut Vec<Vec<u8>>| {
        if !*said && last_join.elapsed() >= Duration::from_secs(1) {
            n.join(to, &join_identity(who)).unwrap();
            *last_join = Instant::now();
        }
        for e in n.poll() {
            match e {
                Event::Joined { .. } if !*said => {
                    hello(n);
                    *said = true;
                }
                Event::Data { bytes, .. } => got.push(bytes),
                _ => {}
            }
        }
    };
    let count = |m: &[u8]| (m.first() == Some(&0xB4) && m.len() == 0x50).then(|| u32::from_le_bytes([m[0x4C], m[0x4D], m[0x4E], m[0x4F]]));
    // A joins and is admitted (its admission ends with `0xC6 (0xE)`)
    let mut a = client("AAAA");
    let (mut a_hello, mut got_a, mut a_join) = (false, Vec::new(), Instant::now());
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !got_a.iter().any(|m: &Vec<u8>| m.first() == Some(&0xC6)) {
        host.poll(0);
        turn(&mut a, "AAAA", &mut a_hello, &mut a_join, &mut got_a);
        std::thread::sleep(Duration::from_millis(5));
    }
    let a_counts: Vec<u32> = got_a.iter().filter_map(|m| count(m)).collect();
    assert_eq!(a_counts, vec![1, 2], "A's own admission: the count before it, then after it");
    // B joins: A hears B's `0xB0` and then the `0xB4` with three players
    let mut b = client("BBBB");
    let (mut b_hello, mut got_b, mut more_a, mut b_join) = (false, Vec::new(), Vec::new(), Instant::now());
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !(got_b.iter().any(|m: &Vec<u8>| m.first() == Some(&0xC6)) && more_a.iter().any(|m: &Vec<u8>| count(m) == Some(3))) {
        host.poll(0);
        turn(&mut b, "BBBB", &mut b_hello, &mut b_join, &mut got_b);
        more_a.extend(data(&mut a));
        std::thread::sleep(Duration::from_millis(5));
    }
    let b_counts: Vec<u32> = got_b.iter().filter_map(|m| count(m)).collect();
    assert_eq!(b_counts, vec![2, 3], "B's admission: the newcomer hears both");
    let b0 = more_a.iter().position(|m| m.first() == Some(&0xB0) && m.get(1..4) == Some(&[0, 0, 2][..]));
    let b4 = more_a.iter().position(|m| count(m) == Some(3));
    assert!(matches!((b0, b4), (Some(x), Some(y)) if x < y), "A: B's 0xB0, then 0xB4 count 3; got {more_a:?}");
    // B leaves: A hears `0xB1 (2)` and then the `0xB4` with two players
    b.leave().unwrap();
    let mut after = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && !after.iter().any(|m: &Vec<u8>| count(m) == Some(2)) {
        host.poll(0);
        after.extend(data(&mut a));
        std::thread::sleep(Duration::from_millis(5));
    }
    let b1 = after.iter().position(|m| m.first() == Some(&0xB1) && m.get(1..4) == Some(&[0, 0, 2][..]));
    let b4 = after.iter().position(|m| count(m) == Some(2));
    assert!(matches!((b1, b4), (Some(x), Some(y)) if x < y), "A: 0xB1 (2), then 0xB4 count 2; got {after:?}");
}

/// D19: the version 0x48DBE0 keeps — the file, else the registry's `Client Version`, else `"0.0"` — and the port's
/// `"3.5"` only when there is no registry export at all (a player's `game/` folder).
#[test]
fn the_version_falls_back_to_the_shipped_one_only_without_a_registry() {
    use nfs_formats::regfile::{RegFile, GAME_KEY};
    let reg = RegFile::parse(format!("Windows Registry Editor Version 5.00\r\n\r\n[{GAME_KEY}]\r\n\"Client Version\"=\"0.0\"\r\n").as_bytes());
    assert!(!reg.is_empty());
    assert_eq!(super::version_from(Some("3.5".into()), Some(&reg)), "3.5");
    assert_eq!(super::version_from(Some("3.5".into()), None), "3.5");
    assert_eq!(super::version_from(None, Some(&reg)), "0.0");
    assert_eq!(super::version_from(None, Some(&RegFile::default())), "3.5");
    assert_eq!(super::version_from(None, None), "3.5");
}
