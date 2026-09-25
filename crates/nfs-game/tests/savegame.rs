//! The original save game against the shipped tables (`RE/notes/savegame.md`).
use nfs_assets::GamePaths;
use nfs_formats::sav::{SaveFile, FE_CAR_IMAGE};
use nfs_game::career::{Career, CareerTables, EventResult, OwnedCar, TournamentState};
use nfs_game::save::{car, career_from_sav, career_to_sav, Profile};

fn tables() -> Option<CareerTables> {
    let paths = GamePaths::locate().ok()?;
    CareerTables::load(&paths).ok()
}

#[test]
fn career_round_trips_through_the_original_format() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t.catalog.iter().find(|c| !c.sim.is_empty() && !c.paint_sets.is_empty()).unwrap().id;
    let mut c = Career::default();
    c.money = 123_456;
    c.year = 1969;
    c.era = nfs_formats::career::Era::Golden;
    let mut owned = OwnedCar::new(car_id, 1, 30_000);
    owned.mileage = 9000;
    owned.wins = 2;
    owned.asking_price = Some(41_000);
    owned.offer = Some(38_500);
    owned.parts = t.parts.iter().take(2).map(|p| p.id).collect();
    c.garage.push(owned.clone());
    let tour = t.tournaments[0].id;
    c.tournaments.insert(
        tour,
        TournamentState {
            entered: true,
            completed: true,
            completions: 2,
            events: vec![EventResult { position: 1, prize: 5000, bonus: 2500, points: [8, 7, 6, 5, 4, 3, 2, 1], earned: [7500, 3000, 2000, 1500, 1000, 800, 600, 400], times: [10_000, 10_500, 11_000, 0, 0, 0, 0, 0] }],
            final_rank: Some(1),
            prize: 20_000,
            failed_attempts: 0,
        },
    );
    c.missions = t.missions.iter().enumerate().map(|(i, _)| if i == 3 { 1 } else { 0 }).collect();

    let p = Profile::new("Hans", 3);
    let sav = career_to_sav(&c, &t, &p);
    let bytes = sav.write();
    // The container the original writes: no header, module names NUL-terminated, in its own order.
    assert!(bytes.starts_with(b"Missions\0"));
    let parsed = SaveFile::parse(&bytes).expect("the writer produces a file the reader accepts");
    assert_eq!(parsed.order, ["Missions", "Tournament", "PlayerInfo", "TrackRecords", "Personalities", "Knockout", "CDPlayer", "accessor"]);
    // the knockout module is the raw 0x84-byte state block, switched off when no series is running
    let k = nfs_formats::sav::KnockoutState::parse(parsed.knockout.as_ref().unwrap());
    assert!(!k.active && k.opponents == 0);
    // the accessor block carries the values the port has, in the original's map order
    let acc = parsed.accessors.as_ref().expect("accessor block");
    assert!(acc.entries.iter().any(|(n, v)| n == "Chronicle.Year" && *v == nfs_formats::sav::AccessorValue::Dword(1969)));
    assert!(acc.entries.iter().any(|(n, _)| n == "OwnedCars.Car"));
    // …the settings screens' values, each a plain dword (§6.1)
    assert!(acc.entries.iter().any(|(n, v)| n == "Audio.MusicLevel" && *v == nfs_formats::sav::AccessorValue::Dword(127)));
    assert!(acc.entries.iter().any(|(n, _)| n == "Graphics.ViewDistance"));
    // …and the selected car's setup, whose floats ride in the same dword
    let setup = acc.entries.iter().find(|(n, _)| n == "FTirePressure").expect("a setup accessor");
    let nfs_formats::sav::AccessorValue::Dword(bits) = setup.1 else { panic!("{setup:?}") };
    assert_eq!(f32::from_bits(bits), 30.0, "the tyre pressure FECar starts a car with");
    // the block stays in byte-wise name order, which is what the original's reader walks
    let names: Vec<&str> = acc.entries.iter().map(|(n, _)| n.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(names, sorted);

    let pi = parsed.player.as_ref().unwrap();
    assert_eq!(pi.money, 123_456);
    assert_eq!(pi.picture, 3);
    assert_eq!(pi.player_name, "Hans");
    assert_eq!(pi.cars.len(), 1);
    let saved = &pi.cars[0];
    assert_eq!(saved.image.len(), FE_CAR_IMAGE);
    // The nfs5.car record sits at car+4, behind the instance number (FECar::init 0x4EEBA0).
    assert_eq!(u16::from_le_bytes([saved.image[car::RECORD], saved.image[car::RECORD + 1]]), car_id);
    assert_eq!(saved.field_u32(car::MILEAGE), 9000);
    assert_eq!(saved.field_u8(car::OWNED), 1, "a garage car is marked as the player's");
    assert_eq!(saved.field_u32(car::PURCHASE_PRICE), 30_000);
    assert_eq!(saved.installed.len(), 2);

    let back = career_from_sav(&parsed, &t);
    assert_eq!(back.money, c.money);
    assert_eq!(back.year, 1969);
    assert_eq!(back.era, nfs_formats::career::Era::Golden);
    assert_eq!(back.garage.len(), 1);
    assert_eq!(back.garage[0].car_id, owned.car_id);
    assert_eq!(back.garage[0].paint_set, owned.paint_set, "the paint set is recovered from the stored colours");
    assert_eq!(back.garage[0].trim, owned.trim, "a bought car's rows are all 0 and come back so");
    assert_eq!(back.garage[0].parts, owned.parts);
    assert_eq!(back.garage[0].mileage, owned.mileage);
    assert_eq!(back.garage[0].wins, owned.wins);
    assert_eq!(back.garage[0].asking_price, owned.asking_price);
    assert_eq!(back.garage[0].offer, owned.offer);
    assert_eq!(back.missions, c.missions);
    let st = back.tournaments.get(&tour).expect("the tournament state survives in the nfs5.trn image");
    assert!(st.completed);
    assert_eq!(st.events.len(), 1);
    assert_eq!(st.events[0].points, c.tournaments[&tour].events[0].points);
    assert_eq!(st.events[0].times, c.tournaments[&tour].events[0].times);
    assert_eq!(st.events[0].earned, c.tournaments[&tour].events[0].earned);
    assert_eq!(st.events[0].prize, 5000);
    assert_eq!(st.events[0].bonus, 2500);
}

/// The embedded tables must stay byte-identical to the shipped ones apart from the runtime fields.
#[test]
fn the_embedded_tables_are_the_shipped_images() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let sav = career_to_sav(&Career::default(), &t, &Profile::new("A", 0));
    assert_eq!(sav.missions.as_ref().unwrap().len(), t.raw.fac.len());
    assert_eq!(sav.tournament.as_ref().unwrap().table.len(), t.raw.trn.len());
    // A default career writes no results, so both images come back unchanged.
    assert_eq!(sav.missions.as_deref(), Some(t.raw.fac.as_slice()));
    assert_eq!(sav.tournament.as_ref().unwrap().table, t.raw.trn);
    assert_eq!(sav.tournament.as_ref().unwrap().year, 1950);
    assert_eq!(sav.tournament.as_ref().unwrap().era_type, 521);
    assert_eq!(sav.tournament.as_ref().unwrap().current_event, -1);
    assert_eq!(sav.knockout.as_ref().map(|k| k.len()), Some(nfs_formats::sav::KNOCKOUT_BLOB));
}

/// The sixteen `FEControllerDeadZoneAccessor`s ride in the accessor block as `u32 selection` plus
/// `u32[16]` — one value per pad — and the port writes the pad it edits into slot 0, leaving the
/// rest at the factory's own 0 / 0x80. `RE/notes/savegame.md` §6.1, `controls.md` §4.17.
#[test]
fn the_dead_zone_accessors_ride_in_the_save() {
    let Some(tables) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let career = Career::default();
    let profile = Profile { name: "DRIVER".into(), picture: 0 };
    let mut settings = nfs_game::save::SavedSettings::default();
    // both opponent counts start at 4 (0x4E5539, 0x4E556C), not at 7
    assert_eq!((settings.opponents, settings.ko_opponents), (4, 4));
    assert_eq!(settings.deadzone, [[0, 0x80]; 8]);
    settings.deadzone[2] = [40, 100];

    let sav = nfs_game::save::career_to_sav_with(&career, &tables, &profile, None, &[], &settings);
    let a = sav.accessors.as_ref().expect("an accessor block");
    let find = |n: &str| a.entries.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());

    // every one of the sixteen is there, in the registration order
    for axis in nfs_game::save::DEADZONE_AXES {
        for (k, suffix) in ["Min", "Max"].iter().enumerate() {
            let v = find(&format!("{axis}.{suffix}")).unwrap_or_else(|| panic!("{axis}.{suffix}"));
            let nfs_formats::sav::AccessorValue::DeadZone { selection, values } = v else {
                panic!("{axis}.{suffix} is not a dead-zone value");
            };
            assert_eq!(selection, 0);
            // pads 1..15 keep the factory's own default for that end
            assert!(values[1..].iter().all(|x| *x == if k == 0 { 0 } else { 0x80 }), "{axis}.{suffix}");
        }
    }
    // and the one the test moved came through on pad 0
    let nfs_formats::sav::AccessorValue::DeadZone { values, .. } = find("ZAxis.Min").unwrap() else { panic!() };
    assert_eq!(values[0], 40);
    let nfs_formats::sav::AccessorValue::DeadZone { values, .. } = find("ZAxis.Max").unwrap() else { panic!() };
    assert_eq!(values[0], 100);

    // the block survives a round trip through the original's own bytes
    let bytes = sav.write();
    let back = SaveFile::parse(&bytes).expect("re-parse");
    let b = back.accessors.as_ref().expect("an accessor block");
    assert_eq!(b.entries.len(), a.entries.len());
}

/// The used-car market (`RE/notes/career.md` §10.5, generator 0x4F80C0 / 0x4F82C0 / 0x4F8590).
#[test]
fn the_used_car_market_fills_and_caps() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut c = Career::default();
    c.year = 1975;
    // the original's 16-bit LCG stands in for `0x538040`; any uniform source exercises the rules
    let mut seed = 12345u32;
    let mut rand = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) & 0x7FFF
    };
    c.refresh_market(&t, &mut rand);
    assert!(!c.market.is_empty(), "a 1975 market should hold something");
    assert!(c.market.len() <= nfs_game::career::USED_CAR_MARKET_MAX);
    for car in &c.market {
        let rec = t.car(car.car_id).expect("a catalogue car");
        // nothing under two years old, and never a flag-2 or flag-3 record (0x4F80FE, 0x4F8114)
        assert!(rec.year as i64 <= c.year as i64 - 2, "{} is too new", rec.display_name);
        assert!(!matches!(rec.dealer_flag_204, 2 | 3));
        // mileage and wins come from their bands, and the price is the car's own value
        assert!(car.mileage <= 100_000);
        assert!(car.wins <= 50);
        assert_eq!(car.asking_price, Some(c.value_of(&t, car).unwrap().max(0) as u32));
        // a market car is mechanically fresh — the generator never damages it
        assert_eq!(car.damage_pct, 0.0);
        assert!(car.part_state.iter().all(|(_, s)| *s == 0.0));
        // and it never carries two engines (0x4F877E's four compares)
        let engines = car
            .parts
            .iter()
            .filter_map(|id| t.parts.iter().find(|p| p.id == *id))
            .filter(|p| p.kind_name == "Engine" || p.kind_name == "Engine (Standard)")
            .count();
        assert!(engines <= 1, "{} has {engines} engines", rec.display_name);
    }
    // refreshing again keeps the cap
    for _ in 0..4 {
        c.refresh_market(&t, &mut rand);
    }
    assert_eq!(c.market.len(), nfs_game::career::USED_CAR_MARKET_MAX);
    // buying takes the money and moves the car across untouched
    c.money = 10_000_000;
    let want = c.market[0].clone();
    c.buy_used_car(&t, 0).unwrap();
    assert_eq!(c.money, 10_000_000 - want.asking_price.unwrap() as i64);
    // the garage is sorted after the purchase (0x4F0EE0), so the car is found, not taken last
    let got = c.garage.iter().find(|g| g.car_id == want.car_id && g.mileage == want.mileage && g.parts == want.parts).expect("the bought car");
    assert_eq!(got.asking_price, None);
}

/// `nfs5.exe`'s virtual addresses mapped to file offsets, so a table the port transcribed can be
/// compared with the bytes it came from (the same helper `nfs-physics`'s `scenarios.rs` uses for the
/// surface tables). Minimal PE walking: `e_lfanew` at 0x3C, the COFF header, then the section whose
/// virtual range holds the RVA.
struct Image {
    bytes: Vec<u8>,
    sections: Vec<(u32, u32, u32)>,
}

impl Image {
    fn open() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/nfs5.exe");
        let bytes = std::fs::read(p).ok()?;
        let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u32at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let pe = u32at(0x3C) as usize;
        let (n, opt) = (u16at(pe + 6), u16at(pe + 20));
        let table = pe + 24 + opt;
        let sections = (0..n).map(|i| { let s = table + i * 40; (u32at(s + 12), u32at(s + 8), u32at(s + 20)) }).collect();
        Some(Self { bytes, sections })
    }

    fn i32s(&self, va: u32, n: usize) -> Vec<i32> {
        let rva = va - 0x400000;
        let (r, _, off) = *self.sections.iter().find(|(r, sz, _)| rva >= *r && rva < r + sz.max(&1)).expect("inside a section");
        let o = (off + (rva - r)) as usize;
        (0..n).map(|i| i32::from_le_bytes(self.bytes[o + i * 4..o + i * 4 + 4].try_into().unwrap())).collect()
    }
}

/// **The used-car generator's four tables, against the bytes they were transcribed from.** 61 + 48
/// integers that decide which cars reach the lot and with what mileage, wins and parts
/// (`career.md` §10.5a). None of them shows up in any other test: a slip would just make the lot
/// subtly wrong for ever.
#[test]
fn the_used_car_tables_match_the_image() {
    use nfs_game::career::{USED_CAR_RARITY, USED_MILEAGE, USED_PART_PCT, USED_WINS};
    let Some(img) = Image::open() else { return };
    // 0x5B7A10: the chance of **not** being offered, in sixtieths, by age 0…60
    let rarity = img.i32s(0x5B7A10, 61);
    for (i, v) in USED_CAR_RARITY.iter().enumerate() {
        assert_eq!(*v as i32, rarity[i], "USED_CAR_RARITY[{i}]");
    }
    // the three `{roll_lo, roll_hi, min, max}` band tables, 16 bytes an entry
    for (va, name, bands) in [
        (0x5B7B08u32, "USED_PART_PCT", &USED_PART_PCT[..]),
        (0x5B7B48, "USED_MILEAGE", &USED_MILEAGE[..]),
        (0x5B7BC8, "USED_WINS", &USED_WINS[..]),
    ] {
        let raw = img.i32s(va, 16);
        for (i, b) in bands.iter().enumerate() {
            assert_eq!([b.0, b.1, b.2, b.3], raw[i * 4..i * 4 + 4], "{name}[{i}]");
        }
    }
    // 0x5B7B88 is a fourth table of the same shape that neither generator reads, and the port is
    // right not to carry it — this pins that it is the one with the 1…20 / 20…40 / 40…100 bands, so
    // a future reader does not mistake it for one of the three above.
    assert_eq!(img.i32s(0x5B7B88, 16), vec![0, 5, 0, 0, 6, 20, 1, 20, 21, 80, 20, 40, 81, 100, 40, 100]);
}

/// The paint shop's rows — interior, stripes, package — go out as the colours they hold
/// (`FECar+0x678…+0x684`, `savegame.md` §4.7a) and come back as the same rows.
#[test]
fn paint_shop_trim_round_trips() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    // a car whose rows are told apart by their colours: distinct stripe-1 colours on sets 0 and
    // 1, distinct stripe-2 colours on sets 0 and 2, and two distinct interiors
    let e = t
        .catalog
        .iter()
        .find(|c| {
            c.paint_sets.len() >= 3
                && c.paint_sets[1].packed[1] != c.paint_sets[0].packed[1]
                && c.paint_sets[2].packed[2] != c.paint_sets[0].packed[2]
                && c.paint_sets[2].packed[2] != c.paint_sets[1].packed[2]
                && c.interior_colours.len() >= 2
                && c.interior_colours[1] != c.interior_colours[0]
        })
        .expect("a car with distinguishable rows");
    let mut c = Career::default();
    let mut owned = OwnedCar::new(e.id, 0, 0);
    owned.trim = nfs_game::career::Trim { interior: 1, stripe1: 1, stripe2: 2, package: 3 };
    c.garage.push(owned.clone());
    let parsed = SaveFile::parse(&career_to_sav(&c, &t, &Profile::new("Hans", 0)).write()).unwrap();
    let img = &parsed.player.as_ref().unwrap().cars[0];
    let f = |o: usize| u32::from_le_bytes([img.image[o], img.image[o + 1], img.image[o + 2], img.image[o + 3]]);
    assert_eq!(f(car::S1_COLOUR), e.paint_sets[1].packed[1]);
    assert_eq!(f(car::S2_COLOUR), e.paint_sets[2].packed[2]);
    assert_eq!(f(car::PACKAGE_NUM), 3);
    assert_eq!(f(car::DECAL_NUM), u32::MAX, "decal −1, 0x502710");
    assert_eq!(career_from_sav(&parsed, &t).garage[0].trim, owned.trim);
}
