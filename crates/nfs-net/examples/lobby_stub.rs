//! **TEST GLUE** — the scripted lobby stub (`nfs_net::stub`) as a process, for scripted front-end runs and manual checks:
//!
//! ```text
//! lobby_stub <port> [name]      # listens on UDP <port> (0 = any; the chosen port is printed), lists itself as 127.0.0.1.<port>.<name>
//! ```
//!
//! It seeds two bots ("Ann" and "Bob", who greet every newcomer) and two games so the lists have rows. Logins: account `bad`,
//! persona `taken`, account `full` are refused as `nfs_net::stub` documents. Stops when stdin closes or after 10 minutes.
use nfs_net::lobby::GameDesc;
use nfs_net::stub::LobbyStub;
use std::time::{Duration, Instant};

fn main() {
    let mut a = std::env::args().skip(1);
    let port: u16 = a.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let name = a.next().unwrap_or_else(|| "Stub Lobby".into());
    let mut stub = LobbyStub::bind(port, &name).expect("bind");
    stub.seed_bot("Ann", "welcome to the lobby");
    stub.seed_bot("Bob", "anyone for a race?");
    let game = |n: &str, track, laps| GameDesc { name: n.into(), version: "3.5".into(), positional: 1, laps, direction: 410, track, limit: 8 };
    stub.seed_game(game("Alps duel", 0, 3), 2);
    stub.seed_game(game("Night run", 1, 5), 7);
    println!("lobby_stub listening on {} ({})", stub.port(), stub.row);
    let end = Instant::now() + Duration::from_secs(600);
    while Instant::now() < end {
        stub.step();
        std::thread::sleep(Duration::from_millis(1));
    }
}
