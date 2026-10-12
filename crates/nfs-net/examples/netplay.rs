//! Two processes, one session, real UDP — the protocol of `RE/notes/network.md` §7 end to end.
//!
//!   cargo run -p nfs-net --example netplay -- host [port]
//!   cargo run -p nfs-net --example netplay -- join <host:port> [name]
//!
//! The host binds, answers `'w'` probes with `'d'`, admits `'j'` with `'a'` and pings its peers;
//! the client discovers, joins, and both then broadcast the 0xBE car state on the original's gate —
//! once every eight ticks of a 64 Hz clock (0x48EE20), which is 8 Hz. Each side prints what it
//! receives so the two transcripts can be compared.

use nfs_net::session::{name16, name32, Event, Net, Role};
use nfs_net::socket::CommSocket;
use nfs_net::state::{CarState, TICK_MASK};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

/// The application GUID at 0x5B4980, zero-padded — see `open_unleashed::netplay` and §7.16.
const KEY: [u8; 32] = [
    0x81, 0x6B, 0xD4, 0x5C, 0xE4, 0xEC, 0xD3, 0x11, 0x9B, 0x6A, 0x00, 0x90, 0x27, 0x24, 0xAB, 0x1E,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("host");

    let mut net = match mode {
        "host" => {
            let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(nfs_net::PORT);
            let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)))
                .unwrap_or_else(|e| panic!("cannot bind {port}: {e}"));
            let mut n = Net::host(sock, name32("Zone Industrial"), KEY, 8);
            n.my_name = name16("host");
            println!("host  listening on {}", n.local_addr().unwrap());
            n
        }
        "join" => {
            let host: SocketAddr = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, nfs_net::PORT)));
            let who = args.get(2).cloned().unwrap_or_else(|| "racer".into());
            let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap();
            let mut n = Net::client(sock, KEY, name16(&who));
            println!("client {} on {} -> {host}", who, n.local_addr().unwrap());
            n.probe_at(host).unwrap();
            n.join(host, b"car=911 turbo").unwrap();
            n
        }
        other => {
            eprintln!("usage: netplay host [port] | netplay join <host:port> [name]");
            eprintln!("(got {other:?})");
            std::process::exit(2);
        }
    };

    let mut id = if net.role == Role::Host { 0u8 } else { 1u8 };
    let mut tick: u64 = 0;
    let started = Instant::now();
    let mut seen = 0usize;

    // the original's clock: 64 Hz, and the state goes out when (tick & 7) == 7
    while started.elapsed() < Duration::from_secs(6) {
        for e in net.poll() {
            match e {
                Event::Found { from, describe } => {
                    let name = String::from_utf8_lossy(&describe.name);
                    println!("  found \"{}\" at {from}: {} slots, mask {:#b}", name.trim_end_matches('\0'), describe.player_limit, describe.slot_mask);
                }
                Event::Joined { slot } => {
                    id = slot as u8 + 1;
                    println!("  joined at slot {slot}, so our player id is {id}");
                }
                Event::Refused { status } => println!("  refused: {status:?}"),
                Event::PeerJoined { slot, from } => println!("  slot {slot} taken by {from}"),
                Event::PeerLeft { slot } => println!("  slot {slot} left"),
                Event::Quit { from } => println!("  {from} quit"),
                Event::Data { from, slot, bytes } => {
                    seen += 1;
                    match CarState::from_packet(&bytes, id) {
                        Some((who, st)) => {
                            if seen % 8 == 1 {
                                println!(
                                    "  car {who} from {from} (slot {slot:?}): pos {:.1?} vel {:.1?} tick {}",
                                    st.position, st.velocity, st.tick
                                );
                            }
                        }
                        None => println!("  {} bytes from {from} that are not a car state", bytes.len()),
                    }
                }
            }
        }
        net.tick();

        if tick & TICK_MASK == TICK_MASK {
            let t = tick as f32 / 64.0;
            let st = CarState {
                steer: 0,
                airborne: false,
                brake: 0,
                buttons: 0,
                tick: tick as u32,
                yaw_rate: 0.0,
                orientation: [0, 0, 0, 16384],
                velocity: [30.0 + id as f32, 0.0, 0.0],
                position: [t * 30.0, 0.5, id as f32 * 4.0],
            };
            let _ = net.send_game(&st.packet(id));
        }

        tick += 1;
        std::thread::sleep(Duration::from_micros(15_625)); // 64 Hz
    }

    let _ = net.leave();
    println!("done: {seen} car-state datagrams received, mask {:#b}", net.mask);
}
