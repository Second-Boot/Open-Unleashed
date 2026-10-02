//! Byte-exact tests against **real captures of the original** (`RE/notes/REFERENCE-2026-10-02-SRVE.md`:
//! two instances of nfs5.exe, an IP create and a join, tcpdump on the loopback). The packets below are
//! copied from those pcaps; the fields that vary from run to run (the joiner's ephemeral port, the
//! stack leftovers the original sends in the gaps) are substituted or masked, and said so.

use nfs_net::comm::{self, Describe, Join, JoinReply, JoinStatus};
use nfs_net::data::Frame;
use nfs_net::session::{name32, Net};
use nfs_net::socket::CommSocket;
use nfs_net::wire::{Addr, Datagram};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// Session 1, the joiner's first datagram (B -> 127.0.0.1:9442, source port 53523, profile TEST): 137 bytes.
const J_S1: &str = "6a00000001020024e27f000001000000000000000000000000000000000000000000000000000000000200d11300000000000000000000000000000000506c6179657200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000544553547e332e357e332e350000000000000000e00a3700";
/// Session 1, the host's answer: 33 bytes.
const A_S1: &str = "6100000001020024e27f0000010000000000000000000000000000000100000000";
/// Session 2 (typed address 192.168.2.71, source port 42206): the same two packets.
const J_S2: &str = "6a00000001020024e2c0a80247000000000000000000000000000000000000000000000000000000000200a4de00000000000000000000000000000000506c6179657200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000544553547e332e357e332e350000000000000000e00a3700";
const A_S2: &str = "6100000001020024e2c0a802470000000000000000000000000000000100000000";
/// The ack-only data frame both sides send (`3d id id seq(fffd) 00 00`).
const ACK: &str = "3d0000fffd0000";
/// The live original host's `'d'` to a `'w'` probe (the 32-byte key), 173 bytes (probe1.out, `w_eagame`).
const D_W: &str = "6400000000020024e2000000000000000000000000000000005cd46b81ece411d39b6a00902724ab1e47616d657e332e3500000000000000000000000000000000000000000000000000010000019a012f00000000000000000000000962000000c46aa8f7620000000000000000000000a8fddf05210000002801000078fddf056caa5f7e000000006200000020fddf0508fddf05620000000000000077000000c0fddf0510fedf0518054601";

#[test]
fn the_joiners_j_is_the_originals_byte_for_byte() {
    for (golden, host_ip, port) in [(J_S1, [127, 0, 0, 1], 53523u16), (J_S2, [192, 168, 2, 71], 42206)] {
        let g = unhex(golden);
        let j = Join::original(1, Addr::ip(host_ip, 9442), Addr::ip([0, 0, 0, 0], port), b"TEST~3.5~3.5\0");
        let mut ours = Datagram::encode(b'j', &j.encode()).unwrap();
        assert_eq!(ours.len(), 137, "tag + 0x88 (0x588107: push 0x88)");
        // +0x84 (datagram +0x85): four stack leftovers in the original, `e0 0a 37 00` in both sessions;
        // the port sends zeros there (APPROXIMATION, nothing reads them)
        assert_eq!(&ours[133..], &[0, 0, 0, 0]);
        assert_eq!(&g[133..], &[0xe0, 0x0a, 0x37, 0x00]);
        ours[133..].copy_from_slice(&g[133..]);
        assert_eq!(ours, g);
        // and a host reads it: the host's address at +04, the identity at +0x70, the name zero
        let d = Join::decode(&g[1..]).unwrap();
        assert_eq!(d.request, 1);
        assert_eq!(d.from, Addr::ip(host_ip, 9442));
        assert_eq!(d.name, [0u8; 16]);
        assert_eq!(d.identity(), b"TEST~3.5~3.5");
        assert_eq!(&d.record[0x14..0x1a], b"Player");
    }
}

#[test]
fn the_hosts_a_is_the_originals_byte_for_byte() {
    for (golden, host_ip) in [(A_S1, [127, 0, 0, 1]), (A_S2, [192, 168, 2, 71])] {
        let g = unhex(golden);
        let a = JoinReply { request: 1, from: Addr::ip(host_ip, 9442), status: JoinStatus::Joined(0) };
        assert_eq!(Datagram::encode(b'a', &a.encode()).unwrap(), g);
        assert_eq!(JoinReply::decode(&g[1..]).unwrap(), a);
    }
}

#[test]
fn the_session_key_is_the_eagame_text_cut_to_32_bytes() {
    assert_eq!(comm::eagame_key(&comm::APP_GUID), "EAGAME:5cd46b81.ece411d3.9b6a0090.2724ab1e");
    assert_eq!(&comm::key32(&comm::APP_GUID)[..], b"EAGAME:5cd46b81.ece411d3.9b6a009");
    let p = comm::probe_payload(&comm::APP_GUID);
    assert_eq!(p.len(), 43, "strlen + 1 (0x560620): the 44-byte datagram the original answers");
    assert_eq!(*p.last().unwrap(), 0);
}

#[test]
fn the_ack_only_frame_is_the_originals() {
    let f = Frame::ack_only(0, nfs_net::reliable::ACK_ONLY, 0).encode().unwrap();
    assert_eq!(Datagram::encode(nfs_net::data::TAG, &f).unwrap(), unhex(ACK));
}

fn live_host() -> Net {
    let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).unwrap();
    let mut n = Net::host(sock, name32("Game~3.5"), comm::key32(&comm::APP_GUID), 9);
    n.desc_bytes = [0, 1, 0, 0];
    n.desc_words = [0x019A, 0x012F];
    n
}

/// Send `datagram` to the host from a raw socket and return what comes back within ~400 ms.
fn ask(h: &mut Net, datagram: &[u8]) -> Option<Vec<u8>> {
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    c.send_to(datagram, h.local_addr().unwrap()).unwrap();
    let mut buf = [0u8; 512];
    for _ in 0..20 {
        h.poll();
        if let Ok(n) = c.recv(&mut buf) {
            return Some(buf[..n].to_vec());
        }
    }
    None
}

#[test]
fn a_port_host_answers_the_probes_the_original_answers_and_only_those() {
    let mut h = live_host();
    let key = comm::eagame_key(&comm::APP_GUID);
    // the 32-byte form, and the whole text with its NUL (what a browse sends): both answered
    let mut w32 = vec![b'w'];
    w32.extend_from_slice(&key.as_bytes()[..32]);
    let mut w44 = vec![b'w'];
    w44.extend_from_slice(key.as_bytes());
    w44.push(0);
    assert_eq!((w32.len(), w44.len()), (33, 44));
    let d32 = ask(&mut h, &w32).expect("the 32-byte probe is answered");
    let d44 = ask(&mut h, &w44).expect("the 43-byte probe is answered");
    assert_eq!(d32, d44);
    // the raw GUID zero-padded to 32 bytes (what the port sent before) and 32 zero bytes: no answer
    let mut raw = vec![b'w'];
    raw.extend_from_slice(&comm::APP_GUID);
    raw.extend_from_slice(&[0; 16]);
    assert!(ask(&mut h, &raw).is_none());
    assert!(ask(&mut h, &[&b"w"[..], &[0u8; 32]].concat()).is_none());

    // the original's `'d'` (173 bytes), field by field; +04 (the socket's own record: 0.0.0.0:9442 on the
    // original, the port's ephemeral test port here) and the gaps +5C..+6B / +70..+AB (stack leftovers,
    // zero here) are not compared
    let g = unhex(D_W);
    assert_eq!(g.len(), 173);
    assert_eq!(d32.len(), g.len());
    let (gp, op) = (&g[1..], &d32[1..]);
    assert_eq!(&op[0..4], &gp[0..4], "request: 0 for a 'w'");
    assert_eq!(&op[0x18..0x5c], &gp[0x18..0x5c], "GUID +18, name +28, descriptor +48..., limit +58 = 9, +50 / +54");
    assert_eq!(&op[0x6c..0x70], &gp[0x6c..0x70], "mask +6C");
    assert_eq!(Describe::decode(op).unwrap().player_limit, 9);
}

#[test]
fn a_port_host_admits_the_originals_j_with_the_originals_a() {
    let mut h = live_host();
    h.identity_gate = Some("3.5".to_string());
    let at = h.local_addr().unwrap();
    // the original's session-1 `'j'`, sent from a raw socket; its +04 record (the typed host address) is
    // this host's own, so the `'a'` echoes the same record
    let mut j = unhex(J_S1);
    let rec = Addr::ip([127, 0, 0, 1], at.port());
    j[5..25].copy_from_slice(&rec.0);
    let a = ask(&mut h, &j).expect("the host answers the original's 'j'");
    let mut want = unhex(A_S1);
    want[5..25].copy_from_slice(&rec.0);
    assert_eq!(a, want);
    assert_eq!(&h.peers[0].as_ref().unwrap().name[..4], b"TEST");
    // the same joiner again from the same socket address would get the same slot (0x588F41); from a new
    // source port it is a second player
    assert_eq!(h.peers.iter().flatten().count(), 1);
    // the mask a `'d'` carries (+6C = `session+0x50`): 0x589440 sets the slot's bit at the admission and 0x587D92 clears it when the
    // slot's 450 spends of 0x587CE0 are gone (gdb on the original host: set 0x589486, cleared 3.52 s later; a `w` probed every
    // 2 s read `0x1` for two probes and `0` after). The player stays in the room; only the bit leaves.
    let mut w = vec![b'w'];
    w.extend_from_slice(&comm::key32(&comm::APP_GUID));
    let d = ask(&mut h, &w).unwrap();
    assert_eq!(Describe::decode(&d[1..]).unwrap().slot_mask, 1, "right after the admission the slot's bit is set");
    h.peers[0].as_mut().unwrap().adv_until = std::time::Instant::now();
    let d = ask(&mut h, &w).unwrap();
    assert_eq!(Describe::decode(&d[1..]).unwrap().slot_mask, 0, "once its spends are gone the bit is clear, the player is still in the room");
    assert_eq!(h.peers.iter().flatten().count(), 1);
}

#[test]
fn the_data_layer_opens_a_link_with_one_empty_q_and_a_q_takes_nothing_away() {
    let mut h = live_host();
    h.identity_gate = None;
    let at = h.local_addr().unwrap();
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    c.send_to(&unhex(J_S1), at).unwrap();
    for _ in 0..10 {
        h.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut buf = [0u8; 512];
    assert_eq!(c.recv(&mut buf).unwrap(), 33, "the 'a'");
    // the host's first frame to the peer (an unreliable one: the 0xBE state) is preceded by one empty `'q'`
    let me = c.local_addr().unwrap();
    h.send_game_to(&[me], &[0xBE, 1, 2, 3]).unwrap();
    h.send_game_to(&[me], &[0xBE, 4, 5, 6]).unwrap();
    let n = c.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"q", "an empty 'q' first (0x582711)");
    let n = c.recv(&mut buf).unwrap();
    assert_eq!(buf[0], b'=', "then the frame, {n} bytes");
    let n = c.recv(&mut buf).unwrap();
    assert_eq!(buf[0], b'=', "and no second 'q': {n} bytes");
    // the peer's own `'q'` is not answered with `'!'` and does not free its slot
    c.send_to(b"q", at).unwrap();
    for _ in 0..10 {
        h.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(c.recv(&mut buf).is_err());
    assert_eq!(h.peers.iter().flatten().count(), 1);
}

// ---- the room messages of the original host (REFERENCE-2026-10-02-SRVX, pcap of two originals, host -> joiner) ----

/// `0xB4` (80 bytes) with one player in the room, and the second one (after the newcomer's `0xB0`) with two.
const B4_ONE: &str = "b4000001000000000000000000000000000000000000000047616d657e332e35000000000000000000000000000000000000000000000000000100009a012f0100000000000000000800000001000000";
const B4_TWO: &str = "b4000001000000000000000000000000000000000000000047616d657e332e35000000000000000000000000000000000000000000000000000100009a012f0100000000000000000800000002000000";

#[test]
fn the_rooms_description_your_id_and_players_are_the_originals_bytes() {
    use nfs_net::room;
    let mut name = [0u8; 32];
    name[..8].copy_from_slice(b"Game~3.5");
    assert_eq!(room::description(1, &name, [0, 1, 0, 0], [0x019A, 0x012F], 1), unhex(B4_ONE));
    assert_eq!(room::description(1, &name, [0, 1, 0, 0], [0x019A, 0x012F], 2), unhex(B4_TWO));
    assert_eq!(room::your_id(1, b"Game"), unhex("b200000147616d6500"));
    assert_eq!(room::player_in(0, b"DRIVER"), unhex("b000000044524956455200"));
    assert_eq!(room::player_in(1, b"DRIVER"), unhex("b000000144524956455200"));
    // 0xC6 (0xE): "you are in", the last message of the admission
    assert_eq!(room::state(room::comm_state::CONNECTED), unhex("c600000e"));
}

#[test]
fn the_conman_hello_and_its_answer_are_the_originals_frames() {
    // joiner -> host, the first data frame: `ff 00 "Player\0" <identity>\0` (24 bytes), seq 1
    let hello = unhex("ff00506c61796572004452495645527e332e357e332e3500");
    let mut ours = vec![0xFF, 0x00];
    ours.extend_from_slice(b"Player\0");
    ours.extend_from_slice(&comm::identity::encode("DRIVER", "3.5", "3.5"));
    assert_eq!(ours, hello);
    // host -> joiner: `fe 01` as the first reliable message (frame ack 1, seq 1)
    let f = Frame { ack: 1, seq: 1, flags: 0, pieces: vec![nfs_net::data::Piece { seq: 1, body: vec![0xFE, 1] }] };
    assert_eq!(f.encode().unwrap(), unhex("000100010002fe01"));
}

#[test]
fn the_originals_file_notices_are_chat_lines_to_nobody_and_the_relay_is_the_ports() {
    // the original joiner sends `bb ff ff fe ".\gamedata\track\Tr00.bin"` (a chat request whose addressee is −2, 0x48D9B0) and the
    // host's relay (0x491324) answers every player with `bb 00 00 01 | ff ff ff fe | text`; a player drops `to == −2` (0x48F3B9)
    use nfs_net::room::{ChatLine, ChatRequest};
    let req = unhex("bbfffffe2e5c67616d65646174615c747261636b5c547230302e62696e00");
    let r = ChatRequest::parse(&req).unwrap();
    assert_eq!((r.to, r.text.as_slice()), (-2, &b".\\gamedata\\track\\Tr00.bin"[..]));
    assert_eq!(r.encode(), req);
    let line = unhex("bb000001fffffffe2e5c67616d65646174615c747261636b5c547230302e62696e00");
    assert_eq!(ChatLine { from: 1, to: -2, text: r.text.clone() }.encode(), line);
    assert!(ChatLine::parse(&line).is_none(), "a player drops a line to −2");
}

#[test]
fn the_hosts_replayed_option_and_row_records_are_the_originals_bytes() {
    // 0x48A390 replays the host's own 0xBC records to the newcomer before 0xB2 — options 0x14 / 0xD / 0xF / 0xC, then the lobby rows
    // 0x226 (car id 0x4C), 0x141 ("'50 356 1100 Cabriolet *"), 0x225 (colour), 0x255 (ready), slot 0
    use nfs_net::{options, slots};
    let opts = options::messages_each(&[vec![0x14, 1], vec![0xD, 0], vec![0xF, 1], vec![0xC, 0x12F]]);
    assert_eq!(opts[0], unhex("bc0000001400000001000000"));
    assert_eq!(opts[1], unhex("bc0000000d00000000000000"));
    assert_eq!(opts[2], unhex("bc0000000f00000001000000"));
    assert_eq!(opts[3], unhex("bc0000000c0000002f010000"));
    let mut rows = slots::car_records(0, 0x4C, b"'50 356 1100 Cabriolet *", 0xFFF3FFCC);
    rows.push(slots::ready_record(0, true));
    let m = options::messages_each(&rows);
    assert_eq!(m[0], unhex("bc00000026020000000000004c000000"));
    assert_eq!(m[1], unhex("bc00000041010000000000002735302033353620313130302043616272696f6c6574202a00000000"));
    assert_eq!(m[2], unhex("bc0000002502000000000000ccfff3ff"));
    assert_eq!(m[3], unhex("bc000000550200000000000001000000"));
}
