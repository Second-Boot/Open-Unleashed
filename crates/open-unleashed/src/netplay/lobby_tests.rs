//! **TEST GLUE** — the internet lobby client against the scripted stub host of `nfs_net::stub` (which plays the entry server and the lobby
//! server of `lobby.md` §11): login, the refusals, the server list, ENTER, the games list, chat, CREATE, JOIN, the way back.

use super::*;
use nfs_net::stub::LobbyStub;

fn stub() -> LobbyStub {
    let mut s = LobbyStub::bind(0, "Stub Lobby").unwrap();
    s.seed_bot("Ann", "welcome");
    s.seed_game(nfs_net::lobby::GameDesc { name: "Alps duel".into(), version: "3.5".into(), positional: 1, laps: 3, direction: 410, track: 0, limit: 8 }, 2);
    s
}

fn client(s: &LobbyStub, persona: &str, account: &str) -> NetPlay {
    let t: SocketAddr = format!("127.0.0.1:{}", s.port()).parse().unwrap();
    NetPlay::start_lobby(Some(t), persona, account, "pw").unwrap()
}

/// Pump the stub and every client until `done` holds (10 s at most).
fn pump(s: &mut LobbyStub, cs: &mut [&mut NetPlay], mut done: impl FnMut(&[&mut NetPlay]) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        s.step();
        for c in cs.iter_mut() {
            c.poll_lobby();
        }
        if done(cs) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

#[test]
fn the_logins_end_in_the_servers_states() {
    use nfs_net::room::comm_state as cs;
    for (persona, account, want) in [("racer", "ann", cs::CONNECTED), ("racer", "bad", cs::BAD_LOGIN), ("taken", "ann", cs::NAME_TAKEN), ("racer", "full", cs::FULL)] {
        let mut s = stub();
        let mut c = client(&s, persona, account);
        assert!(pump(&mut s, &mut [&mut c], |c| c[0].comm_state() == want), "{account}/{persona}: state {}", c.comm_state());
        // the 'j' record carried the credentials as the original builds them (c<account>~<password>, persona~3.5~version)
        assert_eq!(s.logins.last().unwrap(), &(account.to_string(), "pw".to_string(), persona.to_string(), game_version()));
    }
}

#[test]
fn the_entry_server_lists_servers_and_enter_leads_to_the_lobby_with_games_players_and_chat() {
    let mut s = stub();
    let mut c = client(&s, "racer", "ann");
    // the entry link: B8 rows, then 0xC6 0xE; the row shows once its ping is known (echo, or the link's own time)
    assert!(pump(&mut s, &mut [&mut c], |c| c[0].established() && !c[0].server_rows().is_empty()));
    let rows = c.server_rows();
    assert_eq!(rows[0].1.name, "Stub Lobby");
    let target = c.selected_server_target().unwrap();
    assert_eq!(target.port(), s.port());
    // ENTER: a new link to the row's address
    c.leave();
    drop(c);
    let mut c = client(&s, "racer2", "ann");
    c.chat_box(true);
    assert!(pump(&mut s, &mut [&mut c], |c| c[0].established() && c[0].room.players.len() >= 2 && !c[0].game_rows().is_empty() && !c[0].chat_rows("", "", "").is_empty()));
    let names: Vec<String> = c.players("").into_iter().map(|p| p.0).filter(|n| !n.is_empty()).collect();
    assert!(names.contains(&"Ann".to_string()) && names.contains(&"racer2".to_string()), "{names:?}");
    // no 0xB2 in the lobby: no id yet
    assert_eq!(c.my_player_id(), -1);
    let g = &c.game_rows()[0];
    assert_eq!((g.name.as_str(), g.players, g.version.as_str()), ("Alps duel", 2, "3.5"));
    assert_eq!(c.chat_rows("", "", "")[0], "Ann: welcome");
    // chat to everybody reaches the lobby, the sender included
    c.say("hello lobby", "");
    assert!(pump(&mut s, &mut [&mut c], |c| c[0].chat_rows("", "", "").iter().any(|l| l.ends_with("racer2: hello lobby"))));
}

#[test]
fn create_join_and_the_way_back() {
    let mut s = stub();
    let mut a = client(&s, "alice", "x");
    let mut b = client(&s, "bobby", "y");
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c.iter().all(|n| n.established() && n.room.players.len() >= 3)));
    // CREATE: 0xB4 id 0; the server moves the creator in as the first player
    assert!(a.create_game("Ann's race", 1));
    assert!(a.game_pending());
    let ok = pump(&mut s, &mut [&mut a, &mut b], |c| c[0].has_authority() && c[0].room.players.len() == 1 && c[1].game_rows().len() == 2);
    assert!(ok, "a: me {} est {} state {} / b: games {:?}", a.my_player_id(), a.established(), a.comm_state(), b.game_rows().iter().map(|r| (r.addr.clone(), r.name.clone(), r.players)).collect::<Vec<_>>());
    assert!(a.has_authority());
    assert!(!b.has_authority());
    // the game appears for the lobby member, with the creator counted
    let row = b.game_rows().into_iter().find(|r| r.name == "Ann's race").unwrap();
    assert_eq!(row.players, 1);
    // the pre-checks: a full or foreign game is refused client side; a good one is asked for
    let at = b.game_rows().iter().position(|r| r.name == "Ann's race").unwrap();
    assert_eq!(b.join_game(at), Ok(true));
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c[1].my_player_id() == 1 && c[0].room.players.len() == 2 && c[1].room.players.len() == 2));
    assert!(!b.has_authority());
    // the room's names, in the creator's order
    let names: Vec<String> = b.players("").into_iter().map(|p| p.0).filter(|n| !n.is_empty()).collect();
    assert_eq!(names, ["alice", "bobby"]);
    // the joiner goes back: 0xBA, the link stays, the lobby lists come again
    b.back_to_lobby();
    assert_eq!(b.my_player_id(), -1);
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c[1].game_rows().iter().any(|r| r.name == "Ann's race" && r.players == 1) && c[0].room.players.len() == 1 && c[1].established()));
    // a game the pre-check refuses never leaves the client
    let mut l = nfs_net::lobby::Lists::default();
    l.apply(&nfs_net::lobby::GameDesc { name: "Old".into(), version: "3.4".into(), positional: 0, laps: 0, direction: 0, track: 0, limit: 8 }.message(5, 1));
    assert_eq!(l.games[0].joinable(&game_version()), Err(nfs_net::lobby::JoinRefusal::WrongVersion));
}



fn nm(p: &nfs_net::room::Player) -> String {
    String::from_utf8_lossy(&p.name).trim_end_matches('\0').to_string()
}

/// Two clients in one game of the stub (`a` created it, `b` joined).
fn room_of_two(s: &mut LobbyStub) -> (NetPlay, NetPlay) {
    let mut a = client(s, "alice", "x");
    let mut b = client(s, "bobby", "y");
    assert!(pump(s, &mut [&mut a, &mut b], |c| c.iter().all(|n| n.established() && n.room.players.len() >= 3)));
    assert!(a.create_game("Ann's race", 1));
    assert!(pump(s, &mut [&mut a, &mut b], |c| c[0].has_authority() && c[1].game_rows().iter().any(|r| r.name == "Ann's race")));
    let at = b.game_rows().iter().position(|r| r.name == "Ann's race").unwrap();
    assert_eq!(b.join_game(at), Ok(true));
    assert!(pump(s, &mut [&mut a, &mut b], |c| c[1].my_player_id() == 1 && c[0].room.players.len() == 2 && c[1].room.players.len() == 2));
    (a, b)
}

/// EJECTPLAYER on a lobby link is the `0xB1 (id)` of 0x48D8A0 sent to the server, not a message to a peer of our own: the ejected
/// player hears `0xC6 9`, is back in the lobby (`0xB2 −2`) with the lobby's players and none of its old room row, the creator's room
/// shrinks. A joiner's request, one naming the creator itself and one naming nobody do nothing.
#[test]
fn the_creator_ejects_a_player_over_the_lobby_link() {
    use nfs_net::room::comm_state as cs;
    let mut s = stub();
    let (mut a, mut b) = room_of_two(&mut s);
    assert!(!b.eject(0), "a joiner has no authority");
    assert!(!a.eject(0), "the creator does not eject itself");
    assert!(a.eject(1));
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c[1].comm_state() == cs::EJECTED && c[1].my_player_id() == -1 && c[0].room.players.len() == 1));
    // the lobby's players come after the ejection and the ejected player's own room row (id 1) is not among them
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c[1].room.players.iter().any(|p| nm(p) == "bobby")));
    assert!(b.room.players.iter().filter(|p| nm(p) == "bobby").count() == 1, "one row of the ejected player: {:?}", b.room.players.iter().map(|p| (p.id, nm(p))).collect::<Vec<_>>());
    assert!(a.has_authority() && a.room.players.len() == 1);
    // the dialog's back (0xBA while out of the room) leaves the lobby's list as it is
    b.back_to_lobby();
    assert!(b.room.players.iter().any(|p| nm(p) == "bobby"));
}

/// The creator leaves (0xBA): the server hands the room to the next player, who is then the first record of the list — 0x48CAA0,
/// which the join screen's tick turns into `creategame` (`App::net_lobby_step`).
#[test]
fn the_creator_leaving_makes_the_joiner_the_first_record() {
    let mut s = stub();
    let (mut a, mut b) = room_of_two(&mut s);
    assert!(!b.first_player_is_us());
    a.back_to_lobby();
    assert!(pump(&mut s, &mut [&mut a, &mut b], |c| c[1].room.players.len() == 1 && c[1].first_player_is_us()));
    assert!(b.has_authority() && b.my_player_id() == 1);
}

/// The server's own refusals of a JOIN (0x4914F4: the identity's version against the game's `0xC6 0xA`) reach a client that did not
/// pre-check (the client's 0x4B9E80 would have stopped it): the state is the notice's, the link stays up and the client is still in the
/// lobby without an id (`lobby.md` §7.3). The stub's seeded game has version 3.4, the client's identity says 3.5.
#[test]
fn the_servers_version_refusal_of_a_join_keeps_the_client_in_the_lobby() {
    use nfs_net::room::comm_state as cs;
    let mut s = stub();
    s.seed_game(nfs_net::lobby::GameDesc { name: "Old".into(), version: "3.4".into(), positional: 0, laps: 0, direction: 0, track: 0, limit: 8 }, 1);
    let mut b = client(&s, "bobby", "y");
    assert!(pump(&mut s, &mut [&mut b], |c| c[0].established() && c[0].game_rows().iter().any(|r| r.name == "Old")));
    let old = b.game_rows().into_iter().find(|r| r.name == "Old").unwrap();
    assert_eq!(b.game_rows().iter().position(|r| r.name == "Old").map(|i| b.join_game(i)), Some(Err(nfs_net::lobby::JoinRefusal::WrongVersion)));
    // the request itself, as a client without the pre-check would send it
    let id = b.internet().unwrap().lists.games.iter().find(|g| g.desc.name == "Old").unwrap().id;
    b.net.send_game(&nfs_net::lobby::join_request(id)).unwrap();
    // 0x48E3A7 sets `net+0xC6`: a refusal then leaves the link up (0x48F363)
    b.internet_mut().unwrap().pending_game = true;
    assert!(pump(&mut s, &mut [&mut b], |c| c[0].comm_state() == cs::WRONG_VERSION), "state {}", b.comm_state());
    // the link is not torn down (state 0xA with a request pending: the notice shows and 0x509550 puts the state back to 0xE)
    assert!(!b.lost() && b.my_player_id() == -1, "still in the lobby: {old:?}");
    b.restore_state();
    assert!(pump(&mut s, &mut [&mut b], |c| c[0].established()));
}

/// **PORT-DEVIATION D56:** after `0xC6 0xE` the lobby link sends exactly one `0xC7` with the build identity, and the stub (which knows
/// only the original's messages) ignores it like the original's reference server would (default case 0x491C3C).
#[test]
fn the_port_announces_its_build_once_after_the_sign_on() {
    let mut s = stub();
    let mut c = client(&s, "racer", "ann");
    assert!(pump(&mut s, &mut [&mut c], |c| c[0].established()));
    // a few more rounds: the one message must not repeat
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        s.step();
        c.poll_lobby();
        std::thread::sleep(Duration::from_millis(1));
    }
    let builds: Vec<_> = s.heard.iter().filter(|(_, m)| m.first() == Some(&0xC7)).collect();
    assert_eq!(builds.len(), 1, "{:?}", s.heard.iter().map(|h| h.1.first()).collect::<Vec<_>>());
    let b = nfs_net::lobby::parse_client_build(&builds[0].1).unwrap();
    assert_eq!(b.text, crate::brand::build_id());
    assert!(b.sim() == Some(env!("OU_SIM_VERSION")));
}
