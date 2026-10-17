//! Refactor guard for the replay / save readers (`review/nfs-game.md` batch 6: D4, E1).
//!
//! The fixtures in `tests/data/` were written by master 059e30ae: the three `.prpl` / `.rpl` pairs by
//! autotest races (`porsche <track> 993 --autotest 8 --opponents 3`, the save dir a copy of
//! `fe_save_pristine`), `8DRIVER.sav` the profile of that save dir. The digests below are what the
//! master readers made of them; a reader change must keep every one. Re-pinned 2026-10-17 for
//! `CarRecord::livery` (file version 4): the `Debug` digests now print `livery: None`, and a written
//! file is version 4; the `.rpl` bytes the port writes are unchanged.
use nfs_game::replay::{self, CarRecord, NetSeat, NetSetup, NetTraffic, ReplayFile, ReplaySetup};

/// FNV-1a 64 — a stable digest with no dependency.
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0100_0000_01b3))
}

fn digest<T: std::fmt::Debug>(v: &T) -> u64 {
    fnv(format!("{v:?}").as_bytes())
}

fn data(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)).unwrap()
}

/// What the port reader gives for every prefix of `d` (and the whole), folded into one digest.
fn prefix_digest(d: &[u8]) -> u64 {
    let mut h = Vec::new();
    for n in 0..=d.len() {
        h.extend_from_slice(&digest(&ReplayFile::parse(&d[..n])).to_le_bytes());
    }
    fnv(&h)
}

const TRACKS: [&str; 3] = ["alps", "coastal", "monaco1"];

/// `(parse, every prefix)` digests of the master-written `.prpl`s.
const PRPL: [(u64, u64); 3] = [(0x43ea_bfd6_2d29_d175, 0xe8e8_307e_d5d0_a7c2), (0x236e_1679_da5d_7a9f, 0xb47f_b2d2_a69c_695e), (0x1715_9e98_b827_5349, 0x7d4d_19a4_d42a_a5e4)];
/// `parse_original_rpl` digest of the master-written `.rpl`s, and of the prefixes around the snapshot.
const RPL: [(u64, u64); 3] = [(0xd18d_cda9_909d_b7f9, 0x6ebb_e837_b2c4_86b0), (0x7c95_afea_5e84_8805, 0x922f_4163_1c6f_62e7), (0x85d3_c26a_6625_afe3, 0x8521_87d5_04d8_ab1d)];

#[test]
fn master_written_port_replays_parse_and_reserialise_to_the_same_bytes() {
    let mut all = Vec::new();
    for t in TRACKS {
        let d = data(&format!("replays/{t}.prpl"));
        let f = ReplayFile::parse(&d).unwrap_or_else(|| panic!("{t}.prpl parses"));
        // the fixtures are version 3 (no livery byte): they read with every livery absent, and the
        // version-4 file written from them reads back to the same thing
        assert_eq!(u32::from_le_bytes(d[8..12].try_into().unwrap()), 3);
        let cars = f.setup.player.iter().chain(f.setup.opponent_cars.iter().flatten());
        assert!(cars.into_iter().all(|c| c.livery.is_none()), "{t}.prpl: a version-3 record has no livery");
        assert_eq!(ReplayFile::parse(&f.write()).as_ref(), Some(&f), "{t}.prpl round-trips through version 4");
        let got = (digest(&f), prefix_digest(&d));
        println!("prpl {t}: {got:#x?}");
        all.push(got);
    }
    assert_eq!(all, PRPL);
}

#[test]
fn master_written_original_replays_parse_as_before() {
    let mut all = Vec::new();
    for t in TRACKS {
        let d = data(&format!("replays/{t}.rpl"));
        let whole = replay::parse_original_rpl(&d).unwrap_or_else(|| panic!("{t}.rpl parses"));
        let mut h = Vec::new();
        for n in (0..replay::SNAPSHOT_LEN + 64).chain([d.len() - 1]) {
            h.extend_from_slice(&digest(&replay::parse_original_rpl(&d[..n])).to_le_bytes());
        }
        let got = (digest(&whole), fnv(&h));
        println!("rpl {t}: {got:#x?}");
        all.push(got);
    }
    assert_eq!(all, RPL);
}

fn net_file() -> ReplayFile {
    let car = |id: u16, skill: bool| CarRecord {
        car_id: id,
        paint: [1, 2, 3],
        name: format!("car{id}"),
        grid: Some(2),
        decal: 5,
        personality: i16::from(skill),
        difficulty: 1.0,
        skill: skill.then_some([[0.5, 0.1], [0.6, 0.2], [0.7, 0.3], [0.8, 0.4]]),
        parts: Some(vec![11, 12, 13]),
        part_damage: vec![1.0, 2.0, 3.0],
        damage: [1.0; 10],
        setup: Some([0.5; 17]),
        livery: None,
    };
    let net = NetSetup {
        local_index: 1,
        grid: Some(u32::MAX),
        seats: vec![
            NetSeat { race_index: 0, name: "HOST".into(), grid: Some(2), words: vec![(0x13D, vec![0x3939_3339, 0]), (0x225, vec![0xFF11_2233])] },
            NetSeat { race_index: 2, name: "THIRD".into(), grid: None, words: vec![] },
        ],
        traffic: vec![NetTraffic { model: "EAS10".into(), name: b"Bus".to_vec(), colour: 0xFF8C_8C90 }],
        camera0: Some(vec![0, 1, 2, 3]),
        own_words: vec![(0x13D, vec![0x3132_3334, 0]), (0x24F, vec![1])],
        fe_txt: vec![("time".into(), 1), ("weather".into(), 0)],
    };
    let setup = ReplaySetup {
        track: "coastal".into(),
        car: "993".into(),
        sim: "993turbo".into(),
        car_id: 154,
        paint: [1, 2, 3],
        laps: 1,
        opponents: 2,
        traffic: 1,
        damage: true,
        seed: 8165,
        skill: 1,
        driver_name: "ANNA".into(),
        net: Some(net),
        race_type: Some(3),
        fegame_type: Some(9),
        camera: Some(vec![1, 0, 2, 7]),
        player: Some(car(154, false)),
        opponent_cars: Some(vec![car(30, true), car(31, false)]),
        scene: Some(3),
        derby: Some(true),
        nocollision: Some(false),
        transmission: Some(false),
        opponent_transmission: Some(true),
        mission: Some([3011, 0, 0, 3012, 3013, 0, 90, -1, -1, 5, 2, 0]),
        ..Default::default()
    };
    ReplayFile { setup, final_tick: 640, stream: (0..200).map(|i| (i % 251 + 1) as u8).collect() }
}

/// A network race's file with every optional section (`NetSetup` with seats / traffic / camera0 /
/// own words / fe.txt, the launch block, the car records, the mission): the bytes master wrote and
/// what the reader makes of every prefix.
const NET: (u64, u64, u64, u64) = (0x4892_63f3_7273_cb45, 0x00cf_7cc0_4e1b_17b8, 0x936e_708d_316a_5f17, 0x4581_ef48_cd1d_32d3);

#[test]
fn a_network_replay_reads_back_as_master_wrote_it() {
    let f = net_file();
    let d = f.write();
    assert_eq!(ReplayFile::parse(&d).as_ref(), Some(&f));
    let rpl = replay::original_rpl(&f.setup, f.final_tick, &f.stream, replay::Ramp::default(), 4);
    let got = (fnv(&d), prefix_digest(&d), fnv(&rpl), digest(&replay::parse_original_rpl(&rpl)));
    println!("net: {got:#x?}");
    assert_eq!(got, NET);
}

/// `8DRIVER.sav` through the port's career conversion and back: the `Career` / side it reads and the
/// bytes it writes again.
const SAV: (u64, u64, u64) = (0x9901_2848_0975_68d3, 0x9025_c01c_3df6_113e, 0xc970_db8b_8990_791d);

#[test]
fn a_master_written_save_converts_as_before() {
    let Some(t) = nfs_assets::GamePaths::locate().ok().and_then(|p| nfs_game::career::CareerTables::load(&p).ok()) else {
        eprintln!("skipping: no game data");
        return;
    };
    let d = data("8DRIVER.sav");
    let s = nfs_formats::sav::SaveFile::parse(&d).unwrap();
    let c = nfs_game::save::career_from_sav(&s, &t);
    let side = nfs_game::save::side_from_sav(&s);
    let p = nfs_game::save::Profile::from_stem("8DRIVER").unwrap();
    let back = nfs_game::save::career_to_sav_with(&c, &t, &p, &side).write();
    let got = (digest(&c), digest(&side), fnv(&back));
    println!("sav: {got:#x?}");
    assert_eq!(got, SAV);
}
