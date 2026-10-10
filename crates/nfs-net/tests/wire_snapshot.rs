//! A scripted three-node session (a host and two clients, over loopback UDP) whose outgoing
//! datagrams are pinned byte for byte: the guard for refactors of `session.rs` (the delivery paths,
//! the relay, the `conns` map).
//!
//! The datagrams are taken from the socket's own `NFS_NET_DUMP` file (`S` lines), grouped per
//! `(sender, receiver)` stream — the order *between* streams is not pinned (the data layer's pump
//! walks a map), the order *inside* each stream is (an immediate repeat of the same frame is a retransmission racing
//! its ack and is folded). Ephemeral ports are renamed `H`, `A`, `B`, in
//! the address column and inside the payload hex (the join record and the descriptors carry them).
//! `UPDATE_WIRE_SNAPSHOT=1` rewrites `tests/snapshots/three_node.txt`.

use nfs_net::session::{name16, name32, Event, Net};
use nfs_net::socket::CommSocket;
use nfs_net::state::CarState;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

const KEY: [u8; 32] = [0xA5; 32];

fn loopback() -> CommSocket {
    CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap()
}

/// Poll everybody, running the pumps the game's 32 Hz loop runs, until the wire has been quiet for a
/// while (the dump file stopped growing) — so a slow machine only makes the script slower, not
/// different.
fn settle(dump: &std::path::Path, nodes: &mut [&mut Net], events: &mut [Vec<Event>]) {
    let size = || std::fs::metadata(dump).map(|m| m.len()).unwrap_or(0);
    let (mut last, mut quiet) = (size(), 0);
    for _ in 0..2000 {
        for (n, ev) in nodes.iter_mut().zip(events.iter_mut()) {
            ev.extend(n.poll());
            n.pump_outbox();
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        let now = size();
        if now == last {
            quiet += 1;
            if quiet >= 8 {
                return;
            }
        } else {
            (last, quiet) = (now, 0);
        }
    }
}

fn body(kind: u8, value: u32, rest: &[u8]) -> Vec<u8> {
    let mut v = vec![kind, (value >> 16) as u8, (value >> 8) as u8, value as u8];
    v.extend_from_slice(rest);
    v
}

fn run_script(dump: &std::path::Path) -> [u16; 3] {
    // SAFETY: this file holds one test, so nothing else touches the environment meanwhile.
    unsafe { std::env::set_var("NFS_NET_DUMP", dump) };
    let mut h = Net::host(loopback(), name32("Snapshot Room"), KEY, 4);
    h.my_name = name16("host");
    let mut a = Net::client(loopback(), KEY, name16("alpha"));
    let mut b = Net::client(loopback(), KEY, name16("beta"));
    let ports = [h.local_addr().unwrap().port(), a.local_addr().unwrap().port(), b.local_addr().unwrap().port()];
    let host_at = h.local_addr().unwrap();
    let mut ev = [Vec::new(), Vec::new(), Vec::new()];

    // discovery and joins
    a.probe_at(host_at).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    a.join(host_at, b"alpha~3.5~3.5").unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    b.join(host_at, b"beta~3.5~3.5").unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    assert!(a.slot.is_some() && b.slot.is_some(), "both clients joined: {ev:?}");
    h.peer_in(0);
    h.peer_in(1);
    let (a_at, b_at) = (a.local_addr().unwrap(), b.local_addr().unwrap());

    // the unreliable 0xBE state: client -> host -> relayed to the other client; host -> everybody
    let st = CarState { steer: -3, airborne: true, brake: 9, buttons: 1, tick: 4096, yaw_rate: 0.125, orientation: [100, -200, 300, 16384], velocity: [12.5, -0.25, 40.0], position: [1234.5, 6.25, -987.75] };
    a.send_game(&st.packet(1)).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    h.send_game(&st.packet(0)).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);

    // reliable client -> host -> relay: the barrier (echoed to the sender), a file chunk (not), the
    // room heartbeat (unreliable); then a chat request the host acts on itself
    a.send_game(&body(0xBD, 1, &[0])).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    b.send_game(&body(0xBF, 2, &[1, 2, 3, 4, 5])).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    b.send_game(&body(0xC4, 2, &[0, 0, 0, 7])).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    a.send_game(&body(0xBB, 1, b"hello")).unwrap();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);

    // the host's own sends: reliable to one peer, reliable to both (the second waits for the pump),
    // an unreliable one to both, and one to an address that is no peer (nothing goes)
    h.send_game_to(&[a_at], &body(0xB3, 0, &[0, 0, 1, 0])).unwrap();
    h.send_game_to(&[a_at, b_at], &body(0xC5, 0, &[0, 0, 2, 0])).unwrap();
    h.send_game_to(&[a_at, b_at], &body(0xC4, 0, &[0, 0, 3, 0])).unwrap();
    h.send_game_to(&["127.0.0.1:9".parse().unwrap()], &body(0xC5, 0, &[0])).unwrap();
    h.pump_peers();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    h.pump_peers();
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);

    // the keep-alive rounds
    for _ in 0..3 {
        h.tick();
        a.tick();
        b.tick();
    }
    settle(dump, &mut [&mut h, &mut a, &mut b], &mut ev);
    ports
}

/// `S` lines of the dump -> sorted per-stream text, ports renamed.
fn normalise(dump: &str, ports: [u16; 3]) -> String {
    let names = ["H", "A", "B"];
    let name_of = |p: u16| ports.iter().position(|&q| q == p).map(|i| names[i]).unwrap_or("?");
    let mut streams: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for line in dump.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        if f.len() != 5 || f[1] != "S" {
            continue;
        }
        let src = name_of(f[2].parse().unwrap());
        let dst_port: u16 = f[3].rsplit(':').next().unwrap().parse().unwrap();
        let dst = if f[3].starts_with("127.0.0.1:") { name_of(dst_port).to_string() } else { f[3].to_string() };
        let mut hex = f[4].to_string();
        // an address record is `02 00 <port BE> <ip>`; match that whole shape so a port number cannot
        // match payload bytes by accident
        for (i, p) in ports.iter().enumerate() {
            hex = hex.replace(&format!("0200{:02x}{:02x}7f000001", p >> 8, p & 0xFF), &format!("0200<{}>7f000001", names[i]));
        }
        let stream = streams.entry((src.to_string(), dst)).or_default();
        // a frame sent twice in a row is a retransmission that raced its ack: timing, not content
        if stream.last() != Some(&hex) {
            stream.push(hex);
        }
    }
    let mut out = String::new();
    for ((s, d), v) in streams {
        out.push_str(&format!("{s} -> {d}\n"));
        for h in v {
            out.push_str(&format!("  {h}\n"));
        }
    }
    out
}

#[test]
fn the_three_node_session_sends_the_same_datagrams_per_stream() {
    let dump = std::env::temp_dir().join(format!("nfs_wire_snapshot_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&dump);
    let ports = run_script(&dump);
    let got = normalise(&std::fs::read_to_string(&dump).unwrap(), ports);
    let _ = std::fs::remove_file(&dump);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/three_node.txt");
    if std::env::var_os("UPDATE_WIRE_SNAPSHOT").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("tests/snapshots/three_node.txt (UPDATE_WIRE_SNAPSHOT=1 writes it)");
    if got != want {
        let diff: Vec<String> = got.lines().zip(want.lines()).enumerate().filter(|(_, (g, w))| g != w).map(|(i, (g, w))| format!("line {i}:\n  got  {g}\n  want {w}")).collect();
        let extra: Vec<&str> = got.lines().filter(|l| !want.lines().any(|w| w == *l)).collect();
        panic!("the datagrams changed ({} vs {} lines), lines only in the new run: {extra:?}\n{}", got.lines().count(), want.lines().count(), diff.join("\n"));
    }
}
